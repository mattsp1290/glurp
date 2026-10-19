use crate::{error::CollectionError, secure};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};

pub struct Paths {
    pub config: PathBuf,
    pub data: PathBuf,
}

impl Paths {
    pub fn resolve() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .context("set HOME to an absolute directory")?;
        let base = |key: &str, fallback: &str| {
            std::env::var_os(key)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(fallback))
        };
        Ok(Self {
            config: base("XDG_CONFIG_HOME", ".config").join("glurp/config.json"),
            data: base("XDG_DATA_HOME", ".local/share").join("glurp"),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub name: String,
    pub destination: String,
    #[serde(default)]
    pub claude_paths: Vec<String>,
    #[serde(default)]
    pub codex_paths: Vec<String>,
    #[serde(default)]
    pub pi_paths: Vec<String>,
}

impl Host {
    pub fn validate(&self) -> Result<()> {
        if self.name.is_empty()
            || self.name.len() > 64
            || !self
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("host name must contain 1–64 letters, digits, hyphens or underscores");
        }
        if self.destination.is_empty()
            || self.destination.len() > 255
            || self.destination.starts_with('-')
            || !self
                .destination
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"@._-:[]".contains(&b))
        {
            bail!(
                "destination must be an SSH alias or user@host without options or shell characters"
            );
        }
        for root in self
            .claude_paths
            .iter()
            .chain(&self.codex_paths)
            .chain(&self.pi_paths)
        {
            if !(root.starts_with('/') || root.starts_with("~/"))
                || root.contains(['\0', '\n', '\r'])
            {
                bail!(
                    "source roots must be absolute or start with ~/ and contain no control separators"
                );
            }
        }
        Ok(())
    }
}

pub struct Store {
    pub hosts: Vec<Host>,
    _lock: secure::ExclusiveLock,
    dir: secure::Dir,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_inner(path, None)
    }

    pub(crate) fn open_for_collection(
        path: &Path,
        token: &tokio_util::sync::CancellationToken,
    ) -> Result<Self> {
        Self::open_inner(path, Some(token))
    }

    fn open_inner(
        path: &Path,
        token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<Self> {
        if token.is_some_and(|token| token.is_cancelled()) {
            return Err(CollectionError::CancelledBeforePlanning.into());
        }
        if path.file_name() != Some(std::ffi::OsStr::new("config.json")) {
            bail!("configuration path must end in config.json");
        }
        let dir = secure::Dir::open(path.parent().context("config parent missing")?, true)?;
        let lock = dir.file("config.lock", true)?;
        let lock = secure::ExclusiveLock::try_lock(lock).context(CollectionError::ConfigBusy)?;
        let hosts: Vec<Host> = match dir.file("config.json", false) {
            Ok(file) => {
                let mut reader = ConfigReader {
                    file,
                    token,
                    cancelled: false,
                };
                let parsed = serde_json::from_reader(std::io::BufReader::new(&mut reader));
                if reader.cancelled {
                    return Err(CollectionError::CancelledBeforePlanning.into());
                }
                parsed.context("invalid config.json; repair or remove it")?
            }
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                Vec::new()
            }
            Err(e) => return Err(e),
        };
        let mut names = std::collections::HashSet::new();
        for host in &hosts {
            host.validate()?;
            if !names.insert(&host.name) {
                bail!("config contains duplicate host names");
            }
        }
        Ok(Self {
            hosts,
            dir,
            _lock: lock,
        })
    }

    fn save(&self) -> Result<()> {
        self.dir.atomic_json("config.json", &self.hosts)
    }

    pub fn add(&mut self, host: Host, data: &Path) -> Result<()> {
        host.validate()?;
        if self.hosts.iter().any(|h| h.name == host.name) {
            bail!("host already exists; remove it before adding");
        }
        crate::archive::check_binding(data, &host)?;
        self.hosts.push(host);
        self.hosts.sort_by(|a, b| a.name.cmp(&b.name));
        self.save()
    }

    pub fn remove(&mut self, name: &str) -> Result<()> {
        let len = self.hosts.len();
        self.hosts.retain(|h| h.name != name);
        if self.hosts.len() == len {
            bail!("unknown host; use glurp host list");
        }
        self.save()
    }

    pub fn select(&self, names: &[String]) -> Result<Vec<Host>> {
        if self.hosts.is_empty() {
            bail!("no hosts configured; use glurp host add <name> <destination>");
        }
        if names.is_empty() {
            return Ok(self.hosts.clone());
        }
        let mut selected = Vec::new();
        for name in names {
            let host = self
                .hosts
                .iter()
                .find(|h| &h.name == name)
                .context("unknown host; use glurp host list")?;
            if !selected.iter().any(|h: &Host| h.name == host.name) {
                selected.push(host.clone());
            }
        }
        selected.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(selected)
    }
}

struct ConfigReader<'a> {
    file: File,
    token: Option<&'a tokio_util::sync::CancellationToken>,
    cancelled: bool,
}
impl std::io::Read for ConfigReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if self.token.is_some_and(|token| token.is_cancelled()) {
            self.cancelled = true;
            return Err(std::io::Error::other(
                CollectionError::CancelledBeforePlanning,
            ));
        }
        std::io::Read::read(&mut self.file, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn interrupted_config_read_retains_preplanning_cancellation() {
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(b"[]").unwrap();
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0)).unwrap();
        let token = CancellationToken::new();
        let mut reader = ConfigReader {
            file,
            token: Some(&token),
            cancelled: false,
        };
        let mut first = [0_u8; 1];
        assert_eq!(reader.read(&mut first).unwrap(), 1);
        assert_eq!(first, [b'[']);
        token.cancel();
        let error = reader.read(&mut first).unwrap_err();
        assert!(reader.cancelled);
        assert_eq!(
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<CollectionError>()),
            Some(&CollectionError::CancelledBeforePlanning)
        );
    }

    #[test]
    fn caller_supplied_config_filename_is_not_silently_ignored() {
        let temp = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        assert!(Store::open(&temp.path().join("other.json")).is_err());
        assert!(!temp.path().join("config.lock").exists());
    }

    #[test]
    fn config_snapshot_owner_releases_inherited_description() {
        let temp = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let path = temp.path().canonicalize().unwrap().join("config.json");
        let store = Store::open(&path).unwrap();
        let inherited = store._lock.clone_file_for_test().unwrap();
        assert!(Store::open(&path).is_err());
        drop(store);
        // A concurrent process fork can retain this same open-file description
        // until exec. It does not own the completed snapshot's lease.
        assert!(Store::open(&path).is_ok());
        drop(inherited);
    }
}
