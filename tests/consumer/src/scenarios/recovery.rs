//! Durable cleanup and between-harness scenarios for the Linux syscall interposer.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use fs2::FileExt;
use glurp::{
    CancellationToken, CollectionControl, CollectionRequest, Harness, HostSpec, InputSource,
    Limits, TaskErrorKind, TaskStatus, collect,
};

use super::fixture::{Fixture, WAIT, collected, controlled, result, snapshot};

pub(super) async fn partial_cleanup(f: &Fixture) {
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

pub(super) async fn postcommit_cancel(f: &Fixture, partial_cleanup: bool) {
    // The parent seeds the previous generation before enabling interposition.
    // Pause the real cleanup syscall after commit, before the driver can admit
    // another harness. In the failing case backup 0 has already been removed.
    f.remote(".claude/projects/a.jsonl", b"changed!");
    f.remote(".claude/projects/b.jsonl", b"changed!");
    f.remote(".codex/session_index.jsonl", b"must not be collected");
    f.remote(".pi/agent/sessions/session.jsonl", b"must not be collected");
    let contacts_before = f.contacts().len();
    let token = CancellationToken::new();
    let work = tokio::spawn(collect(
        f.request(
            vec![f.host("a_fault", "lab"), f.host("z_unstarted", "lab")],
            vec![Harness::Claude, Harness::Codex, Harness::Pi],
        ),
        controlled(&token),
    ));
    f.wait_file("between-entered").await;
    let base = f.root.join("archive/hosts/a_fault");
    let txn = base.join(".transaction");
    let file_state = |path: &Path| {
        (
            fs::read(path).unwrap(),
            fs::metadata(path).unwrap().modified().unwrap(),
        )
    };
    let evidence = if partial_cleanup {
        assert!(!txn.join("backups/0").exists());
        assert_eq!(fs::read(txn.join("backups/1")).unwrap(), b"original");
        Some((
            file_state(&txn.join("committed")),
            file_state(&txn.join("backups/1")),
        ))
    } else {
        assert!(!txn.exists());
        None
    };
    for path in ["claude/projects/a.jsonl", "claude/projects/b.jsonl"] {
        assert_eq!(fs::read(base.join(path)).unwrap(), b"changed!");
    }
    let committed = snapshot(&base);
    assert_eq!(f.contacts().len(), contacts_before + 1);
    assert!(!work.is_finished());
    token.cancel();
    fs::write(f.root.join("between-release"), b"release").unwrap();
    let report = tokio::time::timeout(WAIT, work)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        report
            .tasks
            .iter()
            .map(|task| (task.host.as_str(), task.harness))
            .collect::<Vec<_>>(),
        [
            ("a_fault", Harness::Claude),
            ("a_fault", Harness::Codex),
            ("a_fault", Harness::Pi),
            ("z_unstarted", Harness::Claude),
            ("z_unstarted", Harness::Codex),
            ("z_unstarted", Harness::Pi),
        ]
    );
    assert!(matches!(
        report.tasks[0].status,
        TaskStatus::Collected {
            artifact_count: 2,
            ..
        }
    ));
    let remaining = if partial_cleanup {
        TaskStatus::Failed {
            kind: TaskErrorKind::RecoveryRequired,
        }
    } else {
        TaskStatus::NotStartedDueToCancellation
    };
    assert!(
        report.tasks[1..3]
            .iter()
            .all(|task| task.status == remaining)
    );
    assert!(
        report.tasks[3..]
            .iter()
            .all(|task| matches!(task.status, TaskStatus::NotStartedDueToCancellation))
    );
    assert!(!report.is_success());
    assert_eq!(f.contacts().len(), contacts_before + 1);
    assert!(!f.root.join("archive/hosts/z_unstarted").exists());
    assert!(!base.join("codex").exists());
    assert!(!base.join("pi").exists());
    assert_eq!(snapshot(&base), committed);
    assert!(!base.join(".recovery").exists());
    assert!(fs::read_dir(&base).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".stage-")
    }));
    if let Some((journal, backup)) = evidence {
        assert_eq!(file_state(&txn.join("committed")), journal);
        assert_eq!(file_state(&txn.join("backups/1")), backup);
        assert!(!txn.join("backups/0").exists());
        // Re-admission still refuses ambiguous evidence and cannot change it.
        let retry = result(f.request(
            vec![f.host("a_fault", "lab")],
            vec![Harness::Claude, Harness::Codex, Harness::Pi],
        ))
        .await;
        assert!(retry.tasks.iter().all(|task| task.status == remaining));
        assert_eq!(file_state(&txn.join("committed")), journal);
        assert_eq!(file_state(&txn.join("backups/1")), backup);
        assert!(!txn.join("backups/0").exists());
        assert_eq!(snapshot(&base), committed);
        assert_eq!(f.contacts().len(), contacts_before + 1);
    } else {
        f.clean("a_fault");
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(base.join("archive.lock"))
        .unwrap();
    lock.try_lock_exclusive()
        .expect("awaited cancellation must release the host lease");
    FileExt::unlock(&lock).unwrap();
}

pub async fn seed_partial_cleanup(f: &Fixture) {
    f.remote(".claude/projects/a.jsonl", b"original");
    f.remote(".claude/projects/b.jsonl", b"original");
    collected(
        &result(f.request(vec![f.host("a_fault", "lab")], vec![Harness::Claude])).await,
        2,
    );
}

// Executed only by the dynamically linked Linux integration test with the
// syscall interposer. No production hook or environment mutation is needed.
pub(super) async fn between_harnesses(f: &Fixture) {
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
