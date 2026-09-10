# Verification and delivery

## Goal and prerequisite state

Prove the CLI works end to end without real credentials or user transcripts, document its operational contract, and finish the clean repository rename. All implementation packages must be complete.

## Exact change surface

- `internal/integration/slurp_test.go` — **new**, under new parent `internal/integration/`. Build the binary and run it against a fake SSH executable plus synthetic remote fixtures.
- `testdata/remote/` — **new**, at repository root. Hold synthetic Claude, Codex, pi, and fake OpenCode exports with no real chat or credential data.
- `.github/workflows/ci.yml` — **new**, under new parent `.github/workflows/`. Run formatting check, `go test -race ./...`, `go vet ./...`, and `go build ./cmd/slurp` on Linux; add a macOS test job for path/process portability.
- `.goreleaser.yaml` — **new**, at repository root. Build `./cmd/slurp` as `slurp` for supported Linux/macOS architectures and inject version/commit values.
- `README.md` — **existing**, replace the placeholder with install/build instructions, host CRUD examples, `slurp slurp` behavior, paths, supported harness surfaces, security warning, troubleshooting, and rename note.
- `docs/harness-compatibility.md` — **new**, under new parent `docs/`. Record upstream evidence and exact harness versions/commits verified for a release.
- `.gitignore` — **existing**, retain current Go rules and add only repository-local generated binary/distribution paths actually introduced by the build/release flow.

Operational changes outside Git:

- Rename the GitHub repository from `mattsp1290/slurpy` to its authoritative `slurp` URL if this has not already occurred.
- Set `origin` to that URL.
- Rename the local checkout directory from `slurpy` to `slurp` after no process depends on the old current working directory.

## Test layers

### Unit

- Table-test command validation, config schema/path resolution, host CRUD, discovery precedence, manifest behavior, error classification, and output redaction.
- Fuzz config loading, protocol decoding, relative-path validation, and archive writes.
- Run unit tests without a network, actual home directory, installed harness, or real `ssh`.

### Component

- Put a fake `ssh` executable first on PATH. It must capture argv and execute only the repository's synthetic fixture protocol.
- Execute the actual embedded collector script under `/bin/sh` on both Linux and macOS CI against synthetic source trees, then feed its stdout through the real decoder/archive path. Keep fake protocol producers for malformed-stream cases only.
- Exercise successful collection, partial host failure, missing harnesses, malformed streams, changing files, oversized data, cancellation, and retry.
- Assert exact archive contents and SHA-256 values, state manifests, owner-only modes, deterministic summaries, and aggregate exit status.

### End to end smoke gate

Before release, use a disposable local SSH server/container with a temporary HOME and synthetic fixtures. Configure its SSH alias through a temporary OpenSSH config, run the built binary, and verify all four archives. This proves the real `ssh` argv and streaming lifecycle without using personal transcript data.

## Required security assertions

- Repository and CI fixtures contain no secrets or copied real transcripts.
- Host destinations and source overrides cannot inject local or remote shell syntax.
- A malicious remote cannot escape the archive root, create a link, overwrite config/state outside its namespace, or cause unbounded metadata allocation.
- Routine and error output contains file counts/paths only at documented verbosity and never body content.
- Config, archives, manifests, and temporary files are owner-only even under a permissive umask.
- Pre-existing broad modes are tightened before use; symlinked or unfixable controlled paths fail safely.
- Cancellation and process errors cannot leave a readable partial file at a final artifact path.

## Documentation contract

README examples must include:

```text
slurp host add buildbox buildbox
slurp host add lab alice@lab.example
slurp host list
slurp slurp
slurp slurp buildbox --harness codex --harness pi
```

Document:

- OpenSSH setup and authentication are external and noninteractive; users must make `ssh -o BatchMode=yes <destination> true` succeed first.
- Default connection and operation timeouts plus the operation-timeout and byte/count limit overrides.
- Default config and data paths plus `--config`/`--data-dir` overrides.
- Per-host archive identity binding and the requirement to use a new name for a different destination.
- OpenSSH alias retargeting cannot be detected from the stored destination string; users must choose a new Slurp host name when an alias starts identifying another machine or account.
- The exact four source surfaces and host-level Claude/Codex/pi path overrides.
- Repeatable source-root behavior for custom/multi-root installations and the failed “installed but unresolved” status.
- Missing harness versus failed detected harness semantics and partial-run exit behavior.
- Cumulative retention: removal from a remote or the registry does not delete local archive data.
- Archives contain private prompts, model replies, tool results, file excerpts, and possibly secrets; filesystem permissions are not encryption.
- OpenCode must be installed and visible on the remote noninteractive PATH.
- OpenCode completeness depends on the tested global DB schema/query, not the root-only `session list`; a schema mismatch fails instead of producing a partial-success archive.
- Linux/macOS remote requirement and unsupported Windows behavior.
- The clean break from `slurpy`; no old command or data migration exists.

## Release gates and rollback

1. Gate R1 confirms the final GitHub URL and `go.mod` module path match.
2. The `Makefile` formatting check passes by running `gofmt -l` over the explicit tracked Go file list and requiring empty output.
3. `go test -race ./...`, `go vet ./...`, and `go build ./cmd/slurp` pass on Linux.
4. CI's macOS job passes collector protocol, path, permission, and cancellation tests.
5. The macOS job executes the actual embedded collector under macOS `/bin/sh`; the disposable real-SSH smoke gate may remain Linux-based but passes all four synthetic sources and a repeated idempotency run.
6. `rg -n 'slurpy' --glob '!\.agents/plans/**'` returns only an intentional historical rename statement, if retained.
7. `git remote get-url origin` identifies the authoritative `slurp` repository.
8. Tag/release only after the gates pass.

If a release is faulty, stop distribution and revert the release commit/tag through the repository's normal release process. The CLI never deletes remote or local transcript artifacts, so rollback does not require a data migration. Preserve archives created by the faulty version for manual inspection; do not auto-delete them.

## Acceptance criteria

- CI enforces all automated gates on a clean clone.
- README commands match the implemented help output.
- A clean disposable environment can configure two hosts, collect all supported fixture histories, rerun without rewrites, and observe a nonzero partial result when one host is made unreachable.
- The known-complete OpenCode inventory contains multiple projects, root sessions, child/subagent sessions, and archived sessions, and every ID has a local export.
- A reviewer can trace every supported source file from fixture through SSH framing into the documented local archive path.
- `docs/harness-compatibility.md` identifies the upstream contract and tested version/commit for every collector.

## Dependencies and exclusions

- Depends on all previous work files.
- Publishing a release is authorized only as a separate explicit user action; this plan prepares release configuration but does not publish.
- Encryption, signing, package-manager formulas, and Windows builds remain follow-up work.
