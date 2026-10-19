//! Successful collection, source equivalence, host admission, and progress.
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use fs2::FileExt;
use glurp::{
    CancellationToken, CollectionControl, CollectionRequest, Harness, InputSource, TaskErrorKind,
    TaskStatus, collect,
};

use super::fixture::{
    Fixture, Snapshot, WAIT, assert_redacted, collected, controlled, result, snapshot,
};

pub(super) async fn smoke(f: &Fixture) {
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

pub(super) async fn equivalence(f: &Fixture) {
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

pub(super) async fn roots(f: &Fixture) {
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

pub(super) async fn busy(f: &Fixture) {
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

pub(super) async fn mixed(f: &Fixture) {
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

pub(super) async fn progress(f: &Fixture) {
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
