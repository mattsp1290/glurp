# Configuration and host registry

## Goal and prerequisite state

Implement a versioned, private, atomically updated registry of named OpenSSH destinations. The CLI foundation and its dependency interfaces must exist.

## Repository evidence

No configuration format or path is established. Compatibility is not required, so the first schema can start at version 1 without migration from `slurpy`.

## Exact change surface

- `internal/paths/paths.go` — **new**, under new parent `internal/paths/`. Define proposed `Resolve(configOverride, dataOverride string, environ map[string]string) (Paths, error)`.
- `internal/config/model.go` — **new**, under new parent `internal/config/`. Define proposed `Config`, `Host`, `SourceOverrides`, and schema-version validation.
- `internal/config/store.go` — **new**. Define proposed `Store` with `Load`, `AddHost`, `RemoveHost`, and atomic locked update behavior.
- `internal/config/store_test.go` — **new**. Test schema, permissions, concurrency, corruption, and atomic replacement.
- `internal/paths/paths_test.go` — **new**. Test XDG defaults, overrides, missing home, and path normalization.
- `internal/cli/host_test.go` — **new**. Test command-visible host CRUD and stable output.

All paths and symbols are proposed. Their parent insertion point is the repository root.

## Version 1 schema

```json
{
  "version": 1,
  "hosts": [
    {
      "name": "buildbox",
      "destination": "buildbox",
      "sources": {
        "claude": [],
        "codex": [],
        "pi": []
      }
    }
  ]
}
```

- An empty source-root list means automatic discovery. A nonempty list replaces automatic discovery and contains remote absolute or `~/`-relative roots. Every listed root is required, which supports pi project/extension layouts and other customized installations without silently skipping an asserted location.
- OpenCode has no path override because collection uses its CLI.
- JSON rejects unknown fields so spelling mistakes cannot silently disable a source.
- Host names match `^[a-z0-9][a-z0-9._-]*$`, are unique, and are capped at 64 bytes.
- Destinations are nonempty, contain no ASCII control characters or whitespace, do not begin with `-`, and are capped at 512 bytes. Users express complex options through `~/.ssh/config`, not this string. SSH authentication must work in BatchMode.
- Source overrides reject NUL and control characters. The remote script expands only an exact leading `~/`; it does not evaluate shell metacharacters or environment-variable syntax.

## Path rules

- Default config: `${XDG_CONFIG_HOME}/slurp/config.json` when `XDG_CONFIG_HOME` is absolute; otherwise `$HOME/.config/slurp/config.json`.
- Default data: `${XDG_DATA_HOME}/slurp` when `XDG_DATA_HOME` is absolute; otherwise `$HOME/.local/share/slurp`.
- CLI `--config` and `--data-dir` override environment defaults after absolute-path resolution.
- Missing home or a relative XDG value falls back only as defined above; if no absolute base can be resolved, fail with an actionable path error.
- Create config and data directories with mode `0700`. Create config files with mode `0600`. Before use, `Lstat` every Slurp-owned existing path, reject non-regular files and symlinked path components, and tighten broader directory/file modes to `0700`/`0600`; if ownership or filesystem policy prevents tightening, fail with an actionable error.

## Persistence and concurrency

1. Open or create a lock file beside `config.json` and acquire an exclusive advisory lock for mutations.
2. Re-read and validate after taking the lock.
3. Serialize deterministic indented JSON plus one trailing newline.
4. Write a unique sibling temporary file with mode `0600`.
5. Flush, `fsync` the file, rename over `config.json`, and `fsync` the parent directory where supported.
6. Remove the temporary file on every failure path.

Reads must reject malformed JSON, unsupported versions, duplicate names, and invalid values. An absent config is equivalent to an empty version-1 registry for `host add`, but `slurp slurp` reports “no hosts configured.” Do not silently replace a corrupt config.

## Host command behavior

- `host add <name> <destination>` adds a new record. A duplicate name fails and points to remove/re-add; no implicit overwrite occurs.
- Source overrides are repeatable optional flags: `--claude-path`, `--codex-path`, and `--pi-path`.
- A configured source override is an assertion that the source exists. A missing, non-directory, or unreadable override is `failed`, never `not-found`, and makes the aggregate collection exit nonzero.
- `host list` succeeds for an empty registry and prints no table rows plus a concise hint. `--json` emits `[]`.
- `host remove <name>` fails for an unknown name and never touches the archive tree.
- Every mutation reports the resolved config path but never reads or writes SSH credentials.

## Tests and acceptance

- Redirect config/data paths into `t.TempDir()` in every test.
- Verify add/list/remove round trips, deterministic ordering, duplicate rejection, validation boundaries, unknown-field rejection, and schema-version rejection.
- Race two store mutations and verify the resulting file is valid and contains both successful operations or an explicit conflict, never truncated JSON.
- Inject failures before rename and verify the prior config remains byte-identical.
- On Unix, verify effective modes are no broader than `0700`/`0600` under a permissive umask.
- Verify removing a host leaves a sentinel archive file unchanged.
- Verify a nonexistent or unreadable explicit source override is retained in config but produces a collection failure, not a successful skip.
- Preseed config/data paths with broad modes and assert they are tightened; assert a symlink or unfixable mode fails before content is read.

## Risks and exclusions

- Advisory locking is local-machine coordination only. Network filesystems with broken locks are unsupported.
- The destination grammar intentionally excludes inline `ssh -p ...` forms. OpenSSH aliases cover that use case safely.
- Do not add config migration logic until a second schema exists.
