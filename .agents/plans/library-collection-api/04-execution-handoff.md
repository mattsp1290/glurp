# Execution handoff

Status: Ready; required planning reviews completed. No implementation performed. User requested a plan and interview, not implementation or publication.

## Order

1. Recheck git status and baseline; preserve unrelated changes and historical plans. Read 00-overview and confirmed application context. No flags, migration or backwards-compatible wrappers are required.
2. WP1: introduce the proposed library boundary, owned request/outcome types and thin CLI driver. Files/symbols and acceptance: [01-library-contract.md](01-library-contract.md). Do not advertise an API using process-global cancellation.
3. WP2: integrate per-call cancellation, Tokio worker ownership, host-session leases and typed redaction. [02-execution-lifecycle.md](02-execution-lifecycle.md). Coordinate edits to main/config/archive/remote/transaction; WP1 and WP2 share boundaries and should not be merged independently as a working public feature.
4. WP3: prove the actual library from a separate crate, both configuration paths, cancellation/concurrency, privacy and unchanged archive safety. Update build/docs and existing release coverage. [03-verification.md](03-verification.md). Tests can be prepared alongside WP2 only after contract names settle.
5. Run all gates on one exact committed implementation tree. Obtain separate authority before pushing, opening a PR, publishing or releasing; this planning document grants none. When an implementation push is authorized, verify the exact remote Git dependency pin before claiming other repos can adopt it.

## Stop/go gates

- Stop for unresolved user-scope changes, mandatory new upstream dependency contracts, or a need to expose registration/recovery as supported library functionality.
- Stop for cancellation that returns before awaited cleanup, library signal changes, raw error leakage, unprotected same-host writes or loss of recovery evidence.
- Do not count CLI-only test success as importable-library proof. Do not count a local path dependency as proof of Git-pinned consumption.
- Resolve/lock Tokio dependencies under Rust 1.98.1 before downstream work relies on them. A new ecosystem/library incompatibility requires re-evaluation, not silent runtime scope expansion.
- Never use actual transcripts, credentials or user production archives in tests. Reverting code is possible; reverting across ambiguous archive transactions must retain evidence and use existing explicit recovery procedures, never deletion.

## Definition of done

A standalone crate can import and call the documented async collection API without the Glurp executable. Both config and explicit modes operate through the same safety-preserving core as CLI. Every requested report/progress/cancellation/busy contract is proven by actual in-process calls and cross-process fixtures. All specified checks pass on the exact implementation revision; runtime/platform coverage limits are recorded. README and API docs contain a verified usable Git revision after authorized publication. Agentcraft adoption, crates.io, HTTP, job scheduling and manual public recovery APIs remain deferred.

## Planning review gate

Two independent reviews and one fresh adversarial review completed. Accepted findings are incorporated, including between-harness recovery readiness, decoder cancellation provenance and the host-owned panic-hook boundary. Full plan read-through and cross-reference checks completed. This completes planning, not authorization to implement. First implementation action, once separately authorized: recheck the baseline and add the WP1 library contract scaffold with tests.
