//! Runtime responsiveness and isolated cancellation, drop, queue, and timeout behavior.
use std::fs;
use std::time::Duration;

use fs2::FileExt;
use glurp::{
    CancellationToken, CollectionControl, CollectionError, Harness, TaskErrorKind, TaskStatus,
    collect,
};

use super::fixture::{Fixture, WAIT, collected, controlled, result, wait_until};

pub(super) async fn timer(f: &Fixture) {
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

pub(super) async fn independent_cancel(f: &Fixture) {
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

pub(super) async fn shared_cancel(f: &Fixture) {
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

pub(super) async fn dropped(f: &Fixture) {
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

pub(super) async fn queued(f: &Fixture) {
    use std::future::Future;
    use std::task::Poll;

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
        let mut queued = Box::pin(collect(f.single("queued", "lab"), controlled(&token)));
        std::future::poll_fn(|cx| {
            assert!(queued.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        if drop_future {
            // Drop the actual future while its worker is still queued. Aborting
            // a spawned task would only request Drop and race worker admission.
            drop(queued);
            assert!(!token.is_cancelled());
            release_tx.send(()).unwrap();
        } else {
            token.cancel();
            release_tx.send(()).unwrap();
            assert!(matches!(
                queued.await,
                Err(CollectionError::CancelledBeforePlanning)
            ));
        }
        occupied.await.unwrap();
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        assert!(f.contacts().is_empty());
        assert!(!f.root.join("archive").exists());
    }
}

pub(super) async fn timeout(f: &Fixture) {
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
