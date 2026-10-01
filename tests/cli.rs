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
        let home = temp.path().join("local");
        let remote = temp.path().join("remote");
        let bin = temp.path().join("bin");
        for dir in [&home, &remote, &bin] {
            fs::create_dir(dir).unwrap();
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
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
unset CODEX_HOME
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
            .env("PATH", format!("{}:/usr/bin:/bin", self.bin.display()))
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
