//! Fixed public categories deliberately exclude private engine diagnostics.
use serde::{Deserialize, Serialize};
use std::fmt;

/// A request could not be planned or its worker did not finish normally.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CollectionError {
    InvalidInput,
    InvalidConfig,
    ConfigBusy,
    MissingRuntime,
    CancelledBeforePlanning,
    WorkerFailure,
}
impl fmt::Display for CollectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "invalid collection request",
            Self::InvalidConfig => "invalid or empty stored configuration",
            Self::ConfigBusy => "configuration is in use; retry later",
            Self::MissingRuntime => "collection requires a Tokio runtime",
            Self::CancelledBeforePlanning => "collection cancelled before planning",
            Self::WorkerFailure => "collection worker failed",
        })
    }
}
impl std::error::Error for CollectionError {}

/// A redacted failure category for one planned host/harness task.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskErrorKind {
    Busy,
    Timeout,
    DestinationBinding,
    RecoveryRequired,
    LimitExceeded,
    InvalidRemoteStream,
    Transport,
    LocalStorage,
    Internal,
}
impl fmt::Display for TaskErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Busy => "archive in use; retry later",
            Self::Timeout => "operation timed out",
            Self::DestinationBinding => "archive destination binding refused",
            Self::RecoveryRequired => "local recovery required; backups retained",
            Self::LimitExceeded => "archive limit or size overflow",
            Self::InvalidRemoteStream => "invalid remote archive stream",
            Self::Transport => "SSH collection failed",
            Self::LocalStorage => "local archive storage failed",
            Self::Internal => "collection failed",
        })
    }
}
impl std::error::Error for TaskErrorKind {}

/// Private provenance marker, never exposing an underlying anyhow chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoreError {
    Cancelled,
    Task(TaskErrorKind),
}
impl fmt::Display for CoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("collection cancelled"),
            Self::Task(kind) => kind.fmt(f),
        }
    }
}
impl std::error::Error for CoreError {}

/// Recovery failures deliberately override earlier causes. Other boundaries
/// retain a typed origin instead of inspecting strings or current token state.
pub(crate) fn classify(error: &anyhow::Error) -> CoreError {
    error
        .downcast_ref::<CoreError>()
        .copied()
        .unwrap_or(CoreError::Task(TaskErrorKind::Internal))
}

/// Assign a category only when a more specific typed origin is not present.
pub(crate) fn with_kind(error: anyhow::Error, kind: TaskErrorKind) -> anyhow::Error {
    if error.downcast_ref::<CoreError>().is_some() {
        error
    } else {
        error.context(CoreError::Task(kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_errors_have_no_sensitive_sources() {
        for error in [
            CollectionError::InvalidInput,
            CollectionError::WorkerFailure,
        ] {
            assert!(std::error::Error::source(&error).is_none());
            assert!(!format!("{error:?} {error}").contains("private"));
        }
        let error = anyhow::anyhow!("private synthetic detail")
            .context(CoreError::Cancelled)
            .context(CoreError::Task(TaskErrorKind::RecoveryRequired));
        assert_eq!(
            classify(&error),
            CoreError::Task(TaskErrorKind::RecoveryRequired)
        );
    }
}
