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
        for utility in ["sh", "cat", "cksum", "find", "mktemp", "rm", "wc", "touch"] {
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

    fn command(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_glurp"))
            .args(args)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("REMOTE_HOME", &self.remote)
            .env("PATH", &self.bin)
            .output()
            .unwrap()
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
