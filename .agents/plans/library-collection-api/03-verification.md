# WP3: Consumer proof, regression coverage and documentation

Prerequisite: integrated WP1 and WP2. This is required acceptance, not optional follow-up.

## New and existing test surfaces

Add new `tests/library.rs` under existing tests and a new standalone consumer fixture under `tests/consumer/` (new directory, own Cargo.toml/workspace declaration). Add new `examples/collect.rs` under a new examples directory at the repository root. The example uses explicit input, cancellation and final outcomes without an HTTP dependency. Keep fixtures free of credentials and real transcripts.

Reuse the actual-script SSH shim pattern from `tests/cli.rs`, not a mocked collection engine. Test processes set HOME/PATH before launch. Inside a dedicated child process, exercise multiple actual in-process library calls. Avoid unsafe concurrent environment mutation in the test harness. CLI-vs-library equivalence must compare bytes, relative archive layout and unchanged mtimes, not just report strings. Run all four harnesses, including OpenCode inventory plus exports and multiple source roots. Keep existing filesystem-fault `tests/fs_faults.c` coverage on dynamically linked Linux.

Required observable cases:

1. Both configuration modes collect all/default and selected harnesses; invalid roots/aliases/duplicates/limits and unknown stored aliases reject before SSH. Direct input works without HOME and does not open stored config. Config snapshot lock is released during a deliberately paused SSH fixture.
2. External Rust crate imports the API without the glurp executable on PATH, performs real fixture collection and receives counts, paths and distinct Collected/NotFound/Failed outcomes. Requests are owned/Send for worker use. Pin Cargo.lock for fixture builds.
3. A paused SSH collection does not starve an independent timer on a current-thread Tokio runtime. Use a handshake and a generous finite failure timeout, not arbitrary sleep ordering.
4. Cancel one of two calls using independent tokens: only it stops; the other succeeds. Then a new call succeeds. Shared-parent behavior is intentional and separately tested. Pre-cancel and cancelled queued work launch no SSH. Missing runtime returns the documented error rather than panic.
5. Explicit cancel plus await returns only after SSH reaping and transaction restoration/evidence handling; verify child PID state and immediate subsequent lease acquisition. Preserve raw bytes/mtimes when rolling back. Test timeout and cancel during fetch, validation and precommit publication.
6. Drop a polled active future while a fixture pauses cleanup. Another call for that host observes Busy until cleanup completes, then succeeds. Drop an unpolled future and prove no side effects. Never assert immediate cleanup merely because Drop returned.
7. Hold a deterministic barrier between harnesses: another library call and a CLI process see Busy for the same host. Different hosts proceed. Release the barrier and prove retry succeeds. A queued call admitted only after release may succeed, per contract.
8. Transaction-level deterministic hooks exercise cancellation immediately before commit, after durable commit, and combined rollback failure. A committed task stays Collected; later tasks remain unstarted. RecoveryRequired outranks Cancelled and prevents further harnesses for that host. Preserve ambiguous evidence.
9. Mixed multi-host results retain completed successes and task order. Busy, ordinary failure, NotFound, cancellation and unstarted tasks are distinguishable. Count includes unchanged input artifacts, excludes inventory and excludes previously stored artifacts absent from this run.
10. Fill progress channel, stop reading it, or drop its receiver. Collection, cancellation and rollback still finish. Final results remain complete when terminal events are lost.
11. In isolated test processes, compare signal handlers before/after import and collection; assert no stdout/stderr for successful and expected-error collection paths and no process exit or environment writes from library calls. Seed synthetic sensitive strings in remote failures, JSON, filenames and worker error paths; public returned-error formatting, Debug, serialization and source iteration never expose them. Test an unexpected worker panic in an isolated subprocess with a caller-installed recording hook: the hook remains unchanged and runs before the sanitized returned WorkerFailure. Use a fixed nonsensitive panic payload and explicitly document that the library cannot suppress arbitrary host/dependency panic-hook output.
12. Keep hostile frame, symlink/hardlink, private mode, destination binding, failed publication, escaped-pipe, timeout and cumulative archive regression tests. Keep upgrade behavior and installer tests unchanged.
13. Inject partial postcommit cleanup failure in a multi-harness call. First task stays Collected; later tasks for that host are RecoveryRequired without additional SSH, staging reuse or evidence mutation. Unrelated hosts still succeed. Reuse the existing cleanup-failure fault seam in tests/cli.rs.
14. Interrupt actual reads inside both OpenCode inventory and export JSON decoding, not only surrounding deadline checks. Assert Cancelled for cancellation and Timeout for deadline expiry, while malformed JSON remains InvalidRemoteStream. Exercise typed cause propagation with deterministic read hooks and seeded private data.
15. Cancel stored-config loading before a selection plan exists: return CancelledBeforePlanning without unknown task placeholders or SSH. After selection, cancelled unstarted tasks appear in a complete report.

## Build and CI surfaces

Update existing `Makefile` test target and `.github/workflows/ci.yml` to run library, integration, example and explicit doctests. Commands from repository root:

- cargo fmt --all -- --check
- cargo clippy --all-targets --locked -- -D warnings
- cargo test --all-targets --locked
- cargo test --doc --locked
- cargo build --release --locked
- python3 -S distribution/tests/run_tests.py
- cargo test --manifest-path tests/consumer/Cargo.toml --locked (proposed fixture, path dependency for local development)

Add a proposed verification script at `distribution/tests/check_git_consumer.py` that creates a temporary standalone consumer pinned to the exact tested local Git commit, builds and runs the fixture there, with no installed Glurp executable. This proves the Cargo Git dependency boundary before remote publication. It must use clean committed implementation contents, not hidden path overrides; record the tested revision. After an authorized implementation branch is pushed, repeat with its exact GitHub revision and document that usable pin. Publication is not authorized by this planning task; lack of a remote pin prevents declaring the library remotely consumable, not plan readiness.

Extend `.github/workflows/release.yml` existing target test selector with --lib and portable library integration cases. Existing binary/unit/upgrade and actual-collector smoke coverage must not silently disappear. Keep fault injection limited to supported dynamically linked Linux tests. New library lifecycle tests should separate portable cases from platform-specific fixtures rather than blanket skipping the library suite. Preserve the existing Linux x86_64/aarch64 and macOS arm64/x86_64 release matrix; do not add routine macOS CI jobs or trigger a publication. Optional authorized workflow_dispatch is dry-run only.

## Documentation and completion

Update existing README.md, docs/rust-interfaces.md and docs/rust-safety.md. Document Linux/macOS/OpenSSH runtime requirements, the exact Git-pinned import example, both inputs, no host-server integration, typed outcomes, per-task limits, Busy identity/admission, partial failure, best-effort progress, runtime lifecycle and cooperative cancellation limits. State that cancel plus await is the cleanup boundary, not future Drop. State registry publication is deferred. No claims of native platform execution without evidence.

Acceptance requires exact-revision test results plus the external-consumer proof. CI acceptance is sufficient; real desktop testing is not a new requirement. No release tag, production SSH, real credential setup or published package is part of verification.
