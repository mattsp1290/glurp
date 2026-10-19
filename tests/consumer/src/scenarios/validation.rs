//! Request and path validation, configuration locking, and error privacy.
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use fs2::FileExt;
use glurp::{
    CollectionControl, CollectionError, CollectionRequest, Harness, InputSource, Limits,
    TaskErrorKind, TaskStatus, collect,
};

use super::fixture::{Fixture, SECRET, WAIT, assert_redacted, collected, result};

pub(super) async fn validation(f: &Fixture) {
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

pub(super) async fn invalid_local_paths(f: &Fixture) {
    f.config(&[f.host("lab", "lab")]);
    for (index, suffix) in ["nul\0component/end", "dot/./end", "parent/../end"]
        .into_iter()
        .enumerate()
    {
        for source in ["explicit", "stored_archive", "stored_config"] {
            let prefix = f.root.join(format!("invalid-{source}-{index}"));
            let invalid = prefix.join(suffix);
            let source = match source {
                "explicit" => InputSource::Explicit {
                    hosts: vec![f.host("lab", "lab")],
                    archive_root: invalid,
                },
                "stored_archive" => InputSource::Stored {
                    config_dir: f.root.join("config"),
                    archive_root: invalid,
                    hosts: vec![],
                },
                "stored_config" => InputSource::Stored {
                    config_dir: invalid,
                    archive_root: f.root.join("archive"),
                    hosts: vec![],
                },
                _ => unreachable!(),
            };
            assert!(matches!(
                collect(
                    CollectionRequest {
                        source,
                        harnesses: vec![Harness::Codex],
                        limits: Limits::default(),
                    },
                    CollectionControl::default(),
                )
                .await,
                Err(CollectionError::InvalidInput)
            ));
            assert!(
                !prefix.exists(),
                "invalid syntax created a prefix directory"
            );
            assert!(!f.root.join("archive").exists());
            assert!(!f.root.join("config/config.lock").exists());
            assert!(f.contacts().is_empty());
        }
    }
}

pub(super) async fn non_utf8_path(f: &Fixture) {
    f.remote(".codex/session_index.jsonl", b"native path bytes\0\xff");
    let root = f
        .root
        .join(std::ffi::OsString::from_vec(b"archive-\xff".to_vec()));
    let report = result(CollectionRequest {
        source: InputSource::Explicit {
            hosts: vec![f.host("lab", "lab")],
            archive_root: root.clone(),
        },
        harnesses: vec![Harness::Codex],
        limits: Limits::default(),
    })
    .await;
    collected(&report, 1);
    let expected = root.join("hosts/lab/codex");
    assert!(matches!(
        &report.tasks[0].status,
        TaskStatus::Collected { archive_dir, .. } if archive_dir == &expected
    ));
    assert_eq!(
        fs::read(expected.join("session_index.jsonl")).unwrap(),
        b"native path bytes\0\xff"
    );
    // Successful native Unix paths need not have a JSON string representation.
    assert!(serde_json::to_string(&report).is_err());
    assert_eq!(f.contacts(), ["lab"]);
}

pub(super) async fn config_lock(f: &Fixture) {
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
    let contacts_before = f.contacts().len();
    assert!(matches!(
        collect(
            f.stored("other", vec![], vec![]),
            CollectionControl::default()
        )
        .await,
        Err(CollectionError::ConfigBusy)
    ));
    assert_eq!(f.contacts().len(), contacts_before);
    assert!(!f.root.join("other").exists());
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

pub(super) async fn config_lock_error(f: &Fixture) {
    f.config(&[f.host("lab", "lab")]);
    let before = fs::read(f.root.join("config/config.json")).unwrap();
    let error = collect(
        f.stored("archive", vec![], vec![Harness::Codex]),
        CollectionControl::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error, CollectionError::InvalidConfig);
    assert!(std::error::Error::source(&error).is_none());
    assert_eq!(serde_json::to_string(&error).unwrap(), "\"invalid_config\"");
    assert_eq!(fs::read(f.root.join("config/config.json")).unwrap(), before);
    assert!(f.contacts().is_empty());
    assert!(!f.root.join("archive").exists());
}

pub(super) async fn privacy(f: &Fixture) {
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
