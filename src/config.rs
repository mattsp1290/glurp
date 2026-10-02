use crate::secure;
use anyhow::{Context, Result, bail};
use fs2::FileExt;
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
    dir: secure::Dir,
    _lock: File,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let dir = secure::Dir::open(path.parent().context("config parent missing")?, true)?;
        let lock = dir.file("config.lock", true)?;
        lock.try_lock_exclusive()
            .context("configuration is in use; retry later")?;
        let hosts: Vec<Host> = match dir.file("config.json", false) {
            Ok(file) => {
                serde_json::from_reader(file).context("invalid config.json; repair or remove it")?
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
