# Importable collection API

Status: Ready. Two independent reviews and one fresh adversarial review completed; accepted findings incorporated. Planning only; no implementation has occurred.

## Application context

```json
{
  "application_context": {
    "has_active_users": false,
    "backward_compatibility_required": false,
    "feature_flags": "not-applicable",
    "confirmation_digest": "0e78fb77f912ac7f725e0e8b24ca928f378d96d2d692d30907c9bcadb7a7b1f2",
    "confirmed_at": "2026-10-19T18:26:20Z"
  }
}
```

Matt explicitly answered no active users, no compatibility requirement, and no feature flags on 2026-10-19. The skill normalizes flags to not-applicable when both booleans are false. This is not permission to weaken archive safety or delete user archives. No migration, staged rollout, legacy API, or behavior flag is required.

## Outcome and boundaries

Change type: extract an in-process Rust library and a shared collection driver from the existing binary package. Another Rust repository can depend on `glurp` by Git revision and await the equivalent of `glurp glurp` without installing or spawning the Glurp executable. OpenSSH remains an external runtime dependency.

Confirmed by Matt: both explicit host/archive inputs and stored configuration; asynchronous per-invocation cancellation; Busy for overlapping same-host collection; structured per-host/harness outcomes with artifact counts, archive paths, errors and optional progress; collection only; Git-pinned dependency sufficient. Agentcraft is the prospective first consumer, not an existing integration, and its integration is explicitly outside this plan.

Success: an independent consumer crate exercises both input modes against the actual collector scripts, produces the same native archives as the CLI, and proves independent cancellation, same-host exclusion, redacted errors and nonblocking async execution. CLI collection uses the same engine.

Non-goals: HTTP server/routes, authentication, Agentcraft changes, registry publication, new repository, persistent job queue, automatic retry/coalescing, parallelism within one batch, Windows support, synchronous public API, registration/recovery library APIs, archive-format redesign, releases or deployment. No upstream request is required: Glurp owns the existing collector and can deliver independently. No Agentcraft request is filed.

## Repository evidence

Baseline: mattsp1290/glurp `6365287edadad8ec90488c3e8744d6001a12e65d`; clean before planning. Skill: mattsp1290/dotfiles `480e3cac55ce4db2a7b1210676f72a9d9a2e22a2`, `.agents/skills/implementation-plan/`. Existing historical plan directories are preserved.

- `Cargo.toml` and `src/main.rs`: binary-only package; no current public crate API. The Glurp command selects hosts, sorts harnesses, continues isolated failures and exits nonzero for failures.
- `src/config.rs`: Host validation and exclusive config snapshot lock; `Paths::resolve` reads HOME/XDG; Store uses the fixed filename config.json in the supplied parent directory.
- `src/archive.rs::collect`: per-harness host file lock, binding verification, recovery, SSH/staging, publication, cleanup; string result.
- `src/remote.rs`: process-global cancellation and signal installation; blocking transport with nonblocking pipes and process-group kill/reap.
- `src/transaction.rs`: durable rollback, preservation of ambiguous evidence; rollback intentionally ignores collection deadlines.
- `tests/cli.rs`: subprocess-isolated actual POSIX collector scripts and filesystem-fault cases. `docs/rust-safety.md` defines security invariants.
- `.github/workflows/release.yml`: release target tests explicitly select the binary and upgrade integration tests. Extraction needs explicit library coverage.

## Proposed architecture and decisions

All names below are proposed, not existing exports. One package gains `src/lib.rs`. Private core modules compile once in the library. `src/main.rs` delegates to a narrow documentation-hidden CLI driver; that driver alone parses arguments, installs signals and prints; only binary main exits the process. No feature flags.

Public flow: owned `CollectionRequest` + `CollectionControl` -> proposed async `collect` -> runtime check and private child cancellation state -> Tokio blocking worker -> validate/config snapshot -> deterministic host/harness plan -> filesystem-backed HostSession per host -> existing collector/transaction engine -> structured `CollectionReport`. Optional bounded progress accompanies work but is never authoritative.

Engineering defaults, not extra user requirements: Tokio async facade over the synchronous engine; task order sequential; overlap judged at host-work admission; drop requests cancellation but cannot await cleanup; explicit cancellation plus awaiting confirms worker completion; progress uses nonblocking bounded sends. These choices avoid rewriting transport/transactions. Runtime-neutral executors and a fully async SSH rewrite are rejected for this bounded first version.

## Risks and decisions

No unresolved product decisions. Implementation naming and exact dependency versions are non-blocking implementer decisions, constrained by the Rust 1.98.1 pin and locked build. No feature decision gates.

- Cancellation is cooperative. Dropping a future leaves cleanup running; callers requiring a cleanup boundary must cancel and await before shutting down their runtime. Kernel-stalled filesystem operations and rollback have no hard wall-clock guarantee.
- Tokio's blocking pool can queue requests. Busy means another operation holds the same archive host lease when this worker admits host work, not reservation of all requested hosts at future creation.
- Existing file locks cover `(archive root, host alias)`, not every spelling of a remote SSH server. Different roots/aliases are distinct archives. Destination binding still forbids alias reuse for a different destination.
- Unexpected panics invoke the host process panic hook; the library does not change it. Ordinary error paths remain silent, and returned worker errors remain redacted.
- Public diagnostic surfaces must not expose transcript bytes, SSH stderr, parser excerpts or raw internal error chains.
- Concurrency, durable publication and cleanup regressions are stop/go gates. Real credentials and production archives are never test fixtures.

## Document map

1. [01-library-contract.md](01-library-contract.md): public request/report, inputs, module boundary and CLI reuse.
2. [02-execution-lifecycle.md](02-execution-lifecycle.md): cancellation, host leases, async worker and redaction.
3. [03-verification.md](03-verification.md): consumer proof, safety regression tests and build gates.
4. [04-execution-handoff.md](04-execution-handoff.md): ordered execution and completion criteria.

## Runtime references

- [Tokio spawn_blocking](https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html): started blocking tasks cannot be aborted; shutdown must account for ongoing work.
- [tokio-util CancellationToken](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html): child-token and cancel-on-drop mechanics.
- [Cargo test target selection](https://doc.rust-lang.org/cargo/commands/cargo-test.html#target-selection): doctests need an explicit gate alongside all-targets checks.
