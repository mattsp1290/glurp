use anyhow::{Context, Result, bail};
use clap::Args;

/// Limits apply to each host/harness operation, including OpenCode's inventory
/// and exports together. Framing overhead has its own derived finite ceiling.
#[derive(Args, Clone, Debug)]
pub struct Limits {
    /// Maximum bytes in one artifact (including OpenCode inventory)
    #[arg(long, default_value_t = 256 * 1024 * 1024)]
    pub max_file_bytes: u64,
    /// Maximum artifacts, counting OpenCode inventory plus exports
    #[arg(long, default_value_t = 100_000)]
    pub max_files: u64,
    /// Maximum combined payload bytes per host/harness
    #[arg(long, default_value_t = 1024 * 1024 * 1024)]
    pub max_total_bytes: u64,
    /// Timeout in seconds for the entire host/harness operation
    #[arg(long, default_value_t = 300)]
    pub timeout_seconds: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            max_file_bytes: 256 * 1024 * 1024,
            max_files: 100_000,
            max_total_bytes: 1024 * 1024 * 1024,
            timeout_seconds: 300,
        }
    }
}
impl Limits {
    pub fn wire_limit(&self) -> Result<u64> {
        if self.timeout_seconds == 0 {
            bail!("timeout must be at least one second");
        }
        self.max_files
            .checked_mul(4130)
            .and_then(|overhead| self.max_total_bytes.checked_add(overhead))
            .and_then(|limit| limit.checked_add(128))
            .context("configured limits overflow")
    }
}
pub struct Budget {
    pub files: u64,
    bytes: u64,
}
impl Budget {
    pub fn new() -> Self {
        Self { files: 0, bytes: 0 }
    }
    pub fn add(&mut self, size: u64, limits: &Limits) -> Result<()> {
        self.files = self.files.checked_add(1).context("file count overflow")?;
        self.bytes = self
            .bytes
            .checked_add(size)
            .context("archive total overflow")?;
        if size > limits.max_file_bytes
            || self.files > limits.max_files
            || self.bytes > limits.max_total_bytes
        {
            bail!("configured archive limit exceeded");
        }
        Ok(())
    }
    pub fn ensure_exports(&self, count: usize, limits: &Limits) -> Result<()> {
        if self
            .files
            .checked_add(count as u64)
            .context("file count overflow")?
            > limits.max_files
        {
            bail!("OpenCode inventory exceeds configured file count");
        }
        Ok(())
    }
}
