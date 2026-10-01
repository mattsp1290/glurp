//! Archive interface: collect successful streams into staging, then publish.
//! Artifact names are untrusted relative UTF-8 paths, independent of remote roots.
use crate::{config::Host, remote, secure};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILES: usize = 100_000;

pub fn check_binding(data: &Path, host: &Host) -> Result<()> {
    // No archive yet: host registration must not create transcript directories.
    let base = data.join("hosts").join(&host.name);
    if fs::symlink_metadata(&base).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(());
    }
    secure::directory(&base)?;
    let path = base.join("destination.json");
    if path.exists() || fs::symlink_metadata(&path).is_ok() {
        let bound: String = serde_json::from_reader(secure::file(&path, false)?)?;
        if bound != host.destination {
            bail!("archive name is bound to a different destination; choose a new host name");
        }
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

pub fn stage(reader: impl Read, stage: &Path) -> Result<Vec<String>> {
    let mut reader = BufReader::new(reader);
    if token(&mut reader)? != "GLURP1" {
        bail!("unsupported archive protocol");
    }
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    let mut total = 0_u64;
    loop {
        match token(&mut reader)?.as_str() {
            "E" => {
                if !reader.fill_buf()?.is_empty() {
                    bail!("unexpected bytes after archive end");
                }
                return Ok(paths);
            }
            "F" => (),
            _ => bail!("unknown archive frame"),
        }
        let path = token(&mut reader)?;
        secure::relative(&path)?;
        if !seen.insert(path.clone()) || paths.len() >= MAX_FILES {
            bail!("duplicate artifact or file count limit exceeded");
        }
        let raw_size = token(&mut reader)?;
        if raw_size.is_empty() || !raw_size.bytes().all(|b| b.is_ascii_digit()) {
            bail!("invalid artifact size");
        }
        let size = raw_size.parse::<u64>().context("artifact size overflow")?;
        total = total.checked_add(size).context("archive total overflow")?;
        if size > MAX_FILE_BYTES || total > remote::MAX_STREAM_BYTES {
            bail!("archive size limit exceeded");
        }
        let target = stage.join(&path);
        secure::directory(target.parent().unwrap())?;
        let mut file = secure::file(&target, true)?;
        let copied = std::io::copy(&mut reader.by_ref().take(size), &mut file)?;
        if copied != size {
            bail!("truncated artifact payload");
        }
        file.sync_all()?;
        paths.push(path);
    }
}

pub fn collect(data: &Path, host: &Host, harness: &str) -> Result<usize> {
    host.validate()?;
    if harness != "codex" {
        bail!("unsupported harness");
    }
    let base = data.join("hosts").join(&host.name);
    secure::directory(&base)?;
    let lock = secure::file(&base.join("archive.lock"), true)?;
    lock.try_lock_exclusive()
        .context("archive is in use; retry later")?;
    check_binding(data, host)?;
    let binding = base.join("destination.json");
    if !binding.exists() {
        let mut temp = tempfile::NamedTempFile::new_in(&base)?;
        serde_json::to_writer(&mut temp, &host.destination)?;
        temp.as_file().sync_all()?;
        temp.persist(&binding)?;
    }
    let staging = tempfile::Builder::new()
        .prefix(".stage-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(&base)?;
    let spool = tempfile::tempfile_in(staging.path())?;
    let stream = remote::fetch(&host.destination, remote::CODEX_SCRIPT, spool)?;
    let artifacts = staging.path().join("artifacts");
    secure::directory(&artifacts)?;
    let paths = stage(stream, &artifacts)?;
    let archive = base.join(harness);
    secure::directory(&archive)?;
    for path in &paths {
        let source = artifacts.join(path);
        let target = archive.join(path);
        secure::directory(target.parent().unwrap())?;
        if fs::symlink_metadata(&target).is_ok() {
            let existing = secure::file(&target, false)?;
            // Exact byte comparison keeps unchanged files and their mtimes.
            let mut a = BufReader::new(existing);
            let mut b = BufReader::new(secure::file(&source, false)?);
            let equal = loop {
                let aa = a.fill_buf()?;
                let bb = b.fill_buf()?;
                let len = aa.len().min(bb.len());
                if len == 0 {
                    break aa.is_empty() && bb.is_empty();
                }
                if aa[..len] != bb[..len] {
                    break false;
                }
                a.consume(len);
                b.consume(len);
            };
            if equal {
                continue;
            }
        }
        fs::rename(&source, &target)?;
    }
    fs::File::open(&archive)?.sync_all()?;
    Ok(paths.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hostile_frames_fail_without_publication() {
        for bytes in [
            b"GLURP1\0F\0../escape\x001\0xE\0".as_slice(),
            b"GLURP1\0F\0ok\0999999999999999999999\0",
            b"GLURP1\0F\0ok\x002\0x",
            b"GLURP1\0E\0trailing",
            b"GLURP1\0F\0ok\x000\0F\0ok\x000\0E\0",
        ] {
            let dir = tempfile::Builder::new()
                .permissions(fs::Permissions::from_mode(0o700))
                .tempdir()
                .unwrap();
            assert!(stage(bytes, dir.path()).is_err());
        }
    }
}
