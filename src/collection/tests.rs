use super::*;
use crate::archive::{TestCheckpoint, TestHooks};
use std::os::unix::fs::PermissionsExt;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

fn request(root: &Path, harnesses: Vec<Harness>) -> CollectionRequest {
    CollectionRequest {
        source: InputSource::Explicit {
            hosts: vec![HostSpec {
                name: "lab".into(),
                destination: "lab".into(),
                ..Default::default()
            }],
            archive_root: root.to_owned(),
        },
        harnesses,
        limits: Limits::default(),
    }
}
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}
fn temp() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[test]
fn stored_cancellation_before_planning_creates_no_placeholder_tasks() {
    let temp = temp();
    let root = temp.path().canonicalize().unwrap();
    let cancellation = CancellationToken::new();
    let hook_token = cancellation.clone();
    let control = CollectionControl {
        cancellation,
        planning_hook: Some(Arc::new(move || hook_token.cancel())),
        ..Default::default()
    };
    let request = CollectionRequest {
        source: InputSource::Stored {
            config_dir: root.join("config"),
            archive_root: root.join("archive"),
            hosts: Vec::new(),
        },
        harnesses: Vec::new(),
        limits: Limits::default(),
    };
    assert_eq!(
        runtime().block_on(collect(request, control)),
        Err(CollectionError::CancelledBeforePlanning)
    );
    assert!(!root.join("config").exists());
    assert!(!root.join("archive").exists());
}

#[test]
fn panic_hook_is_preserved_and_worker_error_is_sanitized() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "collection::tests::panic_hook_child",
            "--nocapture",
        ])
        .env("GLURP_TEST_PANIC_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("synthetic collection worker panic"));
}
#[test]
fn panic_hook_child() {
    if std::env::var_os("GLURP_TEST_PANIC_CHILD").is_none() {
        return;
    }
    let called = Arc::new(AtomicBool::new(false));
    let hook_called = called.clone();
    std::panic::set_hook(Box::new(move |info| {
        assert_eq!(
            info.payload().downcast_ref::<&str>(),
            Some(&"synthetic collection worker panic")
        );
        hook_called.store(true, Ordering::SeqCst);
    }));
    let root = temp();
    let result = runtime().block_on(collect(
        request(root.path(), vec![Harness::Codex]),
        CollectionControl {
            panic_worker: true,
            ..Default::default()
        },
    ));
    assert_eq!(result, Err(CollectionError::WorkerFailure));
    assert!(called.load(Ordering::SeqCst));
    let error = result.unwrap_err();
    assert_eq!(format!("{error:?}"), "WorkerFailure");
    assert_eq!(serde_json::to_string(&error).unwrap(), "\"worker_failure\"");
    assert!(std::error::Error::source(&error).is_none());
    // Prove the exact caller hook is still installed after the worker joined.
    called.store(false, Ordering::SeqCst);
    let _ = std::panic::catch_unwind(|| panic!("synthetic collection worker panic"));
    assert!(called.load(Ordering::SeqCst));
}

struct Gate {
    release: Mutex<bool>,
    wake: Condvar,
}
impl Gate {
    fn new() -> Self {
        Self {
            release: Mutex::new(false),
            wake: Condvar::new(),
        }
    }
    fn wait(&self) {
        let released = self.release.lock().unwrap();
        drop(
            self.wake
                .wait_while(released, |released| !*released)
                .unwrap(),
        );
    }
    fn release(&self) {
        *self.release.lock().unwrap() = true;
        self.wake.notify_all();
    }
}
struct ReleaseOnDrop(Arc<Gate>);
impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[test]
fn dropped_active_future_retains_lease_until_worker_cleanup() {
    let root = temp();
    let root = root.path().canonicalize().unwrap();
    runtime().block_on(async {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered = Mutex::new(Some(entered_tx));
        let gate = Arc::new(Gate::new());
        let _release = ReleaseOnDrop(gate.clone());
        let worker_gate = gate.clone();
        let control = CollectionControl {
            test_hooks: Some(Arc::new(TestHooks {
                checkpoint: Box::new(move |point| {
                    if point == TestCheckpoint::BeforeHarness {
                        if let Some(tx) = entered.lock().unwrap().take() {
                            let _ = tx.send(());
                        }
                        worker_gate.wait();
                    }
                    Ok(())
                }),
            })),
            ..Default::default()
        };
        let parent_token = control.cancellation.clone();
        let mut first = tokio::spawn(collect(request(&root, vec![Harness::Codex]), control));
        tokio::time::timeout(std::time::Duration::from_secs(10), entered_rx)
            .await
            .unwrap()
            .unwrap();
        first.abort();
        assert!((&mut first).await.unwrap_err().is_cancelled());
        assert!(!parent_token.is_cancelled());
        let overlap = collect(
            request(&root, vec![Harness::Codex]),
            CollectionControl::default(),
        )
        .await
        .unwrap();
        let busy = overlap.tasks[0].status == TaskStatus::Busy;
        // Always release before asserting to keep assertion failures finite.
        gate.release();
        assert!(busy);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let host: config::Host = HostSpec {
                    name: "lab".into(),
                    destination: "lab".into(),
                    ..Default::default()
                }
                .into();
                match HostSession::open(&root, &host) {
                    Ok(session) => {
                        drop(session);
                        break;
                    }
                    Err(error) if classify(&error) == CoreError::Task(TaskErrorKind::Busy) => {
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await
                    }
                    Err(error) => {
                        panic!("unexpected sanitized classification {:?}", classify(&error))
                    }
                }
            }
        })
        .await
        .unwrap();
        assert!(!root.join("hosts/lab/.transaction").exists());
    });
}

#[test]
fn queued_dropped_worker_does_no_work_when_eventually_admitted() {
    let root = temp();
    let archive = root.path().canonicalize().unwrap().join("archive");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    rt.block_on(async {
        let gate = Arc::new(Gate::new());
        let _release = ReleaseOnDrop(gate.clone());
        let blocker_gate = gate.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = ready_tx.send(());
            blocker_gate.wait();
        });
        ready_rx.await.unwrap();
        let called = Arc::new(AtomicUsize::new(0));
        let hook_called = called.clone();
        let control = CollectionControl {
            planning_hook: Some(Arc::new(move || {
                hook_called.fetch_add(1, Ordering::SeqCst);
            })),
            ..Default::default()
        };
        let mut future = Box::pin(collect(request(&archive, vec![Harness::Codex]), control));
        // Explicitly poll to Pending while the only blocking thread is held.
        std::future::poll_fn(|cx| match future.as_mut().poll(cx) {
            std::task::Poll::Pending => std::task::Poll::Ready(()),
            std::task::Poll::Ready(_) => panic!("worker must be queued"),
        })
        .await;
        // Drop the actual pinned future, not just a Pin reference.
        drop(future);
        gate.release();
        blocker.await.unwrap();
    });
    // Runtime drop waits for the queued blocking worker; its cancelled token
    // must be checked before planning or filesystem side effects.
    drop(rt);
    assert!(!archive.exists());
}

fn lifecycle_child_output(scenario: &str) -> std::process::Output {
    let temp = temp();
    let root = temp.path().canonicalize().unwrap();
    let bin = root.join("bin");
    let remote = root.join("remote");
    let home = root.join("home");
    for dir in [&bin, &remote, &home] {
        std::fs::create_dir(dir).unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    for utility in ["sh", "cat", "cksum", "find", "mktemp", "rm", "wc", "touch"] {
        let executable = ["/bin", "/usr/bin"]
            .into_iter()
            .map(|dir| PathBuf::from(dir).join(utility))
            .find(|path| path.is_file())
            .unwrap();
        std::os::unix::fs::symlink(executable, bin.join(utility)).unwrap();
    }
    std::fs::write(
        bin.join("ssh"),
        "#!/bin/sh\nexport HOME=\"$REMOTE_HOME\"\nexec /bin/sh -s\n",
    )
    .unwrap();
    std::fs::set_permissions(bin.join("ssh"), std::fs::Permissions::from_mode(0o700)).unwrap();
    if scenario == "commit" {
        std::fs::create_dir(remote.join(".codex")).unwrap();
        std::fs::write(remote.join(".codex/session_index.jsonl"), b"synthetic\n").unwrap();
    }
    std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "collection::tests::lifecycle_child",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", bin)
        .env("HOME", home)
        .env("REMOTE_HOME", remote)
        .env("GLURP_LIFECYCLE_SCENARIO", scenario)
        .output()
        .unwrap()
}

#[test]
fn cleanup_and_between_harness_boundaries_are_owned_by_the_worker() {
    for scenario in ["drop_cleanup", "await_cleanup", "between", "commit"] {
        let output = lifecycle_child_output(scenario);
        assert!(
            output.status.success(),
            "{scenario}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn lifecycle_child() {
    let Ok(scenario) = std::env::var("GLURP_LIFECYCLE_SCENARIO") else {
        return;
    };
    let temp = temp();
    let root = temp.path().canonicalize().unwrap();
    runtime().block_on(async {
        let cancellation = CancellationToken::new();
        if scenario == "commit" {
            let hook_token = cancellation.clone();
            let result = collect(
                request(&root, vec![Harness::Codex, Harness::Pi]),
                CollectionControl {
                    cancellation,
                    test_hooks: Some(Arc::new(TestHooks {
                        checkpoint: Box::new(move |point| {
                            if point == TestCheckpoint::Committed {
                                hook_token.cancel();
                            }
                            Ok(())
                        }),
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            assert!(matches!(
                result.tasks[0].status,
                TaskStatus::Collected {
                    artifact_count: 1,
                    ..
                }
            ));
            assert_eq!(
                result.tasks[1].status,
                TaskStatus::NotStartedDueToCancellation
            );
            assert_eq!(
                std::fs::read(root.join("hosts/lab/codex/session_index.jsonl")).unwrap(),
                b"synthetic\n"
            );
            assert!(!root.join("hosts/lab/.transaction").exists());
            return;
        }
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let entered = Mutex::new(Some(entered_tx));
        let gate = Arc::new(Gate::new());
        let _release = ReleaseOnDrop(gate.clone());
        let worker_gate = gate.clone();
        let seen = AtomicUsize::new(0);
        let between = scenario == "between";
        let control = CollectionControl {
            cancellation: cancellation.clone(),
            test_hooks: Some(Arc::new(TestHooks {
                checkpoint: Box::new(move |point| {
                    let stop = if between {
                        point == TestCheckpoint::BeforeHarness
                            && seen.fetch_add(
                                usize::from(point == TestCheckpoint::BeforeHarness),
                                Ordering::SeqCst,
                            ) == 1
                    } else {
                        point == TestCheckpoint::BeforeRecovery
                    };
                    if stop && let Some(tx) = entered.lock().unwrap().take() {
                        let _ = tx.send(());
                        worker_gate.wait();
                    }
                    Ok(())
                }),
            })),
            ..Default::default()
        };
        let mut first = tokio::spawn(collect(
            request(&root, vec![Harness::Codex, Harness::Pi]),
            control,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(10), entered_rx)
            .await
            .unwrap()
            .unwrap();
        if scenario == "drop_cleanup" {
            first.abort();
            assert!((&mut first).await.unwrap_err().is_cancelled());
            assert!(!cancellation.is_cancelled());
        } else if scenario == "await_cleanup" {
            cancellation.cancel();
            tokio::task::yield_now().await;
            assert!(
                !first.is_finished(),
                "await must not return while cleanup is blocked"
            );
        }
        let overlap = collect(request(&root, vec![]), CollectionControl::default())
            .await
            .unwrap();
        let busy = overlap
            .tasks
            .iter()
            .all(|task| task.status == TaskStatus::Busy);
        if between {
            let mut unrelated = request(&root, vec![Harness::Codex]);
            if let InputSource::Explicit { hosts, .. } = &mut unrelated.source {
                hosts[0].name = "other".into();
            }
            assert!(
                collect(unrelated, CollectionControl::default())
                    .await
                    .unwrap()
                    .is_success()
            );
        }
        gate.release();
        assert!(busy);
        if scenario != "drop_cleanup" {
            let report = first.await.unwrap().unwrap();
            if scenario == "await_cleanup" {
                assert_eq!(
                    report.tasks[1].status,
                    TaskStatus::NotStartedDueToCancellation
                );
            } else {
                assert!(report.is_success());
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let retry = collect(
                    request(&root, vec![Harness::Codex]),
                    CollectionControl::default(),
                )
                .await
                .unwrap();
                if retry.tasks[0].status != TaskStatus::Busy {
                    assert!(retry.is_success());
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!root.join("hosts/lab/.transaction").exists());
    });
}
