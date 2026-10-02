# Private cumulative archive safety

`glurp glurp` collects each selected host/harness independently. Hosts and
harnesses are sorted and deduplicated. A failure produces a fixed, redacted
summary and a nonzero exit code while other tasks continue. Remote stderr,
transcript bytes, remote artifact paths, and JSON parser excerpts never appear
in collection diagnostics. SIGINT/SIGTERM stop collection with a nonzero exit.

Unchanged artifacts retain their exact bytes and mtimes. Changed artifacts use
same-filesystem atomic rename. Remote deletion, removal of a configured host,
and collection of a different harness never delete committed transcripts.
`destination.json` permanently binds a used archive name to its SSH destination;
remove/add cannot repurpose that archive name for another destination.

## Limits and timeout

The collection command accepts:

| Flag | Default | Scope |
| --- | --- | --- |
| `--max-file-bytes` | 268435456 | One artifact, including an OpenCode inventory |
| `--max-files` | 100000 | Artifacts in one host/harness task |
| `--max-total-bytes` | 1073741824 | Combined payload bytes in one host/harness task |
| `--timeout-seconds` | 300 | Whole host/harness task, starting before SSH |

For OpenCode, inventory and all exports share the same count, total byte,
transport, and time budgets. The inventory consumes one artifact in addition to
one artifact per exported session. Zero byte/count limits are supported; a zero
timeout is rejected. Limits and wire-ceiling calculations use checked arithmetic.
A separate finite wire ceiling is payload limit + 4130 bytes per permitted
artifact + 128 bytes of terminal/protocol overhead; it applies cumulatively to
both OpenCode SSH operations. JSON and frame validation occur before publication.
The private spool and staged artifacts together use up to twice the bounded
incoming stream size. Backups additionally need space for replaced originals.
Recovery metadata is capped at 512 MiB *before* any artifact is changed; a task
whose serialized journal exceeds that ceiling fails without publication.

SSH runs in its own process group with `BatchMode=yes` and `ConnectTimeout=15`.
Nonblocking stdin/stdout and a 20ms poll loop bound script delivery, stderr floods,
endless streams, and inherited pipes. There are no blocking pipe-worker joins.
Timeout/error/cancellation closes pipes, kills the SSH group, and reaps SSH.
A descendant that deliberately creates another process group can outlive group
termination, but cannot hold collection open beyond the task deadline.

## Filesystem and transactions

Managed files are owned, singly linked regular files with mode 0600. Managed
directories are owned directories with mode 0700. Ancestor directories must be
owned by the current user or root; writable shared ancestors require the sticky
bit. Storage paths must be absolute without traversal components. Symlinks,
nonregular files, hardlinked files, duplicate artifact paths, option-like SSH
destinations, and shell characters in destinations are rejected. Configured
remote source roots are quoted literal arguments, including spaces/apostrophes;
they are never evaluated as shell code.

All local config, lock, binding, staging, backup, recovery, and publication
operations traverse from open directory descriptors using `openat` with
`O_NOFOLLOW`, `mkdirat`, `renameat`, and `unlinkat`. Publication retains archive and staging root descriptors, traverses target
parents with no-follow opens at each operation, and records file fingerprints.
Descriptor use is bounded independently of artifact count. Replacing a checked pathname with a symlink
cannot redirect subsequent operations to its target. Config and host archive
locks refuse concurrent cooperating writers immediately. Ordinary unexpected
file replacement or content modification is checked before replacement.

The archive lock protects a per-host `.transaction` directory. The entire
artifact batch is validated and its targets preflighted first. Originals are
copied with preserved timestamps to private, synced backups. A bounded journal
is atomically written and synced before the first artifact replacement. Every
replacement syncs its source and target directory. On a late failure, recovery
restores *all* originals and removes newly introduced artifacts. Backups remain
until restoration completes; a failed rollback retains the journal/backups and
refuses further publication until storage is repaired.

Commit atomically renames the complete synced rollback journal from `ready.json`
to `committed`, then syncs the transaction directory. This successful directory
sync is the commit boundary. Only the live publisher that verified that sync
may delete backups as successful-commit cleanup; a cleanup error does not turn
that successful collection into a failed operation.

Restart recovery distinguishes these states under the archive lock, before SSH:

| Visible state | Automatic action |
| --- | --- |
| `ready.json`, `.rollback-required`, or `.recovery` with a journal | Preflight and restore all originals; retain backups on error |
| `committed` without known rollback authority | Refuse; preserve current artifacts, journal, and all remaining backups |
| Staging without a journal | Remove staging; no artifact was published |

A committed journal on restart is ambiguous: its directory sync may have failed,
or the publisher may have succeeded and stopped during cleanup. Failed commit
handling attempts to restore the ready name, move the parent to `.recovery`, or
record `.rollback-required`; none of those fallible operations is necessary for
safety. If all fail, automatic recovery still refuses to delete originals.

Use `glurp recover HOST --rollback` to explicitly choose local rollback of a
retained transaction. This can discard a possibly successful newest generation.
The command verifies the configured host destination binding, takes the archive
lock, and never contacts SSH. It validates the entire journal, every target path,
and every required original backup before changing any target. For an ambiguous
committed journal it establishes rollback authority after that preflight and
before restoring anything. Backups remain until every original is durably
restored; errors retain recovery evidence and permit retry. Recovery retires the
journal only after restoration completes.

Successful-commit cleanup can delete some backups before failing. In that case
explicit rollback refuses before changing any target because complete restoration
is impossible. Manual resolution is required: stop collection for that host,
preserve a private copy of its entire archive directory (including hidden
transaction directories, journal, remaining backups, and timestamps), inspect the
journal and both generations, and restore missing originals from an independently
verified copy if rollback is desired. If choosing to retain the current generation,
verify all journal targets before retiring the transaction manually. Never remove
remaining evidence merely to silence recovery errors; there is no automatic
incomplete-backup rollback or cleanup command. Storage failure may prevent
restoration, but never authorizes deletion of originals.

Ordinary file comparison and backup preparation check the task deadline and
cancellation between bounded chunks (64 KiB for copies). Rollback copies are
also bounded per read/write but deliberately ignore a cancelled or expired
collection deadline so committed originals can still be restored.

These guarantees protect against untrusted paths and ordinary failures, and
coordinate writers using Glurp's locks. A malicious process with the same UID
can still modify already open file contents, move entire owned directories, or
remove journals/backups; Unix permissions cannot isolate mutually hostile
processes sharing one account. Descriptor anchoring prevents arbitrary symlink
redirection; it does not provide an account-level security boundary. Remote
POSIX collectors reject symlinks but cannot eliminate remote same-user races.
Local operation deadlines are checked throughout stream/JSON processing and
publication; a filesystem syscall stalled inside the kernel cannot be forcibly
bounded by the userspace deadline. Permanently unavailable storage may require
repair before retained backups can be restored.

## Synthetic verification

All tests use canonical disposable roots, synthetic local/remote HOME/XDG,
and a utility-only fake remote PATH. CLI fixtures execute actual POSIX collector
scripts and cover exact bytes, private modes, mtime preservation, cumulative
retention, binding, limits (including OpenCode's inventory/export aggregate),
cooperating lock refusal, malformed/truncated/overflow frames, stderr flooding,
timeout, SIGINT/SIGTERM, reaped SSH, and an escaped inherited pipe. Deterministic
unit fixtures force a real later rename failure after both an update and a new
artifact have published, then verify full rollback and original mtimes. Recovery
fixtures model interruption and an obstructed rollback, retain original backups,
and retry recovery. Linux fault regressions compile a small test-only preload
helper with the host C compiler: they inject repeated sync failures through full
collection, fail the reverse transition, fault committed cleanup, and slow bounded
local comparison/backup operations to verify timeout and both cancellation signals
without large disk writes. Directory-swap fixtures prove anchored writes cannot
escape through substituted symlinks.
