//! Nonblocking SSH pipes: no worker can hang on escaped descendants holding
//! an inherited pipe. Stderr goes directly to /dev/null, never to diagnostics.
use anyhow::{Context, Result, bail};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static CANCELLED: AtomicBool = AtomicBool::new(false);
extern "C" fn cancel(_: libc::c_int) {
    CANCELLED.store(true, Ordering::Relaxed);
}
pub fn install_cancellation() -> Result<()> {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = cancel as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        for signal in [libc::SIGINT, libc::SIGTERM] {
            if libc::sigaction(signal, &action, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
    Ok(())
}
pub fn cancelled() -> bool {
    CANCELLED.load(Ordering::Relaxed)
}
pub struct Deadline {
    started: Instant,
    timeout: Duration,
}
impl Deadline {
    pub fn new(seconds: u64) -> Self {
        Self {
            started: Instant::now(),
            timeout: Duration::from_secs(seconds),
        }
    }
    pub fn check(&self) -> Result<()> {
        if cancelled() {
            bail!("collection cancelled");
        }
        if self.started.elapsed() >= self.timeout {
            bail!("collection operation timed out");
        }
        Ok(())
    }
}
pub struct CheckedReader<'a, R> {
    pub reader: R,
    pub deadline: &'a Deadline,
}
impl<R: Read> Read for CheckedReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.deadline.check().map_err(std::io::Error::other)?;
        self.reader.read(bytes)
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
        return Err(std::io::Error::last_os_error().into());
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
            .context("cannot start OpenSSH; install ssh and check PATH")?,
    );
    let mut input = process.0.stdin.take();
    let mut output = process.0.stdout.take().unwrap();
    nonblocking(input.as_ref().unwrap().as_raw_fd())?;
    nonblocking(output.as_raw_fd())?;
    let mut sent = 0;
    let mut eof = false;
    let mut status = None;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        deadline.check()?;
        if status.is_none() {
            status = process.0.try_wait()?;
        }
        if status.is_some_and(|s| !s.success()) {
            bail!(
                "SSH collection failed; check authentication and source access (remote diagnostics suppressed)"
            );
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
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(std::io::Error::last_os_error().into());
        }
        if let Some(stdin) = input.as_mut()
            && poll[1].revents != 0
        {
            match stdin.write(&script.as_bytes()[sent..]) {
                Ok(count) => sent += count,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => bail!("cannot send collector script to ssh"),
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
                        .context("remote stream size limit exceeded")?;
                    spool.write_all(&buffer[..count])?;
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
    if sent != script.len() {
        bail!("SSH did not accept the complete collector script");
    }
    spool.seek(SeekFrom::Start(0))?;
    Ok(spool)
}
