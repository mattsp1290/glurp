# Harness compatibility

Slurp preserves native bytes and deliberately avoids depending on transcript-message schemas. Discovery and inventory surfaces can still change upstream, so every release must update the evidence below and run disposable fixtures without reading personal transcripts.

| Harness | Contract used by Slurp | Upstream evidence | Development verification (2026-09-10) |
| --- | --- | --- | --- |
| Claude Code | `${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects/**/*.jsonl`, including nested subagents | [Claude Code settings](https://code.claude.com/docs/en/settings) and [Anthropic Claude Code](https://github.com/anthropics/claude-code) | Embedded collector run under Linux `/bin/sh` against synthetic native JSONL with locally installed Claude Code 2.1.268 available for version probing. Not yet a release certification. |
| Codex | `sessions/`, `archived_sessions/`, and `session_index.jsonl` under `CODEX_HOME`, preserving `.jsonl` and `.jsonl.zst` | [OpenAI Codex repository](https://github.com/openai/codex) | Embedded collector run under Linux `/bin/sh` against plain, archived, and compressed-representation fixtures with codex-cli 0.154.0 available for version probing. Not yet a release certification. |
| OpenCode | Global `SELECT id FROM session ORDER BY id` inventory through `opencode db`, then `opencode export <id>` | [OpenCode CLI documentation](https://opencode.ai/docs/cli/) and [OpenCode repository](https://github.com/anomalyco/opencode) | Synthetic database inventory with root/child IDs and one validated JSON export per ID; local OpenCode 1.15.6 was present but no personal database was queried. Not yet a release certification. |
| pi | `$PI_CODING_AGENT_SESSION_DIR`, `$PI_CODING_AGENT_DIR/sessions`, or `$HOME/.pi/agent/sessions` JSONL | [pi-mono repository](https://github.com/badlogic/pi-mono) | Embedded collector run under Linux `/bin/sh` against a synthetic nested JSONL tree. pi was not locally installed. Not yet a release certification. |

Linux CI runs all unit and end-to-end fake-SSH fixtures. macOS CI executes the same embedded collector, archive, and integration tests under macOS `/bin/sh`. Before tagging a release, record the exact upstream commits or versions exercised, run a disposable real SSH server containing all four synthetic source surfaces twice, and confirm byte hashes plus unchanged modification times on the second run.

An unavailable version probe does not block raw filesystem collection and is reported as `unknown`; it must not be presented as a verified version. OpenCode schema or export incompatibility is a hard failure because a partial inventory would be misleading.
