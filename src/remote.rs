//! Nonblocking SSH pipes: no worker can hang on escaped descendants holding
//! an inherited pipe. Stderr goes directly to /dev/null, never to diagnostics.
use crate::error::{CoreError, TaskErrorKind, classify};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub struct Deadline {
    started: Instant,
    timeout: Duration,
    cancellation: CancellationToken,
    // Deterministic unit-test clock advancement belongs to this invocation.
    // Production builds always use the monotonic clock above.
    #[cfg(test)]
    expired: std::sync::atomic::AtomicBool,
}
impl Deadline {
    pub fn new(seconds: u64, cancellation: CancellationToken) -> Self {
        Self {
            started: Instant::now(),
            timeout: Duration::from_secs(seconds),
            cancellation,
            #[cfg(test)]
            expired: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn check(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(CoreError::Cancelled.into());
        }
        #[cfg(test)]
        if self.expired.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(CoreError::Task(TaskErrorKind::Timeout).into());
        }
        if self.started.elapsed() >= self.timeout {
            return Err(CoreError::Task(TaskErrorKind::Timeout).into());
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn expire_for_test(&self) {
        self.expired
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Carries reader failures separately from serde_json's syntax errors. JSON
/// decoding erases the original io::Error payload, so checking token state after
/// parsing would conflate a syntax failure with an unrelated later cancellation.
pub struct CheckedReader<'a, R> {
    reader: R,
    deadline: &'a Deadline,
    failure: Option<CoreError>,
}
impl<'a, R> CheckedReader<'a, R> {
    pub fn new(reader: R, deadline: &'a Deadline) -> Self {
        Self {
            reader,
            deadline,
            failure: None,
        }
    }
    pub fn failure(&self) -> Option<CoreError> {
        self.failure
    }
    fn failed(&mut self, cause: CoreError) -> std::io::Error {
        self.failure.get_or_insert(cause);
        std::io::Error::other(cause)
    }
}
impl<R: Read> Read for CheckedReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if let Err(error) = self.deadline.check() {
            return Err(self.failed(classify(&error)));
        }
        // The underlying reader is a local, descriptor-anchored spool file.
        // Preserve errors from that read as storage failures, independently of
        // cancellation that may arrive after it has already failed.
        match self.reader.read(bytes) {
            Ok(count) => Ok(count),
            Err(error) => {
                let cause = error
                    .get_ref()
                    .and_then(|cause| cause.downcast_ref::<CoreError>())
                    .copied()
                    .unwrap_or(CoreError::Task(TaskErrorKind::LocalStorage));
                Err(self.failed(cause))
            }
        }
    }
}
struct Process(Child);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn nonblocking(fd: i32) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error())
            .context(CoreError::Task(TaskErrorKind::Transport));
    }
    Ok(())
}
pub fn fetch(
    destination: &str,
    script: String,
    mut spool: File,
    deadline: &Deadline,
    wire_left: &mut u64,
) -> Result<File> {
    deadline.check()?;
    let mut process = Process(
        Command::new("ssh")
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
            .context(CoreError::Task(TaskErrorKind::Transport))?,
    );
    let mut input = process.0.stdin.take();
    let mut output = process
        .0
        .stdout
        .take()
        .context(CoreError::Task(TaskErrorKind::Internal))?;
    nonblocking(
        input
            .as_ref()
            .context(CoreError::Task(TaskErrorKind::Internal))?
            .as_raw_fd(),
    )?;
    nonblocking(output.as_raw_fd())?;
    let mut sent = 0;
    let mut eof = false;
    let mut status = None;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        deadline.check()?;
        if status.is_none() {
            status = process
                .0
                .try_wait()
                .context(CoreError::Task(TaskErrorKind::Transport))?;
        }
        if status.is_some_and(|s| !s.success()) {
            return Err(CoreError::Task(TaskErrorKind::Transport).into());
        }
        if eof && status.is_some() {
            break;
        }
        let mut poll = [
            libc::pollfd {
                fd: output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: input.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLOUT,
                revents: 0,
            },
        ];
        let result = unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as _, 20) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context(CoreError::Task(TaskErrorKind::Transport));
        }
        if let Some(stdin) = input.as_mut()
            && poll[1].revents != 0
        {
            match stdin.write(&script.as_bytes()[sent..]) {
                Ok(count) => sent += count,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    return Err(error).context(CoreError::Task(TaskErrorKind::Transport));
                }
            }
            if sent == script.len() {
                input.take();
            }
        }
        // At most one fixed-size read per iteration, so cancellation is observed
        // even when an adversarial producer writes without pause.
        if !eof && poll[0].revents != 0 {
            match output.read(&mut buffer) {
                Ok(0) => eof = true,
                Ok(count) => {
                    *wire_left = wire_left
                        .checked_sub(count as u64)
                        .context(CoreError::Task(TaskErrorKind::LimitExceeded))?;
                    spool
                        .write_all(&buffer[..count])
                        .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    return Err(error).context(CoreError::Task(TaskErrorKind::Transport));
                }
            }
        }
    }
    if sent != script.len() {
        return Err(CoreError::Task(TaskErrorKind::Transport).into());
    }
    spool
        .seek(SeekFrom::Start(0))
        .context(CoreError::Task(TaskErrorKind::LocalStorage))?;
    Ok(spool)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Error, ErrorKind};

    #[test]
    fn deadlines_are_isolated_and_preserve_typed_contexts() {
        let cancelled = CancellationToken::new();
        let active = CancellationToken::new();
        let first = Deadline::new(60, cancelled.clone());
        let second = Deadline::new(60, active);
        cancelled.cancel();
        let error = first
            .check()
            .context("outer synthetic private context")
            .unwrap_err();
        assert_eq!(classify(&error), CoreError::Cancelled);
        assert!(second.check().is_ok());
        assert!(Deadline::new(60, CancellationToken::new()).check().is_ok());
    }

    #[test]
    fn parent_cancellation_reaches_only_its_descendants() {
        let parent = CancellationToken::new();
        let child = parent.child_token();
        let independent = CancellationToken::new();
        let deadline = Deadline::new(60, child);
        let other = Deadline::new(60, independent);
        parent.cancel();
        assert_eq!(
            classify(&deadline.check().unwrap_err()),
            CoreError::Cancelled
        );
        assert!(other.check().is_ok());
    }

    #[test]
    fn expired_deadline_is_timeout() {
        let deadline = Deadline::new(0, CancellationToken::new());
        assert_eq!(
            classify(&deadline.check().unwrap_err()),
            CoreError::Task(TaskErrorKind::Timeout)
        );
    }

    #[test]
    fn checked_reader_captures_only_the_failure_it_encountered() {
        let token = CancellationToken::new();
        let deadline = Deadline::new(60, token.clone());
        let mut reader =
            CheckedReader::new(Cursor::new(b"synthetic-private-transcript"), &deadline);
        let mut bytes = [0; 1];
        assert_eq!(reader.read(&mut bytes).unwrap(), 1);
        assert_eq!(reader.failure(), None);
        token.cancel();
        let error = reader.read(&mut bytes).unwrap_err();
        assert_eq!(reader.failure(), Some(CoreError::Cancelled));
        assert_eq!(error.kind(), ErrorKind::Other);
        assert!(!error.to_string().contains("synthetic-private"));
    }

    #[test]
    fn checked_reader_preserves_timeout() {
        let deadline = Deadline::new(60, CancellationToken::new());
        let mut reader = CheckedReader::new(Cursor::new(b"private"), &deadline);
        deadline.expire_for_test();
        let error = reader.read(&mut [0; 1]).unwrap_err();
        assert_eq!(
            reader.failure(),
            Some(CoreError::Task(TaskErrorKind::Timeout))
        );
        assert!(!error.to_string().contains("private"));
    }

    #[test]
    fn failed_local_read_is_not_inferred_as_cancellation() {
        struct FailedRead(CancellationToken);
        impl Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                self.0.cancel();
                Err(Error::other("synthetic-private-path-and-transcript"))
            }
        }
        let token = CancellationToken::new();
        let deadline = Deadline::new(60, token.clone());
        let mut reader = CheckedReader::new(FailedRead(token), &deadline);
        let error = reader.read(&mut [0; 1]).unwrap_err();
        assert_eq!(
            reader.failure(),
            Some(CoreError::Task(TaskErrorKind::LocalStorage))
        );
        assert!(!format!("{error:?}").contains("synthetic-private"));
    }

    #[test]
    fn typed_underlying_io_error_retains_its_origin() {
        struct Failure;
        impl Read for Failure {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(Error::other(CoreError::Cancelled))
            }
        }
        let deadline = Deadline::new(60, CancellationToken::new());
        let mut reader = CheckedReader::new(Failure, &deadline);
        reader.read(&mut [0; 1]).unwrap_err();
        assert_eq!(reader.failure(), Some(CoreError::Cancelled));
        assert!(deadline.check().is_ok());
    }

    #[test]
    fn reader_keeps_the_first_observed_failure() {
        let token = CancellationToken::new();
        let deadline = Deadline::new(60, token.clone());
        let mut reader = CheckedReader::new(Cursor::new(b"private"), &deadline);
        deadline.expire_for_test();
        reader.read(&mut [0; 1]).unwrap_err();
        token.cancel();
        reader.read(&mut [0; 1]).unwrap_err();
        assert_eq!(
            reader.failure(),
            Some(CoreError::Task(TaskErrorKind::Timeout))
        );
    }

    #[test]
    fn nonblocking_setup_failure_is_transport() {
        assert_eq!(
            classify(&nonblocking(-1).unwrap_err()),
            CoreError::Task(TaskErrorKind::Transport)
        );
    }

    #[test]
    fn dropping_process_kills_and_reaps_it() {
        let child = Command::new("sh")
            .args(["-c", "exec sleep 60"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        drop(Process(child));
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}
