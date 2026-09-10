# Slurp CLI remote chat archive

Status: Implemented (2026-09-10)

The first implementation described by this document set is present in the repository.

## Application context

```json
{
  "application_context": {
    "has_active_users": false,
    "backward_compatibility_required": false,
    "feature_flags": "not-applicable",
    "confirmation_digest": "8ad904fed75eb1b4817669656127c57bdaa439830f4722126343c8d8ed25cfcd",
    "confirmed_at": "2026-09-10T21:10:50Z"
  }
}
```

The user confirmed that the tool has no active users or external consumers and that compatibility with `slurpy` is not required. The implementation can make a clean naming and configuration break without feature flags, migration code, or legacy aliases.

## Change classification

- Change type: new Go CLI and repository rename.
- Affected areas: repository identity, Go module and command graph, host configuration, SSH subprocess lifecycle, harness-specific chat discovery, local archive storage, tests, CI, and user documentation.
- Existing implementation surface: `README.md`, `.gitignore`, and `LICENSE` only. No Go module, commands, tests, build automation, contributor guide, or repository-local `AGENTS.md` exists.

## Requested outcome

Build a Go CLI named `slurp` with [Fang v2](https://github.com/charmbracelet/fang). Users manage named OpenSSH destinations, then run `slurp slurp` to collect chat history from Claude Code, Codex, OpenCode, and pi on every configured host into a private local archive.

## Success criteria

1. `go build ./cmd/slurp` produces a binary whose root command and generated help identify it as `slurp` and are rendered through Fang.
2. `slurp host add`, `slurp host list`, and `slurp host remove` persist and manage multiple named destinations without storing credentials or private-key material.
3. With two configured fixture SSH hosts, `slurp slurp` attempts all four harness collectors on both hosts and exits nonzero if any selected host cannot be contacted, any explicit source override is missing, or any selected, detected harness fails.
4. Claude Code and pi native JSONL files plus Codex native `.jsonl`/`.jsonl.zst` session files arrive byte-for-byte under the configured archive root. OpenCode root, child/subagent, archived, and cross-project sessions arrive as one canonical `opencode export` JSON document per database session ID.
5. A repeated run is idempotent: unchanged artifacts are not rewritten, updated artifacts are replaced atomically, and artifacts no longer present remotely are retained locally.
6. A failed or interrupted transfer never exposes a partially written artifact at its final path.
7. Archive directories and files are created with owner-only permissions, transcript contents never appear in routine terminal output, and hostile archive paths cannot escape the destination root.
8. Unit and integration tests pass with `go test ./...`; `go vet ./...` passes; CI runs both commands from a clean checkout.
9. Tracked naming, module path, documentation, and release examples use `slurp`, and the origin URL points at the renamed `slurp` repository before release.

## Scope

- Build a Linux/macOS-oriented command-line tool on Go 1.25 or newer.
- Pin `charm.land/fang/v2` at `v2.0.1` initially and use its Cobra integration.
- Treat names in the host registry as stable archive keys and OpenSSH destinations as opaque single arguments.
- Delegate authentication, host aliases, ports, identities, jump hosts, and host-key verification to the installed OpenSSH `ssh` client and the user's SSH configuration.
- Discover the default or documented environment-overridden transcript roots for Claude Code, Codex, and pi, with repeatable explicit roots for customized installations.
- Use OpenCode's public CLI database-query and export commands instead of copying its live SQLite database or relying on the root-only `session list` view.
- Preserve harness-native content. Do not parse or normalize transcript messages.
- Continue to later hosts and harnesses after an isolated failure, then return a summary and nonzero status for a partial run.

## Non-goals

- No support for the old `slurpy` command, repository name, config directory, or stored archive layout.
- No SSH key, password, agent, known-hosts, or connection-profile management.
- No Windows client or Windows remote support in the first release.
- No transcript search, rendering, normalization, deduplication across hosts, retention policy, deletion mirroring, encryption, compression-at-rest, cloud upload, or restore command.
- No collection of API credentials, general application configuration, caches, telemetry, or unrelated logs.
- No daemon, schedule, background synchronization, GUI, or interactive host picker.

## Repository findings

- `README.md` contains only the heading `slurpy` and the sentence “CLI to grab chats from common harnesses.” It establishes intent but no public contract.
- `.gitignore` is the GitHub Go deny-list template plus `.env` and editor entries.
- `origin` still points at `git@github.com:mattsp1290/slurpy.git` even though the requested target is `slurp`. Git does not track the checkout directory name, so the remote rename and local directory rename are operational steps, not source patches.
- The working tree was clean when planning started.
- The planning environment has Go 1.26 and `/usr/bin/ssh`; the selected Fang v2.0.1 module declares Go 1.25.
- Fang v2 wraps a Cobra root command with `fang.Execute`, supplies help/error styling, version output, completions, and a hidden manpage command.
- Claude Code transcripts are JSONL files below `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/`; nested subagent transcripts can also occur there.
- Codex stores rollouts below `${CODEX_HOME:-$HOME/.codex}/sessions/` and can also store archived rollouts under `archived_sessions/`; current source reads both plain `.jsonl` and cold-compressed `.jsonl.zst` representations.
- pi stores JSONL sessions below `${PI_CODING_AGENT_SESSION_DIR:-${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/sessions}`. A `sessionDir` setting can override the default, so the collector must report the resolved path and allow a host-level override when automatic resolution cannot reproduce the remote interactive environment.
- Current OpenCode stores sessions in SQLite. Its public `opencode db <query> --format json` and `opencode export <sessionID>` commands avoid copying a live database. `opencode session list` is insufficient because current source requests root sessions only.

## Key decisions

### Use OpenSSH as the transport

Invoke the system `ssh` executable with `exec.CommandContext`. This preserves existing `~/.ssh/config`, SSH agent, ProxyJump, security-key, and host-key behavior. A native Go SSH stack would have to reimplement those behaviors and would widen the credential-handling surface.

Reject destinations beginning with `-`, pass the destination as exactly one argv element, and never interpolate names or destinations into a local shell command. The remote side receives only versioned, embedded collector scripts with validated inputs.

### Keep a transparent mirror, not a normalized database

Store native transcript files in a host-and-harness namespace. This keeps the first release reversible, inspectable, and resilient to undocumented schema changes. OpenCode is the one exception because a live SQLite copy is unsafe; store the CLI's canonical JSON export for each session.

Each file update uses a sibling temporary file, streaming SHA-256 calculation, `fsync`, permission enforcement, and atomic rename. Treat content as unchanged only when the incoming hash, manifest, and verified final file agree. Never delete a local artifact because it disappeared remotely.

### Use stable host names as archive identity

The registry separates a user-selected name from the SSH destination. Renaming or removing a host does not rename or delete an existing archive. Reject duplicate names and names outside `^[a-z0-9][a-z0-9._-]*$` so paths and command output remain unambiguous.

Persist an owner-only identity record that binds each archive namespace to its configured SSH destination string. Refuse to collect if a removed name is later assigned to a different string. This cannot detect an OpenSSH alias retargeted in `~/.ssh/config`, so documentation must require a new Slurp host name when an alias begins identifying a different machine or account.

### Keep the configuration explicit and dependency-light

Use versioned JSON at `${XDG_CONFIG_HOME:-$HOME/.config}/slurp/config.json`. Use `${XDG_DATA_HOME:-$HOME/.local/share}/slurp` as the default archive root. Persist configuration atomically with owner-only permissions. Do not embed SSH options or credentials; users put connection behavior in OpenSSH config.

### Define absence separately from failure

A collector returns `not-found` when automatic discovery cannot find its transcript root or executable. This is normal because not every host uses every harness. A missing explicit override or a detected source that cannot be read/exported is a failure. `slurp slurp` succeeds only when all contacted hosts succeeded and every detected selected source completed.

## Target architecture

```text
cmd/slurp/main.go
    -> internal/cli root + host/slurp commands
        -> internal/config atomic host registry
        -> internal/collect orchestration
            -> internal/ssh OpenSSH process runner
            -> internal/harness collector descriptors
            -> internal/archive validated, atomic local writer

config.json
    -> hosts [{name, destination, optional source overrides}]

remote host
    -> fixed embedded discovery/export scripts
    -> versioned framed filesystem streams or one raw JSON stream per OpenCode export

local archive root
    -> hosts/<host-name>/<harness>/<native relative path>
    -> state/<host-name>/identity.json + <harness>.json
    -> locks/<host-name>/<harness>.lock
```

Control flow for `slurp slurp`:

1. Load and validate the registry.
2. Resolve all hosts, or only positional host names when supplied.
3. Probe the remote platform, home directory, source roots, required commands, and installed harnesses over SSH using noninteractive key/agent authentication.
4. Run each detected selected collector with bounded concurrency and context cancellation.
5. Validate streamed entry paths and write each artifact atomically.
6. Commit a per-harness state manifest only after that harness completes.
7. Print counts, bytes, unchanged files, skipped sources, and failures without transcript data.
8. Return a nonzero error after all possible work completes when any host or detected source failed.

## Constraints and operational gates

- Gate R1: before creating `go.mod`, confirm that the GitHub repository rename has produced `github.com/mattsp1290/slurp`; if the owner or final URL differs, use that authoritative module path everywhere.
- Gate R2: before the first release, rename the local checkout directory outside Git from `slurpy` to `slurp` and update `origin`. These actions are not represented by a commit.
- Gate S1: collectors must not read credential/config files or print transcript payloads.
- Gate S2: protocol decoding must reject absolute paths, `..`, duplicate conflicting entries, non-regular-file records, oversized fields, malformed lengths, and any resolved path outside the harness directory.
- Gate S3: no final artifact or state manifest is updated until its content is complete and durable.
- Gate V1: integration fixtures must exercise spaces, leading dashes, traversal names, interrupted streams, missing harnesses, a failed host, and a changing source file.
- Gate V2: the actual embedded collector script must run under both Linux and macOS `/bin/sh`; a fake producer alone is insufficient for remote portability.

## Risks

- Harness storage paths and formats are not stable public APIs. Encapsulate each source and test it against fixtures; surface detected-version and command failures instead of silently claiming success.
- A transcript can change during collection. The implementation must detect size/metadata drift when the remote protocol supports it and fail that harness run rather than commit an internally inconsistent batch.
- OpenCode database schema/export behavior can change across installed versions. Validate DB-query JSON and IDs, record the tested version matrix, fail loudly on schema mismatch, and never expose raw subprocess stderr.
- Remote utilities differ between GNU/Linux and macOS. Keep scripts POSIX-oriented, probe required capabilities, and test Linux plus macOS-compatible fixture behavior before claiming both platforms.
- Archives contain sensitive conversation and tool output. Owner-only modes reduce accidental exposure but do not provide encryption at rest.

## Assumptions

- “SSH hosts” means named OpenSSH destinations reachable by the local `ssh` client.
- “All logs” means all native conversation transcript files from the four named harnesses, including nested Claude subagent JSONL and Codex archived sessions, but not general diagnostic logs.
- Users want a cumulative local mirror. Remote deletion must not cause local deletion.
- Raw native logs are preferred over a shared transcript schema.
- The first release can require a POSIX shell and basic archive utilities on remote Linux/macOS hosts.
- SSH authentication is noninteractive. A destination must pass `ssh -o BatchMode=yes <destination> true` before Slurp can use it.

## Unresolved decisions

No blocking product decision remains. During implementation, Gate R1 must resolve the authoritative GitHub URL from repository state; this is a readiness verification, not a user-facing design choice.

Non-blocking follow-up: decide after field use whether archive encryption, normalized indexing, or scheduled runs deserve separate changes.

## Document map

1. [01-repository-and-cli.md](01-repository-and-cli.md) defines repository identity, the Go module, Fang/Cobra command graph, build metadata, and CLI contracts.
2. [02-configuration-and-hosts.md](02-configuration-and-hosts.md) defines paths, the versioned registry, atomic persistence, and host-management commands.
3. [03-ssh-and-archive-pipeline.md](03-ssh-and-archive-pipeline.md) defines transport, remote framing, safe extraction, orchestration, failure semantics, and local storage.
4. [04-harness-collectors.md](04-harness-collectors.md) defines discovery and collection for Claude Code, Codex, OpenCode, and pi.
5. [05-verification-and-delivery.md](05-verification-and-delivery.md) defines tests, CI, documentation, security checks, and release readiness.
6. [06-execution-handoff.md](06-execution-handoff.md) gives dependency-ordered implementation packages and final gates.
