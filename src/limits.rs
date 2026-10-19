use crate::error::{CoreError, TaskErrorKind};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Limits apply to each host/harness operation, including OpenCode's inventory
/// and exports together. Framing overhead has its own derived finite ceiling.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limits {
    /// Maximum bytes in one artifact (including OpenCode inventory)
    pub max_file_bytes: u64,
    /// Maximum artifacts, counting OpenCode inventory plus exports
    pub max_files: u64,
    /// Maximum combined payload bytes per host/harness
    pub max_total_bytes: u64,
    /// Timeout in seconds for the entire host/harness operation
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
    pub(crate) fn wire_limit(&self) -> Result<u64> {
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
        self.files = self
            .files
            .checked_add(1)
            .context(CoreError::Task(TaskErrorKind::LimitExceeded))?;
        self.bytes = self
            .bytes
            .checked_add(size)
            .context(CoreError::Task(TaskErrorKind::LimitExceeded))?;
        if size > limits.max_file_bytes
            || self.files > limits.max_files
            || self.bytes > limits.max_total_bytes
        {
            return Err(CoreError::Task(TaskErrorKind::LimitExceeded).into());
        }
        Ok(())
    }
    pub fn ensure_exports(&self, count: usize, limits: &Limits) -> Result<()> {
        if self
            .files
            .checked_add(count as u64)
            .context(CoreError::Task(TaskErrorKind::LimitExceeded))?
            > limits.max_files
        {
            return Err(CoreError::Task(TaskErrorKind::LimitExceeded).into());
        }
        Ok(())
    }
}
