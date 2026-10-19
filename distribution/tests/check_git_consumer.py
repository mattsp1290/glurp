#!/usr/bin/env python3
"""Build/run the committed standalone consumer with an exact Git dependency.

Local example:
  python3 -S distribution/tests/check_git_consumer.py \
    --repository . --revision "$(git rev-parse HEAD)"
Remote example (after the authorized branch push):
  python3 -S distribution/tests/check_git_consumer.py \
    --repository https://github.com/mattsp1290/glurp --revision FULL_COMMIT_SHA

The source repository is read-only. No Cargo path patch/override is used, and
fixture source is copied from the requested commit, never the working tree.
The fixture itself proves that `glurp` cannot be executed from its isolated PATH.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def run(*arguments: str, cwd: Path | None = None) -> str:
    result = subprocess.run(arguments, cwd=cwd, check=True, text=True, capture_output=True)
    return result.stdout.strip()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True, help="local Git checkout or HTTPS Git URL")
    parser.add_argument("--revision", required=True, help="exact, full 40-character commit SHA")
    arguments = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", arguments.revision):
        parser.error("--revision must be the exact lowercase 40-character commit SHA")
    revision = arguments.revision
    local = Path(arguments.repository).expanduser()

    with tempfile.TemporaryDirectory(prefix="glurp-git-consumer-") as temporary:
        temporary_path = Path(temporary)
        if local.exists():
            source = local.resolve()
            # Only committed Git objects below enter the consumer. Unrelated
            # worktree/review artifacts cannot affect this exact-revision proof.
            repository = source.as_uri()
        else:
            if not arguments.repository.startswith("https://"):
                parser.error("a remote repository must use HTTPS")
            repository = arguments.repository
            source = temporary_path / "source"
            source.mkdir()
            run("git", "init", "--quiet", str(source))
            run("git", "remote", "add", "origin", repository, cwd=source)
            run("git", "fetch", "--quiet", "--depth=1", "origin", revision, cwd=source)
        resolved = run("git", "rev-parse", "--verify", f"{revision}^{{commit}}", cwd=source)
        if resolved != revision:
            raise SystemExit("The resolved commit does not match the requested revision.")

        consumer = temporary_path / "consumer"
        consumer.mkdir()
        tracked = run("git", "ls-tree", "-r", "--name-only", revision, "--", "tests/consumer", cwd=source)
        if not tracked:
            raise SystemExit("The requested commit does not contain tests/consumer.")
        for name in tracked.splitlines():
            relative = Path(name).relative_to("tests/consumer")
            if ".." in relative.parts or relative.is_absolute():
                raise SystemExit("Unsafe committed fixture path.")
            destination = consumer / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            result = subprocess.run(["git", "show", f"{revision}:{name}"], cwd=source, check=True, capture_output=True)
            destination.write_bytes(result.stdout)

        manifest = consumer / "Cargo.toml"
        original = manifest.read_text()
        path_dependency = 'glurp = { path = "../.." }'
        if original.count(path_dependency) != 1:
            raise SystemExit("Expected exactly one standalone fixture path dependency to replace.")
        manifest.write_text(original.replace(path_dependency,
            f"glurp = {{ git = {json.dumps(repository)}, rev = {json.dumps(revision)} }}"))

        # Keep the committed dependency versions. Only the glurp package's
        # source changes from a path to the exact Git revision; --locked below
        # verifies this is a complete and valid lockfile, without re-resolution.
        lock = consumer / "Cargo.lock"
        text = lock.read_text()
        pattern = r'(\[\[package\]\]\nname = "glurp"\nversion = "[^"\n]+"\n)'
        source_line = f"source = {json.dumps(f'git+{repository}?rev={revision}#{revision}')}\n"
        text, count = re.subn(pattern, lambda match: match[1] + source_line, text)
        if count != 1:
            raise SystemExit("Expected exactly one glurp entry in the committed consumer lockfile.")
        lock.write_text(text)
        environment = dict(os.environ)
        environment["CARGO_TARGET_DIR"] = str(temporary_path / "target")
        # This directory is outside the source tree and has no .cargo/config or
        # patch section. Cargo must fetch and compile the specified Git package.
        metadata_command = ["cargo", "metadata", "--manifest-path", str(manifest),
                            "--locked", "--format-version", "1"]
        metadata = json.loads(subprocess.run(metadata_command, cwd=consumer, env=environment,
                                             check=True, text=True, capture_output=True).stdout)
        packages = [package for package in metadata["packages"] if package["name"] == "glurp"]
        expected_source = f"git+{repository}?rev={revision}#{revision}"
        if len(packages) != 1 or packages[0].get("source") != expected_source:
            raise SystemExit("Cargo resolved glurp through an override instead of the requested Git revision.")
        command = ["cargo", "run", "--manifest-path", str(manifest), "--locked", "--bin", "glurp-library-consumer"]
        subprocess.run(command, cwd=consumer, env=environment, check=True)
        print(json.dumps({"repository": repository, "tested_revision": revision,
                          "dependency": "git", "installed_glurp": False}, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        if error.stdout:
            print(error.stdout.decode() if isinstance(error.stdout, bytes) else error.stdout, end="")
        if error.stderr:
            print(error.stderr.decode() if isinstance(error.stderr, bytes) else error.stderr, end="")
        raise SystemExit(f"Git consumer verification failed (exit {error.returncode}).") from error
