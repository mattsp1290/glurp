//! Owned requests and the single shared collection driver.
use crate::{
    archive::{ArchiveOutcome, HostSession},
    cancellation::Invocation,
    config,
    error::{CollectionError, CoreError, TaskErrorKind, classify},
    limits::Limits,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Native archive format to collect. An empty selection means all four.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    Claude,
    Codex,
    OpenCode,
    Pi,
}
impl Harness {
    pub(crate) const ALL: [Self; 4] = [Self::Claude, Self::Codex, Self::OpenCode, Self::Pi];
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
            Self::Pi => "pi",
        }
    }
}
impl std::fmt::Display for Harness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An owned SSH destination and optional literal per-harness source roots.
/// Validation occurs before archive admission. Debug is intentionally omitted.
#[derive(Clone, Default)]
pub struct HostSpec {
    pub name: String,
    pub destination: String,
    pub claude_paths: Vec<String>,
    pub codex_paths: Vec<String>,
    pub pi_paths: Vec<String>,
}
impl From<HostSpec> for config::Host {
    fn from(host: HostSpec) -> Self {
        Self {
            name: host.name,
            destination: host.destination,
            claude_paths: host.claude_paths,
            codex_paths: host.codex_paths,
            pi_paths: host.pi_paths,
        }
    }
}

/// Explicit inputs never open the host registry. Stored inputs always load
/// `config.json` in `config_dir`, and never mutate registered host records.
#[derive(Clone)]
pub enum InputSource {
    Explicit {
        hosts: Vec<HostSpec>,
        archive_root: PathBuf,
    },
    Stored {
        config_dir: PathBuf,
        archive_root: PathBuf,
        hosts: Vec<String>,
    },
}
/// All data is owned; no caller borrows survive into the blocking worker.
#[derive(Clone)]
pub struct CollectionRequest {
    pub source: InputSource,
    pub harnesses: Vec<Harness>,
    pub limits: Limits,
}

/// Optional best-effort progress. Full or closed channels never block work.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProgressEvent {
    pub host: String,
    pub harness: Harness,
    pub phase: ProgressPhase,
    pub artifact_count: Option<u64>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressPhase {
    Starting,
    Finished,
}

/// Cancelling the parent token cancels its calls. Each invocation derives its
/// own child: dropping one call never cancels siblings or the parent token.
#[derive(Clone, Default)]
pub struct CollectionControl {
    pub cancellation: CancellationToken,
    pub progress: Option<mpsc::Sender<ProgressEvent>>,
    #[cfg(test)]
    pub(crate) test_hooks: Option<std::sync::Arc<crate::archive::TestHooks>>,
    #[cfg(test)]
    pub(crate) planning_hook: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    #[cfg(test)]
    pub(crate) panic_worker: bool,
}

/// A task's native outcome. Successful archive paths may contain non-UTF-8
/// Unix bytes; serde serialization of those paths returns an error.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TaskStatus {
    Collected {
        artifact_count: u64,
        archive_dir: PathBuf,
    },
    NotFound,
    Busy,
    Failed {
        kind: TaskErrorKind,
    },
    Cancelled,
    NotStartedDueToCancellation,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskReport {
    pub host: String,
    pub harness: Harness,
    pub status: TaskStatus,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CollectionReport {
    pub tasks: Vec<TaskReport>,
}
impl CollectionReport {
    /// True only when every selected task collected or found no source.
    pub fn is_success(&self) -> bool {
        self.tasks.iter().all(|task| {
            matches!(
                task.status,
                TaskStatus::Collected { .. } | TaskStatus::NotFound
            )
        })
    }
}

/// Explicit paths resolved from HOME/XDG only when requested by the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectionPaths {
    pub config_dir: PathBuf,
    pub archive_root: PathBuf,
}
pub fn default_paths() -> Result<CollectionPaths, CollectionError> {
    let paths = config::Paths::resolve().map_err(|_| CollectionError::InvalidInput)?;
    Ok(CollectionPaths {
        config_dir: paths
            .config
            .parent()
            .ok_or(CollectionError::InvalidInput)?
            .to_owned(),
        archive_root: paths.data,
    })
}

/// Collect a finite sequential batch on Tokio's blocking pool.
///
/// The caller must enter a Tokio runtime. Cancelling and awaiting this future
/// waits for process reaping and transaction cleanup. Dropping the future only
/// requests cancellation; the worker retains its host lease through cleanup.
/// Rollback and kernel-stalled filesystem operations have no hard deadline.
/// No signal handlers, panic hooks, environment variables, or stdout/stderr are
/// changed by this API. Unexpected worker panics still invoke the caller's hook.
///
/// ```no_run
/// use glurp::{collect, CollectionControl, CollectionRequest, Harness, HostSpec,
///     InputSource, Limits};
/// # async fn example() -> Result<(), glurp::CollectionError> {
/// let control = CollectionControl::default();
/// let cancellation = control.cancellation.clone();
/// let request = CollectionRequest {
///     source: InputSource::Explicit {
///         hosts: vec![HostSpec { name: "buildbox".into(),
///             destination: "buildbox".into(), ..Default::default() }],
///         archive_root: "/absolute/private/archive".into(),
///     },
///     harnesses: vec![Harness::Codex],
///     limits: Limits::default(),
/// };
/// // Another task can request cancellation.cancel(); still await this call.
/// let report = collect(request, control).await?;
/// assert!(report.is_success()); // An application should handle partial failure.
/// # Ok(()) }
/// ```
pub async fn collect(
    request: CollectionRequest,
    control: CollectionControl,
) -> Result<CollectionReport, CollectionError> {
    let runtime =
        tokio::runtime::Handle::try_current().map_err(|_| CollectionError::MissingRuntime)?;
    let invocation = Invocation::new(&control.cancellation);
    if invocation.token.is_cancelled() {
        return Err(CollectionError::CancelledBeforePlanning);
    }
    let token = invocation.token.clone();
    let worker = runtime.spawn_blocking(move || collect_blocking(request, control, token));
    let result = worker.await.map_err(|_| CollectionError::WorkerFailure);
    invocation.completed();
    result?
}

fn valid_absolute(path: &Path) -> bool {
    crate::secure::valid_absolute_path(path)
}
fn check_cancel(token: &CancellationToken) -> Result<(), CollectionError> {
    if token.is_cancelled() {
        Err(CollectionError::CancelledBeforePlanning)
    } else {
        Ok(())
    }
}
fn plan(
    source: InputSource,
    token: &CancellationToken,
) -> Result<(PathBuf, Vec<config::Host>), CollectionError> {
    check_cancel(token)?;
    let (root, mut hosts) = match source {
        InputSource::Explicit {
            hosts,
            archive_root,
        } => {
            if hosts.is_empty() || !valid_absolute(&archive_root) {
                return Err(CollectionError::InvalidInput);
            }
            let hosts: Vec<config::Host> = hosts.into_iter().map(Into::into).collect();
            let mut aliases = HashSet::new();
            for host in &hosts {
                check_cancel(token)?;
                host.validate().map_err(|_| CollectionError::InvalidInput)?;
                if !aliases.insert(&host.name) {
                    return Err(CollectionError::InvalidInput);
                }
            }
            (archive_root, hosts)
        }
        InputSource::Stored {
            config_dir,
            archive_root,
            hosts,
        } => {
            if !valid_absolute(&config_dir) || !valid_absolute(&archive_root) {
                return Err(CollectionError::InvalidInput);
            }
            let store = config::Store::open_for_collection(&config_dir.join("config.json"), token)
                .map_err(|error| {
                    error
                        .downcast_ref::<CollectionError>()
                        .copied()
                        .unwrap_or(CollectionError::InvalidConfig)
                })?;
            check_cancel(token)?;
            let hosts = store
                .select(&hosts)
                .map_err(|_| CollectionError::InvalidConfig)?;
            // Config lock is dropped before admission and before any SSH.
            (archive_root, hosts)
        }
    };
    hosts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((root, hosts))
}
fn emit(
    control: &CollectionControl,
    host: &config::Host,
    harness: Harness,
    phase: ProgressPhase,
    status: Option<&TaskStatus>,
) {
    if let Some(sender) = &control.progress {
        let artifact_count = match status {
            Some(TaskStatus::Collected { artifact_count, .. }) => Some(*artifact_count),
            _ => None,
        };
        let _ = sender.try_send(ProgressEvent {
            host: host.name.clone(),
            harness,
            phase,
            artifact_count,
        });
    }
}
fn failure(error: &anyhow::Error) -> TaskStatus {
    match classify(error) {
        CoreError::Cancelled => TaskStatus::Cancelled,
        CoreError::Task(TaskErrorKind::Busy) => TaskStatus::Busy,
        CoreError::Task(kind) => TaskStatus::Failed { kind },
    }
}
fn collect_blocking(
    request: CollectionRequest,
    control: CollectionControl,
    token: CancellationToken,
) -> Result<CollectionReport, CollectionError> {
    check_cancel(&token)?;
    #[cfg(test)]
    if control.panic_worker {
        panic!("synthetic collection worker panic");
    }
    #[cfg(test)]
    if let Some(hook) = &control.planning_hook {
        hook();
    }
    check_cancel(&token)?;
    request
        .limits
        .wire_limit()
        .map_err(|_| CollectionError::InvalidInput)?;
    let (root, hosts) = plan(request.source, &token)?;
    let mut harnesses = request.harnesses;
    if harnesses.is_empty() {
        harnesses.extend(Harness::ALL);
    }
    harnesses.sort();
    harnesses.dedup();
    let mut report = CollectionReport { tasks: Vec::new() };
    for host in hosts {
        // The filesystem lease is the authority at worker admission, not at
        // future creation. Never reserve unrelated hosts ahead of their work.
        let session = if token.is_cancelled() {
            None
        } else {
            Some(HostSession::open(&root, &host))
        };
        let mut recovery_required = matches!(&session, Some(Err(error))
            if classify(error) == CoreError::Task(TaskErrorKind::RecoveryRequired));
        for &harness in &harnesses {
            // A successful commit can leave ambiguous evidence when cleanup
            // fails. Inspect it while retaining this host's lease, even if
            // cancellation would otherwise skip every remaining harness.
            if !recovery_required
                && let Some(Ok(session)) = &session
                && session.ensure_ready().is_err()
            {
                recovery_required = true;
            }
            let status = if recovery_required {
                TaskStatus::Failed {
                    kind: TaskErrorKind::RecoveryRequired,
                }
            } else if token.is_cancelled() {
                TaskStatus::NotStartedDueToCancellation
            } else if let Some(Err(error)) = &session {
                failure(error)
            } else if let Some(Ok(session)) = &session {
                emit(&control, &host, harness, ProgressPhase::Starting, None);
                #[cfg(test)]
                let outcome = if let Some(hooks) = &control.test_hooks {
                    session.collect_with_hooks(harness, &request.limits, token.clone(), hooks)
                } else {
                    session.collect(harness, &request.limits, token.clone())
                };
                #[cfg(not(test))]
                let outcome = session.collect(harness, &request.limits, token.clone());
                match outcome {
                    Ok(ArchiveOutcome::Collected { artifact_count }) => TaskStatus::Collected {
                        artifact_count,
                        archive_dir: root.join("hosts").join(&host.name).join(harness.as_str()),
                    },
                    Ok(ArchiveOutcome::NotFound) => TaskStatus::NotFound,
                    Err(error) => failure(&error),
                }
            } else {
                TaskStatus::NotStartedDueToCancellation
            };
            recovery_required |= matches!(
                status,
                TaskStatus::Failed {
                    kind: TaskErrorKind::RecoveryRequired
                }
            );
            emit(
                &control,
                &host,
                harness,
                ProgressPhase::Finished,
                Some(&status),
            );
            report.tasks.push(TaskReport {
                host: host.name.clone(),
                harness,
                status,
            });
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests;
