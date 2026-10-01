use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::process::{Command, Output};

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    remote: PathBuf,
    bin: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let canonical = temp.path().canonicalize().unwrap();
        let home = canonical.join("local");
        let remote = canonical.join("remote");
        let bin = canonical.join("bin");
        for dir in [&home, &remote, &bin] {
            fs::create_dir(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        for utility in [
            "sh", "cat", "cksum", "find", "mktemp", "rm", "wc", "touch", "sleep",
        ] {
            let system = ["/bin", "/usr/bin"]
                .into_iter()
                .map(|directory| PathBuf::from(directory).join(utility))
                .find(|path| path.is_file())
                .unwrap();
            std::os::unix::fs::symlink(system, bin.join(utility)).unwrap();
        }
        let ssh = bin.join("ssh");
        // Execute the actual embedded script under POSIX sh, with a remote HOME.
        fs::write(
            &ssh,
            r#"#!/bin/sh
[ "$1" = -o ] && [ "$2" = BatchMode=yes ] || exit 81
[ "$3" = -o ] && [ "$4" = ConnectTimeout=15 ] || exit 82
[ "$6" = 'sh -s' ] || exit 83
case "$5" in bad) printf 'private remote error' >&2; exit 9;; esac
export HOME="$REMOTE_HOME"
unset CODEX_HOME CLAUDE_CONFIG_DIR PI_CODING_AGENT_SESSION_DIR PI_CODING_AGENT_DIR XDG_DATA_HOME XDG_CONFIG_HOME
exec /bin/sh -s
"#,
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        Self {
            _temp: temp,
            home,
            remote,
            bin,
        }
    }

    fn process(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_glurp"));
        command
            .env_clear()
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("REMOTE_HOME", &self.remote)
            .env("PATH", &self.bin);
        command
    }

    fn command(&self, args: &[&str]) -> Output {
        self.process(args).output().unwrap()
    }

    fn ok(&self, args: &[&str]) -> Output {
        let out = self.command(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    fn source(&self, path: &str, bytes: &[u8]) {
        let file = self.remote.join(".codex").join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, bytes).unwrap();
    }

    fn archive(&self, path: &str) -> PathBuf {
        self.home.join("data/glurp/hosts/lab/codex").join(path)
    }
}

#[test]
fn actual_posix_collector_preserves_bytes_permissions_and_retention() {
    let f = Fixture::new();
    f.ok(&["--help"]);
    assert!(String::from_utf8_lossy(&f.ok(&["glurp", "--help"]).stdout).contains("Collect remote"));
    assert!(!f.command(&["glurp"]).status.success());
    f.ok(&["host", "add", "lab", "lab"]);
    let files: [(&str, &[u8]); 5] = [
        ("sessions/2026/10/raw.jsonl", b"{\"raw\":true}\n"),
        (
            "sessions/with space/compressed.jsonl.zst",
            b"\x28\xb5\x2f\xfd\0\xffraw",
        ),
        ("archived_sessions/deep/old.jsonl", b"old\n"),
        ("archived_sessions/old.jsonl.zst", b"\0\x80compressed"),
        ("session_index.jsonl", b"index\n"),
    ];
    for (path, bytes) in files {
        f.source(path, bytes);
    }
    f.source("sessions/ignore.txt", b"ignored");
    f.ok(&["glurp", "lab", "--harness", "codex"]);
    for (path, bytes) in files {
        let file = f.archive(path);
        assert_eq!(fs::read(&file).unwrap(), bytes);
        assert_eq!(fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            fs::metadata(file.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        fs::metadata(f.home.join("config/glurp/config.json"))
            .unwrap()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!f.archive("sessions/ignore.txt").exists());
    let before = fs::metadata(f.archive(files[0].0))
        .unwrap()
        .modified()
        .unwrap();
    f.ok(&["glurp"]);
    assert_eq!(
        before,
        fs::metadata(f.archive(files[0].0))
            .unwrap()
            .modified()
            .unwrap()
    );
    f.source(files[0].0, b"changed\n");
    fs::remove_file(f.remote.join(".codex").join(files[2].0)).unwrap();
    f.ok(&["glurp"]);
    assert_eq!(fs::read(f.archive(files[0].0)).unwrap(), b"changed\n");
    assert_eq!(fs::read(f.archive(files[2].0)).unwrap(), files[2].1);
    f.ok(&["host", "remove", "lab"]);
    assert!(f.archive(files[0].0).exists());
    assert!(
        !f.command(&["host", "add", "lab", "different"])
            .status
            .success()
    );
    f.ok(&["host", "add", "lab", "lab"]);
    assert!(String::from_utf8_lossy(&f.ok(&["host", "list"]).stdout).contains("lab\tlab"));
    assert!(!f.command(&["glurp", "unknown"]).status.success());
}

#[test]
fn failed_host_does_not_prevent_good_host_and_diagnostics_are_suppressed() {
    let f = Fixture::new();
    f.source("session_index.jsonl", b"synthetic\n");
    f.ok(&["host", "add", "bad", "bad"]);
    f.ok(&["host", "add", "lab", "lab"]);
    let out = f.command(&["glurp"]);
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("private remote error"));
    assert_eq!(
        fs::read(f.archive("session_index.jsonl")).unwrap(),
        b"synthetic\n"
    );
    let repeated = f.command(&["glurp"]);
    assert_eq!(out.stdout, repeated.stdout);
    assert_eq!(out.stderr, repeated.stderr);
}

#[test]
fn symlinks_and_injection_are_rejected() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    for destination in ["-oProxyCommand=evil", "lab;evil", "lab\nother", "$(evil)"] {
        assert!(
            !f.command(&["host", "add", "lab", destination])
                .status
                .success()
        );
    }
    f.ok(&["host", "add", "lab", "lab"]);
    f.source("sessions/good.jsonl", b"good");
    fs::write(f.remote.join("secret.jsonl"), b"secret").unwrap();
    symlink(
        f.remote.join("secret.jsonl"),
        f.remote.join(".codex/sessions/link.jsonl"),
    )
    .unwrap();
    f.ok(&["glurp"]);
    assert!(!f.archive("sessions/link.jsonl").exists());
    let old = f.archive("sessions/good.jsonl");
    fs::remove_file(&old).unwrap();
    symlink(f.remote.join("secret.jsonl"), &old).unwrap();
    assert!(!f.command(&["glurp"]).status.success());
    assert_eq!(fs::read(f.remote.join("secret.jsonl")).unwrap(), b"secret");
}

#[test]
fn malformed_successful_transport_retains_prior_archive() {
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    f.source("session_index.jsonl", b"original");
    f.ok(&["glurp"]);
    // A producer may exit successfully while still emitting an invalid frame.
    fs::write(f.bin.join("ssh"), "#!/bin/sh\ncat >/dev/null\nprintf 'GLURP1\\000F\\000session_index.jsonl\\0007\\000changedF\\000../escape\\0001\\000xE\\000'\n").unwrap();
    assert!(!f.command(&["glurp"]).status.success());
    assert_eq!(
        fs::read(f.archive("session_index.jsonl")).unwrap(),
        b"original"
    );
    let base = f.home.join("data/glurp/hosts/lab");
    assert!(fs::read_dir(&base).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".stage-")
    }));
    assert!(!base.join("escape").exists());
}

impl Fixture {
    fn write_remote(&self, path: &str, bytes: &[u8]) {
        let path = self.remote.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn opencode(&self, inventory: &str, export: &str) {
        self.write_remote("opencode.db", b"synthetic database marker");
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\ndb) if [ \"$2\" = path ]; then printf '%s/opencode.db\\n' \"$HOME\"; exit 0; fi\n[ \"$2\" = 'SELECT id FROM session ORDER BY id' ] && [ \"$3\" = --format ] && [ \"$4\" = json ] || exit 90\ncat <<'JSON'\n{inventory}\nJSON\n;;\nexport)\n{export}\n;;\n*) exit 91;; esac\n"
        );
        let path = self.bin.join("opencode");
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
}

#[test]
fn all_four_collectors_preserve_native_nested_bytes() {
    let f = Fixture::new();
    f.source("sessions/raw.jsonl.zst", b"\0\xffnative");
    f.write_remote(
        ".claude/projects/project/session/subagents/agent.jsonl",
        b"claude\n\0",
    );
    f.write_remote(
        ".pi/agent/sessions/project/nested/session.jsonl",
        b"pi\n\xff",
    );
    f.opencode(
        r#"[{"id":"ses_root"},{"id":"ses_child"}]"#,
        r#"printf '{"info":{"id":"%s"},"messages":[]}\n' "$2""#,
    );
    f.ok(&["host", "add", "lab", "lab"]);
    let output = f.ok(&["glurp"]);
    assert!(String::from_utf8_lossy(&output.stdout).contains("opencode: collected 2"));
    let base = f.home.join("data/glurp/hosts/lab");
    assert_eq!(
        fs::read(base.join("claude/projects/project/session/subagents/agent.jsonl")).unwrap(),
        b"claude\n\0"
    );
    assert_eq!(
        fs::read(base.join("pi/project/nested/session.jsonl")).unwrap(),
        b"pi\n\xff"
    );
    for id in ["ses_root", "ses_child"] {
        assert_eq!(
            fs::read(base.join(format!("opencode/{id}.json"))).unwrap(),
            format!("{{\"info\":{{\"id\":\"{id}\"}},\"messages\":[]}}\n").as_bytes()
        );
    }
    assert!(!base.join("opencode/inventory.json").exists());
}

#[test]
fn repeated_literal_roots_tilde_and_selected_harnesses() {
    let f = Fixture::new();
    let root = "odd ' $(touch INJECTED) space";
    f.write_remote(&format!("{root}/projects/nested/agent.jsonl"), b"first");
    f.write_remote("second/projects/agent.jsonl", b"second");
    f.write_remote("pi custom/nested/session.jsonl", b"pi");
    f.ok(&[
        "host",
        "add",
        "lab",
        "lab",
        "--claude-path",
        &format!("~/{root}"),
        "--claude-path",
        "~/second",
        "--pi-path",
        "~/pi custom",
    ]);
    f.ok(&["glurp", "--harness", "claude", "--harness", "pi"]);
    let base = f.home.join("data/glurp/hosts/lab");
    assert_eq!(
        fs::read(base.join("claude/root-0001/projects/nested/agent.jsonl")).unwrap(),
        b"first"
    );
    assert_eq!(
        fs::read(base.join("claude/root-0002/projects/agent.jsonl")).unwrap(),
        b"second"
    );
    assert_eq!(
        fs::read(base.join("pi/root-0001/nested/session.jsonl")).unwrap(),
        b"pi"
    );
    assert!(!base.join("codex").exists());
    assert!(!f.remote.join("INJECTED").exists());
    // Invalid harness parsing happens before transport (replace ssh with a marker).
    fs::write(
        f.bin.join("ssh"),
        format!(
            "#!/bin/sh\ntouch '{}'\n",
            f.home.join("ssh-called").display()
        ),
    )
    .unwrap();
    assert!(!f.command(&["glurp", "--harness", "bogus"]).status.success());
    assert!(!f.home.join("ssh-called").exists());
}

#[test]
fn missing_roots_detection_and_independent_harnesses() {
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    assert_eq!(
        String::from_utf8_lossy(&f.ok(&["glurp"]).stdout)
            .matches("not-found")
            .count(),
        4
    );
    fs::write(f.bin.join("claude"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(f.bin.join("claude"), fs::Permissions::from_mode(0o700)).unwrap();
    f.source("session_index.jsonl", b"good");
    let out = f.command(&["glurp"]);
    assert!(!out.status.success());
    assert_eq!(fs::read(f.archive("session_index.jsonl")).unwrap(), b"good");
    f.ok(&["host", "remove", "lab"]);
    f.ok(&[
        "host",
        "add",
        "lab",
        "lab",
        "--pi-path",
        "~/missing private source",
    ]);
    let out = f.command(&["glurp", "--harness", "pi"]);
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains("missing private source"));
}

#[test]
fn opencode_invalid_inventory_and_exports_never_publish_partial_batch() {
    for inventory in [
        "{}",
        "[{}]",
        r#"[{"id":"../bad"}]"#,
        r#"[{"id":"ses_a"},{"id":"ses_a"}]"#,
        r#"[{"id":7}]"#,
    ] {
        let f = Fixture::new();
        f.opencode(inventory, "exit 99");
        f.ok(&["host", "add", "lab", "lab"]);
        assert!(
            !f.command(&["glurp", "--harness", "opencode"])
                .status
                .success()
        );
        assert!(!f.home.join("data/glurp/hosts/lab/opencode").exists());
    }
    for export in [
        "exit 5",
        "printf 'not json'",
        r#"printf '{"info":{"id":"ses_wrong"},"messages":[]}'"#,
        r#"printf '{"info":{"id":"%s"},"messages":{}}' "$2""#,
        r#"[ "$2" = ses_b ] && printf '{}' || printf '{"info":{"id":"%s"},"messages":[]}' "$2""#,
    ] {
        let f = Fixture::new();
        f.opencode(r#"[{"id":"ses_a"},{"id":"ses_b"}]"#, export);
        f.ok(&["host", "add", "lab", "lab"]);
        assert!(
            !f.command(&["glurp", "--harness", "opencode"])
                .status
                .success()
        );
        assert!(!f.home.join("data/glurp/hosts/lab/opencode").exists());
    }
}

#[test]
fn environment_overrides_and_remote_symlink_roots() {
    let f = Fixture::new();
    f.write_remote("claude override/projects/sub/child.jsonl", b"claude");
    f.write_remote("codex override/session_index.jsonl", b"codex");
    f.write_remote("pi override/deep/session.jsonl", b"pi");
    let ssh = f.bin.join("ssh");
    let script = fs::read_to_string(&ssh).unwrap().replace(
        "exec /bin/sh -s",
        r#"export CLAUDE_CONFIG_DIR="$HOME/claude override"
export CODEX_HOME="$HOME/codex override"
export PI_CODING_AGENT_SESSION_DIR="$HOME/pi override"
exec /bin/sh -s"#,
    );
    fs::write(&ssh, script).unwrap();
    f.ok(&["host", "add", "lab", "lab"]);
    f.ok(&["glurp"]);
    assert_eq!(
        fs::read(f.archive("session_index.jsonl")).unwrap(),
        b"codex"
    );
    assert_eq!(
        fs::read(f.home.join("data/glurp/hosts/lab/pi/deep/session.jsonl")).unwrap(),
        b"pi"
    );
    // PI_CODING_AGENT_DIR is the fallback when SESSION_DIR is unset.
    f.write_remote("agent override/sessions/another.jsonl", b"agent");
    let script = fs::read_to_string(&ssh).unwrap().replace(
        "export PI_CODING_AGENT_SESSION_DIR=\"$HOME/pi override\"",
        "export PI_CODING_AGENT_DIR=\"$HOME/agent override\"",
    );
    fs::write(&ssh, script).unwrap();
    f.ok(&["glurp", "--harness", "pi"]);
    assert_eq!(
        fs::read(f.home.join("data/glurp/hosts/lab/pi/another.jsonl")).unwrap(),
        b"agent"
    );
    std::os::unix::fs::symlink(
        f.remote.join("codex override"),
        f.remote.join("linked root"),
    )
    .unwrap();
    f.ok(&["host", "remove", "lab"]);
    f.ok(&["host", "add", "lab", "lab", "--codex-path", "~/linked root"]);
    assert!(!f.command(&["glurp", "--harness", "codex"]).status.success());
}

#[test]
fn installed_opencode_with_missing_database_fails_without_query() {
    let f = Fixture::new();
    f.opencode("[]", "exit 1");
    fs::remove_file(f.remote.join("opencode.db")).unwrap();
    f.ok(&["host", "add", "lab", "lab"]);
    assert!(
        !f.command(&["glurp", "--harness", "opencode"])
            .status
            .success()
    );
    assert!(!f.home.join("data/glurp/hosts/lab/opencode").exists());
}

#[test]
fn repeated_absolute_codex_roots_override_automatic_discovery() {
    let f = Fixture::new();
    f.source("session_index.jsonl", b"automatic ignored");
    f.write_remote(
        "first codex/sessions/nested/session.jsonl.zst",
        b"\x28\xb5\x2f\xfdnative",
    );
    f.write_remote("second codex/session_index.jsonl", b"second index");
    let first = f.remote.join("first codex");
    let second = f.remote.join("second codex");
    f.ok(&[
        "host",
        "add",
        "lab",
        "lab",
        "--codex-path",
        first.to_str().unwrap(),
        "--codex-path",
        second.to_str().unwrap(),
    ]);
    f.ok(&["glurp", "--harness", "codex"]);
    assert_eq!(
        fs::read(f.archive("root-0001/sessions/nested/session.jsonl.zst")).unwrap(),
        b"\x28\xb5\x2f\xfdnative"
    );
    assert_eq!(
        fs::read(f.archive("root-0002/session_index.jsonl")).unwrap(),
        b"second index"
    );
    assert!(!f.archive("session_index.jsonl").exists());
}

#[test]
fn trailing_root_separators_preserve_explicit_and_automatic_artifacts() {
    for suffix in ["/", "//", "////"] {
        for explicit in [false, true] {
            let f = Fixture::new();
            f.write_remote(
                "claude source/projects/p/subagents/session.jsonl",
                b"claude bytes",
            );
            f.write_remote("codex source/sessions/p/session.jsonl.zst", b"codex bytes");
            f.write_remote("pi source/p/deep/session.jsonl", b"pi bytes");
            if explicit {
                f.ok(&[
                    "host",
                    "add",
                    "lab",
                    "lab",
                    "--claude-path",
                    &format!("~/claude source{suffix}"),
                    "--codex-path",
                    &format!("~/codex source{suffix}"),
                    "--pi-path",
                    &format!("~/pi source{suffix}"),
                ]);
            } else {
                f.ok(&["host", "add", "lab", "lab"]);
                let ssh = f.bin.join("ssh");
                let script = fs::read_to_string(&ssh).unwrap().replace("exec /bin/sh -s", &format!("export CLAUDE_CONFIG_DIR=\"$HOME/claude source{suffix}\"\nexport CODEX_HOME=\"$HOME/codex source{suffix}\"\nexport PI_CODING_AGENT_SESSION_DIR=\"$HOME/pi source{suffix}\"\nexec /bin/sh -s"));
                fs::write(ssh, script).unwrap();
            }
            f.ok(&[
                "glurp",
                "--harness",
                "claude",
                "--harness",
                "codex",
                "--harness",
                "pi",
            ]);
            let prefix = if explicit { "root-0001/" } else { "" };
            let base = f.home.join("data/glurp/hosts/lab");
            for (harness, path, bytes) in [
                (
                    "claude",
                    "projects/p/subagents/session.jsonl",
                    b"claude bytes".as_slice(),
                ),
                (
                    "codex",
                    "sessions/p/session.jsonl.zst",
                    b"codex bytes".as_slice(),
                ),
                ("pi", "p/deep/session.jsonl", b"pi bytes".as_slice()),
            ] {
                assert_eq!(
                    fs::read(base.join(harness).join(format!("{prefix}{path}"))).unwrap(),
                    bytes
                );
            }
        }
    }
}

#[test]
fn slash_only_root_uses_relative_paths_with_controlled_find() {
    for root in ["/", "//", "////"] {
        let f = Fixture::new();
        f.write_remote("isolated.jsonl", b"isolated pi bytes");
        // Never traverse /. The fake find executes only the real collector's
        // embedded callback with one explicitly selected synthetic fixture.
        let find = f.bin.join("find");
        fs::remove_file(&find).unwrap();
        fs::write(
            &find,
            r#"#!/bin/sh
[ "$1" = / ] && [ "$2" = -type ] && [ "$3" = f ] && [ "$4" = -exec ] || exit 90
shift 4
shell=$1; shift
[ "$1" = -c ] || exit 91
shift
callback=$1; shift
[ "$1" = sh ] || exit 92
shift
exec "$shell" -c "$callback" sh "$1" "$2" "$3" "$HOME/isolated.jsonl"
"#,
        )
        .unwrap();
        fs::set_permissions(find, fs::Permissions::from_mode(0o700)).unwrap();
        f.ok(&["host", "add", "lab", "lab", "--pi-path", root]);
        f.ok(&["glurp", "--harness", "pi"]);
        let relative = f
            .remote
            .join("isolated.jsonl")
            .strip_prefix("/")
            .unwrap()
            .to_owned();
        assert_eq!(
            fs::read(
                f.home
                    .join("data/glurp/hosts/lab/pi/root-0001")
                    .join(relative)
            )
            .unwrap(),
            b"isolated pi bytes"
        );
    }
}

impl Fixture {
    fn fake_ssh(&self, body: &str) {
        fs::write(self.bin.join("ssh"), format!("#!/bin/sh\n{body}\n")).unwrap();
    }
    fn bounded(&self, args: &[&str], signal: Option<i32>) -> Output {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        use std::time::{Duration, Instant};
        let mut child = self
            .process(args)
            .process_group(0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        let mut signalled = false;
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if !signalled && started.elapsed() > Duration::from_millis(200) {
                if let Some(signal) = signal {
                    unsafe {
                        libc::kill(child.id() as i32, signal);
                    }
                }
                signalled = true;
            }
            if started.elapsed() > Duration::from_secs(5) {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                panic!("collector exceeded five-second fixture deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
}

#[test]
fn configured_limits_and_overflow_refuse_updates_and_preserve_mtimes() {
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    f.source("session_index.jsonl", b"old data");
    f.ok(&["glurp", "--harness", "codex"]);
    let before = fs::metadata(f.archive("session_index.jsonl"))
        .unwrap()
        .modified()
        .unwrap();
    f.source("session_index.jsonl", b"new data");
    for flags in [
        ["--max-file-bytes", "7"],
        ["--max-total-bytes", "7"],
        ["--max-files", "0"],
        ["--max-files", "18446744073709551615"],
        ["--max-total-bytes", "18446744073709551615"],
        ["--timeout-seconds", "0"],
    ] {
        assert!(
            !f.bounded(&["glurp", "--harness", "codex", flags[0], flags[1]], None)
                .status
                .success()
        );
        assert_eq!(
            fs::read(f.archive("session_index.jsonl")).unwrap(),
            b"old data"
        );
        assert_eq!(
            fs::metadata(f.archive("session_index.jsonl"))
                .unwrap()
                .modified()
                .unwrap(),
            before
        );
    }
    f.ok(&[
        "glurp",
        "--harness",
        "codex",
        "--max-file-bytes",
        "8",
        "--max-total-bytes",
        "8",
        "--max-files",
        "1",
    ]);
    assert_eq!(
        fs::read(f.archive("session_index.jsonl")).unwrap(),
        b"new data"
    );
}

#[test]
fn opencode_inventory_and_exports_share_count_and_byte_budgets() {
    let f = Fixture::new();
    let inventory = r#"[{"id":"ses_a"},{"id":"ses_b"}]"#;
    f.opencode(
        inventory,
        r#"printf '{"info":{"id":"%s"},"messages":[]}\n' "$2""#,
    );
    f.ok(&["host", "add", "lab", "lab"]);
    let export = "{\"info\":{\"id\":\"ses_a\"},\"messages\":[]}\n";
    let exact_total = (inventory.len() + 1 + export.len() * 2).to_string();
    f.ok(&[
        "glurp",
        "--harness",
        "opencode",
        "--max-files",
        "3",
        "--max-total-bytes",
        &exact_total,
    ]);
    let path = f.home.join("data/glurp/hosts/lab/opencode/ses_a.json");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    for flags in [
        ("--max-files", "2".to_string()),
        (
            "--max-total-bytes",
            (exact_total.parse::<usize>().unwrap() - 1).to_string(),
        ),
        ("--max-file-bytes", "20".to_string()),
    ] {
        assert!(
            !f.command(&["glurp", "--harness", "opencode", flags.0, &flags.1])
                .status
                .success()
        );
        assert_eq!(fs::read(&path).unwrap(), export.as_bytes());
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }
}

#[test]
fn cooperating_archive_writer_is_refused_before_ssh() {
    use fs2::FileExt;
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    f.source("session_index.jsonl", b"old");
    f.ok(&["glurp", "--harness", "codex"]);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.home.join("data/glurp/hosts/lab/archive.lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    f.fake_ssh(&format!("touch '{}'", f.home.join("ssh-called").display()));
    assert!(
        !f.bounded(&["glurp", "--harness", "codex"], None)
            .status
            .success()
    );
    assert!(!f.home.join("ssh-called").exists());
    assert_eq!(fs::read(f.archive("session_index.jsonl")).unwrap(), b"old");
}

#[test]
fn timeout_signals_malformed_streams_and_stderr_flood_are_bounded() {
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    f.source("session_index.jsonl", b"prior committed data");
    f.ok(&["glurp", "--harness", "codex"]);
    for (script, signal) in [
        ("cat >/dev/null\nsleep 30", None),
        // The process doesn't read stdin, testing nonblocking script delivery too.
        (
            "while :; do printf 'PRIVATE-STDERR-SECRET\\n' >&2; done",
            None,
        ),
        (
            "cat >/dev/null\nprintf 'malformed PRIVATE-TRANSCRIPT-SECRET'",
            None,
        ),
        ("cat >/dev/null\nsleep 30 &\nwait", Some(libc::SIGINT)),
        ("cat >/dev/null\nsleep 30 &\nwait", Some(libc::SIGTERM)),
        (
            "cat >/dev/null\nprintf 'GLURP1\\000F\\000a\\00018446744073709551616\\000'",
            None,
        ),
        (
            "cat >/dev/null\nprintf 'GLURP1\\000F\\000a\\0009\\000short'",
            None,
        ),
    ] {
        f.fake_ssh(script);
        let output = f.bounded(
            &["glurp", "--harness", "codex", "--timeout-seconds", "1"],
            signal,
        );
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("PRIVATE-"));
        assert_eq!(
            fs::read(f.archive("session_index.jsonl")).unwrap(),
            b"prior committed data"
        );
        assert!(!f.home.join("data/glurp/hosts/lab/.transaction").exists());
    }
}

// Executed only as a disposable fake-SSH helper. It deliberately escapes the
// SSH process group and retains the stdout pipe, proving the poll loop has no
// blocking worker join even in a case group termination cannot close the pipe.
#[test]
fn escaped_pipe_fixture() {
    let Some(pid_file) = std::env::var_os("GLURP_FIXTURE_ESCAPED_PID") else {
        return;
    };
    unsafe {
        let pid = libc::fork();
        assert!(pid >= 0);
        if pid == 0 {
            libc::setsid();
            libc::sleep(10);
            libc::_exit(0);
        }
        fs::write(pid_file, pid.to_string()).unwrap();
    }
}

#[test]
fn escaped_inherited_pipe_cannot_hang_transport_after_timeout() {
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    let pid_file = f.home.join("escaped.pid");
    let helper = std::env::current_exe().unwrap();
    f.fake_ssh(&format!("cat >/dev/null\nexport GLURP_FIXTURE_ESCAPED_PID='{}'\nexec '{}' --exact escaped_pipe_fixture --nocapture", pid_file.display(), helper.display()));
    let result = f.bounded(
        &["glurp", "--harness", "codex", "--timeout-seconds", "1"],
        None,
    );
    // Clean the deliberately escaped disposable helper explicitly.
    let pid: i32 = fs::read_to_string(pid_file).unwrap().parse().unwrap();
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    assert!(!result.status.success());
}

#[test]
fn cancellation_reaps_ssh_process_group() {
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let pid_file = f.home.join("ssh.pid");
        f.fake_ssh(&format!(
            "cat >/dev/null\nprintf '%s' \"$$\" > '{}'\nexec sleep 30",
            pid_file.display()
        ));
        let result = f.bounded(
            &["glurp", "--harness", "codex", "--timeout-seconds", "30"],
            Some(signal),
        );
        let pid: i32 = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        assert!(!result.status.success());
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "SSH child was not reaped"
        );
    }
}

#[test]
fn archive_batch_is_not_limited_by_one_descriptor_per_artifact() {
    use std::os::unix::process::CommandExt;
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    for index in 0..180 {
        f.source(
            &format!("sessions/directory-{index}/artifact.jsonl"),
            b"first batch",
        );
    }
    for expected in [b"first batch".as_slice(), b"second batch".as_slice()] {
        if expected == b"second batch" {
            for index in 0..180 {
                f.source(
                    &format!("sessions/directory-{index}/artifact.jsonl"),
                    expected,
                );
            }
        }
        let mut command = f.process(&["glurp", "--harness", "codex"]);
        unsafe {
            command.pre_exec(|| {
                let limit = libc::rlimit {
                    rlim_cur: 64,
                    rlim_max: 64,
                };
                if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for index in 0..180 {
            assert_eq!(
                fs::read(f.archive(&format!("sessions/directory-{index}/artifact.jsonl"))).unwrap(),
                expected
            );
        }
    }
}

#[test]
fn interrupted_transport_and_wire_flood_preserve_committed_data() {
    let f = Fixture::new();
    f.ok(&["host", "add", "lab", "lab"]);
    f.source("session_index.jsonl", b"original");
    f.ok(&["glurp", "--harness", "codex"]);
    for script in [
        "cat >/dev/null\nprintf 'GLURP1\\000F\\000session_index.jsonl\\0007\\000changed'\nexit 9",
        "while :; do printf 'PRIVATE-TRANSCRIPT-FLOOD'; done",
    ] {
        f.fake_ssh(script);
        let output = f.bounded(
            &[
                "glurp",
                "--harness",
                "codex",
                "--max-files",
                "1",
                "--max-total-bytes",
                "8",
                "--timeout-seconds",
                "1",
            ],
            None,
        );
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE-"));
        assert_eq!(
            fs::read(f.archive("session_index.jsonl")).unwrap(),
            b"original"
        );
    }
}
