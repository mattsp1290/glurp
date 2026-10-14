# Releasing Glurp

Glurp follows the install and update pattern used by
[beans](https://github.com/mattsp1290/beans): GitHub Releases is the sole default
distribution origin, a POSIX installer bootstraps the CLI, and `glurp upgrade`
replaces an installed binary. No package-registry release is configured.

Releases are produced by `.github/workflows/release.yml` from an annotated
`vX.Y.Z` tag on `main`. Pushing a tag publishes a release and requires the
repository owner's approval. The initial rollout needs a successful dry run
and the first approved release before the documented latest install URL works.

## Release assets

Each release contains six raw, unarchived assets:

- `glurp-linux-x86_64` and `glurp-linux-aarch64`: static musl binaries.
- `glurp-macos-x86_64` and `glurp-macos-aarch64`: macOS binaries.
- `glurp-manifest.json`: release version, asset URLs, and SHA-256 digests.
- `install.sh`: the tagged copy of `distribution/install.sh`.

Every binary reports `glurp vX.Y.Z`. Both consumers fetch the manifest from
`<base>/latest/download/glurp-manifest.json`, or
`<base>/download/<tag>/glurp-manifest.json` for a pinned release. The default
base is `https://github.com/mattsp1290/glurp/releases`; `GLURP_RELEASE_BASE_URL`
overrides it with an HTTPS mirror or a local `file://` release tree.

## Manifest contract (schema 1)

`distribution/manifest.py` writes the manifest from all four built binaries:

```json
{
  "schema": 1,
  "version": "0.1.0",
  "tag": "v0.1.0",
  "assets": {
    "linux-x86_64": "https://github.com/mattsp1290/glurp/releases/download/v0.1.0/glurp-linux-x86_64"
  },
  "sha256": {
    "linux-x86_64": "<64 lowercase hex>"
  }
}
```

The abbreviated example shows one target; published manifests contain the four
targets listed above. `version` is canonical `X.Y.Z`, and `tag` is `v` plus
that version. `assets` and `sha256` name the same targets. Each asset URL must
equal `<base>/download/<tag>/glurp-<target>` exactly; consumers reject a
different URL or schema. Digests are 64 lowercase hex characters.

Formatting is part of the shell installer's contract: two-space indentation,
one key per line, `"assets": {` and `"sha256": {` on their own lines, and no
quote or newline inside strings. Always use the generator.

The manifest and binaries share an origin. SHA-256 protects against corruption,
not compromise of the release origin. Artifact attestations are not produced.

## Installation and upgrades

The installer uses `curl`, `awk`, and an available SHA-256 utility. It selects
the host target, verifies the manifest and digest, checks the downloaded
binary's version, then renames a temporary file within the install directory
over `glurp`. It defaults to `$HOME/.local/bin`, supports `GLURP_INSTALL_DIR`
and `GLURP_INSTALL_VERSION`, and never uses sudo or edits shell profiles.
It prints PATH and OpenSSH guidance when needed.

`glurp upgrade` uses the same manifest and target names, with an in-process
SHA-256 check. It stages the binary beside the current executable, runs its
version check with a deadline, syncs the file, and atomically replaces the
executable only after validation succeeds. Download, checksum, version, or
smoke-test errors preserve the installed binary. Temporary files are removed
on normal failure; stale upgrade temporaries are cleaned on subsequent runs.

- `glurp upgrade --check` reports without replacing the executable.
- `--json` prints `current`, `latest`, `target`, `path`, `update_available`,
  and `upgraded`; update availability alone is not an error.
- `--version vX.Y.Z` selects a pinned release, including an older rollback.
- `--force` allows replacement of Cargo-installed, build-tree, current, or
  incomparable development builds. Without it, rebuild managed binaries with
  `make install` or `make build`.
- Versions are compared numerically. A newer local version is not downgraded
  by an unpinned, unforced upgrade. Git-describe development versions compare
  from their base release tag.

Upgrade runs before host configuration is opened and never touches archives or
SSH. In-progress collection processes keep running their old executable.
Uninstalling the binary leaves configuration and archives intact.

## Workflow and dry run

The workflow serializes tag runs and uses these stages:

1. `validate` requires an annotated tag on `main`, equal to `Cargo.toml`, with
   no already-published release for that tag and no equal or newer stable release.
2. `ci` invokes the normal Linux/macOS formatting, lint, tests, build, and
   distribution-test gates for that commit.
3. `build` compiles four targets with `GLURP_VERSION` set to the selected tag,
   checks version/help, runs the installer suite, native upgrade tests, and a
   selected real collector journey. The full Linux fault-injection suite runs
   in normal GNU CI because static musl cannot use its `LD_PRELOAD` shim.
4. `package` gathers the four binaries, manifest, and installer.
5. `publish` creates a draft, uploads/verifies assets, and publishes a prerelease.
6. `smoke` performs pinned HTTPS install and forced in-place upgrade on all
   four target platforms, then checks version/help.
7. `promote` refuses if a same-or-newer stable release has appeared, promotes
   the release to latest, and checks the documented latest install and JSON
   update-check paths.

Drafts and prereleases are ignored by `releases/latest`. Default installs and
upgrades therefore see a release only after the four-platform smoke stage.
The x86_64 macOS binary is built and executed through Rosetta on the arm runner.

```sh
gh workflow run release.yml -f version=v0.0.0
```

Manual dispatch is a nonpublishing dry run: CI, four builds, and packaging run;
validation, publication, public HTTPS smoke, and promotion are skipped. Run it
after changes to release inputs and before tagging. The tagged commit's
workflow is used for real releases, so rerunning a tag cannot pick up workflow
fixes from a later commit.

## Release checklist and recovery

1. Set the version in `Cargo.toml` and the `glurp` entry in `Cargo.lock`. Merge
   through a normal pull request with `make check` green.
2. Confirm a dry run is green on the intended `main` commit.
3. With the owner's approval, create and push an annotated tag:
   `git tag -a vX.Y.Z -m "glurp vX.Y.Z"`, then `git push origin vX.Y.Z`.
4. Watch the workflow and verify the published release has six assets, is
   neither draft nor prerelease, and is the latest release.
5. Confirm installer, `glurp --version`, `glurp upgrade` (up to date), and
   `glurp upgrade --check --json` work.

Never delete or move a pushed tag without the owner's approval. If a dry run
fails, fix and rerun it. Before publication, transient tag-run failures can be
retried; a code/workflow fix needs the next patch version. Failed publication
may leave a draft, which a retry replaces. Failed smoke leaves a prerelease
that latest installs ignore; remove it with approval, fix, and release the
next patch. If promotion fails after marking latest, restore the previous
latest release or remove the faulty release with approval. Do not rerun a
superseded tag's publication/promotion jobs.

A killed installer may leave `.glurp-install.*` in its destination; remove it
manually. Users can roll back with `glurp upgrade --version <older-tag>` or
`GLURP_INSTALL_VERSION=<older-tag>` through the installer.

## Adding a target

Change these together: `distribution/manifest.py`, the installer uname mapping,
`distribution/tests/run_tests.py`, `src/upgrade/manifest.rs`, the release build
and smoke matrices and published asset list, and the platform lists in this
document and README.
