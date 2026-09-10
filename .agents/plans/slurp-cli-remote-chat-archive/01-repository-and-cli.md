# Repository and CLI foundation

## Goal and prerequisites

Create the `slurp` Go module, Fang-powered entry point, testable Cobra command tree, and build/version boundary. Gate R1 in the overview must resolve the authoritative module path first.

## Repository evidence

- The repository has no `go.mod`, Go source, task runner, CI, or releases.
- `README.md` and the Git remote still say `slurpy`.
- Fang v2.0.1 uses module path `charm.land/fang/v2`, requires Go 1.25, accepts a `*cobra.Command` in `fang.Execute`, and supplies version/help/completion/manpage behavior.

## Exact change surface

- `go.mod` — **new**, at repository root. Declare the Gate R1 module path, `go 1.25.0`, direct dependencies on `charm.land/fang/v2 v2.0.1` and the Cobra version selected by `go mod tidy`.
- `go.sum` — **new**, generated from the resolved dependency graph.
- `cmd/slurp/main.go` — **new**, under new parent `cmd/slurp/`. Construct the root command, call `fang.Execute(context.Background(), root, fang.WithVersion(...), fang.WithCommit(...), fang.WithNotifySignal(os.Interrupt, syscall.SIGTERM))`, and map failure to exit code 1.
- `internal/cli/root.go` — **new**, under new parent `internal/cli/`. Define proposed `NewRoot(Dependencies) *cobra.Command` with `Use: "slurp"` and no operational work during construction.
- `internal/cli/host.go` — **new**. Define the `host add`, `host list`, and `host remove` command constructors.
- `internal/cli/slurp.go` — **new**. Define the explicit `slurp` subcommand and its selection flags.
- `internal/buildinfo/buildinfo.go` — **new**, under new parent `internal/buildinfo/`. Hold linker-overridable `Version` and `Commit` values with development defaults.
- `internal/cli/root_test.go` and `internal/cli/slurp_test.go` — **new**. Exercise command parsing through injected dependencies and buffers.
- `Makefile` — **new**, at repository root. Provide `build`, `test`, `vet`, and `check` targets using ordinary Go commands.

All named symbols are proposed; no Go symbols currently exist.

## Command contract

```text
slurp
├── host
│   ├── add <name> <destination>
│   ├── list [--json]
│   └── remove <name>
└── slurp [host-name ...]
    ├── --harness <claude|codex|opencode|pi>   repeatable
    ├── --jobs <n>                             default 4
    ├── --operation-timeout <duration>         default 30m; 0 disables
    ├── --max-file-bytes <n>                   default 2147483648
    ├── --max-files <n>                        default 1000000
    ├── --max-total-bytes <n>                  default 107374182400; 0 disables
    ├── --config <path>                        global persistent flag
    └── --data-dir <path>                      global persistent flag
```

- No root arguments: show help and exit 0.
- Unknown host or harness: fail before opening any SSH connection.
- No configured hosts: return an actionable error that points to `slurp host add`.
- No positional host names: select every configured host.
- No `--harness`: select all four collectors.
- `--jobs` must be in `1..32`; it bounds host/harness tasks, not subprocesses per individual OpenCode export.
- `--operation-timeout` applies independently to each host/harness task. SSH connect attempts additionally use OpenSSH `ConnectTimeout=15`. Zero disables the operation deadline for intentionally large archives.
- Limit flags accept nonnegative base-10 byte/count values, use overflow-safe accounting, and fail the affected harness before accepting a record beyond the limit. Defaults cap one file at 2 GiB, one harness batch at 1,000,000 files and 100 GiB; zero disables only the total-byte limit.
- `host list` sorts by stable host name. Human output includes name and destination. `--json` writes a stable machine-readable array and no decoration.
- `host remove` changes configuration only. It explicitly reports that archived data remains.

## Behavior and invariants

- Command constructors receive interfaces for config loading, collection, clock, stdout, and stderr. Tests must not mutate the real home directory or invoke real SSH.
- Normal stdout contains status and summaries. Diagnostics go to stderr. Neither contains transcript bodies.
- Cobra argument validation happens before business logic.
- Context cancellation reaches every collector and child process. Main returns only after child-process cleanup completes.
- Fang owns help, styled errors, completions, manpage generation, and version rendering. Do not duplicate those commands.
- Linker values feed `fang.WithVersion` and `fang.WithCommit`; a source build remains identifiable as development/unknown without failing.

## Error paths

- Preserve typed causes internally for invalid config, unknown selection, SSH failure, remote prerequisite failure, source read failure, archive rejection, and local write failure.
- Convert expected user errors into concise messages with host and harness context.
- Never include remote stdout or stderr in an error. Drain each stream as required, but render only typed category, host, harness, exit status, and whether suppressed diagnostic bytes existed.

## Tests and acceptance

- Execute the root command with in-memory dependencies and assert exact exit/error categories for zero hosts, an unknown host, an invalid `--jobs`, and a collector partial failure.
- Assert help contains `host`, `slurp`, Fang-provided completion behavior, and the `slurp` product name.
- Assert `--version` uses injected build metadata.
- Assert command tests are color-independent by disabling color through the test environment or testing Cobra command behavior below Fang styling.
- Assert timeout and limit defaults, zero semantics, invalid/overflow values, and propagation to the coordinator.
- `go build ./cmd/slurp`, `go test ./...`, and `go vet ./...` pass.

## Dependencies and exclusions

- Depends on Gate R1 only.
- Configuration and collector implementations land later behind injected interfaces.
- Do not add a second CLI framework, interactive prompts, telemetry, or network calls outside SSH.
