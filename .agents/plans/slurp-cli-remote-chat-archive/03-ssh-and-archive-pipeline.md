# SSH and archive pipeline

## Goal and prerequisite state

Build a cancellable, bounded, secure path from a configured OpenSSH destination to a cumulative local mirror. Configuration models and CLI dependency boundaries must exist.

## Exact change surface

- `internal/ssh/runner.go` — **new**, under new parent `internal/ssh/`. Define proposed `Runner` and `CommandResult` abstractions around `exec.CommandContext`.
- `internal/ssh/runner_test.go` — **new**. Use a fake `ssh` executable to record argv, stream fixtures, block until cancellation, and emit arbitrary secret-bearing stderr for suppression tests.
- `internal/protocol/protocol.go` — **new**, under new parent `internal/protocol/`. Define a versioned metadata envelope and payload framing independent of terminal text.
- `internal/protocol/protocol_test.go` — **new**. Fuzz malformed lengths, truncation, duplicate records, and invalid UTF-8 metadata.
- `internal/archive/store.go` — **new**, under new parent `internal/archive/`. Define proposed `Store.BeginHostHarness`, `Batch.Put`, `Batch.Commit`, and `Batch.Abort`.
- `internal/archive/manifest.go` — **new**. Define versioned state entries containing relative path, SHA-256, byte count, source metadata, and last successful collection time.
- `internal/archive/store_test.go` and `internal/archive/fuzz_test.go` — **new**. Test containment, atomicity, permissions, collisions, cancellation, and manifest recovery.
- `internal/collect/coordinator.go` — **new**, under new parent `internal/collect/`. Define proposed `Coordinator.Run(ctx, Selection) Report` with a semaphore and deterministic report ordering.
- `internal/collect/report.go` — **new**. Define statuses `collected`, `unchanged`, `not-found`, and `failed` plus redacted output formatting. “Installed but source location unresolved” is a typed failed reason, not a successful absence.
- `internal/collect/coordinator_test.go` — **new**. Test concurrency limits, cancellation, continuation, and aggregate status.

All files and symbols are proposed.

## SSH process contract

- Resolve `ssh` with `exec.LookPath` once per command invocation and fail before scheduling hosts if absent.
- Use argv equivalent to `ssh -o BatchMode=yes -o ConnectTimeout=15 <destination> "sh -s -- 1"`. The remote command is a fixed ASCII string and contains no user-controlled values.
- Do not invoke a local shell.
- Generate the embedded POSIX shell collector program locally and send it over stdin. Embed source overrides only as POSIX single-quoted assignment literals using one centralized encoder that replaces every `'` with `'"'"'`; reject control characters before encoding. The script never calls `eval`, never sources a profile/config, and never places a config value in executable syntax. Test this encoder independently and through a real SSH server.
- Capture stdout only as the expected framed data protocol or single OpenCode JSON response. Drain stderr into a counting discard sink; never retain or render its bytes because a remote process can write transcript content or secrets there.
- Inherit OpenSSH config and host-key policy. Do not add `StrictHostKeyChecking=no`, password helpers, private-key paths, or connection-control defaults.
- On context cancellation, terminate the process group, allow a short bounded grace interval, then kill remaining children and wait. Never leave an `ssh` child running.
- Apply a 30-minute task deadline by default and allow `--operation-timeout 0` to disable it. A timeout fails that host/harness, releases its lock, and allows independent tasks to finish.
- Classify exit 255 as transport/authentication failure. Collector-specific exit/status records classify missing prerequisites and read/export failures.

## Remote protocol

Use one binary-safe version-1 stream so filenames and content cannot be confused with diagnostics. Do not use tar. Headers are fixed record tags followed by NUL-delimited UTF-8 harness/path fields and overflow-checked base-10 byte lengths; content is read by its declared length, so NUL and newline bytes in content are opaque:

```text
magic + protocol version
host probe metadata record
zero or more artifact records:
    harness, logical relative path, declared byte length, source cksum/byte count
    exact content bytes
terminal status record per selected harness
end record
```

- Cap metadata at 64 KiB per record, relative paths at 16 KiB, OpenCode database-query stdout at 64 MiB, one artifact at `--max-file-bytes` (2 GiB default), one batch at `--max-files` (1,000,000 default), and batch bytes at `--max-total-bytes` (100 GiB default). Count with checked unsigned arithmetic before allocation or disk writes.
- Stream bodies; never load a transcript or complete batch into memory.
- Reject unknown protocol versions, oversized fields, invalid status transitions, artifacts after terminal status, missing end records, and trailing bytes.
- The remote script emits only regular-file bodies. It does not emit symlinks, devices, sockets, FIFOs, credentials, or general config trees.
- For filesystem collectors, capture POSIX `cksum` plus byte count before and after emission. If either changes, emit failure and do not commit that harness manifest. A growth/shrink that violates framing is also a protocol failure.
- OpenCode does not use a multi-artifact framed body. Enumerate IDs in one capped JSON command, then give each export its own SSH process whose stdout is exactly one artifact and whose EOF terminates that artifact. Stream it into a capped local temporary file, validate one complete JSON document, then commit through the same archive batch. This avoids buffering in memory and leaves no sensitive remote temporary file.

## Archive layout

```text
<data-root>/
├── hosts/
│   └── <host-name>/
│       ├── claude/<native-relative-path>
│       ├── codex/<native-relative-path>
│       ├── opencode/sessions/<session-id>.json
│       └── pi/<native-relative-path>
├── state/<host-name>/<harness>.json
├── state/<host-name>/identity.json
├── locks/<host-name>/<harness>.lock
└── .tmp/<host-name>/<harness>/<run-id>/...
```

- Treat every remote logical path as untrusted. Clean it lexically, require a nonempty relative path, reject absolute paths and any `..` component, join beneath the harness root, and verify containment before opening.
- Reject duplicate logical paths in one batch and file/directory collisions with existing archive entries.
- Open with no-follow semantics where the platform permits. Walk each existing parent with `Lstat`; reject symlinks anywhere between the data root and target.
- Create directories with `0700` and artifacts/manifests with `0600`.
- Before reading or reporting unchanged, `Lstat` each controlled parent and target, reject symlinks/non-regular targets, and tighten broader modes. Fail if safe modes cannot be enforced.
- Bind each host archive namespace to its configured destination in owner-only `identity.json`. Re-adding the same name/destination may reuse it; if the name points to a different destination, refuse collection and require a new host name. Do not offer implicit adoption in version 1.
- Acquire an exclusive advisory lock for the host/harness before inspecting state or staging data and hold it through manifest commit/abort. Fail fast with an actionable “collection already active” error when locked. After acquiring the lock, remove stale staging children only inside that host/harness namespace; never perform global startup cleanup.
- Write a unique temporary sibling or run-staging file while hashing. Flush and atomically rename only after declared length, EOF, and source stability checks succeed.
- Report unchanged only when the incoming hash/size, prior manifest entry, and freshly verified regular final file all agree. If final bytes or mode diverge from the manifest, atomically replace or repair it even when the incoming hash equals the manifest.
- Commit a new manifest atomically only after every artifact in that harness batch succeeds. Existing completed artifact replacements can remain if a later file fails, but the prior manifest stays authoritative and the report identifies the incomplete batch. A subsequent run safely retries.
- Never delete artifacts missing from the new remote listing. Mark them absent from the latest manifest while retaining their bytes.
- Clean the current run's staging directory on success and failure. Recovery happens only while holding the matching host/harness lock and is scoped to validated children of that namespace.

## Orchestration and exit semantics

- Build deterministic tasks in host-name then harness-name order.
- Run at most `--jobs` tasks simultaneously. Serialize tasks for the same host initially to avoid multiple authentication prompts and excessive remote load.
- `not-found` is successful only when neither a supported source nor the harness executable is detected. A detected executable with no automatically resolved source is `failed` with path-override guidance. An explicitly requested harness that is genuinely absent remains `not-found`.
- Continue after isolated failures unless the root context is cancelled or local archive integrity is at risk.
- Print one line per host/harness plus totals for discovered artifacts, written artifacts, unchanged artifacts, bytes, skipped sources, and failures.
- Return success for all-success/not-found results. Return nonzero for connection, detected-source, protocol, or local-storage failures.

## Tests and acceptance

- The fake SSH executable asserts the destination is one argv element and values containing shell metacharacters are never executed locally.
- A disposable real SSH server test uses override paths containing spaces, quotes, `$()`, backticks, semicolons, leading dashes, and glob characters. Newlines/control bytes are rejected at config validation. Assert literal selection and verify a sentinel command side effect never occurs.
- Protocol fuzzing cannot panic, allocate beyond caps, hang on truncation, or write outside a temporary data root.
- Traversal, absolute, symlink, hardlink-equivalent collision, duplicate, oversized, short-body, long-body, and trailing-data fixtures fail closed.
- Inject cancellation at probe, metadata, body, flush, and rename boundaries. Assert no partial final file and no surviving child process.
- Run unchanged, appended, truncated, and remotely deleted artifact scenarios. Assert idempotency, atomic replacement, and retention semantics.
- Cover manifest/final-file divergence, remote reversion to an older hash, crash after artifact rename but before manifest rename, and disk-full retry. The next run must restore agreement between bytes and manifest.
- Run two independent CLI processes against changing fixtures. Assert the second fails fast on the namespace lock, no process removes the other's staging data, and final bytes/manifests remain consistent.
- Test exact boundary, one-over-boundary, overflow, over-count, over-total, oversized OpenCode list, and endless-stream cancellation cases for every protocol limit.
- Inject unique secret markers into remote SSH/harness stderr and assert neither stdout, stderr, returned errors, nor Fang-rendered errors contains them.
- Run multiple hosts with one failure and assert all schedulable tasks finish, report order remains deterministic, and the aggregate result is nonzero.

## Dependencies, risks, and exclusions

- Depends on configuration and CLI abstractions.
- Collector-specific discovery populates the protocol defined here.
- A local disk-full condition can leave completed atomic replacements with an older manifest. Recovery relies on content hashes, and the next run must reconcile rather than delete data.
- Do not use `scp`, unsafe tar extraction, or a live SQLite file copy.
