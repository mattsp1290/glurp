# Execution handoff

## Implementation state

Implementation completed on 2026-09-10. The work packages below remain as the delivery and regression checklist.

## Work package 0: resolve repository identity

### Result

The implementation uses the authoritative `slurp` repository URL and module path.

### Actions

- Verify the GitHub repository currently intended by the user.
- If `github.com/mattsp1290/slurp` is authoritative, use it as the Go module path. If GitHub reports another owner/path after the rename, use that path and update every proposed import/document reference consistently.
- Record `git remote get-url origin` before changing it. Update `origin` only after the remote repository exists.
- Do not rename the checkout directory while tools still have the old directory as their current working directory; perform that operational step after source work or restart the shell from the parent directory.

### Verification

- `git ls-remote origin HEAD` succeeds after the remote update.
- The selected module path resolves to the same owner/repository identity.

### Gate

Stop before Work Package 1 if the authoritative repository cannot be identified. Decision owner: repository owner. Unblock by providing or creating the final GitHub repository URL.

## Work package 1: create the Go and CLI skeleton

### Result

A buildable `slurp` binary uses Fang v2.0.1 and exposes the documented command graph through injected, initially stubbed interfaces.

### Change surface

- **New:** `go.mod`, `go.sum`, `cmd/slurp/main.go`, `internal/buildinfo/buildinfo.go`, `internal/cli/root.go`, `internal/cli/host.go`, `internal/cli/slurp.go`, CLI tests, and `Makefile`.
- Symbols: proposed `NewRoot`, command constructors, dependency interfaces, and linker variables from [01-repository-and-cli.md](01-repository-and-cli.md).

### Verification

- `go build ./cmd/slurp`
- `go test ./internal/cli/...`
- `go run ./cmd/slurp --help`
- `go run ./cmd/slurp --version`

### Acceptance

Help identifies `slurp`, includes `host` and `slurp`, and is Fang-rendered. No constructor touches the real filesystem or network.

## Work package 2: implement paths, config, and host CRUD

### Result

Users can safely manage a version-1 list of named destinations and optional source overrides.

### Change surface

- **New:** `internal/paths/*`, `internal/config/*`, and `internal/cli/host_test.go`.
- Interfaces and schema from [02-configuration-and-hosts.md](02-configuration-and-hosts.md).

### Prerequisites and parallelization

- Requires Work Package 1.
- Path resolution and config model tests may proceed in parallel, but atomic persistence depends on both.

### Verification

- `go test -race ./internal/paths ./internal/config ./internal/cli`
- Run host CRUD with `--config` and `--data-dir` inside a temporary directory and inspect modes plus deterministic JSON.

### Acceptance

Concurrent mutations cannot corrupt config. Removing a host cannot remove its archive. Reusing its name for a different destination string cannot merge archive content; documentation covers the undetectable SSH-alias-retargeting case.

## Work package 3: implement SSH, framing, and archive storage

### Result

Synthetic remote artifacts stream through a fake SSH process into an owner-only, traversal-safe, atomic local mirror.

### Change surface

- **New:** `internal/ssh/*`, `internal/protocol/*`, `internal/archive/*`, and `internal/collect/*`.
- Interfaces, state machine, layout, and error semantics from [03-ssh-and-archive-pipeline.md](03-ssh-and-archive-pipeline.md).

### Prerequisites and parallelization

- Requires Work Package 2 interfaces.
- SSH runner, protocol codec, and archive store can proceed in parallel against agreed interfaces.
- Coordinator integration waits for all three.

### Verification

- `go test -race ./internal/ssh ./internal/protocol ./internal/archive ./internal/collect`
- `go test -fuzz=FuzzDecode -fuzztime=10s ./internal/protocol`
- `go test -fuzz=FuzzRelativePath -fuzztime=10s ./internal/archive`

### Acceptance

All malicious/truncated fixtures fail closed; cancellation leaves no child process or partial final artifact; unchanged content is not rewritten; manifest/on-disk divergence is repaired; namespace locks prevent concurrent-run corruption.

## Work package 4: implement the four collectors

### Result

Claude Code, Codex, pi, and OpenCode histories are discovered and archived under isolated native namespaces.

### Change surface

- **New:** `internal/harness/*`, `internal/harness/testdata/*`, `docs/harness-compatibility.md`, and collector tests.
- Collector paths, precedence, exclusions, and OpenCode export behavior from [04-harness-collectors.md](04-harness-collectors.md).

### Prerequisites and parallelization

- Requires Work Package 3 contracts.
- The four collectors can be implemented in parallel after the shared filesystem collector is stable.
- OpenCode uses command/JSON fixtures rather than the filesystem collector.

### Verification

- `go test -race ./internal/harness ./internal/collect`
- Compare fixture source SHA-256 values with archived outputs for all native-file collectors.
- Compare native Codex `.jsonl` and `.jsonl.zst` bytes, including both-sibling and representation-transition fixtures.
- Parse every archived OpenCode export as exactly one JSON document.
- Compare OpenCode exports against a known-complete DB inventory containing multiple projects, roots, children/subagents, and archived sessions.
- Execute the actual embedded collector script under Linux and macOS `/bin/sh` and record the upstream/tested versions in `docs/harness-compatibility.md`.

### Acceptance

All four fixture collectors pass; genuine harness absence is `not-found`; installed-but-unresolved sources, missing explicit roots, schema mismatches, and detected unreadable/malformed sources fail the aggregate run without preventing other hosts from completing.

## Work package 5: integrate commands and end-to-end behavior

### Result

`slurp slurp` loads the registry, selects hosts/harnesses, runs bounded collection, prints a deterministic redacted report, and returns the correct status.

### Change surface

- **Modify proposed files:** `internal/cli/slurp.go`, `internal/cli/slurp_test.go`, `cmd/slurp/main.go`.
- **New:** `internal/integration/slurp_test.go` and `testdata/remote/*`.

### Prerequisites

- Requires Work Packages 2–4.

### Verification

- `go test -race ./...`
- Build and run against fake SSH fixtures with two hosts, all harnesses, an idempotent rerun, one absent harness, and one failed host.

### Acceptance

Archive contents, summaries, continuation after failure, explicit-override failure behavior, limits/timeouts, archive identity binding, and exit codes match the success criteria in [00-overview.md](00-overview.md).

## Work package 6: documentation, CI, release setup, and rename finish

### Result

The repository consistently presents and verifies a releasable `slurp` CLI.

### Change surface

- **Modify existing:** `README.md`, `.gitignore`.
- **New:** `.github/workflows/ci.yml`, `.goreleaser.yaml`.
- **Operational:** update `origin` and rename the checkout directory as described in [05-verification-and-delivery.md](05-verification-and-delivery.md).

### Prerequisites

- Requires Work Package 5.

### Verification

- `make check`
- `go test -race ./...`
- `go vet ./...`
- `go build ./cmd/slurp`
- `rg -n 'slurpy' --glob '!\.agents/plans/**'`
- `git remote get-url origin`
- Run the disposable real-SSH smoke gate twice.

### Acceptance

CI is green on Linux and macOS; docs match help and archive behavior; tracked identity is `slurp`; remote identity is authoritative; no release is published by this work package.

## Integration and regression gates

1. Config corruption and conflicting host mutations fail without data loss.
2. Host strings and path overrides cannot execute shell content.
3. A disposable real-SSH test proves metacharacter-bearing path overrides remain literal and create no sentinel side effect.
4. Remote traversal/link records cannot escape the archive namespace.
5. Transfer interruption cannot expose partial final files.
6. Missing automatically discovered harnesses remain distinguishable from missing explicit overrides and failed detected harnesses.
7. A partial host failure does not prevent independent hosts from being collected.
8. A repeated run preserves hashes and modification times for unchanged artifacts, while manifest/file divergence is repaired.
9. Concurrent collection cannot mix manifests, remove live staging data, or merge different configured destination strings under one name; alias-retargeting limitations remain documented.
10. Remote deletion does not delete local data.
11. Exact/over-limit and timeout cases fail with bounded resource use.
12. No test reads the developer's home transcript trees or real credentials.

## Definition of done

- Every success criterion in `00-overview.md` has an automated test or named smoke procedure.
- All commands and package interfaces described in this plan exist or the implementation updates the plan before diverging.
- `go test -race ./...`, `go vet ./...`, and `go build ./cmd/slurp` pass in CI.
- The disposable SSH smoke test passes for all four collectors and an idempotent rerun.
- The compatibility matrix records current upstream evidence and the exact versions/commits exercised.
- README documents security, storage, source coverage, partial failure, and the clean rename.
- The repository and binary use `slurp`; no compatibility layer for `slurpy` remains.
- Implementation commits contain no real transcript, credential, temporary archive, or generated release binary.

## Deferred work

- Encryption at rest, normalized indexing/search, archive pruning, remote deletion mirroring, scheduling, restore/import, Windows support, additional harnesses, package-manager publication, and cloud storage.
- Revisit content-addressed storage only if field data shows mirror duplication or version history is required.
