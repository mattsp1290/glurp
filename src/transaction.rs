//! A host lock protects one durable transaction. Backups are complete and synced
//! before ready.json becomes durable. Until committed exists, recovery rolls back
//! every entry; it is safe to repeat recovery after a second interruption.
use crate::{
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
    recover_inner(base, false)
}
/// A failed publication must never be reinterpreted as successful commit cleanup.
pub fn recover_failed(base: &Dir) -> Result<()> {
    recover_inner(base, true)
}
fn recover_inner(base: &Dir, failed: bool) -> Result<()> {
    // .recovery records a failed decision transition whose reverse rename
    // could not be completed. Its journal is always interpreted as rollback.
    let recovery = match base.child(".recovery", false) {
        Ok(dir) => Some(dir),
        Err(error) if secure::not_found(&error) => None,
        Err(error) => return Err(error),
    };
    let recovering = recovery.is_some();
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
    let rollback = failed || recovering || marked_failed;
    if !rollback
        && txn.optional_file("committed")?.is_some()
        && txn.optional_file("ready.json")?.is_none()
    {
        base.remove_tree(transaction_name)?;
        return Ok(());
    }
    if failed && !marked_failed && txn.optional_file("committed")?.is_some() {
        // Record failed-publication intent before touching a single restore
        // target. This is independent of both journal and parent renames.
        // Even a subsequent sync failure leaves the visible marker conservative.
        txn.create_new(".rollback-required")?.sync_all()?;
        txn.sync()?;
    }
    let journal_name = if rollback && txn.optional_file("ready.json")?.is_none() {
        "committed"
    } else {
        "ready.json"
    };
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
                backups.file(&index.to_string(), false)?;
            }
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

pub fn publish(
    base: &Dir,
    txn: &Dir,
    artifacts: &Dir,
    harness: &str,
    paths: &[String],
    deadline: &Deadline,
) -> Result<()> {
    publish_with(base, txn, artifacts, harness, paths, deadline, |_| Ok(()))
}
fn publish_with(
    base: &Dir,
    txn: &Dir,
    artifacts: &Dir,
    harness: &str,
    paths: &[String],
    deadline: &Deadline,
    mut checkpoint: impl FnMut(usize) -> Result<()>,
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
        for (index, change) in changes.iter().enumerate() {
            deadline.check()?;
            // Refuse a substituted nonregular target. A cooperating writer is
            // excluded by archive.lock; descriptors prevent directory redirection.
            let (target, leaf) = archive.parent(&change.path, false)?;
            unchanged(&target, &leaf, change)?;
            let (source, source_leaf) = artifacts.parent(&change.path, false)?;
            source.rename(&source_leaf, &target, &leaf)?;
            checkpoint(index + 1)?;
        }
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
        if recover_failed(base).is_err() {
            bail!("publication failed and rollback needs recovery; private backups retained");
        }
        return Err(error);
    }
    // A durable commit is authoritative. Cleanup failure leaves a committed
    // journal for the next run, and is not reported as a failed publication.
    let _ = base.remove_tree(".transaction");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
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
        let result = publish_with(
            &base,
            &txn,
            &artifacts,
            "codex",
            &paths,
            &Deadline::new(10),
            |index| {
                if index == 2 {
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
        // Model an interrupted reverse transition in its conservative parent
        // name. The complete journal is still usable without rewriting it.
        base.rename(".transaction", &base, ".recovery").unwrap();
        recover(&base).unwrap();
        assert_eq!(read(&base, "a"), b"old-a");
        assert_eq!(read(&base, "b"), b"old-b");
    }
}
