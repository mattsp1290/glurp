//! A host lock protects one durable transaction. Backups are complete and synced
//! before ready.json becomes durable. Restart-visible committed journals are
//! ambiguous: only the live publisher knows whether the commit sync succeeded.
#[cfg(test)]
use crate::archive::{TestCheckpoint, TestHooks};
use crate::{
    error::{CoreError, TaskErrorKind},
    remote::Deadline,
    secure::{self, Dir},
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;

const MAX_JOURNAL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    old: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    harness: String,
    entries: Vec<Entry>,
}
struct Change {
    path: String,
    version: Option<(u64, u64, u64, i64, i64)>,
}

fn version(file: &std::fs::File) -> Result<(u64, u64, u64, i64, i64)> {
    let meta = file.metadata()?;
    Ok((
        meta.dev(),
        meta.ino(),
        meta.len(),
        meta.mtime(),
        meta.mtime_nsec(),
    ))
}
fn unchanged(target: &Dir, leaf: &str, change: &Change) -> Result<Option<std::fs::File>> {
    let current = target.optional_file(leaf)?;
    let actual = current.as_ref().map(version).transpose()?;
    if actual != change.version {
        bail!("concurrent archive modification detected");
    }
    Ok(current)
}
fn equal(a: std::fs::File, b: std::fs::File, deadline: &Deadline) -> Result<bool> {
    let (mut a, mut b) = (BufReader::new(a), BufReader::new(b));
    loop {
        deadline.check()?;
        let aa = a.fill_buf()?;
        let bb = b.fill_buf()?;
        let len = aa.len().min(bb.len());
        if len == 0 {
            return Ok(aa.is_empty() && bb.is_empty());
        }
        if aa[..len] != bb[..len] {
            return Ok(false);
        }
        a.consume(len);
        b.consume(len);
    }
}
fn copy_backup(
    mut old: std::fs::File,
    backups: &Dir,
    index: usize,
    deadline: Option<&Deadline>,
) -> Result<()> {
    let meta = old.metadata()?;
    old.seek(SeekFrom::Start(0))?;
    let mut backup = backups.create_new(&index.to_string())?;
    let mut bytes = [0_u8; 64 * 1024];
    loop {
        if let Some(deadline) = deadline {
            deadline.check()?;
        }
        let count = old.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        if let Some(deadline) = deadline {
            deadline.check()?;
        }
        backup.write_all(&bytes[..count])?;
    }
    if let Some(deadline) = deadline {
        deadline.check()?;
    }
    // Rollback preserves original timestamps, including unchanged entries that
    // had backups but were not reached before the publication failure.
    let times = [
        libc::timespec {
            tv_sec: meta.atime() as _,
            tv_nsec: meta.atime_nsec() as _,
        },
        libc::timespec {
            tv_sec: meta.mtime() as _,
            tv_nsec: meta.mtime_nsec() as _,
        },
    ];
    if unsafe { libc::futimens(backup.as_raw_fd(), times.as_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    backup.sync_all()?;
    Ok(())
}
fn valid_harness(harness: &str) -> bool {
    matches!(harness, "claude" | "codex" | "pi" | "opencode")
}

pub fn recover(base: &Dir) -> Result<()> {
    recover_inner(base, Recovery::Automatic)
}
/// A failed publication must never be reinterpreted as successful commit cleanup.
pub fn recover_failed(base: &Dir) -> Result<()> {
    recover_inner(base, Recovery::FailedPublication)
}
pub fn rollback(base: &Dir) -> Result<()> {
    recover_inner(base, Recovery::ExplicitRollback)
}
#[derive(Clone, Copy)]
enum Recovery {
    Automatic,
    FailedPublication,
    ExplicitRollback,
}
#[derive(Clone, Copy)]
enum TransactionState {
    Staging,
    Rollback,
    AmbiguousCommit,
}
fn recover_inner(base: &Dir, mode: Recovery) -> Result<()> {
    // .recovery records a failed decision transition whose reverse rename
    // could not be completed. Its journal is always interpreted as rollback.
    let recovery = match base.child(".recovery", false) {
        Ok(dir) => Some(dir),
        Err(error) if secure::not_found(&error) => None,
        Err(error) => return Err(error),
    };
    let recovering = recovery.is_some();
    if recovering {
        match base.child(".transaction", false) {
            Ok(_) => {
                bail!("conflicting recovery directories; preserve evidence and resolve manually")
            }
            Err(error) if secure::not_found(&error) => (),
            Err(error) => return Err(error),
        }
    }
    let transaction_name = if recovering {
        ".recovery"
    } else {
        ".transaction"
    };
    let txn = match recovery
        .map(Ok)
        .unwrap_or_else(|| base.child(".transaction", false))
    {
        Ok(dir) => dir,
        Err(error) if secure::not_found(&error) => return Ok(()),
        Err(error) => return Err(error),
    };
    let marked_failed = txn.optional_file(".rollback-required")?.is_some();
    let ready = txn.optional_file("ready.json")?.is_some();
    let committed = txn.optional_file("committed")?.is_some();
    let state = if ready || (committed && (recovering || marked_failed)) {
        TransactionState::Rollback
    } else if committed {
        TransactionState::AmbiguousCommit
    } else {
        TransactionState::Staging
    };
    if matches!(state, TransactionState::AmbiguousCommit) && matches!(mode, Recovery::Automatic) {
        bail!(
            "ambiguous recovery decision; backups retained; use glurp recover HOST --rollback or preserve evidence for manual resolution"
        );
    }
    let journal_name = if ready { "ready.json" } else { "committed" };
    if let Some(file) = txn.optional_file(journal_name)? {
        // The journal is locally generated, but must not accept arbitrary paths
        // or unbounded data if another process damaged the transaction.
        let journal: Journal = serde_json::from_reader(file.take(MAX_JOURNAL_BYTES))
            .context("invalid recovery journal; preserve transaction and repair storage")?;
        if !valid_harness(&journal.harness) {
            bail!("invalid recovery harness");
        }
        let archive = base.child(&journal.harness, false)?;
        let backups = txn.child("backups", false)?;
        let mut seen = std::collections::HashSet::new();
        // Preflight all rollback paths before restoring any.
        for (index, entry) in journal.entries.iter().enumerate() {
            secure::relative(&entry.path)?;
            if !seen.insert(&entry.path) {
                bail!("duplicate recovery path");
            }
            let (parent, leaf) = archive.parent(&entry.path, false)?;
            parent.optional_file(&leaf)?;
            if entry.old {
                backups.file(&index.to_string(), false).context(
                    "recovery backup unavailable; preserve evidence and resolve manually",
                )?;
            }
        }
        if matches!(state, TransactionState::AmbiguousCommit) {
            // Persist rollback authority only after the complete preflight. A
            // failed marker creation leaves ambiguity, never cleanup permission.
            txn.create_new(".rollback-required")?.sync_all()?;
            txn.sync()?;
        }
        // Copy the backup to a durable temporary and rename it over the target.
        // Keep backups until *all* rollback work succeeds, permitting retries.
        for (index, entry) in journal.entries.iter().enumerate() {
            let (parent, leaf) = archive.parent(&entry.path, false)?;
            let exists = parent.optional_file(&leaf)?.is_some();
            if entry.old {
                let temp = index.to_string();
                if txn.optional_file(&temp)?.is_some() {
                    txn.remove(&temp)?;
                }
                copy_backup(backups.file(&index.to_string(), false)?, &txn, index, None)?;
                // copy_backup uses the numeric name in txn; backups remain untouched.
                txn.rename(&index.to_string(), &parent, &leaf)?;
            } else if exists {
                parent.remove(&leaf)?;
            }
        }
        // Once restoration is durable, clear the rollback instruction before
        // deleting any backup. A crash during cleanup cannot need deleted backups.
        txn.remove(journal_name)?;
    }
    base.remove_tree(transaction_name)?;
    Ok(())
}

#[cfg(not(test))]
pub fn publish(
    base: &Dir,
    txn: &Dir,
    artifacts: &Dir,
    harness: &str,
    paths: &[String],
    deadline: &Deadline,
) -> Result<()> {
    publish_inner(base, txn, artifacts, harness, paths, deadline)
}
#[cfg(test)]
pub(crate) fn publish_with_hooks(
    base: &Dir,
    txn: &Dir,
    artifacts: &Dir,
    harness: &str,
    paths: &[String],
    deadline: &Deadline,
    hooks: Option<&TestHooks>,
) -> Result<()> {
    publish_inner(base, txn, artifacts, harness, paths, deadline, |point| {
        hooks.map_or(Ok(()), |hooks| hooks.hit(point))
    })
}
fn publish_inner(
    base: &Dir,
    txn: &Dir,
    artifacts: &Dir,
    harness: &str,
    paths: &[String],
    deadline: &Deadline,
    #[cfg(test)] mut checkpoint: impl FnMut(TestCheckpoint) -> Result<()>,
) -> Result<()> {
    let archive = base.child(harness, true)?;
    let backups = txn.child("backups", true)?;
    let mut changes = Vec::new();
    // Preflight the whole batch. Retain root descriptors and file fingerprints,
    // then traverse with O_NOFOLLOW per operation. FD use is independent of count.
    for path in paths {
        deadline.check()?;
        let (target, leaf) = archive.parent(path, true)?;
        let old = target.optional_file(&leaf)?;
        let version = old.as_ref().map(version).transpose()?;
        let (source, source_leaf) = artifacts.parent(path, false)?;
        if let Some(existing) = old.as_ref()
            && equal(
                existing.try_clone()?,
                source.file(&source_leaf, false)?,
                deadline,
            )?
        {
            if Some(self::version(existing)?) != version {
                bail!("concurrent archive modification detected");
            }
            continue;
        }
        changes.push(Change {
            path: path.clone(),
            version,
        });
    }

    let mut entries = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        deadline.check()?;
        let (target, leaf) = archive.parent(&change.path, false)?;
        if let Some(old) = unchanged(&target, &leaf, change)? {
            copy_backup(old, &backups, index, Some(deadline))?;
        }
        unchanged(&target, &leaf, change)?;
        entries.push(Entry {
            path: change.path.clone(),
            old: change.version.is_some(),
        });
    }

    backups.sync()?;
    let journal = Journal {
        harness: harness.to_owned(),
        entries,
    };
    txn.atomic_json_limited("ready.json", &journal, MAX_JOURNAL_BYTES)?;
    let result = (|| {
        #[cfg(test)]
        let mut published = 0;
        for change in &changes {
            deadline.check()?;
            // Refuse a substituted nonregular target. A cooperating writer is
            // excluded by archive.lock; descriptors prevent directory redirection.
            let (target, leaf) = archive.parent(&change.path, false)?;
            unchanged(&target, &leaf, change)?;
            let (source, source_leaf) = artifacts.parent(&change.path, false)?;
            source.rename(&source_leaf, &target, &leaf)?;
            #[cfg(test)]
            {
                published += 1;
                checkpoint(TestCheckpoint::Publication(published))?;
            }
        }
        #[cfg(test)]
        checkpoint(TestCheckpoint::BeforeCommit)?;
        deadline.check()?;
        // Move the complete durable journal; never unlink and reconstruct it.
        // Either name always retains every rollback instruction and backup.
        txn.rename_unsynced("ready.json", txn, "committed")?;
        if let Err(error) = txn.sync() {
            // Rename back does not depend on a successful metadata rewrite or
            // sync. Recovery will see ready.json even if another fsync fails.
            if txn.rename_unsynced("committed", txn, "ready.json").is_err() {
                // Moving the parent records rollback intent even when the
                // transaction directory itself no longer permits renames.
                base.rename_unsynced(".transaction", base, ".recovery")
                    .context("commit failed; rollback journal and backups need repair")?;
                // No metadata rewriting is required. If this sync also fails,
                // the visible .recovery state remains conservative on retry.
                let _ = base.sync();
            }
            return Err(error);
        }
        Ok(())
    })();
    if let Err(error) = result {
        #[cfg(test)]
        checkpoint(TestCheckpoint::BeforeRecovery)
            .context(CoreError::Task(TaskErrorKind::RecoveryRequired))?;
        recover_failed(base).context(CoreError::Task(TaskErrorKind::RecoveryRequired))?;
        return Err(error);
    }
    // Only this live publisher observed successful commit-directory sync.
    // Cleanup failure leaves ambiguity on restart, never cleanup authority.
    #[cfg(test)]
    if checkpoint(TestCheckpoint::Committed)
        .and_then(|_| checkpoint(TestCheckpoint::BeforeCleanup))
        .is_err()
    {
        return Ok(());
    }
    let _ = base.remove_tree(".transaction");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use tokio_util::sync::CancellationToken;
    fn setup() -> (tempfile::TempDir, Dir, Dir, Dir) {
        let temp = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let base = Dir::open(&temp.path().canonicalize().unwrap(), true).unwrap();
        let archive = base.child("codex", true).unwrap();
        for (path, bytes) in [("a", b"old-a"), ("b", b"old-b")] {
            archive.create_new(path).unwrap().write_all(bytes).unwrap();
        }
        let txn = base.child(".transaction", true).unwrap();
        let artifacts = txn.child("artifacts", true).unwrap();
        for (path, bytes) in [("a", b"new-a"), ("b", b"new-b")] {
            artifacts
                .create_new(path)
                .unwrap()
                .write_all(bytes)
                .unwrap();
        }
        (temp, base, txn, artifacts)
    }
    fn read(base: &Dir, path: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        base.child("codex", false)
            .unwrap()
            .read(path)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }
    #[test]
    fn late_local_failure_rolls_back_whole_batch_and_mtime() {
        let (_temp, base, txn, artifacts) = setup();
        let old = base
            .child("codex", false)
            .unwrap()
            .read("a")
            .unwrap()
            .metadata()
            .unwrap();
        artifacts
            .create_new("c")
            .unwrap()
            .write_all(b"new-c")
            .unwrap();
        let paths = vec!["a".into(), "c".into(), "b".into()];
        let result = publish_inner(
            &base,
            &txn,
            &artifacts,
            "codex",
            &paths,
            &Deadline::new(10, CancellationToken::new()),
            |point| {
                if point == TestCheckpoint::Publication(2) {
                    // Force an actual ENOENT from the later rename, after both
                    // a replacement and a new artifact have already published.
                    artifacts.remove("b")?;
                }
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(read(&base, "a"), b"old-a");
        assert_eq!(read(&base, "b"), b"old-b");
        assert!(
            base.child("codex", false)
                .unwrap()
                .optional_file("c")
                .unwrap()
                .is_none()
        );
        let restored = base
            .child("codex", false)
            .unwrap()
            .read("a")
            .unwrap()
            .metadata()
            .unwrap();
        assert_eq!(
            (old.mtime(), old.mtime_nsec()),
            (restored.mtime(), restored.mtime_nsec())
        );
        assert!(secure::not_found(
            &base.child(".transaction", false).err().unwrap()
        ));
    }
    #[test]
    fn interrupted_publication_is_recoverable_and_recovery_is_retryable() {
        let (_temp, base, txn, artifacts) = setup();
        let archive = base.child("codex", false).unwrap();
        let backups = txn.child("backups", true).unwrap();
        for (index, path) in ["a", "b"].iter().enumerate() {
            copy_backup(archive.read(path).unwrap(), &backups, index, None).unwrap();
        }
        backups.sync().unwrap();
        txn.atomic_json(
            "ready.json",
            &Journal {
                harness: "codex".into(),
                entries: vec![
                    Entry {
                        path: "a".into(),
                        old: true,
                    },
                    Entry {
                        path: "b".into(),
                        old: true,
                    },
                ],
            },
        )
        .unwrap();
        artifacts.rename("a", &archive, "a").unwrap();
        // An obstacle causes rollback to stop safely with all originals retained.
        archive.rename("b", &archive, "blocked").unwrap();
        archive.child("b", true).unwrap();
        assert!(recover(&base).is_err());
        assert!(backups.file("0", false).is_ok());
        assert!(backups.file("1", false).is_ok());
        archive.remove_tree("b").unwrap();
        archive.rename("blocked", &archive, "b").unwrap();
        recover(&base).unwrap();
        recover(&base).unwrap();
        assert_eq!(read(&base, "a"), b"old-a");
        assert_eq!(read(&base, "b"), b"old-b");
    }
    #[test]
    fn interrupted_failed_commit_transition_recovers_from_preserved_journal_name() {
        let (_temp, base, txn, artifacts) = setup();
        let archive = base.child("codex", false).unwrap();
        let backups = txn.child("backups", true).unwrap();
        for (index, path) in ["a", "b"].iter().enumerate() {
            copy_backup(archive.read(path).unwrap(), &backups, index, None).unwrap();
        }
        backups.sync().unwrap();
        txn.atomic_json(
            "ready.json",
            &Journal {
                harness: "codex".into(),
                entries: vec![
                    Entry {
                        path: "a".into(),
                        old: true,
                    },
                    Entry {
                        path: "b".into(),
                        old: true,
                    },
                ],
            },
        )
        .unwrap();
        artifacts.rename("a", &archive, "a").unwrap();
        txn.rename_unsynced("ready.json", &txn, "committed")
            .unwrap();
        // An interrupted commit decision alone conveys no cleanup authority.
        assert!(recover(&base).is_err());
        assert_eq!(read(&base, "a"), b"new-a");
        assert_eq!(
            backups.file("0", false).unwrap().metadata().unwrap().len(),
            5
        );
        // Model an interrupted reverse transition in its conservative parent
        // name. The complete journal is still usable without rewriting it.
        base.rename(".transaction", &base, ".recovery").unwrap();
        recover(&base).unwrap();
        assert_eq!(read(&base, "a"), b"old-a");
        assert_eq!(read(&base, "b"), b"old-b");
    }

    #[test]
    fn precommit_cancellation_restores_all_originals_and_mtimes() {
        let (_temp, base, txn, artifacts) = setup();
        let old = base
            .child("codex", false)
            .unwrap()
            .read("a")
            .unwrap()
            .metadata()
            .unwrap();
        artifacts
            .create_new("c")
            .unwrap()
            .write_all(b"new-c")
            .unwrap();
        let cancellation = CancellationToken::new();
        let deadline = Deadline::new(30, cancellation.clone());
        let result = publish_inner(
            &base,
            &txn,
            &artifacts,
            "codex",
            &["a".into(), "c".into(), "b".into()],
            &deadline,
            |point| {
                if point == TestCheckpoint::BeforeCommit {
                    cancellation.cancel();
                }
                Ok(())
            },
        );
        assert_eq!(
            crate::error::classify(&result.unwrap_err()),
            CoreError::Cancelled
        );
        assert_eq!(read(&base, "a"), b"old-a");
        assert_eq!(read(&base, "b"), b"old-b");
        let archive = base.child("codex", false).unwrap();
        assert!(archive.optional_file("c").unwrap().is_none());
        let restored = archive.read("a").unwrap().metadata().unwrap();
        assert_eq!(
            (old.mtime(), old.mtime_nsec()),
            (restored.mtime(), restored.mtime_nsec())
        );
        assert!(secure::not_found(
            &base.child(".transaction", false).err().unwrap()
        ));
    }

    #[test]
    fn cancellation_after_durable_commit_keeps_the_committed_generation() {
        let (_temp, base, txn, artifacts) = setup();
        let cancellation = CancellationToken::new();
        let deadline = Deadline::new(30, cancellation.clone());
        publish_inner(
            &base,
            &txn,
            &artifacts,
            "codex",
            &["a".into(), "b".into()],
            &deadline,
            |point| {
                if point == TestCheckpoint::Committed {
                    cancellation.cancel();
                }
                Ok(())
            },
        )
        .unwrap();
        assert!(cancellation.is_cancelled());
        assert_eq!(read(&base, "a"), b"new-a");
        assert_eq!(read(&base, "b"), b"new-b");
        assert!(secure::not_found(
            &base.child(".transaction", false).err().unwrap()
        ));
    }

    #[test]
    fn failed_rollback_outranks_cancellation_and_preserves_retryable_evidence() {
        let (_temp, base, txn, artifacts) = setup();
        let archive = base.child("codex", false).unwrap();
        let cancellation = CancellationToken::new();
        let deadline = Deadline::new(30, cancellation.clone());
        let result = publish_inner(
            &base,
            &txn,
            &artifacts,
            "codex",
            &["a".into(), "b".into()],
            &deadline,
            |point| {
                if point == TestCheckpoint::BeforeCommit {
                    cancellation.cancel();
                    archive.rename("b", &archive, "blocked")?;
                    archive.child("b", true)?;
                }
                Ok(())
            },
        );
        assert_eq!(
            crate::error::classify(&result.unwrap_err()),
            CoreError::Task(TaskErrorKind::RecoveryRequired)
        );
        assert!(txn.optional_file("ready.json").unwrap().is_some());
        let backups = txn.child("backups", false).unwrap();
        assert_eq!(backups.read("0").unwrap().metadata().unwrap().len(), 5);
        assert_eq!(backups.read("1").unwrap().metadata().unwrap().len(), 5);
        assert_eq!(read(&base, "a"), b"new-a");
        assert!(recover_failed(&base).is_err());
        archive.remove_tree("b").unwrap();
        archive.rename("blocked", &archive, "b").unwrap();
        // Restoration is deliberately allowed even while the task token remains cancelled.
        recover_failed(&base).unwrap();
        assert_eq!(read(&base, "a"), b"old-a");
        assert_eq!(read(&base, "b"), b"old-b");
    }

    #[test]
    fn partial_postcommit_cleanup_retains_success_and_refuses_ambiguous_recovery() {
        let (_temp, base, txn, artifacts) = setup();
        publish_inner(
            &base,
            &txn,
            &artifacts,
            "codex",
            &["a".into(), "b".into()],
            &Deadline::new(30, CancellationToken::new()),
            |point| {
                if point == TestCheckpoint::BeforeCleanup {
                    txn.child("backups", false)?.remove("0")?;
                    bail!("synthetic cleanup failure");
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(read(&base, "a"), b"new-a");
        assert_eq!(read(&base, "b"), b"new-b");
        let journal = txn.read("committed").unwrap().metadata().unwrap();
        for _ in 0..2 {
            assert!(recover(&base).is_err());
            assert!(rollback(&base).is_err());
            assert!(txn.optional_file(".rollback-required").unwrap().is_none());
            assert!(
                txn.child("backups", false)
                    .unwrap()
                    .optional_file("0")
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                txn.child("backups", false)
                    .unwrap()
                    .read("1")
                    .unwrap()
                    .metadata()
                    .unwrap()
                    .len(),
                5
            );
            let retained = txn.read("committed").unwrap().metadata().unwrap();
            assert_eq!(
                (journal.ino(), journal.mtime(), journal.mtime_nsec()),
                (retained.ino(), retained.mtime(), retained.mtime_nsec())
            );
        }
        assert_eq!(read(&base, "a"), b"new-a");
        assert_eq!(read(&base, "b"), b"new-b");
    }
}
