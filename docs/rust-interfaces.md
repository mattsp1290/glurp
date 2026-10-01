# Rust archive interfaces

The Rust binary collects Claude Code, Codex, pi, and OpenCode. The existing Go
implementation is historical behavior evidence and is never invoked by Rust.
Build-system cutover remains a subsequent milestone slice.

Configuration is a JSON array of host objects (`name`, `destination`, and optional `claude_paths`,
`codex_paths`, `pi_paths` arrays) at
`$XDG_CONFIG_HOME/glurp/config.json`, falling back to `$HOME/.config/glurp`.
Archives live at `$XDG_DATA_HOME/glurp/hosts/<name>/<harness>/<relative artifact>`,
falling back to `$HOME/.local/share/glurp`. Host removal retains archives. A
`destination.json` beside the harness directories binds an archive name to its
SSH destination. Config and archive operations use separate exclusive locks.
All managed directories are 0700 and files are 0600. Symlinked or untrusted
parents, nonregular files and hardlinked managed files are rejected.

`remote::fetch(destination, script, spool)` invokes OpenSSH with BatchMode=yes
and ConnectTimeout=15 and sends a fixed POSIX sh script on stdin. It suppresses
remote stderr and spools at most 1 GiB to a private temporary file. The operation
has a 300 second timeout; process-group termination also closes inherited pipes.
Other harnesses should implement fixed scripts or rigorously quoted literal
arguments without evaluating configured paths.

Wire format version 1 is `GLURP1 NUL`, followed by zero or more artifacts:
`F NUL relative-path NUL decimal-byte-count NUL exact-payload`, then exactly one `T NUL ok|not-found NUL`, followed by `E NUL`
and EOF. No newline separators or text transformations occur. Paths must be
UTF-8, at most 4096 bytes, relative, and contain no empty, dot, parent, newline,
carriage-return or backslash components. Sizes are unsigned decimal, bounded to
256 MiB per artifact, 1 GiB per stream and 100000 artifacts. Duplicate paths,
unknown frames, truncation, overflow and trailing bytes fail validation.

`archive::stage(reader, directory)` validates a complete stream into private
staging and returns artifact paths and a found/not-found status. `archive::collect` publishes only after SSH succeeds and staging passes.
Unchanged artifacts preserve exact bytes and mtimes. Changed artifacts are
individually replaced by same-filesystem rename; missing remote artifacts remain.
The next safety slice must add transaction-wide rollback for local publication
failures, configurable limits/timeouts, cancellation tests and descriptor-relative
filesystem traversal to close same-user parent replacement races. Current locks
coordinate cooperating Glurp processes; they do not prevent another process
running as the same user from racing pathname checks. Source checksums detect
ordinary source changes during transfer but remote filesystem traversal also
has same-user races. Automatic absent harnesses return not-found success; configured missing roots or
detected executables with unresolved source data fail with redacted guidance.

Configured roots are literal absolute paths or exact `~/` prefixes; only that
prefix expands to the remote HOME. No shell expansion or evaluation occurs.
Explicit roots override environment/default discovery and use stable
`root-0001/`, `root-0002/` namespaces even with one root. Claude roots contain
`projects/`; Codex roots contain `sessions/` and/or `archived_sessions/`; pi roots
are session directories themselves. Remote ancestor symlinks are rejected and
find does not follow descendant symlinks.

OpenCode uses two SSH operations: a global database inventory, validated locally,
then a single batch of exports. `opencode db path` must resolve to an existing,
readable nonsymlink database before the global SQL query runs. Inventory rows
must contain exactly one unique safe `ses_` ID; each export must be one JSON
object with matching `info.id` and an array of messages (each having object info
and array parts). Every export validates before any export is published.
Inventory is an internal staging artifact and is never archived. Transcript
bytes are preserved; these structural checks do not normalize or reserialize.
