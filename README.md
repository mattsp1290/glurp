# Glurp

`glurp` is a Linux/macOS CLI that privately collects AI coding-chat archives from remote machines over OpenSSH. It collects Claude Code, Codex, OpenCode, and pi histories without parsing or normalizing transcript content.

## Build

Build on Linux or macOS with stable Rust with edition 2024 support,
Cargo, rustfmt, and clippy. The tested local toolchain is Rust 1.97.0. Building
requires Cargo dependency access and a native linker/toolchain (on macOS, Xcode
Command Line Tools). Running requires OpenSSH locally and POSIX `sh` plus standard
Unix utilities on the remote. Linux tests also use a C compiler for test-only
filesystem fault injection.

```sh
cargo build --release --locked
./target/release/glurp --help
# Run formatting, Clippy, all-target tests, and the release build:
make check
```

Use `target/release/glurp` directly or place it on your `PATH`. Linux and macOS CI
run the same four Rust gates, including synthetic fixtures that execute the actual
POSIX collector scripts. Release packaging and publication are outside this
cutover; this repository does not configure a release publisher.

## Configure hosts

Glurp stores an opaque OpenSSH destination under a stable local name. Authentication, ports, identities, jump hosts, aliases, and host-key policy remain in OpenSSH configuration. First make this succeed noninteractively:

```sh
ssh -o BatchMode=yes buildbox true
```

Then configure one or more hosts:

```sh
glurp host add buildbox buildbox
glurp host add lab alice@lab.example
glurp host list
glurp host remove lab
```

Names contain 1–64 ASCII letters, digits, underscores, or hyphens. A destination is one argument and cannot contain options or whitespace; put complex connection behavior in `~/.ssh/config`. Glurp never stores passwords, keys, agents, or known-host data.

Custom transcript roots replace automatic discovery and are repeatable. Every explicit root is required:

```sh
glurp host add lab lab \
  --claude-path '~/custom claude' \
  --codex-path /srv/codex-one \
  --codex-path /srv/codex-two \
  --pi-path /srv/pi/sessions
```

Claude roots contain `projects/`; Codex roots contain `sessions/` and/or
`archived_sessions/`; pi roots are session directories themselves.
Only an exact leading `~/` is expanded remotely. Shell and environment-variable expressions are treated literally, not evaluated.

## Collect

```sh
glurp glurp
glurp glurp buildbox --harness codex --harness pi
```

With no host names, all configured hosts are selected. With no `--harness`, all four collectors run. Tasks run sequentially in sorted host/harness order. Tasks continue after an
isolated failure and the report is deterministic and redacted; the command exits nonzero if a host, asserted source, detected harness, protocol, or local write fails. An automatically absent harness is reported as `not-found` and is not an error.

Useful controls:

```text
--timeout-seconds 300           whole host/harness timeout; must be positive
--max-file-bytes 268435456      maximum single artifact size (256 MiB)
--max-files 100000              maximum artifacts in one host/harness task
--max-total-bytes 1073741824    maximum combined task payload (1 GiB)
```

These are defaults. Zero byte limits allow only empty payloads within their
scope; a zero count limit allows no artifacts. Zero does not disable limits. OpenCode inventory and exports share one task budget, and the
inventory counts as an artifact. A finite wire ceiling additionally bounds framing
overhead. See [archive safety](docs/rust-safety.md) for storage and deadline limits.

OpenSSH connections additionally use `ConnectTimeout=15` and `BatchMode=yes`.

## Sources and archive layout

- Claude Code: `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/**/*.jsonl`, including nested subagent transcripts.
- Codex: `.jsonl` and `.jsonl.zst` below `${CODEX_HOME:-$HOME/.codex}/{sessions,archived_sessions}`, plus `session_index.jsonl`.
- pi: `${PI_CODING_AGENT_SESSION_DIR}`, `${PI_CODING_AGENT_DIR}/sessions`, or `$HOME/.pi/agent/sessions`, in that order. Configure every project- or extension-selected `sessionDir` with repeated `--pi-path` flags because Glurp does not execute pi settings or extensions to discover them.
- OpenCode: the global session inventory from `opencode db`, followed by one native `opencode export` JSON document per database ID. `opencode` must be on the remote noninteractive `PATH`. A schema mismatch fails instead of silently falling back to the root-only session list.

Defaults are:

```text
config: ${XDG_CONFIG_HOME:-$HOME/.config}/glurp/config.json
data:   ${XDG_DATA_HOME:-$HOME/.local/share}/glurp
```

Set absolute `HOME`, `XDG_CONFIG_HOME`, and `XDG_DATA_HOME` to select storage
locations; relative XDG values fall back to HOME defaults. Artifacts appear below
`hosts/<name>/<harness>/`; `hosts/<name>/destination.json` records destination
identity. Private lock and transaction files also live within managed storage.
Explicit roots use stable `root-0001/`, `root-0002/`, … prefixes, even for one root.

Runs are cumulative. Unchanged files retain exact bytes and mtimes, changed files
are atomically replaced, and remotely deleted files remain in the local archive.
A complete host/harness batch validates before publication, with durable backups
and transaction-wide rollback on a late failure. Removing a registry entry also leaves its archive intact.

Each archive name is permanently bound to its stored destination string. Reusing the name with a different destination is refused. Glurp cannot detect an OpenSSH alias being retargeted, so use a new Glurp host name whenever an alias begins identifying another machine or account.

## Security and troubleshooting

Archive directories are owner-only and files are mode `0600`. Incoming paths are treated as hostile, symlinks are rejected in controlled paths, payloads are staged and synced before atomic publication, and remote stderr is discarded rather than echoed. The filesystem collectors do not follow remote symlinks.

Config and archive storage locations must have a trusted parent chain. Glurp rejects non-sticky directories writable by other accounts; a sticky shared parent such as `/tmp` is permitted, while Glurp-owned descendants are tightened to owner-only modes.

These archives contain private prompts, model replies, tool results, file excerpts, and possibly credentials or other secrets. Filesystem permissions are not encryption; protect and back up the data directory accordingly.

If an installed Claude Code, Codex, or pi executable is found but its automatic source cannot be resolved, Glurp reports a failure with explicit-path guidance. If OpenCode collection fails, verify its CLI version, noninteractive `PATH`, and database schema against [the tested compatibility matrix](docs/harness-compatibility.md). Windows clients and remotes are not supported.

Interrupted transactions are recovered before new SSH work when rollback intent
is known. An ambiguous committed journal blocks collection and preserves evidence.
To explicitly choose local restoration of retained originals:

```sh
glurp recover HOST --rollback
```

This never contacts SSH and can discard a possibly successful newest generation.
It verifies the destination binding, lock, journal, targets, and required backups
before changing files. If successful-commit cleanup already removed some backups,
rollback refuses without partially restoring data; manual resolution is required.
Stop collection for that host, preserve a private copy of its entire archive
including hidden journals/backups and timestamps, and inspect both generations.
Restore missing originals only from verified copies, or verify all journal targets
before manually retiring evidence to keep the current generation. Do not delete
evidence merely to clear an error. Read [the recovery contract](docs/rust-safety.md)
before resolving ambiguity.

This is a fresh Rust implementation. The only fixed legacy CLI requirement is
`glurp glurp`; earlier Go flags, JSON host-list output, concurrency controls, config
schema, archive layout, and protocol are intentionally not compatible. There is no
legacy command alias or automatic import of earlier Go Glurp or Slurp config and
archives. Existing legacy state is not automatically read, migrated, or deleted.
Historical `.agents/plans/` documents are retained as provenance. Current internal
interfaces are documented in [rust-interfaces.md](docs/rust-interfaces.md).
