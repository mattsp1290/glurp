# Rust collection API and archive interfaces

The Rust library and CLI collect Claude Code, Codex, pi, and OpenCode through
the same engine. Production code, builds, and CI use Rust and OpenSSH.
Historical plans under `.agents/plans/` are provenance only; they do not describe the current CLI or build contract.

## Supported public API

The package exports collection only. Import `glurp` by an exact Git revision;
see the [dependency and executable example](../README.md#use-as-a-rust-library).
Registry publication is deferred. OpenSSH remains an external dependency on
Linux/macOS, but no installed Glurp executable, HTTP service, or host-server
integration is required. The internal modules are private. The hidden CLI
entrypoint is unsupported package glue, not an embedding API; it alone handles
arguments, output, and signals. Only the binary entrypoint exits the process.

The entrypoints are:

```rust,ignore
pub async fn collect(
    request: CollectionRequest,
    control: CollectionControl,
) -> Result<CollectionReport, CollectionError>;

pub fn default_paths() -> Result<CollectionPaths, CollectionError>;
```

### Owned inputs

`CollectionRequest { source, harnesses, limits }` owns all input data:

- `InputSource::Explicit { hosts: Vec<HostSpec>, archive_root: PathBuf }`
  requires at least one host and an absolute local archive root. A
  `HostSpec { name, destination, claude_paths, codex_paths, pi_paths }` owns its
  strings and source-root vectors. Empty source-root vectors retain automatic
  discovery. This mode never opens or changes the host registry and works
  without `HOME`.
- `InputSource::Stored { config_dir: PathBuf, archive_root: PathBuf,
  hosts: Vec<String> }` requires explicit absolute directories. It reads
  `config_dir/config.json`; a custom filename is not accepted. An empty alias
  list selects every registered host. The loader may create the private config
  directory and `config.lock` but never changes registered host records. The
  snapshot lock is released before SSH begins. Missing/empty/invalid config,
  unknown aliases, and lock contention are request errors.
- `harnesses: Vec<Harness>` selects `Claude`, `Codex`, `OpenCode`, and/or `Pi`.
  Empty selects all four. Hosts and harnesses are sorted and deduplicated into a
  deterministic plan; duplicate explicit host aliases are rejected.
- `Limits { max_file_bytes, max_files, max_total_bytes, timeout_seconds }` applies
  separately to each host/harness, not to the entire batch. Defaults are 256 MiB,
  100000 artifacts, 1 GiB, and 300 seconds. Zero byte/count limits allow only
  empty payloads/no artifacts, rather than disabling limits. Timeout must be
  positive and derived budget arithmetic must not overflow. OpenCode inventory
  and exports share the task's limits; inventory consumes budget even though it
  is not archived or included in the reported artifact count.

Validation rejects bad aliases, duplicate aliases, destinations, source roots,
local roots, and limits before SSH or archive transaction creation. Source-root
rules are the same as CLI rules below. Direct inputs and explicit stored paths
need no environment-based discovery. The optional `default_paths()` helper
reads existing `HOME`/XDG rules and returns
`CollectionPaths { config_dir, archive_root }`; it does not mutate the environment.

`CollectionControl { cancellation: CancellationToken,
progress: Option<tokio::sync::mpsc::Sender<ProgressEvent>> }` supplies control.
`CancellationToken` is re-exported for callers. `collect` requires an existing
Tokio runtime and does not start a nested or hidden runtime. The complete finite
batch, including configuration and file locks, executes on Tokio's blocking
pool so collection does not block a current-thread executor's other async tasks.
The application owns admission, scheduling, and runtime shutdown.

### Results and diagnostics

A `CollectionReport { tasks: Vec<TaskReport> }` contains a `TaskReport` for every
planned host/harness in deterministic order. Each result identifies the validated host
alias and typed harness. `TaskStatus` distinguishes:

| Status | Meaning |
| --- | --- |
| `Collected { artifact_count, archive_dir }` | Validated task artifacts were durably committed; the local harness archive directory is returned. |
| `NotFound` | Automatic discovery found no harness data; no archive directory is promised. |
| `Busy` | The archive host lease was already held at admission. |
| `Failed { kind }` | A fixed `TaskErrorKind` describes the failure. |
| `Cancelled` | The active task was cancelled before durable commit and cleanup completed. |
| `NotStartedDueToCancellation` | The planned task never began because this call was cancelled. |

`artifact_count` includes unchanged artifacts present in this collection,
excludes the OpenCode inventory, and excludes older retained artifacts absent
from this run. Per-artifact names and contents are not returned. `Ok(report)`
means a report is available, not that every task succeeded. Only reports with
all results `Collected` or `NotFound` are successful; `report.is_success()`
checks this condition. Ordinary task failure and
Busy do not prevent independent tasks from running. Cancellation preserves
finished results and explicitly marks remaining planned tasks.

`TaskErrorKind` has `Busy`, `Timeout`, `DestinationBinding`, `RecoveryRequired`,
`LimitExceeded`, `InvalidRemoteStream`, `Transport`, `LocalStorage`, and
`Internal`. An unknown internal failure uses the fixed generic `Internal`
category. The normal lease-contention result is the dedicated `Busy` status.
Request-level `CollectionError` has `InvalidInput`, `InvalidConfig`,
`ConfigBusy`, `MissingRuntime`, `CancelledBeforePlanning`, and `WorkerFailure`.
Before selection produces a plan, cancellation returns `CancelledBeforePlanning`
without inventing task identities. After planning, the final report contains
all tasks. Error enums are non-exhaustive; downstream matches should keep a
fallback arm.

Public errors have fixed `Display`, `Debug`, and serialized representations
and no raw internal source chain. Request and task error categories serialize
in snake_case. `TaskStatus` uses an internally tagged `status` field with
snake_case variant names; `Harness` serializes as `claude`, `codex`, `opencode`,
or `pi`. Ordinary library execution produces no stdout/stderr and never
writes environment variables, installs signal handlers, changes a panic hook,
or exits the application. The application controls all logging. Final successful
task results may contain a caller-selected local archive path; protect reports
according to that context.

`ProgressEvent { host, harness, phase, artifact_count }` contains only task
identity, `ProgressPhase::Starting` or `Finished`, and an optional coarse count.
Progress never includes remote filenames, transcripts, SSH stderr, or parser
excerpts. The worker only attempts nonblocking sends on a caller-created bounded
channel. Full/disconnected receivers can lose events, including terminal events;
this never blocks or fails collection or cancels a call. Use the final report
for authoritative results.

### Cancellation, leases, and runtime ownership

Every call uses a private child of the supplied cancellation token. Independent
caller tokens isolate requests; cancelling a shared parent intentionally stops
its descendants. Dropping one call does not cancel siblings or the parent. An
unpolled future performs no work. A dropped, already-polled future requests
cancellation but its worker can remain alive while terminating SSH and finishing
cleanup. A queued worker checks cancellation before side effects when it starts.
Aborting a Tokio blocking task does not stop a worker already running.

Call `cancellation.cancel()` and keep awaiting the collection future when you
need a cleanup boundary. The future waits for worker completion rather than
returning immediately on cancellation. Cancel and await retained operations
before runtime shutdown. Runtime destruction and future Drop are not bounded
cleanup APIs. Rollback deliberately ignores cancellation/deadline expiration;
kernel-stalled filesystem operations and recovery have no hard wall-clock bound.
Catastrophic panic/abort can leave recoverable transaction evidence. Unexpected
worker panics still invoke the application's current panic hook before a
sanitized `WorkerFailure` is returned; arbitrary host/dependency hook output
cannot be suppressed by the library.

One filesystem-backed lease covers `(archive_root, host alias)` through all
selected harnesses and cleanup. The same lease coordinates library instances,
CLI collection, and explicit CLI rollback. `Busy` is evaluated when a worker
admits host work, not when a future is created or queued. Different roots or
aliases are distinct archives even if they use the same SSH destination. No
reservation, automatic wait, retry, or coalescing is performed. Destination
binding still rejects reuse of an archive alias for another destination.

Recovery readiness is checked before each harness. A task whose durable commit
finished remains `Collected` even if cancellation arrives or later cleanup fails.
If cleanup leaves ambiguous evidence, remaining tasks for that host become
`Failed { kind: RecoveryRequired }` without new SSH or staging reuse; independent
hosts can still run. Failed rollback also returns `RecoveryRequired` ahead of
cancellation and preserves evidence. Full transaction details are in
[rust-safety.md](rust-safety.md).

## Internal archive and transport contract

Configuration is a JSON array of host objects (`name`, `destination`, and optional `claude_paths`,
`codex_paths`, `pi_paths` arrays) at
`$XDG_CONFIG_HOME/glurp/config.json`, falling back to `$HOME/.config/glurp`.
Archives live at `$XDG_DATA_HOME/glurp/hosts/<name>/<harness>/<relative artifact>`,
falling back to `$HOME/.local/share/glurp`. Host removal retains archives. A
`destination.json` beside the harness directories binds an archive name to its
SSH destination. Config snapshots and host archive sessions use separate
exclusive locks.
All managed directories are 0700 and files are 0600. Symlinked or untrusted
parents, nonregular files and hardlinked managed files are rejected.

`remote::fetch` invokes OpenSSH with BatchMode=yes and ConnectTimeout=15 and
sends a fixed POSIX sh script on nonblocking stdin. It suppresses remote stderr
and spools a bounded stream to a private descriptor-relative file. A shared task
deadline and wire budget cover both OpenCode operations. Timeout and cancellation
terminate the SSH process group and close inherited pipes without blocking
pipe-worker joins. The public collection future still awaits its owning blocking
worker. Other harnesses implement fixed scripts or rigorously quoted literal arguments
without evaluating configured paths. See [rust-safety.md](rust-safety.md) for CLI
limits, durable recovery, and filesystem guarantees.

Wire format version 1 is `GLURP1 NUL`, followed by zero or more artifacts:
`F NUL relative-path NUL decimal-byte-count NUL exact-payload`, then exactly one `T NUL ok|not-found NUL`, followed by `E NUL`
and EOF. No newline separators or text transformations occur. Paths must be
UTF-8, at most 4096 bytes, relative, and contain no empty, dot, parent, newline,
carriage-return or backslash components. Sizes are unsigned decimal, bounded by configurable
file/count/total budgets (defaults: 256 MiB per artifact, 1 GiB total payload,
100000 artifacts). Duplicate paths,
unknown frames, truncation, overflow and trailing bytes fail validation.

The internal staging parser validates a complete stream into private anchored
storage and returns artifact paths and a found/not-found status. Collection
publishes only after SSH succeeds and staging passes. Unchanged artifacts
preserve exact bytes and mtimes; missing remote artifacts remain. Full-batch
preflight, synced backups, and a durable bounded journal permit transaction-wide
rollback and recovery on the next run. All local filesystem operations use
open directory descriptors and no-follow traversal. Automatic absent harnesses
return not-found success; configured missing roots or detected executables with
unresolved source data fail with redacted guidance.

Configured roots are literal absolute paths or exact `~/` prefixes; only that
prefix expands to the remote HOME. No shell expansion or evaluation occurs.
Explicit roots override environment/default discovery and use stable
`root-0001/`, `root-0002/` namespaces even with one root. Claude roots contain
`projects/`; Codex roots contain `sessions/` and/or `archived_sessions/`; pi roots
are session directories themselves. Remote ancestor symlinks are rejected and
find does not follow descendant symlinks.

OpenCode uses two SSH operations: a global database inventory, validated locally,
then a single batch of exports. `opencode db path` must resolve to an existing,
readable nonsymlink database before the global SQL query runs. Inventory rows
must contain exactly one unique safe `ses_` ID; each export must be one JSON
object with matching `info.id` and an array of messages (each having object info
and array parts). Every export validates before any export is published.
Inventory is an internal staging artifact and is never archived. Transcript
bytes are preserved; these structural checks do not normalize or reserialize.

## Verification gates

Run `make check` from a clean, committed implementation tree. It includes
formatting, Clippy, library/integration/example targets, explicit doctests, the
locked standalone path consumer, release build, and installer tests, followed
by exact-commit Git dependency verification. The standalone fixture under
`tests/consumer/` has its own workspace and lockfile; it imports only the public
API and runs synthetic collection without a Glurp executable on its fixture PATH.
`distribution/tests/check_git_consumer.py --repository <URL-or-path>
--revision <full-commit>` tests the same fixture from a temporary exact Git pin,
without hidden path overrides, and records the tested revision. A local Git
proof does not establish availability on GitHub; repeat against the remote
revision after an authorized push before documenting an adoptable pin.

Linux CI runs the full dynamically linked suite, including supported filesystem
fault injection. The existing release matrix remains Linux x86_64/aarch64 and
macOS aarch64/x86_64, with library and binary unit tests, portable library
integration cases, upgrade flow, actual-collector smoke, and installer tests.
Portable library entry-point tests use `portable_`; Linux-specific fault cases
remain separate. Workflow definitions are not evidence of platform execution;
report which exact revisions and platforms actually passed. No publication,
release tag, or workflow dispatch is needed for local library verification.
