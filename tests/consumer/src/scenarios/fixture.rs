//! Disposable process/filesystem fixtures and shared public-API assertions.
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use glurp::{
    CancellationToken, CollectionControl, CollectionReport, CollectionRequest, Harness, HostSpec,
    InputSource, Limits, TaskStatus, collect,
};

pub(super) const SECRET: &str = "SYNTHETIC_PRIVATE_PAYLOAD_46a9";
pub(super) const WAIT: Duration = Duration::from_secs(15);

pub struct Fixture {
    _temp: Option<tempfile::TempDir>,
    pub root: PathBuf,
}

impl Fixture {
    pub fn create() -> Self {
        let temp = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let fixture = Self {
            _temp: Some(temp),
            root,
        };
        for name in ["home", "remote", "empty", "bin", "config"] {
            fixture.private_dir(name);
        }
        for utility in [
            "sh", "cat", "cksum", "find", "mktemp", "rm", "wc", "touch", "sleep",
        ] {
            let system = ["/bin", "/usr/bin"]
                .into_iter()
                .map(|directory| PathBuf::from(directory).join(utility))
                .find(|path| path.is_file())
                .unwrap();
            std::os::unix::fs::symlink(system, fixture.root.join("bin").join(utility)).unwrap();
        }
        fixture.reset_ssh();
        fixture
    }

    pub fn from_environment() -> Self {
        Self {
            _temp: None,
            root: PathBuf::from(std::env::var_os("GLURP_TEST_ROOT").unwrap()),
        }
    }

    pub(super) fn private_dir(&self, name: &str) {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    pub fn command(&self, executable: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = Command::new(executable);
        command
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("PATH", self.root.join("bin"))
            .env("REMOTE_HOME", self.root.join("remote"))
            .env("GLURP_TEST_ROOT", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("cli-config"))
            .env("XDG_DATA_HOME", self.root.join("data"));
        command
    }

    pub fn bounded_output(&self, mut command: Command) -> Output {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(45) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("public library fixture exceeded its finite deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }

    pub(super) fn assert_no_executable(&self) {
        let path = std::env::var_os("PATH").unwrap();
        assert!(std::env::split_paths(&path).all(|directory| !directory.join("glurp").exists()));
        let error = Command::new("glurp").arg("--version").output().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    pub(super) fn reset_ssh(&self) {
        self.script("ssh", r#"#!/bin/sh
[ "$1" = -o ] && [ "$2" = BatchMode=yes ] || exit 81
[ "$3" = -o ] && [ "$4" = ConnectTimeout=15 ] || exit 82
[ "$6" = 'sh -s' ] || exit 83
printf '%s\n' "$5" >> "$GLURP_TEST_ROOT/ssh.log"
printf '%s' "$$" > "$GLURP_TEST_ROOT/pid-$5"
if [ -f "$GLURP_TEST_ROOT/pause-$5" ]; then
  touch "$GLURP_TEST_ROOT/entered-$5"
  while [ ! -f "$GLURP_TEST_ROOT/release-$5" ]; do sleep 0.01; done
fi
case "$5" in
bad) printf 'SYNTHETIC_PRIVATE_PAYLOAD_46a9' >&2; exit 9;;
malformed) cat >/dev/null; printf 'SYNTHETIC_PRIVATE_PAYLOAD_46a9'; exit 0;;
missing) export HOME="$GLURP_TEST_ROOT/empty";;
*) export HOME="$REMOTE_HOME";;
esac
unset CODEX_HOME CLAUDE_CONFIG_DIR PI_CODING_AGENT_SESSION_DIR PI_CODING_AGENT_DIR XDG_DATA_HOME XDG_CONFIG_HOME
exec /bin/sh -s
"#);
    }

    pub(super) fn script(&self, name: &str, contents: &str) {
        let path = self.root.join("bin").join(name);
        fs::write(&path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    pub(super) fn remote(&self, path: &str, bytes: &[u8]) {
        let path = self.root.join("remote").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    pub(super) fn opencode(&self, inventory: &str, export: &str) {
        self.remote("opencode.db", b"synthetic database marker");
        self.script("opencode", &format!(
            "#!/bin/sh\ncase \"$1\" in\ndb) if [ \"$2\" = path ]; then printf '%s/opencode.db\\n' \"$HOME\"; exit 0; fi\n[ \"$2\" = 'SELECT id FROM session ORDER BY id' ] && [ \"$3\" = --format ] && [ \"$4\" = json ] || exit 90\ncat <<'JSON'\n{inventory}\nJSON\n;;\nexport)\n{export}\n;;\n*) exit 91;; esac\n"
        ));
    }

    pub(super) fn seed_all(&self) {
        self.remote(
            ".claude/projects/project/session/subagents/agent.jsonl",
            b"claude\n\0",
        );
        self.remote(
            ".codex/sessions/nested/raw.jsonl.zst",
            b"\x28\xb5\x2f\xfd\0\xffnative",
        );
        self.remote(".codex/session_index.jsonl", b"index\n");
        self.remote(
            ".pi/agent/sessions/project/nested/session.jsonl",
            b"pi\n\xff",
        );
        self.opencode(
            r#"[{"id":"ses_a"},{"id":"ses_b"}]"#,
            r#"printf '{"info":{"id":"%s"},"messages":[]}\n' "$2""#,
        );
    }

    pub fn host(&self, name: &str, destination: &str) -> HostSpec {
        HostSpec {
            name: name.into(),
            destination: destination.into(),
            claude_paths: vec![],
            codex_paths: vec![],
            pi_paths: vec![],
        }
    }

    pub(super) fn request(
        &self,
        hosts: Vec<HostSpec>,
        harnesses: Vec<Harness>,
    ) -> CollectionRequest {
        CollectionRequest {
            source: InputSource::Explicit {
                hosts,
                archive_root: self.root.join("archive"),
            },
            harnesses,
            limits: Limits::default(),
        }
    }

    pub(super) fn single(&self, name: &str, destination: &str) -> CollectionRequest {
        self.request(vec![self.host(name, destination)], vec![Harness::Codex])
    }

    pub(super) fn stored(
        &self,
        archive: &str,
        hosts: Vec<String>,
        harnesses: Vec<Harness>,
    ) -> CollectionRequest {
        CollectionRequest {
            source: InputSource::Stored {
                config_dir: self.root.join("config"),
                archive_root: self.root.join(archive),
                hosts,
            },
            harnesses,
            limits: Limits::default(),
        }
    }

    pub(super) fn config(&self, hosts: &[HostSpec]) {
        let data: Vec<_> = hosts.iter().map(|host| serde_json::json!({
            "name": host.name, "destination": host.destination,
            "claude_paths": host.claude_paths, "codex_paths": host.codex_paths, "pi_paths": host.pi_paths,
        })).collect();
        let path = self.root.join("config/config.json");
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    pub(super) async fn wait_file(&self, name: &str) {
        wait_until(|| self.root.join(name).exists()).await;
    }

    pub(super) fn pause(&self, destination: &str) {
        fs::write(self.root.join(format!("pause-{destination}")), b"").unwrap();
    }
    pub(super) fn release(&self, destination: &str) {
        fs::write(self.root.join(format!("release-{destination}")), b"").unwrap();
    }
    pub(super) fn contacts(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("ssh.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    pub(super) fn archived(&self, host: &str, path: &str) -> PathBuf {
        self.root.join("archive/hosts").join(host).join(path)
    }
    pub(super) fn clean(&self, host: &str) {
        let root = self.root.join("archive/hosts").join(host);
        assert!(!root.join(".transaction").exists());
        assert!(!root.join(".recovery").exists());
        if root.exists() {
            assert!(fs::read_dir(root).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".stage-")
            }));
        }
    }
    pub(super) fn reaped(&self, destination: &str) {
        let pid: i32 = fs::read_to_string(self.root.join(format!("pid-{destination}")))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            -1,
            "SSH child remains alive or unreaped after collect completed"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}

pub(super) async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture handshake timed out");
}

pub(super) fn controlled(token: &CancellationToken) -> CollectionControl {
    CollectionControl {
        cancellation: token.clone(),
        progress: None,
    }
}

pub(super) async fn result(request: CollectionRequest) -> CollectionReport {
    tokio::time::timeout(WAIT, collect(request, CollectionControl::default()))
        .await
        .unwrap()
        .unwrap()
}

pub(super) fn collected(report: &CollectionReport, count: u64) {
    assert!(report.is_success(), "{report:?}");
    assert_eq!(report.tasks.len(), 1);
    assert!(
        matches!(&report.tasks[0].status, TaskStatus::Collected { artifact_count, archive_dir } if *artifact_count == count && archive_dir.is_absolute() && archive_dir.is_dir())
    );
}

pub(super) fn assert_redacted(value: &impl std::fmt::Debug, serialized: String) {
    assert!(!format!("{value:?}").contains(SECRET));
    assert!(!serialized.contains(SECRET));
}

pub(super) type Snapshot = BTreeMap<PathBuf, (Vec<u8>, u32, SystemTime)>;
pub(super) fn snapshot(root: &Path) -> Snapshot {
    fn visit(base: &Path, path: &Path, output: &mut Snapshot) {
        if !path.exists() {
            return;
        }
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                visit(base, &entry.path(), output);
            } else {
                output.insert(
                    entry.path().strip_prefix(base).unwrap().to_owned(),
                    (
                        fs::read(entry.path()).unwrap(),
                        metadata.mode() & 0o777,
                        metadata.modified().unwrap(),
                    ),
                );
            }
        }
    }
    let mut output = BTreeMap::new();
    for harness in ["claude", "codex", "opencode", "pi"] {
        visit(root, &root.join(harness), &mut output);
    }
    output
}
