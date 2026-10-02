# Harness compatibility

Glurp preserves native bytes and deliberately avoids depending on transcript-message schemas. Discovery and inventory surfaces can still change upstream, so every release must update the evidence below and run disposable fixtures without reading personal transcripts.

| Harness | Contract used by Glurp | Upstream evidence | Rust synthetic verification (2026-10-01) |
| --- | --- | --- | --- |
| Claude Code | `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/**/*.jsonl`, including nested subagents | [Claude Code settings](https://code.claude.com/docs/en/settings) and [Anthropic Claude Code](https://github.com/anthropics/claude-code) | Synthetic nested subagent JSONL preserved bytewise by POSIX sh fixture. |
| Codex | `sessions/`, `archived_sessions/`, and `session_index.jsonl` under `CODEX_HOME`, preserving `.jsonl` and `.jsonl.zst` | [OpenAI Codex repository](https://github.com/openai/codex) | Synthetic raw, archived, compressed bytes and index preserved by POSIX sh fixture. |
| OpenCode | Global `SELECT id FROM session ORDER BY id` inventory through `opencode db`, then `opencode export <id>` | [OpenCode CLI documentation](https://opencode.ai/docs/cli/) and [OpenCode repository](https://github.com/anomalyco/opencode) | Fake inventory includes root and child IDs; all exports validated before publication. No personal database queried. |
| pi | `$PI_CODING_AGENT_SESSION_DIR`, `$PI_CODING_AGENT_DIR/sessions`, or `$HOME/.pi/agent/sessions` JSONL | [pi-mono repository](https://github.com/badlogic/pi-mono) | Synthetic nested JSONL and both environment overrides exercised by POSIX sh fixture. |

The Rust CI matrix runs all-target unit and end-to-end fake-SSH fixtures on Linux and macOS, including the embedded POSIX sh scripts. Linux-only preload fault injection additionally requires a C compiler. Before tagging a release, record the exact upstream commits or versions exercised, run a disposable real SSH server containing all four synthetic source surfaces twice, and confirm byte hashes plus unchanged modification times on the second run.

Rust does not run harness version probes. Executable presence only informs absent-data discovery; it must not be presented as version certification. OpenCode schema or export incompatibility is a hard failure because a partial inventory would be misleading.

The Rust collector defaults to all four harnesses. Repeat `--harness claude`,
`--harness codex`, `--harness pi`, or `--harness opencode` to select a subset.
`host add` accepts repeated `--claude-path`, `--codex-path`, and `--pi-path`.
Claude paths are configuration roots containing `projects/` (not the projects
directory itself); Codex paths are archive roots containing `sessions/`; pi paths
are the session directories. Explicit roots replace discovery and are archived
under stable `root-0001/` namespaces. Absolute paths and literal `~/` roots may
contain spaces and quotes; only the exact `~/` prefix expands. Environment
fallbacks are CLAUDE_CONFIG_DIR, CODEX_HOME, PI_CODING_AGENT_SESSION_DIR, then
PI_CODING_AGENT_DIR/sessions. Missing automatic data is not-found when its
executable is absent; missing required roots or installed but unresolved sources
fail. No source path or remote stderr is echoed in failure summaries.

OpenCode's official upstream commit
`aa481b8f5652f5576c55f914a64ed270e7daa7e0` was re-fetched on 2026-10-01,
byte-matching cached evidence and SHA-256 hashes. Its
[db command](https://github.com/anomalyco/opencode/blob/aa481b8f5652f5576c55f914a64ed270e7daa7e0/packages/opencode/src/cli/cmd/db.ts)
provides `db path` and JSON query output; its
[session table](https://github.com/anomalyco/opencode/blob/aa481b8f5652f5576c55f914a64ed270e7daa7e0/packages/core/src/session/sql.ts)
includes parent IDs, so Glurp queries `SELECT id FROM session ORDER BY id`
without a parent filter. Its
[export command](https://github.com/anomalyco/opencode/blob/aa481b8f5652f5576c55f914a64ed270e7daa7e0/packages/opencode/src/cli/cmd/export.ts)
emits `{info, messages}` JSON for an explicit session ID. Glurp validates the
whole inventory and export batch before publishing; malformed JSON, schema or
identity mismatch, duplicates, and command failures fail visibly. Exports retain
exact original JSON bytes, including formatting.

Rust fixtures use isolated canonical HOME/XDG roots, a utility-only fake remote
PATH, fake OpenCode, and fake SSH that executes the embedded POSIX sh scripts.
They exercise nested Claude subagents, Codex raw/compressed/index artifacts,
nested pi, all environment overrides, repeated literal roots, remote symlinks,
absent and required sources, and root/child OpenCode exports and failure batches.
No installed harness or personal database is queried. Local test evidence establishes Linux fixture compatibility. The CI matrix is
configured for both platforms; an actual macOS CI run and disposable real SSH
verification must be recorded separately before claiming final acceptance.
Transaction-wide rollback, configurable bounds, cancellation, descriptor-relative
traversal, and explicit local recovery are implemented and documented in
[rust-safety.md](rust-safety.md).
