# Harness collectors

## Goal and prerequisite state

Implement four isolated collectors on top of the SSH protocol and archive interfaces. The transport/protocol contract must be stable first.

## Shared exact change surface

- `internal/harness/harness.go` — **new**, under new parent `internal/harness/`. Define proposed identifiers, selection parsing, `Collector` interface, discovery result, and registry.
- `internal/harness/filesystem.go` — **new**. Provide the shared regular-file walker protocol for JSONL-based sources.
- `internal/harness/claude.go`, `codex.go`, `opencode.go`, and `pi.go` — **new**. Define one collector per harness.
- `internal/harness/testdata/` — **new** directory containing synthetic, non-secret filesystem and command-output fixtures.
- `internal/harness/*_test.go` — **new** tests for path resolution, discovery, selection, native-path preservation, and error mapping.

All symbols are proposed. Each collector returns content streams and metadata; only `internal/archive` writes local files.

## Shared collector rules

- Probe remote `uname`, home directory, command availability, and relevant environment variables without starting a login shell that sources arbitrary interactive startup files.
- Accept only Linux and Darwin from the remote platform probe. Return an unsupported-platform failure otherwise.
- A nonempty host-level list of source roots replaces automatic discovery. Otherwise use the documented environment override visible to the remote process, then the default below.
- If any explicit root is missing, non-directory, or unreadable, return `failed`. Reserve `not-found` for automatic discovery.
- Expand only `$HOME` defaults and an exact `~/` prefix in user overrides. Reject unresolved, relative, empty-after-expansion, or NUL-containing paths.
- Follow neither file nor directory symlinks. Report skipped links by count. This prevents a transcript-tree link from expanding collection into unrelated data.
- Preserve relative directory structure. Select regular files only. Apply deterministic lexical ordering so reports and tests are stable.
- For explicit multi-root collectors, preserve config-list order and prefix logical paths with `root-0001/`, `root-0002/`, and so on before the native relative path. Appending a root does not move earlier archives; reordering roots is a documented identity change and must be rejected when prior manifest root bindings disagree.
- Treat unreadable matching files as failures, not skips.
- Cap metadata and artifact size through the shared protocol, while streaming accepted content.
- Record collector identity and best-effort remote harness version in the manifest without making version discovery a prerequisite.

## Claude Code

### Discovery

- Resolve explicit `sources.claude`, else `${CLAUDE_CONFIG_DIR}/projects` when the variable is absolute, else `$HOME/.claude/projects`.
- Detect the source only if the resolved directory exists.

### Collection

- Recursively collect regular `*.jsonl` files, including project sessions and nested `subagents/agent-*.jsonl` sessions.
- Preserve each path relative to `projects/`.
- Do not collect `.credentials.json`, settings, shell snapshots, memory Markdown, plugin data, or project-local `.claude` content.
- Do not trust `sessions-index.json` as the source of truth because valid transcript JSONL can exist without an index entry.

### Acceptance

- A fixture containing top-level sessions, nested subagent sessions, a stale index, unrelated JSON/Markdown, and a symlink collects only regular JSONL files byte-for-byte.
- An absent `projects/` directory returns `not-found` only when `claude` is also absent; an installed executable with no resolved root or an unreadable JSONL returns `failed`.

## Codex

### Discovery

- Resolve explicit `sources.codex`, else `${CODEX_HOME}` when it is absolute, else `$HOME/.codex`.
- Detect Codex if `sessions/`, `archived_sessions/`, or `session_index.jsonl` exists below the root.

### Collection

- Recursively collect regular `.jsonl` and `.jsonl.zst` files below `sessions/` and `archived_sessions/` without decompressing or renaming either representation.
- Collect regular `session_index.jsonl` as related chat metadata when present.
- Preserve paths relative to the Codex root.
- Do not collect `auth.json`, config, logs, skills, caches, or SQLite state.

### Acceptance

- A fixture with dated rollout trees, archived rollouts, `.jsonl`-only, `.jsonl.zst`-only, both-sibling, index metadata, auth/config decoys, and symlinks collects only the named session surfaces.
- Missing one optional subtree does not fail when another supported surface exists.
- If compression changes representation during a batch, missing/changed-file detection fails that batch. A later run collects the surviving representation; local cumulative retention keeps any previously archived sibling.
- An absent Codex root returns `not-found` only when `codex` is also absent; an installed executable with no resolved root returns `failed` with `--codex-path` guidance.

## pi

### Discovery

- Resolve every explicit `sources.pi` root first and collect all of them under stable `root-<index>/` namespaces.
- Otherwise use absolute `${PI_CODING_AGENT_SESSION_DIR}` when visible remotely.
- Otherwise resolve `${PI_CODING_AGENT_DIR}/sessions` when `PI_CODING_AGENT_DIR` is absolute.
- Otherwise use `$HOME/.pi/agent/sessions`.
- Do not read pi's global/project settings or execute extensions merely to discover paths. When automatic discovery finds no default/env directory but `pi` is installed, fail as unresolved and direct users with configured `sessionDir` locations to repeat `slurp host add --pi-path` for every root. The collector cannot infer per-project or extension-chosen locations absent from the noninteractive environment.

### Collection

- Recursively collect regular `*.jsonl` files and preserve paths relative to the sessions root.
- Do not collect `auth.json`, `settings.json`, prompts, extensions, skills, models, or project memory.

### Acceptance

- Fixtures cover the default, each environment override, an explicit host override, nested encoded-working-directory sessions, and unrelated config/credential decoys.
- Precedence is explicit host override, session-dir environment, agent-dir environment, then default.
- Any missing explicit root fails. Fixtures representing global, project, multiple-root, and extension-selected session directories fail as unresolved when `pi` is installed until all matching `--pi-path` roots are configured, after which every root collects successfully.

## OpenCode

### Discovery

- Resolve `opencode` on the remote PATH using `command -v`; absence returns `not-found`.
- Record `opencode --version`, then run `opencode db "SELECT id FROM session ORDER BY id" --format json` through a fixed, versioned command template. This global database inventory includes root, child/subagent, archived, and cross-project rows instead of the root-only `session list` view.
- A command failure, missing `session` table/`id` column, non-JSON stdout, duplicate ID, or invalid record returns `failed` with a typed schema/version diagnostic and no raw subprocess output.

### Collection

1. Parse database-query JSON locally under the 64 MiB cap and extract unique session IDs.
2. Accept IDs only when they match the documented observed `ses_` identifier form and a conservative `^[A-Za-z0-9_-]+$` transport grammar. Treat a violating ID as a protocol failure rather than interpolating it.
3. For each sorted ID, run `opencode export <session-id>` remotely with ID as a validated argument.
4. Give each export a separate SSH process. Stream stdout directly to a capped local temporary file until EOF, require one valid JSON document and successful exit, then commit the exact bytes to `sessions/<session-id>.json`.
5. Bound per-host exports to one at a time and stop the OpenCode collector on its first failed export. Preserve prior local data and manifest.

Do not copy `opencode.db`, WAL/SHM files, legacy storage directories, auth state, or server logs. Do not pass `--sanitize` by default because the archive is intended to preserve the user's complete private chat; document that archives can contain secrets.

### Acceptance

- A fake OpenCode binary returns an unordered global DB inventory containing multiple projects, roots, children/subagents, and archived sessions. The archive contains sorted, byte-preserved JSON exports for the complete known inventory.
- Fixtures cover zero sessions, missing/changed database schema, malformed query JSON, duplicate/hostile IDs, large streamed exports, invalid export JSON, nonzero query/export exits, secret-bearing stderr suppression, interruption during export, and cancellation between exports.
- Zero sessions is a successful detected collector with zero artifacts.

## Cross-harness verification

- A single fake remote containing all four sources produces distinct host/harness namespaces with no filename collisions.
- Selecting `--harness codex --harness pi` never probes or invokes Claude/OpenCode collectors.
- Adding a future collector requires a registry entry and tests, not changes to SSH or archive path validation.

## Compatibility evidence and release matrix

Create `docs/harness-compatibility.md` (**new**, under new parent `docs/`) and record exact versions or commits exercised at release time. Seed it with these upstream contracts and label behavior that upstream does not guarantee as “observed”:

| Harness | Collection boundary | Upstream evidence to re-check | Release evidence |
| --- | --- | --- | --- |
| Claude Code | `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/**/*.jsonl` | Current `anthropics/claude-code` product documentation and official repository reports for project transcripts and `CLAUDE_CONFIG_DIR` | Version output plus actual embedded-script discovery against a synthetic tree shaped from that version |
| Codex | `${CODEX_HOME:-$HOME/.codex}/{sessions,archived_sessions}/**/*.{jsonl,jsonl.zst}` and `session_index.jsonl` | Current `openai/codex` rollout/compression source for both physical representations and `CODEX_HOME` | Codex version/commit plus fixtures checked against a disposable emitted/compressed session or current source fixture |
| OpenCode | global `opencode db` inventory; `opencode export <id>` | Current official OpenCode CLI documentation and `anomalyco/opencode` command/session-schema source, including root-only limitation of `session list` | Installed version plus complete multi-project/root/child/archive inventory comparison |
| pi | `${PI_CODING_AGENT_SESSION_DIR:-${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/sessions}/**/*.jsonl` | Current official pi session/settings documentation and repository source | Installed version plus a session-format fixture matching the documented version |

- Update matrix links and tested versions during release verification instead of treating planning-time observations as permanent guarantees.
- Report the best-effort remote harness version with each result. Show unavailable versions as `unknown`; this does not block raw filesystem collection, but the matrix must not claim that version as verified.
- Smoke tests must exercise each installed harness's actual discovery path without copying personal transcripts. Use disposable homes and generated sessions where supported; otherwise compare current upstream source/schema fixtures and record the limitation.

## Dependencies and exclusions

- Depends on the SSH/protocol/archive pipeline.
- Storage formats are external facts and can change. Tests prove the supported surfaces, while errors prevent silent false-success.
- Transcript parsing, schema migration, content redaction, and semantic deduplication remain out of scope.
