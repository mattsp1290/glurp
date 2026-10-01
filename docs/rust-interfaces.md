# Rust archive interfaces

The Rust binary currently collects Codex. The existing Go implementation is
historical behavior evidence and is never invoked by Rust. Additional harnesses
and build-system cutover are subsequent milestone slices.

Configuration is a JSON array of host objects (`name`, `destination`) at
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
`F NUL relative-path NUL decimal-byte-count NUL exact-payload`, then `E NUL`
and EOF. No newline separators or text transformations occur. Paths must be
UTF-8, at most 4096 bytes, relative, and contain no empty, dot, parent, newline,
carriage-return or backslash components. Sizes are unsigned decimal, bounded to
256 MiB per artifact, 1 GiB per stream and 100000 artifacts. Duplicate paths,
unknown frames, truncation, overflow and trailing bytes fail validation.

`archive::stage(reader, directory)` validates a complete stream into private
staging. `archive::collect` publishes only after SSH succeeds and staging passes.
Unchanged artifacts preserve exact bytes and mtimes. Changed artifacts are
individually replaced by same-filesystem rename; missing remote artifacts remain.
The next safety slice must add transaction-wide rollback for local publication
failures, configurable limits/timeouts, cancellation tests and descriptor-relative
filesystem traversal to close same-user parent replacement races. Current locks
coordinate cooperating Glurp processes; they do not prevent another process
running as the same user from racing pathname checks. Source checksums detect
ordinary source changes during transfer but remote filesystem traversal also
has same-user races. Automatic missing Codex roots produce an empty successful
collection in this first slice; richer not-found/required-root reporting belongs
to the multi-harness slice.

For that extension, add a terminal status frame before `E` (for example
`T NUL ok|not-found|failed NUL`) and have `stage` return a typed status with the
artifact paths. The remote discovery branch can then emit `not-found` while an
existing but empty root emits `ok`; explicitly configured roots must fail if
missing or unreadable. Keep configured roots in host configuration as separate
per-harness arrays, shell-quote each literal root, and prefix multiple roots with
stable `root-0001/` artifact namespaces to avoid collisions. Validate names and
required-root errors before publishing, without reflecting remote messages.
