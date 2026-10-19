//! Public-API-only fixtures shared with the repository integration tests.
//!
//! All environment changes occur on Command before launching a disposable
//! process. Several collect calls deliberately overlap inside that process.
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant, SystemTime};

use fs2::FileExt;
use glurp::{
    CancellationToken, CollectionControl, CollectionError, CollectionReport, CollectionRequest,
    Harness, HostSpec, InputSource, Limits, TaskErrorKind, TaskStatus, collect,
};

const SECRET: &str = "SYNTHETIC_PRIVATE_PAYLOAD_46a9";
const WAIT: Duration = Duration::from_secs(15);

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

    fn private_dir(&self, name: &str) {
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

    pub fn assert_no_executable(&self) {
        let path = std::env::var_os("PATH").unwrap();
        assert!(std::env::split_paths(&path).all(|directory| !directory.join("glurp").exists()));
        let error = Command::new("glurp").arg("--version").output().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    pub fn reset_ssh(&self) {
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

    pub fn script(&self, name: &str, contents: &str) {
        let path = self.root.join("bin").join(name);
        fs::write(&path, contents).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    pub fn remote(&self, path: &str, bytes: &[u8]) {
        let path = self.root.join("remote").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn opencode(&self, inventory: &str, export: &str) {
        self.remote("opencode.db", b"synthetic database marker");
        self.script("opencode", &format!(
            "#!/bin/sh\ncase \"$1\" in\ndb) if [ \"$2\" = path ]; then printf '%s/opencode.db\\n' \"$HOME\"; exit 0; fi\n[ \"$2\" = 'SELECT id FROM session ORDER BY id' ] && [ \"$3\" = --format ] && [ \"$4\" = json ] || exit 90\ncat <<'JSON'\n{inventory}\nJSON\n;;\nexport)\n{export}\n;;\n*) exit 91;; esac\n"
        ));
    }

    fn seed_all(&self) {
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

    pub fn request(&self, hosts: Vec<HostSpec>, harnesses: Vec<Harness>) -> CollectionRequest {
        CollectionRequest {
            source: InputSource::Explicit {
                hosts,
                archive_root: self.root.join("archive"),
            },
            harnesses,
            limits: Limits::default(),
        }
    }

    fn single(&self, name: &str, destination: &str) -> CollectionRequest {
        self.request(vec![self.host(name, destination)], vec![Harness::Codex])
    }

    fn stored(
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

    fn config(&self, hosts: &[HostSpec]) {
        let data: Vec<_> = hosts.iter().map(|host| serde_json::json!({
            "name": host.name, "destination": host.destination,
            "claude_paths": host.claude_paths, "codex_paths": host.codex_paths, "pi_paths": host.pi_paths,
        })).collect();
        let path = self.root.join("config/config.json");
        fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    async fn wait_file(&self, name: &str) {
        wait_until(|| self.root.join(name).exists()).await;
    }

    fn pause(&self, destination: &str) {
        fs::write(self.root.join(format!("pause-{destination}")), b"").unwrap();
    }
    fn release(&self, destination: &str) {
        fs::write(self.root.join(format!("release-{destination}")), b"").unwrap();
    }
    fn contacts(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("ssh.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
    fn archived(&self, host: &str, path: &str) -> PathBuf {
        self.root.join("archive/hosts").join(host).join(path)
    }
    fn clean(&self, host: &str) {
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
    fn reaped(&self, destination: &str) {
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

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(WAIT, async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("fixture handshake timed out");
}

fn controlled(token: &CancellationToken) -> CollectionControl {
    CollectionControl {
        cancellation: token.clone(),
        progress: None,
    }
}

async fn result(request: CollectionRequest) -> CollectionReport {
    tokio::time::timeout(WAIT, collect(request, CollectionControl::default()))
        .await
        .unwrap()
        .unwrap()
}

fn collected(report: &CollectionReport, count: u64) {
    assert!(report.is_success(), "{report:?}");
    assert_eq!(report.tasks.len(), 1);
    assert!(
        matches!(&report.tasks[0].status, TaskStatus::Collected { artifact_count, archive_dir } if *artifact_count == count && archive_dir.is_absolute() && archive_dir.is_dir())
    );
}

fn assert_redacted(value: &impl std::fmt::Debug, serialized: String) {
    assert!(!format!("{value:?}").contains(SECRET));
    assert!(!serialized.contains(SECRET));
}

type Snapshot = BTreeMap<PathBuf, (Vec<u8>, u32, SystemTime)>;
fn snapshot(root: &Path) -> Snapshot {
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

pub async fn run(name: &str, f: &Fixture) {
    f.assert_no_executable();
    match name {
        "smoke" => smoke(f).await,
        "equivalence" => equivalence(f).await,
        "roots" => roots(f).await,
        "validation" => validation(f).await,
        "config_lock" => config_lock(f).await,
        "timer" => timer(f).await,
        "independent_cancel" => independent_cancel(f).await,
        "shared_cancel" => shared_cancel(f).await,
        "drop" => dropped(f).await,
        "queued" => queued(f).await,
        "busy" => busy(f).await,
        "mixed" => mixed(f).await,
        "progress" => progress(f).await,
        "privacy" => privacy(f).await,
        "timeout" => timeout(f).await,
        "partial_cleanup" => partial_cleanup(f).await,
        "between_harnesses" => between_harnesses(f).await,
        "seed_partial_cleanup" => seed_partial_cleanup(f).await,
        _ => panic!("unknown fixture scenario"),
    }
}

async fn smoke(f: &Fixture) {
    fn require_send<T: Send + 'static>() {}
    require_send::<CollectionRequest>();
    require_send::<CollectionControl>();
    f.remote(".codex/session_index.jsonl", b"synthetic native\0\xff\n");
    let config = f.root.join("config/config.json");
    fs::write(&config, b"deliberately invalid stored registry").unwrap();
    let report = result(f.single("lab", "lab")).await;
    collected(&report, 1);
    assert_eq!(
        fs::read(&config).unwrap(),
        b"deliberately invalid stored registry"
    );
    assert!(!f.root.join("config/config.lock").exists());
    let archived = f.archived("lab", "codex/session_index.jsonl");
    assert_eq!(fs::read(&archived).unwrap(), b"synthetic native\0\xff\n");
    let before = fs::metadata(&archived).unwrap().modified().unwrap();
    collected(&result(f.single("lab", "lab")).await, 1);
    assert_eq!(fs::metadata(&archived).unwrap().modified().unwrap(), before);
    f.config(&[f.host("lab", "lab")]);
    let saved = fs::read(&config).unwrap();
    collected(
        &result(f.stored("stored-archive", vec![], vec![Harness::Codex])).await,
        1,
    );
    assert_eq!(fs::read(config).unwrap(), saved);
    assert_eq!(
        fs::read(
            f.root
                .join("stored-archive/hosts/lab/codex/session_index.jsonl")
        )
        .unwrap(),
        fs::read(archived).unwrap()
    );
    let mixed = result(f.request(
        vec![
            f.host("a_bad", "bad"),
            f.host("b_missing", "missing"),
            f.host("c_good", "lab"),
        ],
        vec![Harness::Codex],
    ))
    .await;
    assert!(!mixed.is_success());
    assert!(matches!(
        mixed.tasks[0].status,
        TaskStatus::Failed {
            kind: TaskErrorKind::Transport
        }
    ));
    assert!(matches!(mixed.tasks[1].status, TaskStatus::NotFound));
    assert!(!f.archived("b_missing", "codex").exists());
    assert!(matches!(
        mixed.tasks[2].status,
        TaskStatus::Collected {
            artifact_count: 1,
            ..
        }
    ));
    assert_redacted(&mixed, serde_json::to_string(&mixed).unwrap());
}

async fn equivalence(f: &Fixture) {
    f.seed_all();
    let host = f.host("lab", "lab");
    f.config(std::slice::from_ref(&host));
    let direct = result(f.request(vec![host], vec![])).await;
    assert!(direct.is_success(), "{direct:?}");
    assert_eq!(
        direct
            .tasks
            .iter()
            .map(|task| task.harness)
            .collect::<Vec<_>>(),
        vec![
            Harness::Claude,
            Harness::Codex,
            Harness::OpenCode,
            Harness::Pi
        ]
    );
    assert_eq!(
        direct
            .tasks
            .iter()
            .map(|task| match task.status {
                TaskStatus::Collected { artifact_count, .. } => artifact_count,
                _ => panic!("expected collected"),
            })
            .collect::<Vec<_>>(),
        [1, 2, 2, 1]
    );
    let base = f.root.join("archive/hosts/lab");
    let first = snapshot(&base);
    assert_eq!(first.len(), 6);
    assert!(first.values().all(|(_, mode, _)| *mode == 0o600));
    assert!(!base.join("opencode/inventory.json").exists());
    assert!(
        result(f.request(vec![f.host("lab", "lab")], vec![]))
            .await
            .is_success()
    );
    assert_eq!(first, snapshot(&base));
    assert!(
        result(f.stored("stored-archive", vec![], vec![]))
            .await
            .is_success()
    );
    let stored_base = f.root.join("stored-archive/hosts/lab");
    let stored = snapshot(&stored_base);
    let content = |snap: &Snapshot| {
        snap.iter()
            .map(|(p, (bytes, mode, _))| (p.clone(), (bytes.clone(), *mode)))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(content(&first), content(&stored));
    assert!(
        result(f.stored("stored-archive", vec!["lab".into()], vec![]))
            .await
            .is_success()
    );
    assert_eq!(stored, snapshot(&stored_base));
    if let Some(cli) = std::env::var_os("GLURP_TEST_CLI") {
        let mut add = f.command(&cli);
        add.args(["host", "add", "lab", "lab"]);
        let output = f.bounded_output(add);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut command = f.command(&cli);
        command.arg("glurp");
        let output = f.bounded_output(command);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let cli_base = f.root.join("data/glurp/hosts/lab");
        let cli_snapshot = snapshot(&cli_base);
        assert_eq!(content(&first), content(&cli_snapshot));
        let mut again = f.command(&cli);
        again.arg("glurp");
        assert!(f.bounded_output(again).status.success());
        assert_eq!(cli_snapshot, snapshot(&cli_base));
    }
    // A report counts this input batch, even though the archive retains old files.
    fs::remove_file(f.root.join("remote/.codex/sessions/nested/raw.jsonl.zst")).unwrap();
    collected(&result(f.single("lab", "lab")).await, 1);
    assert!(base.join("codex/sessions/nested/raw.jsonl.zst").exists());
}

async fn roots(f: &Fixture) {
    let first = "odd ' $(touch INJECTED) space";
    f.remote(&format!("{first}/projects/nested/agent.jsonl"), b"first");
    f.remote("second/projects/agent.jsonl", b"second");
    f.remote("codex first/sessions/raw.jsonl.zst", b"\0\xffcodex");
    f.remote("codex second/session_index.jsonl", b"index");
    f.remote("pi custom/nested/session.jsonl", b"pi");
    let mut host = f.host("lab", "lab");
    host.claude_paths = vec![format!("~/{first}"), "~/second".into()];
    host.codex_paths = vec![
        f.root.join("remote/codex first").to_str().unwrap().into(),
        "~/codex second".into(),
    ];
    host.pi_paths = vec!["~/pi custom".into()];
    f.config(std::slice::from_ref(&host));
    let selected = vec![
        Harness::Pi,
        Harness::Codex,
        Harness::Claude,
        Harness::Claude,
    ];
    let report = result(f.request(vec![host], selected.clone())).await;
    assert!(report.is_success());
    assert_eq!(report.tasks.len(), 3);
    for (path, bytes) in [
        (
            "claude/root-0001/projects/nested/agent.jsonl",
            b"first".as_slice(),
        ),
        (
            "claude/root-0002/projects/agent.jsonl",
            b"second".as_slice(),
        ),
        (
            "codex/root-0001/sessions/raw.jsonl.zst",
            b"\0\xffcodex".as_slice(),
        ),
        ("codex/root-0002/session_index.jsonl", b"index".as_slice()),
        ("pi/root-0001/nested/session.jsonl", b"pi".as_slice()),
    ] {
        assert_eq!(fs::read(f.archived("lab", path)).unwrap(), bytes);
    }
    assert!(!f.archived("lab", "opencode").exists());
    assert!(!f.root.join("remote/INJECTED").exists());
    assert!(
        result(f.stored("stored-archive", vec!["lab".into()], selected))
            .await
            .is_success()
    );
    let direct = snapshot(&f.root.join("archive/hosts/lab"));
    let stored = snapshot(&f.root.join("stored-archive/hosts/lab"));
    assert_eq!(
        direct.keys().collect::<Vec<_>>(),
        stored.keys().collect::<Vec<_>>()
    );
    for (path, (bytes, _, _)) in direct {
        assert_eq!(bytes, stored[&path].0);
    }
}

async fn validation(f: &Fixture) {
    let mut requests = vec![
        f.request(vec![], vec![]),
        f.request(vec![f.host("../unsafe", "lab")], vec![]),
        f.request(vec![f.host("lab", "-oProxyCommand=private")], vec![]),
        f.request(vec![f.host("lab", "lab"), f.host("lab", "other")], vec![]),
    ];
    for root in ["relative", "~/line\nbreak", "/nul\0byte"] {
        let mut host = f.host("lab", "lab");
        host.codex_paths.push(root.into());
        requests.push(f.request(vec![host], vec![]));
    }
    let mut relative = f.single("lab", "lab");
    relative.source = InputSource::Explicit {
        hosts: vec![f.host("lab", "lab")],
        archive_root: "relative".into(),
    };
    requests.push(relative);
    for (config_dir, archive_root) in [
        (PathBuf::from("relative"), f.root.join("archive")),
        (f.root.join("config"), PathBuf::from("relative")),
    ] {
        requests.push(CollectionRequest {
            source: InputSource::Stored {
                config_dir,
                archive_root,
                hosts: vec![],
            },
            harnesses: vec![],
            limits: Limits::default(),
        });
    }
    let mut zero = f.single("lab", "lab");
    zero.limits.timeout_seconds = 0;
    requests.push(zero);
    let mut overflow = f.single("lab", "lab");
    overflow.limits.max_files = u64::MAX;
    requests.push(overflow);
    for request in requests {
        assert!(matches!(
            collect(request, CollectionControl::default()).await,
            Err(CollectionError::InvalidInput)
        ));
    }
    assert!(f.contacts().is_empty());
    assert!(!f.root.join("archive").exists());
    // A missing or empty stored registry is a request error, not an empty
    // successful report with invented task identities.
    for empty in [false, true] {
        if empty {
            fs::write(f.root.join("config/config.json"), b"[]").unwrap();
        }
        assert!(matches!(
            collect(
                f.stored("archive", vec![], vec![]),
                CollectionControl::default()
            )
            .await,
            Err(CollectionError::InvalidConfig)
        ));
    }
    f.config(&[f.host("lab", "lab")]);
    assert!(
        collect(
            f.stored("archive", vec!["unknown".into()], vec![]),
            CollectionControl::default()
        )
        .await
        .is_err()
    );
    assert!(f.contacts().is_empty());
    assert!(!f.root.join("archive").exists());
    fs::write(
        f.root.join("config/config.json"),
        format!("{{\"private\":\"{SECRET}\"}}"),
    )
    .unwrap();
    let error = collect(
        f.stored("archive", vec![], vec![]),
        CollectionControl::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, CollectionError::InvalidConfig));
    assert!(!error.to_string().contains(SECRET));
    assert_redacted(&error, serde_json::to_string(&error).unwrap());
    assert!(std::error::Error::source(&error).is_none());
    assert!(f.contacts().is_empty());
}

async fn config_lock(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"config snapshot");
    f.config(&[f.host("lab", "paused")]);
    f.pause("paused");
    let work = tokio::spawn(collect(
        f.stored("archive", vec![], vec![Harness::Codex]),
        CollectionControl::default(),
    ));
    f.wait_file("entered-paused").await;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root.join("config/config.lock"))
        .unwrap();
    lock.try_lock_exclusive()
        .expect("the configuration snapshot lock leaked into SSH work");
    // An independent stored invocation reports config contention before transport.
    assert!(matches!(
        collect(
            f.stored("other", vec![], vec![]),
            CollectionControl::default()
        )
        .await,
        Err(CollectionError::ConfigBusy)
    ));
    FileExt::unlock(&lock).unwrap();
    f.release("paused");
    collected(
        &tokio::time::timeout(WAIT, work)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        1,
    );
}

async fn timer(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"timer");
    f.pause("paused");
    let work = tokio::spawn(collect(
        f.single("lab", "paused"),
        CollectionControl::default(),
    ));
    f.wait_file("entered-paused").await;
    // This runtime has exactly one async worker. A blocking API never reaches
    // this timer or the release handshake and is killed by the outer deadline.
    tokio::time::timeout(
        Duration::from_secs(2),
        tokio::time::sleep(Duration::from_millis(20)),
    )
    .await
    .unwrap();
    assert!(!work.is_finished());
    f.release("paused");
    collected(&work.await.unwrap().unwrap(), 1);
}

async fn independent_cancel(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"original");
    collected(&result(f.single("a", "pause_a")).await, 1);
    let old = f.archived("a", "codex/session_index.jsonl");
    let mtime = fs::metadata(&old).unwrap().modified().unwrap();
    f.remote(".codex/session_index.jsonl", b"changed!");
    f.pause("pause_a");
    f.pause("pause_b");
    let first_token = CancellationToken::new();
    let second_token = CancellationToken::new();
    let first = tokio::spawn(collect(
        f.request(
            vec![f.host("a", "pause_a"), f.host("z_unstarted", "lab")],
            vec![Harness::Codex, Harness::Pi],
        ),
        controlled(&first_token),
    ));
    let second = tokio::spawn(collect(f.single("b", "pause_b"), controlled(&second_token)));
    f.wait_file("entered-pause_a").await;
    f.wait_file("entered-pause_b").await;
    first_token.cancel();
    let cancelled = tokio::time::timeout(WAIT, first)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(cancelled.tasks.len(), 4);
    assert!(matches!(cancelled.tasks[0].status, TaskStatus::Cancelled));
    assert!(
        cancelled.tasks[1..]
            .iter()
            .all(|task| matches!(task.status, TaskStatus::NotStartedDueToCancellation))
    );
    assert!(!cancelled.is_success());
    assert!(!second_token.is_cancelled());
    assert!(!second.is_finished());
    f.reaped("pause_a");
    f.clean("a");
    assert_eq!(fs::read(&old).unwrap(), b"original");
    assert_eq!(fs::metadata(old).unwrap().modified().unwrap(), mtime);
    f.release("pause_a");
    // Awaiting cancellation has already released the same-host lease.
    collected(&result(f.single("a", "pause_a")).await, 1);
    f.release("pause_b");
    collected(&second.await.unwrap().unwrap(), 1);
    collected(&result(f.single("new", "lab")).await, 1);
}

async fn shared_cancel(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"shared");
    f.pause("pause_a");
    f.pause("pause_b");
    let parent = CancellationToken::new();
    let first = tokio::spawn(collect(f.single("a", "pause_a"), controlled(&parent)));
    let second = tokio::spawn(collect(f.single("b", "pause_b"), controlled(&parent)));
    f.wait_file("entered-pause_a").await;
    f.wait_file("entered-pause_b").await;
    parent.cancel();
    for work in [first, second] {
        let report = tokio::time::timeout(WAIT, work)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(matches!(report.tasks[0].status, TaskStatus::Cancelled));
    }
    for destination in ["pause_a", "pause_b"] {
        f.reaped(destination);
    }
    let count = f.contacts().len();
    assert!(matches!(
        collect(f.single("pre", "lab"), controlled(&parent)).await,
        Err(CollectionError::CancelledBeforePlanning)
    ));
    assert_eq!(count, f.contacts().len());
    assert!(!f.root.join("archive/hosts/pre").exists());
    collected(&result(f.single("fresh", "lab")).await, 1);
}

async fn dropped(f: &Fixture) {
    let parent = CancellationToken::new();
    drop(collect(f.single("unpolled", "lab"), controlled(&parent)));
    assert!(!parent.is_cancelled());
    assert!(!f.root.join("archive").exists());
    assert!(f.contacts().is_empty());
    f.remote(".codex/session_index.jsonl", b"drop");
    f.pause("paused");
    let work = tokio::spawn(collect(f.single("lab", "paused"), controlled(&parent)));
    f.pause("sibling_pause");
    let sibling = tokio::spawn(collect(
        f.single("sibling", "sibling_pause"),
        controlled(&parent),
    ));
    f.wait_file("entered-paused").await;
    f.wait_file("entered-sibling_pause").await;
    work.abort();
    assert!(work.await.unwrap_err().is_cancelled());
    assert!(!parent.is_cancelled());
    assert!(!sibling.is_finished());
    // Drop requests cleanup. It does not claim that cleanup finished on return.
    wait_until(|| {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(f.root.join("archive/hosts/lab/archive.lock"))
            .unwrap();
        file.try_lock_exclusive().is_ok()
    })
    .await;
    f.reaped("paused");
    f.clean("lab");
    f.release("paused");
    collected(&result(f.single("lab", "paused")).await, 1);
    f.release("sibling_pause");
    collected(&sibling.await.unwrap().unwrap(), 1);
}

async fn queued(f: &Fixture) {
    // run() uses max_blocking_threads(1), making worker admission deterministic.
    for drop_future in [false, true] {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let occupied = tokio::task::spawn_blocking(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.await.unwrap();
        let token = CancellationToken::new();
        let queued = tokio::spawn(collect(f.single("queued", "lab"), controlled(&token)));
        tokio::task::yield_now().await;
        if drop_future {
            queued.abort();
        } else {
            token.cancel();
        }
        release_tx.send(()).unwrap();
        if drop_future {
            assert!(queued.await.unwrap_err().is_cancelled());
            assert!(!token.is_cancelled());
        } else {
            assert!(matches!(
                queued.await.unwrap(),
                Err(CollectionError::CancelledBeforePlanning)
            ));
        }
        occupied.await.unwrap();
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        assert!(f.contacts().is_empty());
        assert!(!f.root.join("archive").exists());
    }
}

async fn busy(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"busy");
    f.pause("paused");
    let active = tokio::spawn(collect(
        f.request(
            vec![f.host("lab", "paused")],
            vec![Harness::Codex, Harness::Pi],
        ),
        CollectionControl::default(),
    ));
    f.wait_file("entered-paused").await;
    let before = f.contacts().len();
    let refused = result(f.request(
        vec![f.host("lab", "paused"), f.host("other", "lab")],
        vec![Harness::Codex, Harness::Pi],
    ))
    .await;
    assert_eq!(refused.tasks.len(), 4);
    assert!(
        refused.tasks[..2]
            .iter()
            .all(|task| matches!(task.status, TaskStatus::Busy))
    );
    assert!(matches!(
        refused.tasks[2].status,
        TaskStatus::Collected { .. }
    ));
    assert!(matches!(refused.tasks[3].status, TaskStatus::NotFound));
    assert_eq!(f.contacts().len(), before + 2);
    if let Some(cli) = std::env::var_os("GLURP_TEST_CLI") {
        // The CLI must see the same filesystem lease, not an in-process mutex.
        let config = f.root.join("cli-config/glurp");
        fs::create_dir_all(&config).unwrap();
        fs::set_permissions(&config, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(
            config.join("config.json"),
            br#"[{"name":"lab","destination":"paused"}]"#,
        )
        .unwrap();
        fs::set_permissions(
            config.join("config.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let data = f.root.join("busy-data");
        fs::create_dir(&data).unwrap();
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
        // Use a real root path; secure directory traversal intentionally rejects symlinks.
        fs::create_dir_all(data.join("glurp/hosts/lab")).unwrap();
        for directory in [
            data.join("glurp"),
            data.join("glurp/hosts"),
            data.join("glurp/hosts/lab"),
        ] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        // The API gets the same root as the CLI for a second independently paused host.
        let mut request = f.single("lab", "cli_pause");
        request.source = InputSource::Explicit {
            hosts: vec![f.host("lab", "cli_pause")],
            archive_root: data.join("glurp"),
        };
        fs::write(
            config.join("config.json"),
            br#"[{"name":"lab","destination":"cli_pause"}]"#,
        )
        .unwrap();
        f.pause("cli_pause");
        let api = tokio::spawn(collect(request, CollectionControl::default()));
        f.wait_file("entered-cli_pause").await;
        let mut command = f.command(cli);
        command
            .env("XDG_DATA_HOME", &data)
            .args(["glurp", "--harness", "codex"]);
        let contacts_before = f.contacts().len();
        let output = f.bounded_output(command);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("archive in use"),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(f.contacts().len(), contacts_before);
        f.release("cli_pause");
        collected(&api.await.unwrap().unwrap(), 1);
    }
    f.release("paused");
    assert!(active.await.unwrap().unwrap().is_success());
    collected(&result(f.single("lab", "paused")).await, 1);
}

async fn mixed(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"mixed");
    collected(&result(f.single("b_busy", "lab")).await, 1);
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root.join("archive/hosts/b_busy/archive.lock"))
        .unwrap();
    lock.try_lock_exclusive().unwrap();
    let report = result(f.request(
        vec![
            f.host("d_missing", "missing"),
            f.host("c_failed", "bad"),
            f.host("b_busy", "lab"),
            f.host("a_good", "lab"),
        ],
        vec![Harness::Codex],
    ))
    .await;
    assert_eq!(
        report
            .tasks
            .iter()
            .map(|task| task.host.as_str())
            .collect::<Vec<_>>(),
        ["a_good", "b_busy", "c_failed", "d_missing"]
    );
    assert!(matches!(
        report.tasks[0].status,
        TaskStatus::Collected { .. }
    ));
    assert!(matches!(report.tasks[1].status, TaskStatus::Busy));
    assert!(matches!(report.tasks[2].status, TaskStatus::Failed { .. }));
    assert!(matches!(report.tasks[3].status, TaskStatus::NotFound));
    assert!(!report.is_success());
    FileExt::unlock(&lock).unwrap();
    f.pause("cancelled");
    let token = CancellationToken::new();
    let work = tokio::spawn(collect(
        f.request(
            vec![
                f.host("a_good", "lab"),
                f.host("b_cancel", "cancelled"),
                f.host("c_unstarted", "lab"),
            ],
            vec![Harness::Codex],
        ),
        controlled(&token),
    ));
    f.wait_file("entered-cancelled").await;
    token.cancel();
    let report = work.await.unwrap().unwrap();
    assert!(matches!(
        report.tasks[0].status,
        TaskStatus::Collected { .. }
    ));
    assert!(matches!(report.tasks[1].status, TaskStatus::Cancelled));
    assert!(matches!(
        report.tasks[2].status,
        TaskStatus::NotStartedDueToCancellation
    ));
}

async fn progress(f: &Fixture) {
    f.seed_all();
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let report = tokio::time::timeout(
        WAIT,
        collect(
            f.request(vec![f.host("lab", "lab")], vec![]),
            CollectionControl {
                cancellation: CancellationToken::new(),
                progress: Some(tx),
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(report.is_success());
    assert_eq!(report.tasks.len(), 4);
    let event = rx.try_recv().unwrap();
    assert_redacted(&event, serde_json::to_string(&event).unwrap());
    assert!(
        rx.try_recv().is_err(),
        "the full channel should have dropped later progress events"
    );
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    drop(rx);
    collected(
        &collect(
            f.single("closed", "lab"),
            CollectionControl {
                cancellation: CancellationToken::new(),
                progress: Some(tx),
            },
        )
        .await
        .unwrap(),
        2,
    );
    let (tx, _unread) = tokio::sync::mpsc::channel(1);
    let token = CancellationToken::new();
    f.pause("paused");
    let work = tokio::spawn(collect(
        f.single("cancel", "paused"),
        CollectionControl {
            cancellation: token.clone(),
            progress: Some(tx),
        },
    ));
    f.wait_file("entered-paused").await;
    token.cancel();
    assert!(matches!(
        tokio::time::timeout(WAIT, work)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .tasks[0]
            .status,
        TaskStatus::Cancelled
    ));
    f.reaped("paused");
    f.clean("cancel");
}

async fn privacy(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"prior synthetic bytes");
    collected(&result(f.single("lab", "lab")).await, 1);
    for destination in ["bad", "malformed"] {
        let report = result(f.single(destination, destination)).await;
        assert!(matches!(report.tasks[0].status, TaskStatus::Failed { .. }));
        assert_redacted(&report, serde_json::to_string(&report).unwrap());
    }
    // A secret embedded in a syntactically valid remote filename must not be
    // surfaced when frame validation rejects it.
    f.script("ssh", &format!("#!/bin/sh\ncat >/dev/null\nprintf 'GLURP1\\000F\\000../{SECRET}.jsonl\\0001\\000xE\\000'\n"));
    let report = result(f.single("lab", "lab")).await;
    assert!(matches!(
        report.tasks[0].status,
        TaskStatus::Failed {
            kind: TaskErrorKind::InvalidRemoteStream
        }
    ));
    assert_redacted(&report, serde_json::to_string(&report).unwrap());
    assert_eq!(
        fs::read(f.archived("lab", "codex/session_index.jsonl")).unwrap(),
        b"prior synthetic bytes"
    );
    f.reset_ssh();
    for (inventory, export) in [
        (format!("[{{\"id\":\"../{SECRET}\"}}]"), "exit 9".to_owned()),
        (
            "[{\"id\":\"ses_a\"}]".to_owned(),
            format!("printf '{{\"private\":\"{SECRET}\"'"),
        ),
    ] {
        f.opencode(&inventory, &export);
        let report = result(f.request(vec![f.host("json", "lab")], vec![Harness::OpenCode])).await;
        assert!(matches!(
            report.tasks[0].status,
            TaskStatus::Failed {
                kind: TaskErrorKind::InvalidRemoteStream
            }
        ));
        assert_redacted(&report, serde_json::to_string(&report).unwrap());
        f.clean("json");
    }
}

async fn timeout(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"original");
    collected(&result(f.single("lab", "paused")).await, 1);
    let path = f.archived("lab", "codex/session_index.jsonl");
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    f.pause("paused");
    let mut request = f.single("lab", "paused");
    request.limits.timeout_seconds = 1;
    let report = result(request).await;
    assert!(matches!(
        report.tasks[0].status,
        TaskStatus::Failed {
            kind: TaskErrorKind::Timeout
        }
    ));
    f.reaped("paused");
    f.clean("lab");
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), before);
    f.release("paused");
    collected(&result(f.single("lab", "paused")).await, 1);
}

async fn partial_cleanup(f: &Fixture) {
    // The outer process populates a previous generation before enabling the
    // LD_PRELOAD fault. This process never mutates its own environment.
    f.remote(".claude/projects/a.jsonl", b"changed!");
    f.remote(".claude/projects/b.jsonl", b"changed!");
    f.remote(".codex/session_index.jsonl", b"codex");
    let contacts_before = f.contacts().len();
    let report = result(f.request(
        vec![f.host("a_fault", "lab"), f.host("z_other", "lab")],
        vec![Harness::Claude, Harness::Codex],
    ))
    .await;
    assert_eq!(report.tasks.len(), 4);
    assert_eq!(f.contacts().len(), contacts_before + 3);
    assert!(matches!(
        report.tasks[0].status,
        TaskStatus::Collected {
            artifact_count: 2,
            ..
        }
    ));
    assert!(matches!(
        report.tasks[1].status,
        TaskStatus::Failed {
            kind: TaskErrorKind::RecoveryRequired
        }
    ));
    assert!(matches!(
        report.tasks[2].status,
        TaskStatus::Collected { .. }
    ));
    assert!(matches!(
        report.tasks[3].status,
        TaskStatus::Collected { .. }
    ));
    let txn = f.root.join("archive/hosts/a_fault/.transaction");
    assert!(txn.join("committed").is_file());
    assert!(!txn.join("backups/0").exists());
    assert_eq!(fs::read(txn.join("backups/1")).unwrap(), b"original");
    assert_eq!(
        fs::read(f.archived("a_fault", "claude/projects/a.jsonl")).unwrap(),
        b"changed!"
    );
    assert_eq!(
        fs::read(f.archived("a_fault", "claude/projects/b.jsonl")).unwrap(),
        b"changed!"
    );
    assert!(!f.archived("a_fault", "codex").exists());
    let evidence = fs::read(txn.join("committed")).unwrap();
    let contact_count = f.contacts().len();
    let retry = result(f.request(
        vec![f.host("a_fault", "lab")],
        vec![Harness::Claude, Harness::Codex],
    ))
    .await;
    assert!(retry.tasks.iter().all(|task| matches!(
        task.status,
        TaskStatus::Failed {
            kind: TaskErrorKind::RecoveryRequired
        }
    )));
    assert_eq!(f.contacts().len(), contact_count);
    assert_eq!(fs::read(txn.join("committed")).unwrap(), evidence);
    assert_eq!(fs::read(txn.join("backups/1")).unwrap(), b"original");
}

pub async fn seed_partial_cleanup(f: &Fixture) {
    f.remote(".claude/projects/a.jsonl", b"original");
    f.remote(".claude/projects/b.jsonl", b"original");
    collected(
        &result(f.request(vec![f.host("a_fault", "lab")], vec![Harness::Claude])).await,
        2,
    );
}

/// Current-thread runtime is intentional: it detects synchronous work on the
/// async executor. Queued-worker case uses exactly one blocking worker.
pub fn runtime(scenario: &str) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(if scenario == "queued" { 1 } else { 8 })
        .enable_all()
        .build()
        .unwrap()
}

// Executed only by the dynamically linked Linux integration test with the
// syscall interposer. No production hook or environment mutation is needed.
async fn between_harnesses(f: &Fixture) {
    f.remote(".claude/projects/session.jsonl", b"first harness");
    f.remote(".codex/session_index.jsonl", b"second harness");
    f.private_dir("cli-config/glurp");
    let config = f.root.join("cli-config/glurp/config.json");
    fs::write(&config, br#"[{"name":"lab","destination":"lab"}]"#).unwrap();
    fs::set_permissions(config, fs::Permissions::from_mode(0o600)).unwrap();
    let request = |hosts: Vec<HostSpec>| CollectionRequest {
        source: InputSource::Explicit {
            hosts,
            archive_root: f.root.join("data/glurp"),
        },
        harnesses: vec![Harness::Claude, Harness::Codex],
        limits: Limits::default(),
    };
    let active = tokio::spawn(collect(
        request(vec![f.host("lab", "lab")]),
        CollectionControl::default(),
    ));
    f.wait_file("between-entered").await;
    let base = f.root.join("data/glurp/hosts/lab");
    assert_eq!(
        fs::read(base.join("claude/projects/session.jsonl")).unwrap(),
        b"first harness"
    );
    assert!(!base.join(".transaction").exists());
    assert!(!base.join("codex").exists());
    assert_eq!(
        f.contacts().len(),
        1,
        "second harness started before the barrier was released"
    );
    assert!(!active.is_finished());

    let competing = result(request(vec![f.host("lab", "lab"), f.host("other", "lab")])).await;
    assert_eq!(competing.tasks.len(), 4);
    assert!(
        competing.tasks[..2]
            .iter()
            .all(|task| matches!(task.status, TaskStatus::Busy))
    );
    assert!(competing.tasks[2..].iter().all(|task| matches!(
        task.status,
        TaskStatus::Collected {
            artifact_count: 1,
            ..
        }
    )));
    assert_eq!(f.contacts().len(), 3);

    let cli = std::env::var_os("GLURP_TEST_CLI")
        .expect("the integration fixture requires the actual CLI binary");
    let mut command = f.command(&cli);
    command.args(["glurp", "--harness", "claude", "--harness", "codex"]);
    let output = f.bounded_output(command);
    assert!(!output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stderr)
            .matches("archive in use")
            .count(),
        2
    );
    assert_eq!(
        f.contacts().len(),
        3,
        "busy CLI contacted SSH while the between-harness lease was held"
    );
    assert!(!base.join("codex").exists());
    assert!(!active.is_finished());

    fs::write(f.root.join("between-release"), b"release").unwrap();
    let report = tokio::time::timeout(WAIT, active)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(report.is_success());
    assert_eq!(report.tasks.len(), 2);
    assert_eq!(f.contacts().len(), 4);
    assert_eq!(
        fs::read(base.join("codex/session_index.jsonl")).unwrap(),
        b"second harness"
    );
    assert!(
        result(request(vec![f.host("lab", "lab")]))
            .await
            .is_success()
    );
    let mut retry = f.command(cli);
    retry.args(["glurp", "--harness", "claude", "--harness", "codex"]);
    assert!(f.bounded_output(retry).status.success());
}
