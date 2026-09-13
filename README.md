# slurp

`slurp` is a Linux/macOS CLI that privately mirrors AI coding-chat archives from remote machines over OpenSSH. It collects Claude Code, Codex, OpenCode, and pi histories without parsing or normalizing transcript content.

## Build

Go 1.25 or newer is required.

```sh
go build ./cmd/slurp
# or
make check
```

The command UI, help, completions, and version output are rendered by [Fang](https://github.com/charmbracelet/fang).

## Configure hosts

Slurp stores an opaque OpenSSH destination under a stable local name. Authentication, ports, identities, jump hosts, aliases, and host-key policy remain in OpenSSH configuration. First make this succeed noninteractively:

```sh
ssh -o BatchMode=yes buildbox true
```

Then configure one or more hosts:

```sh
slurp host add buildbox buildbox
slurp host add lab alice@lab.example
slurp host list
slurp host list --json
slurp host remove lab
```

Names contain lowercase letters, digits, dots, underscores, or dashes. A destination is one argument and cannot contain options or whitespace; put complex connection behavior in `~/.ssh/config`. Slurp never stores passwords, keys, agents, or known-host data.

Custom transcript roots replace automatic discovery and are repeatable. Every explicit root is required:

```sh
slurp host add lab lab \
  --claude-path '~/custom claude/projects' \
  --codex-path /srv/codex-one \
  --codex-path /srv/codex-two \
  --pi-path /srv/pi/sessions
```

Only an exact leading `~/` is expanded remotely. Shell and environment-variable expressions are treated literally, not evaluated.

## Collect

```sh
slurp slurp
slurp slurp buildbox --harness codex --harness pi
```

With no host names, all configured hosts are selected. With no `--harness`, all four collectors run. Tasks continue after an isolated failure and the report is deterministic and redacted; the command exits nonzero if a host, asserted source, detected harness, protocol, or local write fails. An automatically absent harness is reported as `not-found` and is not an error.

Useful controls:

```text
--jobs 4                         concurrent hosts, from 1 through 32
--operation-timeout 30m         independent host/harness timeout; 0 disables
--max-file-bytes 2147483648     maximum single artifact size
--max-files 1000000             maximum files in one harness batch
--max-total-bytes 107374182400  maximum batch bytes; 0 disables
```

OpenSSH connections additionally use `ConnectTimeout=15` and `BatchMode=yes`.

## Sources and archive layout

- Claude Code: `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/**/*.jsonl`, including nested subagent transcripts.
- Codex: `.jsonl` and `.jsonl.zst` below `${CODEX_HOME:-$HOME/.codex}/{sessions,archived_sessions}`, plus `session_index.jsonl`.
- pi: `${PI_CODING_AGENT_SESSION_DIR}`, `${PI_CODING_AGENT_DIR}/sessions`, or `$HOME/.pi/agent/sessions`, in that order. Configure every project- or extension-selected `sessionDir` with repeated `--pi-path` flags because Slurp does not execute pi settings or extensions to discover them.
- OpenCode: the global session inventory from `opencode db`, followed by one canonical `opencode export` JSON document per database ID. `opencode` must be on the remote noninteractive `PATH`. A schema mismatch fails instead of silently falling back to the root-only session list.

Defaults are:

```text
config: ${XDG_CONFIG_HOME:-$HOME/.config}/slurp/config.json
data:   ${XDG_DATA_HOME:-$HOME/.local/share}/slurp
```

Use global `--config` and `--data-dir` flags to override them. Artifacts appear below `hosts/<name>/<harness>/`; manifests and identity records are under `state/<name>/`. Multiple explicit roots use stable `root-0001/`, `root-0002/`, … prefixes.

Runs are cumulative. Unchanged files are not rewritten, changed files are atomically replaced, and remotely deleted files remain in the local archive. Removing a registry entry also leaves its archive intact.

Each archive name is permanently bound to its stored destination string. Reusing the name with a different destination is refused. Slurp cannot detect an OpenSSH alias being retargeted, so use a new Slurp host name whenever an alias begins identifying another machine or account.

## Security and troubleshooting

Archive directories are owner-only and files are mode `0600`. Incoming paths are treated as hostile, symlinks are rejected in controlled paths, payloads are staged and synced before atomic publication, and remote stderr is discarded rather than echoed. The filesystem collectors do not follow remote symlinks.

Config and archive overrides must have a trusted parent chain. Slurp rejects non-sticky directories writable by other accounts; a sticky shared parent such as `/tmp` is permitted, while Slurp-owned descendants are tightened to owner-only modes.

These archives contain private prompts, model replies, tool results, file excerpts, and possibly credentials or other secrets. Filesystem permissions are not encryption; protect and back up the data directory accordingly.

If an installed Claude Code, Codex, or pi executable is found but its automatic source cannot be resolved, Slurp reports a failure with explicit-path guidance. If OpenCode collection fails, verify its CLI version, noninteractive `PATH`, and database schema against [the tested compatibility matrix](docs/harness-compatibility.md). Windows clients and remotes are not supported.

This repository and command are a clean rename from `slurpy`; there is no legacy command, configuration, archive migration, or compatibility alias.
