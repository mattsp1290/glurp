//! Run with: cargo run --example collect -- lab /absolute/private/archive
//! OpenSSH must be installed and the destination's authentication configured.
use glurp::{
    CancellationToken, CollectionControl, CollectionRequest, Harness, HostSpec, InputSource,
    Limits, TaskStatus, collect,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let destination = arguments
        .next()
        .ok_or("usage: collect SSH_DESTINATION ABSOLUTE_ARCHIVE_ROOT")?;
    let archive_root = arguments
        .next()
        .ok_or("missing absolute archive root")?
        .into();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let cancellation = CancellationToken::new();
        // An embedding application's shutdown handler can cancel this clone.
        // Keep awaiting collect afterward: cancel + await is the cleanup boundary.
        let shutdown = cancellation.clone();
        let request = CollectionRequest {
            source: InputSource::Explicit {
                hosts: vec![HostSpec {
                    name: "lab".into(),
                    destination,
                    claude_paths: vec![],
                    codex_paths: vec![],
                    pi_paths: vec![],
                }],
                archive_root,
            },
            harnesses: vec![Harness::Codex],
            limits: Limits::default(),
        };
        let collection = collect(
            request,
            CollectionControl {
                cancellation,
                progress: None,
            },
        );
        tokio::pin!(collection);
        // This example application owns its Ctrl-C listener. The library never
        // installs handlers. Even a listener error must await worker cleanup.
        let report = tokio::select! {
            result = &mut collection => result?,
            signal = tokio::signal::ctrl_c() => {
                shutdown.cancel();
                let result = collection.await;
                signal?;
                result?
            }
        };
        for task in &report.tasks {
            match &task.status {
                TaskStatus::Collected {
                    artifact_count,
                    archive_dir,
                } => {
                    println!(
                        "{} {:?}: {artifact_count} artifacts in {}",
                        task.host,
                        task.harness,
                        archive_dir.display()
                    );
                }
                status => println!("{} {:?}: {status:?}", task.host, task.harness),
            }
        }
        if report.is_success() {
            Ok(())
        } else {
            Err("one or more collection tasks did not succeed".into())
        }
    })
}
