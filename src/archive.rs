//! Fully validate successful bounded streams before transaction publication.
use crate::{
    collectors,
    config::Host,
    limits::{Budget, Limits},
    remote::{self, Deadline},
    secure::{self, Dir},
    transaction,
};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

pub fn check_binding(data: &Path, host: &Host) -> Result<()> {
    let base = match Dir::open(&data.join("hosts").join(&host.name), false) {
        Ok(base) => base,
        Err(error) if secure::not_found(&error) => return Ok(()),
        Err(error) => return Err(error),
    };
    verify_binding(&base, host)
}
fn verify_binding(base: &Dir, host: &Host) -> Result<()> {
    if let Some(file) = base.optional_file("destination.json")? {
        let bound: String = serde_json::from_reader(file.take(4096))?;
        if bound != host.destination {
            bail!("archive name is bound to a different destination; choose a new host name");
        }
    } else if base
        .entries()?
        .iter()
        .any(|name| matches!(name.as_str(), "claude" | "codex" | "pi" | "opencode"))
    {
        bail!("archive destination binding is missing; repair storage before collection");
    }
    Ok(())
}
fn token(reader: &mut impl BufRead) -> Result<String> {
    let mut bytes = Vec::new();
    let read = reader.take(4098).read_until(0, &mut bytes)?;
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
                if !reader.fill_buf()?.is_empty() {
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
        let size = raw_size.parse::<u64>().context("artifact size overflow")?;
        budget.add(size, limits)?;
        let (parent, leaf) = stage.parent(&path, true)?;
        let mut file = parent.create_new(&leaf)?;
        let mut remaining = size;
        let mut bytes = [0_u8; 64 * 1024];
        while remaining > 0 {
            deadline.check()?;
            let len = remaining.min(bytes.len() as u64) as usize;
            let count = reader.read(&mut bytes[..len])?;
            if count == 0 {
                bail!("truncated artifact payload");
            }
            std::io::Write::write_all(&mut file, &bytes[..count])?;
            remaining -= count as u64;
        }
        file.sync_all()?;
        parent.sync()?;
        paths.push(path);
    }
}
pub fn collect(data: &Path, host: &Host, harness: &str, limits: &Limits) -> Result<String> {
    host.validate()?;
    let deadline = Deadline::new(limits.timeout_seconds);
    let mut wire_left = limits.wire_limit()?;
    let script = collectors::script(host, harness)?;
    let root = Dir::open(data, true)?;
    let base = root.child("hosts", true)?.child(&host.name, true)?;
    let lock = base.file("archive.lock", true)?;
    lock.try_lock_exclusive()
        .context("archive is in use; retry later")?;
    verify_binding(&base, host)?;
    transaction::recover(&base)?;
    if base.optional_file("destination.json")?.is_none() {
        base.atomic_json("destination.json", &host.destination)?;
    }
    let txn = base.child(".transaction", true)?;
    let mut published = false;
    let result = (|| {
        let spool = txn.create_new("spool")?;
        let stream = remote::fetch(&host.destination, script, spool, &deadline, &mut wire_left)?;
        let artifacts = txn.child("artifacts", true)?;
        let mut budget = Budget::new();
        let (mut paths, found) = stage(stream, &artifacts, limits, &mut budget, &deadline)?;
        if !found {
            return Ok("not-found".into());
        }
        if harness == "opencode" {
            if paths != ["inventory.json"] {
                bail!("OpenCode inventory stream is invalid");
            }
            let ids = collectors::inventory(&artifacts, &deadline)?;
            budget.ensure_exports(ids.len(), limits)?;
            deadline.check()?;
            artifacts.remove("inventory.json")?;
            let spool = txn.create_new("exports-spool")?;
            let stream = remote::fetch(
                &host.destination,
                collectors::exports(&ids),
                spool,
                &deadline,
                &mut wire_left,
            )?;
            let (exports, found) = stage(stream, &artifacts, limits, &mut budget, &deadline)?;
            if !found {
                bail!("OpenCode became unavailable after inventory");
            }
            collectors::validate_exports(&artifacts, &ids, &exports, &deadline)?;
            paths = exports;
        }
        deadline.check()?;
        transaction::publish(&base, &txn, &artifacts, harness, &paths, &deadline)?;
        published = true;
        Ok(format!("collected {} artifacts", paths.len()))
    })();
    if published {
        return result;
    }
    // On validation failure, no ready journal exists. On publication failure,
    // recover retries a durable rollback and retains backups if it still fails.
    if transaction::recover(&base).is_err() {
        bail!(
            "archive transaction needs recovery; private backups retained, repair storage and retry"
        );
    }
    result
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
                    &Deadline::new(5)
                )
                .is_err()
            );
        }
    }
}
