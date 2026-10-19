//! In-process collection of private native chat archives over OpenSSH.
//!
//! Requires Linux or macOS and an existing Tokio runtime. See [`collect`] for
//! cancellation and worker lifecycle guarantees. The CLI uses this same driver.
//! Registry publication is deferred; use a verified Git revision dependency.
mod archive;
mod cancellation;
mod cli;
mod collection;
mod collectors;
mod config;
mod error;
mod limits;
mod remote;
mod secure;
mod transaction;
mod upgrade;

pub use collection::{
    CollectionControl, CollectionPaths, CollectionReport, CollectionRequest, Harness, HostSpec,
    InputSource, ProgressEvent, ProgressPhase, TaskReport, TaskStatus, collect, default_paths,
};
pub use error::{CollectionError, TaskErrorKind};
pub use limits::Limits;
pub use tokio_util::sync::CancellationToken;

/// Unsupported embedding glue for this package's command-line binary.
#[doc(hidden)]
pub fn cli_main() -> i32 {
    cli::main()
}
