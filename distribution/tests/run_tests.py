#!/usr/bin/env python3
"""Run the real manifest generator and installer against file:// release trees.

Hermetic: no network. Set DISTRIBUTION_TEST_SH to pick the shell (default sh).
"""
import glob
import json
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import unittest

sys.dont_write_bytecode = True
os.environ["PYTHONDONTWRITEBYTECODE"] = "1"
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MANIFEST = os.path.join(ROOT, "distribution", "manifest.py")
INSTALL = os.path.join(ROOT, "distribution", "install.sh")
SHELL = os.environ.get("DISTRIBUTION_TEST_SH", "sh")
TARGETS = ("linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64")
KEYS = ("schema", "version", "tag", "assets", "sha256")


def host_target():
    system, machine = os.uname().sysname, os.uname().machine
    os_name = {"Linux": "linux", "Darwin": "macos"}[system]
    arch = {"x86_64": "x86_64", "amd64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}[machine]
    return "%s-%s" % (os_name, arch)


def write_file(path, text, mode=0o644):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    os.chmod(path, mode)


def fake_binary(reported):
    return "#!/bin/sh\necho '%s'\n" % reported


def run_manifest(*args):
    return subprocess.run(
        [sys.executable, "-S", MANIFEST, *args], capture_output=True, text=True
    )


class TempCase(unittest.TestCase):
    def setUp(self):
        # Resolved: macOS temp directories sit behind a /var symlink, and the
        # installer reports physical paths.
        self.tmp = os.path.realpath(tempfile.mkdtemp(prefix="glurp-dist-test-"))
        self.addCleanup(shutil.rmtree, self.tmp, True)


class ManifestTests(TempCase):
    def binaries(self):
        d = os.path.join(self.tmp, "bins")
        for i, t in enumerate(TARGETS):
            write_file(os.path.join(d, "glurp-" + t), "binary %d\n" % i, 0o755)
        return d

    def generate(self, base="https://example.test/releases"):
        out = os.path.join(self.tmp, "glurp-manifest.json")
        r = run_manifest("--tag", "v1.2.3", "--base-url", base, "--dir", self.binaries(), "--output", out)
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(out) as f:
            return f.read()

    def test_manifest_fields_and_digests(self):
        import hashlib

        text = self.generate()
        data = json.loads(text)
        self.assertEqual(tuple(data), KEYS)
        self.assertEqual(data["schema"], 1)
        self.assertEqual(data["version"], "1.2.3")
        self.assertEqual(data["tag"], "v1.2.3")
        self.assertEqual(tuple(data["assets"]), TARGETS)
        self.assertEqual(tuple(data["sha256"]), TARGETS)
        for i, t in enumerate(TARGETS):
            self.assertEqual(
                data["assets"][t], "https://example.test/releases/download/v1.2.3/glurp-" + t
            )
            want = hashlib.sha256(("binary %d\n" % i).encode()).hexdigest()
            self.assertEqual(data["sha256"][t], want)
        self.assertTrue(text.endswith("}\n"))

    def test_manifest_line_shape(self):
        lines = self.generate().split("\n")
        for block, value in (("assets", r"https://[^\"\n]+"), ("sha256", "[0-9a-f]{64}")):
            start = lines.index('  "%s": {' % block)
            end = min(i for i in range(start, len(lines)) if lines[i] in ("  }", "  },"))
            body = lines[start + 1 : end]
            self.assertEqual(len(body), len(TARGETS))
            for line, t in zip(body, TARGETS):
                self.assertRegex(line, r'^    "%s": "%s",?$' % (t, value))

    def test_manifest_strips_trailing_slash(self):
        data = json.loads(self.generate("https://example.test/releases///"))
        self.assertEqual(
            data["assets"]["linux-x86_64"],
            "https://example.test/releases/download/v1.2.3/glurp-linux-x86_64",
        )

    def test_manifest_missing_binary(self):
        d = self.binaries()
        os.remove(os.path.join(d, "glurp-macos-x86_64"))
        os.remove(os.path.join(d, "glurp-linux-aarch64"))
        r = run_manifest("--tag", "v1.2.3", "--base-url", "https://x", "--dir", d,
                         "--output", os.path.join(self.tmp, "m.json"))
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("glurp-macos-x86_64", r.stderr)
        self.assertIn("glurp-linux-aarch64", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.tmp, "m.json")))

    def test_manifest_malformed_tag(self):
        for tag in ("1.2.3", "v1.2", "v1.2.3-rc1", "v1.2.3\n", "vx.y.z", "v01.2.3", "v1.2.00"):
            r = run_manifest("--tag", tag, "--base-url", "https://x", "--dir", self.binaries(),
                             "--output", os.path.join(self.tmp, "m.json"))
            self.assertNotEqual(r.returncode, 0, tag)
            self.assertIn("invalid tag", r.stderr, tag)


class InstallerTests(TempCase):
    def setUp(self):
        super().setUp()
        self.target = host_target()
        self.releases = os.path.join(self.tmp, "releases")
        self.home = os.path.join(self.tmp, "home")
        self.dest = os.path.join(self.tmp, "dest")
        os.makedirs(self.home)
        self.base = "file://" + self.releases

    def release(self, tag, reported=None, latest=False, base=None):
        """Create binaries and manifest for tag; return the manifest path."""
        reported = reported or "glurp " + tag
        bins = os.path.join(self.releases, "download", tag)
        for t in TARGETS:
            write_file(os.path.join(bins, "glurp-" + t), fake_binary(reported), 0o755)
        place = os.path.join(self.releases, "latest/download" if latest else "download/" + tag)
        manifest = os.path.join(place, "glurp-manifest.json")
        os.makedirs(place, exist_ok=True)
        r = run_manifest("--tag", tag, "--base-url", base or self.base, "--dir", bins, "--output", manifest)
        self.assertEqual(r.returncode, 0, r.stderr)
        return manifest

    def edit(self, manifest, fn):
        with open(manifest) as f:
            text = f.read()
        with open(manifest, "w") as f:
            f.write(fn(text))

    def env(self, **extra):
        env = {k: v for k, v in os.environ.items() if not k.startswith("GLURP_")}
        env["HOME"] = self.home
        env["GLURP_RELEASE_BASE_URL"] = self.base
        env.update(extra)
        return env

    def install(self, env=None, **extra):
        env = env if env is not None else self.env(GLURP_INSTALL_DIR=self.dest, **extra)
        return subprocess.run([SHELL, INSTALL], env=env, capture_output=True, text=True, timeout=60)

    def installed(self, base_dir=None):
        return os.path.join(base_dir or self.dest, "glurp")

    def leftovers(self, d=None):
        return glob.glob(os.path.join(d or self.dest, ".glurp-install.*"))

    def put_existing(self):
        write_file(self.installed(), "old glurp\n", 0o755)

    def assert_untouched(self):
        with open(self.installed()) as f:
            self.assertEqual(f.read(), "old glurp\n")
        self.assertEqual(self.leftovers(), [])

    def test_latest_install(self):
        self.release("v9.9.9", latest=True)
        r = self.install()
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.installed()) as f:
            self.assertEqual(f.read(), fake_binary("glurp v9.9.9"))
        self.assertTrue(os.stat(self.installed()).st_mode & stat.S_IXUSR)
        self.assertIn("Installed glurp v9.9.9", r.stdout)
        self.assertEqual(self.leftovers(), [])

    def test_default_directory(self):
        self.release("v9.9.9", latest=True)
        r = self.install(env=self.env())
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertTrue(os.path.isfile(os.path.join(self.home, ".local", "bin", "glurp")))

    def test_pinned_version(self):
        self.release("v9.9.9", latest=True)
        self.release("v9.9.8")
        r = self.install(GLURP_INSTALL_VERSION="v9.9.8")
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.installed()) as f:
            self.assertEqual(f.read(), fake_binary("glurp v9.9.8"))

    def test_invalid_pinned_version(self):
        self.release("v9.9.9", latest=True)
        for bad in ("9.9.9", "v9.9", "v9.9.9/../x", "v9.9.9 ", "v1.2.3\nx", "v09.9.9"):
            r = self.install(GLURP_INSTALL_VERSION=bad)
            self.assertNotEqual(r.returncode, 0, repr(bad))
            self.assertIn("GLURP_INSTALL_VERSION", r.stderr)

    def test_replace_existing(self):
        self.release("v9.9.9", latest=True)
        self.put_existing()
        r = self.install()
        self.assertEqual(r.returncode, 0, r.stderr)
        with open(self.installed()) as f:
            self.assertEqual(f.read(), fake_binary("glurp v9.9.9"))

    def test_checksum_mismatch(self):
        self.release("v9.9.9", latest=True)
        write_file(os.path.join(self.releases, "download", "v9.9.9", "glurp-" + self.target),
                   fake_binary("glurp v9.9.9") + "# tampered\n", 0o755)
        self.put_existing()
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("checksum", r.stderr)
        self.assert_untouched()

    def test_smoke_mismatch(self):
        self.release("v9.9.9", reported="glurp v0.0.0", latest=True)
        self.put_existing()
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("glurp v0.0.0", r.stderr)
        self.assert_untouched()

    def test_smoke_failure(self):
        manifest = self.release("v9.9.9", latest=True)
        bins = os.path.join(self.releases, "download", "v9.9.9")
        write_file(os.path.join(bins, "glurp-" + self.target), "#!/bin/sh\nexit 7\n", 0o755)
        r = run_manifest("--tag", "v9.9.9", "--base-url", self.base,
                         "--dir", bins, "--output", manifest)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.put_existing()
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("does not run", r.stderr)
        self.assert_untouched()

    def test_invalid_digest(self):
        for bad in ("", "0" * 63, "0" * 65, "G" * 64, "A" * 64):
            m = self.release("v9.9.9", latest=True)
            with open(m) as f:
                digest = json.load(f)["sha256"][self.target]
            self.edit(m, lambda text: text.replace(digest, bad))
            self.put_existing()
            r = self.install()
            self.assertNotEqual(r.returncode, 0, repr(bad))
            self.assertIn("invalid sha256", r.stderr)
            self.assert_untouched()

    def test_inconsistent_tag(self):
        m = self.release("v9.9.9", latest=True)
        self.edit(m, lambda text: text.replace('"tag": "v9.9.9"', '"tag": "v9.9.8"'))
        self.put_existing()
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("tag does not match", r.stderr)
        self.assert_untouched()

    def test_curl_configuration_is_ignored(self):
        self.release("v9.9.9", latest=True)
        curl_home = os.path.join(self.tmp, "curl-home")
        # This changes curl's output unless -q is the first argument.
        write_file(os.path.join(curl_home, ".curlrc"), 'write-out = "UNEXPECTED-CURL-CONFIG"\n')
        r = self.install(CURL_HOME=curl_home)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("UNEXPECTED-CURL-CONFIG", r.stdout + r.stderr)
        self.assertEqual(self.leftovers(), [])

    def test_target_missing(self):
        m = self.release("v9.9.9", latest=True)
        self.edit(m, lambda t: re.sub(r'^.*"%s".*\n' % self.target, "", t, flags=re.M))
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn(self.target, r.stderr)
        self.assertFalse(os.path.exists(self.installed()))

    def test_unsupported_schema(self):
        m = self.release("v9.9.9", latest=True)
        self.edit(m, lambda t: t.replace('"schema": 1', '"schema": 2'))
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("schema", r.stderr)

    def test_asset_url_outside_base(self):
        m = self.release("v9.9.9", latest=True)
        self.edit(m, lambda t: t.replace(self.base, "file:///elsewhere"))
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("asset URL", r.stderr)

    def test_asset_url_path_escape(self):
        m = self.release("v9.9.9", latest=True)
        self.edit(m, lambda t: t.replace("/download/v9.9.9/glurp-" + self.target,
                                         "/download/../x/glurp-" + self.target))
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("asset URL", r.stderr)

    def test_pinned_tag_mismatch(self):
        m = self.release("v9.9.8")
        self.edit(m, lambda t: t.replace('"version": "9.9.8"', '"version": "9.9.7"')
                                .replace('"tag": "v9.9.8"', '"tag": "v9.9.7"'))
        r = self.install(GLURP_INSTALL_VERSION="v9.9.8")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("does not match", r.stderr)

    def test_base_trailing_slash(self):
        self.release("v9.9.9", latest=True)
        a = self.install()
        self.assertEqual(a.returncode, 0, a.stderr)
        with open(self.installed(), "rb") as f:
            first = f.read()
        shutil.rmtree(self.dest)
        b = self.install(GLURP_RELEASE_BASE_URL=self.base + "//")
        self.assertEqual(b.returncode, 0, b.stderr)
        with open(self.installed(), "rb") as f:
            self.assertEqual(f.read(), first)

    def test_bad_base_scheme(self):
        r = self.install(GLURP_RELEASE_BASE_URL="http://example.invalid/releases")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("https:// or file://", r.stderr)
        self.assertFalse(os.path.exists(self.dest))

    def test_not_on_path(self):
        self.release("v9.9.9", latest=True)
        r = self.install()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("is not on your PATH", r.stderr)
        self.assertIn('export PATH="%s:$PATH"' % self.dest, r.stderr)

    def test_on_path_no_warning(self):
        self.release("v9.9.9", latest=True)
        env = self.env(GLURP_INSTALL_DIR=self.dest)
        env["PATH"] = self.dest + os.pathsep + env["PATH"]
        r = self.install(env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertNotIn("not on your PATH", r.stderr)
        self.assertNotIn("shadows", r.stderr)

    def test_shadowing_warning(self):
        self.release("v9.9.9", latest=True)
        other = os.path.join(self.tmp, "other")
        write_file(os.path.join(other, "glurp"), "#!/bin/sh\n", 0o755)
        env = self.env(GLURP_INSTALL_DIR=self.dest)
        env["PATH"] = other + os.pathsep + self.dest + os.pathsep + env["PATH"]
        r = self.install(env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("shadows", r.stderr)

    def test_destination_is_a_directory(self):
        self.release("v9.9.9", latest=True)
        os.makedirs(self.installed())
        r = self.install()
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("is a directory", r.stderr)
        self.assertEqual(os.listdir(self.installed()), [])
        self.assertEqual(self.leftovers(), [])

    def test_install_dir_spelling(self):
        """A trailing slash or a relative directory names the same install."""
        self.release("v9.9.9", latest=True)
        env = self.env(GLURP_INSTALL_DIR=self.dest + "//")
        env["PATH"] = self.dest + os.pathsep + env["PATH"]
        r = self.install(env=env)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stderr, "")
        self.assertIn("to %s/glurp" % self.dest, r.stdout)

        r = subprocess.run([SHELL, INSTALL], env=self.env(GLURP_INSTALL_DIR="relative"), cwd=self.tmp,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(r.returncode, 0, r.stderr)
        relative = os.path.join(self.tmp, "relative")
        self.assertTrue(os.path.isfile(os.path.join(relative, "glurp")))
        self.assertIn('export PATH="%s:$PATH"' % relative, r.stderr)

    def test_hash_tool_fallbacks(self):
        """Each supported SHA-256 tool verifies the download when it is the only one."""
        self.release("v9.9.9", latest=True)
        needed = ("uname", "curl", "awk", "mkdir", "mktemp", "chmod", "mv", "rm")
        ran = []
        for hasher in ("sha256sum", "shasum", "openssl"):
            if not shutil.which(hasher):
                continue
            tools = os.path.join(self.tmp, "tools-" + hasher)
            os.makedirs(tools)
            for name in needed + (hasher,):
                os.symlink(shutil.which(name), os.path.join(tools, name))
            dest = os.path.join(self.tmp, "dest-" + hasher)
            env = self.env(GLURP_INSTALL_DIR=dest)
            env["PATH"] = tools
            r = subprocess.run([shutil.which(SHELL), INSTALL], env=env, capture_output=True, text=True, timeout=60)
            self.assertEqual(r.returncode, 0, "%s: %s" % (hasher, r.stderr))
            with open(os.path.join(dest, "glurp")) as f:
                self.assertEqual(f.read(), fake_binary("glurp v9.9.9"))
            ran.append(hasher)
        self.assertTrue(ran, "no SHA-256 tool on PATH")
        print("hash tools exercised: %s" % ", ".join(ran))

    def test_trap_list(self):
        with open(INSTALL) as f:
            text = f.read()
        self.assertRegex(text, r"(?m)^\s*trap cleanup EXIT$")
        self.assertRegex(text, r"(?m)^\s*trap 'cleanup; exit 1' INT TERM HUP$")
        self.assertTrue(text.rstrip("\n").endswith('main "$@"'))

    def test_interrupted_run(self):
        self.release("v9.9.9", latest=True)
        real = shutil.which("curl")
        self.assertTrue(real)
        shim = os.path.join(self.tmp, "shim")
        write_file(os.path.join(shim, "curl"),
                   '#!/bin/sh\nfor a in "$@"; do case $a in */.glurp-install.*) exec sleep 60;; esac; done\n'
                   'exec "%s" "$@"\n' % real, 0o755)
        env = self.env(GLURP_INSTALL_DIR=self.dest)
        env["PATH"] = shim + os.pathsep + env["PATH"]
        p = subprocess.Popen([SHELL, INSTALL], env=env, start_new_session=True,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.time() + 10
            while not self.leftovers() and time.time() < deadline:
                time.sleep(0.05)
            self.assertTrue(self.leftovers(), "installer never created its temp file")
            time.sleep(0.3)
            os.killpg(p.pid, signal.SIGTERM)
            self.assertNotEqual(p.wait(timeout=10), 0)
        finally:
            if p.poll() is None:
                os.killpg(p.pid, signal.SIGKILL)
                p.wait()
        self.assertEqual(self.leftovers(), [])
        self.assertFalse(os.path.exists(self.installed()))


class StaticTests(unittest.TestCase):
    def test_shell_syntax(self):
        r = subprocess.run([SHELL, "-n", INSTALL], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_shellcheck(self):
        if not shutil.which("shellcheck"):
            self.skipTest("shellcheck not found on PATH")
        for script in (INSTALL,):
            r = subprocess.run(["shellcheck", "-s", "sh", script], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_installer_is_executable(self):
        self.assertTrue(os.stat(INSTALL).st_mode & stat.S_IXUSR)


if __name__ == "__main__":
    print("installer shell: %s" % SHELL)
    result = unittest.TextTestRunner(verbosity=2).run(unittest.defaultTestLoader.loadTestsFromModule(sys.modules[__name__]))
    sys.exit(not result.wasSuccessful())
