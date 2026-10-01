//! SSH runs a fixed POSIX script over stdin. Remote stderr is discarded.
//! Spooling bounds memory and permits validating the entire successful stream
//! before any archived transcript changes. Later collectors supply another script.
use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const MAX_STREAM_BYTES: u64 = 1024 * 1024 * 1024;
pub const OPERATION_TIMEOUT: Duration = Duration::from_secs(300);

pub fn fetch(destination: &str, script: String, mut spool: File) -> Result<File> {
    let mut child = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=15",
            destination,
            "sh -s",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("cannot start OpenSSH; install ssh and check PATH")?;
    let pid = child.id() as i32;
    let mut stdin = child.stdin.take().unwrap();
    let input = std::thread::spawn(move || stdin.write_all(script.as_bytes()));
    let mut stdout = child.stdout.take().unwrap();
    let mut output = Some(std::thread::spawn(move || -> Result<File> {
        let copied = std::io::copy(&mut stdout.by_ref().take(MAX_STREAM_BYTES + 1), &mut spool)?;
        if copied > MAX_STREAM_BYTES {
            bail!("remote archive exceeds 1 GiB stream limit");
        }
        Ok(spool)
    }));
    let mut completed = None;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => (),
            Err(_) => break Err(anyhow::anyhow!("cannot wait for ssh")),
        }
        if started.elapsed() >= OPERATION_TIMEOUT {
            break Err(anyhow::anyhow!(
                "SSH collection timed out after 300 seconds"
            ));
        }
        // Observe spool failures immediately, even if a hostile producer ignores EPIPE.
        if output.as_ref().is_some_and(|worker| worker.is_finished()) {
            completed = Some(
                output
                    .take()
                    .unwrap()
                    .join()
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("SSH output worker failed"))),
            );
            if completed.as_ref().is_some_and(|result| result.is_err()) {
                break Err(anyhow::anyhow!(
                    "SSH output exceeded limits or could not be staged"
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // Kill lingering descendants too: they may retain pipes after ssh itself exits.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.wait();
    let write_result = input
        .join()
        .map_err(|_| anyhow::anyhow!("SSH input worker failed"))?;
    let stream_result = match completed {
        Some(result) => result,
        None => output
            .unwrap()
            .join()
            .map_err(|_| anyhow::anyhow!("SSH output worker failed"))?,
    };
    let status = status?;
    if !status.success() {
        bail!(
            "SSH collection failed; check destination, authentication and remote source access (remote diagnostics suppressed)"
        );
    }
    write_result.context("cannot send collector script to ssh")?;
    let mut spool = stream_result?;
    spool.seek(SeekFrom::Start(0))?;
    Ok(spool)
}
