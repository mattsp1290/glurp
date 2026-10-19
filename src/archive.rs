//! Fully validate successful bounded streams before transaction publication.
use crate::{
    collection::Harness,
    collectors,
    config::Host,
    error::{CoreError, TaskErrorKind, with_kind},
    limits::{Budget, Limits},
    remote::{self, Deadline},
    secure::{self, Dir},
    transaction,
};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub fn check_binding(data: &Path, host: &Host) -> Result<()> {
    let base = match Dir::open(&data.join("hosts").join(&host.name), false) {
        Ok(base) => base,
        Err(error) if secure::not_found(&error) => return Ok(()),
        Err(error) => return Err(error),
    };
    verify_binding(&base, host)
}
fn verify_binding(base: &Dir, host: &Host) -> Result<()> {
    verify_binding_inner(base, host).map_err(|error| with_kind(error, TaskErrorKind::LocalStorage))
}
fn verify_binding_inner(base: &Dir, host: &Host) -> Result<()> {
    if let Some(file) = base.optional_file("destination.json")? {
        let bound: String = serde_json::from_reader(file.take(4096))
            .context(CoreError::Task(TaskErrorKind::DestinationBinding))?;
        if bound != host.destination {
            bail!(CoreError::Task(TaskErrorKind::DestinationBinding));
        }
    } else if base
        .entries()?
        .iter()
        .any(|name| matches!(name.as_str(), "claude" | "codex" | "pi" | "opencode"))
    {
        bail!(CoreError::Task(TaskErrorKind::DestinationBinding));
    }
    Ok(())
}
/// Explicit local rollback uses the same binding and exclusion as collection.
pub fn rollback(data: &Path, host: &Host) -> Result<()> {
    host.validate()?;
    let base = Dir::open(&data.join("hosts").join(&host.name), false)?;
    let _lock = lock_host(&base)?;
    verify_binding(&base, host)?;
    transaction::rollback(&base)
}
fn token(reader: &mut impl BufRead) -> Result<String> {
    let mut bytes = Vec::new();
    let read = reader
        .take(4098)
        .read_until(0, &mut bytes)
        .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
    if read == 0 || bytes.pop() != Some(0) || bytes.len() > 4096 {
        bail!("malformed or truncated archive frame");
    }
    String::from_utf8(bytes).context("archive frame is not UTF-8")
}
fn stage(
    reader: impl Read,
    stage: &Dir,
    limits: &Limits,
    budget: &mut Budget,
    deadline: &Deadline,
) -> Result<(Vec<String>, bool)> {
    let mut reader = BufReader::new(reader);
    if token(&mut reader)? != "GLURP1" {
        bail!("unsupported archive protocol");
    }
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let mut status: Option<bool> = None;
    loop {
        deadline.check()?;
        match token(&mut reader)?.as_str() {
            "E" => {
                if !reader
                    .fill_buf()
                    .context(CoreError::Task(TaskErrorKind::LocalStorage))?
                    .is_empty()
                {
                    bail!("unexpected bytes after archive end");
                }
                let found = status.context("missing terminal archive status")?;
                if !found && !paths.is_empty() {
                    bail!("not-found stream contains artifacts");
                }
                return Ok((paths, found));
            }
            "T" if status.is_none() => {
                status = Some(match token(&mut reader)?.as_str() {
                    "ok" => true,
                    "not-found" => false,
                    _ => bail!(
                        "remote source discovery failed; check configured roots and permissions"
                    ),
                });
                continue;
            }
            "F" if status.is_none() => (),
            _ => bail!("unknown archive frame"),
        }
        let path = token(&mut reader)?;
        secure::relative(&path)?;
        if !seen.insert(path.clone()) {
            bail!("duplicate artifact path");
        }
        let raw_size = token(&mut reader)?;
        if raw_size.is_empty() || !raw_size.bytes().all(|b| b.is_ascii_digit()) {
            bail!("invalid artifact size");
        }
        let size = raw_size
            .parse::<u64>()
            .context(CoreError::Task(TaskErrorKind::LimitExceeded))?;
        budget
            .add(size, limits)
            .map_err(|error| with_kind(error, TaskErrorKind::LimitExceeded))?;
        let (parent, leaf) = stage
            .parent(&path, true)
            .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
        let mut file = parent
            .create_new(&leaf)
            .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
        let mut remaining = size;
        let mut bytes = [0_u8; 64 * 1024];
        while remaining > 0 {
            deadline.check()?;
            let len = remaining.min(bytes.len() as u64) as usize;
            let count = reader
                .read(&mut bytes[..len])
                .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
            if count == 0 {
                bail!("truncated artifact payload");
            }
            std::io::Write::write_all(&mut file, &bytes[..count])
                .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
            remaining -= count as u64;
        }
        file.sync_all()
            .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
        parent
            .sync()
            .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
        paths.push(path);
    }
}
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ArchiveOutcome {
    Collected { artifact_count: u64 },
    NotFound,
}

/// Own the safe directory anchor and authoritative filesystem lease for a whole
/// selected host batch, including recovery and postcommit cleanup.
pub(crate) struct HostSession<'host> {
    _lock: secure::ExclusiveLock,
    base: Dir,
    host: &'host Host,
}

fn lock_host(base: &Dir) -> Result<secure::ExclusiveLock> {
    let file = base
        .file("archive.lock", true)
        .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
    secure::ExclusiveLock::try_lock(file).map_err(|error| {
        let kind = if error.kind() == std::io::ErrorKind::WouldBlock {
            TaskErrorKind::Busy
        } else {
            TaskErrorKind::LocalStorage
        };
        anyhow::Error::new(error).context(CoreError::Task(kind))
    })
}

/// Invocation-scoped test checkpoints. These callbacks do not exist in builds
/// consumed by callers; no process-global or environment-controlled hooks exist.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestCheckpoint {
    BeforeHarness,
    Publication(usize),
    BeforeCommit,
    Committed,
    BeforeCleanup,
    BeforeRecovery,
}
#[cfg(test)]
pub(crate) struct TestHooks {
    pub(crate) checkpoint: Box<dyn Fn(TestCheckpoint) -> Result<()> + Send + Sync>,
}
#[cfg(test)]
impl TestHooks {
    pub(crate) fn hit(&self, point: TestCheckpoint) -> Result<()> {
        (self.checkpoint)(point)
    }
}

impl<'host> HostSession<'host> {
    pub(crate) fn open(data: &Path, host: &'host Host) -> Result<Self> {
        host.validate()
            .map_err(|error| with_kind(error, TaskErrorKind::Internal))?;
        let base = (|| {
            let root = Dir::open(data, true)?;
            root.child("hosts", true)?.child(&host.name, true)
        })()
        .map_err(|error| with_kind(error, TaskErrorKind::LocalStorage))?;
        let lock = lock_host(&base)?;
        verify_binding(&base, host)?;
        transaction::recover(&base).context(CoreError::Task(TaskErrorKind::RecoveryRequired))?;
        if base
            .optional_file("destination.json")
            .context(CoreError::Task(TaskErrorKind::LocalStorage))?
            .is_none()
        {
            base.atomic_json("destination.json", &host.destination)
                .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
        }
        Ok(Self {
            _lock: lock,
            base,
            host,
        })
    }

    /// Inspect retained transaction evidence under this session's host lease.
    /// Readiness must take precedence over cancellation of subsequent tasks.
    pub(crate) fn ensure_ready(&self) -> Result<()> {
        transaction::recover(&self.base).context(CoreError::Task(TaskErrorKind::RecoveryRequired))
    }

    pub(crate) fn collect(
        &self,
        harness: Harness,
        limits: &Limits,
        cancellation: CancellationToken,
    ) -> Result<ArchiveOutcome> {
        self.collect_inner(
            harness,
            limits,
            cancellation,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn collect_with_hooks(
        &self,
        harness: Harness,
        limits: &Limits,
        cancellation: CancellationToken,
        hooks: &TestHooks,
    ) -> Result<ArchiveOutcome> {
        self.collect_inner(harness, limits, cancellation, Some(hooks))
    }

    fn collect_inner(
        &self,
        harness: Harness,
        limits: &Limits,
        cancellation: CancellationToken,
        #[cfg(test)] hooks: Option<&TestHooks>,
    ) -> Result<ArchiveOutcome> {
        #[cfg(test)]
        if let Some(hooks) = hooks {
            hooks.hit(TestCheckpoint::BeforeHarness)?;
        }
        // A prior task can be Collected while durable committed evidence remains
        // after partial cleanup. Never reuse that staging directory or start SSH.
        self.ensure_ready()?;
        let deadline = Deadline::new(limits.timeout_seconds, cancellation);
        deadline.check()?;
        let mut wire_left = limits
            .wire_limit()
            .map_err(|error| with_kind(error, TaskErrorKind::LimitExceeded))?;
        let script = collectors::script(self.host, harness);
        let txn = self
            .base
            .child(".transaction", true)
            .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
        let mut published = false;
        let mut publication_attempted = false;
        let result = (|| {
            let spool = txn
                .create_new("spool")
                .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
            let stream = remote::fetch(
                &self.host.destination,
                script,
                spool,
                &deadline,
                &mut wire_left,
            )
            .map_err(|error| with_kind(error, TaskErrorKind::Transport))?;
            let artifacts = txn
                .child("artifacts", true)
                .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
            let mut budget = Budget::new();
            let (mut paths, found) = stage(stream, &artifacts, limits, &mut budget, &deadline)
                .map_err(|error| with_kind(error, TaskErrorKind::InvalidRemoteStream))?;
            if !found {
                return Ok(ArchiveOutcome::NotFound);
            }
            if harness == Harness::OpenCode {
                if paths != ["inventory.json"] {
                    bail!(CoreError::Task(TaskErrorKind::InvalidRemoteStream));
                }
                let ids = collectors::inventory(&artifacts, &deadline)
                    .map_err(|error| with_kind(error, TaskErrorKind::InvalidRemoteStream))?;
                budget
                    .ensure_exports(ids.len(), limits)
                    .map_err(|error| with_kind(error, TaskErrorKind::LimitExceeded))?;
                deadline.check()?;
                artifacts
                    .remove("inventory.json")
                    .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
                let spool = txn
                    .create_new("exports-spool")
                    .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
                let stream = remote::fetch(
                    &self.host.destination,
                    collectors::exports(&ids),
                    spool,
                    &deadline,
                    &mut wire_left,
                )
                .map_err(|error| with_kind(error, TaskErrorKind::Transport))?;
                let (exports, found) = stage(stream, &artifacts, limits, &mut budget, &deadline)
                    .map_err(|error| with_kind(error, TaskErrorKind::InvalidRemoteStream))?;
                if !found {
                    bail!(CoreError::Task(TaskErrorKind::InvalidRemoteStream));
                }
                collectors::validate_exports(&artifacts, &ids, &exports, &deadline)
                    .map_err(|error| with_kind(error, TaskErrorKind::InvalidRemoteStream))?;
                paths = exports;
            }
            deadline.check()?;
            publication_attempted = true;
            #[cfg(test)]
            let publication = transaction::publish_with_hooks(
                &self.base,
                &txn,
                &artifacts,
                harness.as_str(),
                &paths,
                &deadline,
                hooks,
            );
            #[cfg(not(test))]
            let publication = transaction::publish(
                &self.base,
                &txn,
                &artifacts,
                harness.as_str(),
                &paths,
                &deadline,
            );
            publication.map_err(|error| with_kind(error, TaskErrorKind::LocalStorage))?;
            published = true;
            Ok(ArchiveOutcome::Collected {
                artifact_count: paths.len() as u64,
            })
        })();
        if published {
            // Do not inspect cancellation after the durable commit boundary.
            return result;
        }
        #[cfg(test)]
        if let Some(hooks) = hooks {
            hooks
                .hit(TestCheckpoint::BeforeRecovery)
                .context(CoreError::Task(TaskErrorKind::RecoveryRequired))?;
        }
        // Rollback intentionally ignores cancellation and the collection deadline.
        // Recovery failure outranks any interrupted-operation result.
        let recovery = if publication_attempted {
            transaction::recover_failed(&self.base)
        } else {
            transaction::recover(&self.base)
        };
        recovery.context(CoreError::Task(TaskErrorKind::RecoveryRequired))?;
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn hostile_frames_fail_without_publication() {
        for bytes in [
            b"GLURP1\0F\0../escape\x001\0xE\0".as_slice(),
            b"GLURP1\0F\0ok\0999999999999999999999\0",
            b"GLURP1\0F\0ok\x002\0x",
            b"GLURP1\0E\0trailing",
            b"GLURP1\0F\0ok\x000\0F\0ok\x000\0E\0",
        ] {
            let temp = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            let dir = Dir::open(&temp.path().canonicalize().unwrap(), true).unwrap();
            assert!(
                stage(
                    bytes,
                    &dir,
                    &Limits::default(),
                    &mut Budget::new(),
                    &Deadline::new(5, CancellationToken::new())
                )
                .is_err()
            );
        }
    }

    fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Host) {
        let temp = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let host = Host {
            name: "lab".into(),
            destination: "lab".into(),
            claude_paths: Vec::new(),
            codex_paths: Vec::new(),
            pi_paths: Vec::new(),
        };
        (temp, root, host)
    }

    fn assert_busy(root: &Path, host: &Host) {
        let error = match HostSession::open(root, host) {
            Ok(_) => panic!("overlapping host lease was admitted"),
            Err(error) => error,
        };
        assert_eq!(
            crate::error::classify(&error),
            CoreError::Task(TaskErrorKind::Busy)
        );
    }

    #[test]
    fn host_lease_covers_successful_cleanup_and_every_harness() {
        use std::io::Write;
        let (_temp, root, host) = fixture();
        let session = HostSession::open(&root, &host).unwrap();
        let hooks = TestHooks {
            checkpoint: Box::new({
                let root = root.clone();
                let host = host.clone();
                move |point| {
                    if point == TestCheckpoint::BeforeCleanup {
                        assert_busy(&root, &host);
                        // Explicit CLI rollback must honor this same file lock.
                        assert!(rollback(&root, &host).is_err());
                    }
                    Ok(())
                }
            }),
        };
        for harness in ["codex", "pi"] {
            let txn = session.base.child(".transaction", true).unwrap();
            let artifacts = txn.child("artifacts", true).unwrap();
            artifacts
                .create_new("session.jsonl")
                .unwrap()
                .write_all(b"private fixture")
                .unwrap();
            transaction::publish_with_hooks(
                &session.base,
                &txn,
                &artifacts,
                harness,
                &["session.jsonl".into()],
                &Deadline::new(30, CancellationToken::new()),
                Some(&hooks),
            )
            .unwrap();
            // The lease survives complete transaction removal and the gap before
            // the next selected harness, rather than being reacquired per task.
            assert_busy(&root, &host);
            assert!(secure::not_found(
                &session.base.child(".transaction", false).err().unwrap()
            ));
            let mut other = host.clone();
            other.name = "independent".into();
            HostSession::open(&root, &other).unwrap();
        }
        drop(session);
        HostSession::open(&root, &host).unwrap();
    }

    #[test]
    fn next_harness_refuses_postcommit_evidence_before_staging_or_transport() {
        use std::io::Write;
        let (_temp, root, host) = fixture();
        let session = HostSession::open(&root, &host).unwrap();
        let archive = session.base.child("codex", true).unwrap();
        for path in ["a", "b"] {
            archive
                .create_new(path)
                .unwrap()
                .write_all(b"original")
                .unwrap();
        }
        let txn = session.base.child(".transaction", true).unwrap();
        let artifacts = txn.child("artifacts", true).unwrap();
        for path in ["a", "b"] {
            artifacts
                .create_new(path)
                .unwrap()
                .write_all(b"changed!")
                .unwrap();
        }
        let hooks = TestHooks {
            checkpoint: Box::new({
                let root = root.clone();
                move |point| {
                    if point == TestCheckpoint::BeforeCleanup {
                        let base = Dir::open(&root.join("hosts/lab"), false)?;
                        base.child(".transaction", false)?
                            .child("backups", false)?
                            .remove("0")?;
                        bail!("synthetic partial cleanup failure");
                    }
                    Ok(())
                }
            }),
        };
        transaction::publish_with_hooks(
            &session.base,
            &txn,
            &artifacts,
            "codex",
            &["a".into(), "b".into()],
            &Deadline::new(30, CancellationToken::new()),
            Some(&hooks),
        )
        .unwrap();
        let mut original_journal = Vec::new();
        txn.read("committed")
            .unwrap()
            .read_to_end(&mut original_journal)
            .unwrap();
        // Calling collect must fail readiness before trying to invoke any SSH,
        // even when the task token already requests cancellation.
        for cancelled in [false, true] {
            let cancellation = CancellationToken::new();
            if cancelled {
                cancellation.cancel();
            }
            let error = session
                .collect(Harness::Pi, &Limits::default(), cancellation)
                .unwrap_err();
            assert_eq!(
                crate::error::classify(&error),
                CoreError::Task(TaskErrorKind::RecoveryRequired)
            );
            assert!(secure::not_found(
                &session.base.child("pi", false).err().unwrap()
            ));
            assert!(txn.optional_file("spool").unwrap().is_none());
            assert!(txn.optional_file(".rollback-required").unwrap().is_none());
            let mut journal = Vec::new();
            txn.read("committed")
                .unwrap()
                .read_to_end(&mut journal)
                .unwrap();
            assert_eq!(journal, original_journal);
            let backups = txn.child("backups", false).unwrap();
            assert!(backups.optional_file("0").unwrap().is_none());
            let mut remaining = Vec::new();
            backups
                .read("1")
                .unwrap()
                .read_to_end(&mut remaining)
                .unwrap();
            assert_eq!(remaining, b"original");
        }
    }

    #[test]
    fn destination_binding_admission_has_a_typed_boundary() {
        let (_temp, root, mut host) = fixture();
        drop(HostSession::open(&root, &host).unwrap());
        host.destination = "other".into();
        let error = match HostSession::open(&root, &host) {
            Ok(_) => panic!("changed destination was admitted"),
            Err(error) => error,
        };
        assert_eq!(
            crate::error::classify(&error),
            CoreError::Task(TaskErrorKind::DestinationBinding)
        );
    }

    #[test]
    fn completed_host_lease_is_released_even_with_an_inherited_file_description() {
        let (_temp, root, host) = fixture();
        let session = HostSession::open(&root, &host).unwrap();
        // dup and fork share the same flock-owning open-file description. Keep
        // a duplicate alive to model a child not yet having reached CLOEXEC,
        // without actually forking a multithreaded test runner.
        let inherited = session._lock.clone_file_for_test().unwrap();
        assert_busy(&root, &host);
        drop(session);
        assert!(inherited.metadata().is_ok());
        let next = HostSession::open(&root, &host);
        assert!(
            next.is_ok(),
            "completed lease was retained by an inherited descriptor"
        );
    }
}
