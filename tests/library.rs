//! Integration coverage uses only glurp's supported public exports. Each test
//! runs a private child process so PATH/HOME and platform fault injection never
//! race unrelated tests or mutate the embedding process's environment.
#[path = "consumer/src/scenarios.rs"]
mod scenarios;

use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
#[cfg(all(target_os = "linux", not(target_env = "musl")))]
use std::process::Command;

use glurp::{CollectionControl, CollectionError, CollectionRequest, InputSource, Limits, collect};
use scenarios::Fixture;

fn check_child(f: &Fixture, scenario: &str, preload: Option<&std::path::Path>) {
    let mut command = f.command(std::env::current_exe().unwrap());
    command
        .args(["--exact", "library_child", "--nocapture"])
        .env("GLURP_TEST_SCENARIO", scenario)
        .env("GLURP_TEST_CLI", env!("CARGO_BIN_EXE_glurp"));
    if scenario == "smoke" {
        command.env_remove("HOME");
    }
    if let Some(preload) = preload {
        command
            .env("LD_PRELOAD", preload)
            .env(
                "GLURP_FAULT_MODE",
                match scenario {
                    "between_harnesses" | "clean_postcommit_cancel" => "pause-after-cleanup",
                    "partial_cleanup" => "commit-partial-cleanup",
                    "partial_cleanup_cancel" => "commit-partial-cleanup-cancel",
                    "config_lock_enolck" => "config-lock-enolck",
                    "config_lock_eio" => "config-lock-eio",
                    _ => panic!("scenario does not define an external fault mode"),
                },
            )
            .env("GLURP_FAULT_LOG", f.root.join("fault.log"))
            .env(
                "GLURP_PAUSE_HOST_DIR",
                f.root.join(if scenario == "between_harnesses" {
                    "data/glurp/hosts/lab"
                } else {
                    "archive/hosts/a_fault"
                }),
            )
            .env("GLURP_PAUSE_ENTERED", f.root.join("between-entered"))
            .env("GLURP_PAUSE_RELEASE", f.root.join("between-release"));
    }
    let output = f.bounded_output(command);
    assert!(
        output.status.success(),
        "scenario {scenario} failed\nstdout: {}\nstderr: {}\ncaptured stdout: {}\ncaptured stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        fs::read_to_string(f.root.join("captured.stdout")).unwrap_or_default(),
        fs::read_to_string(f.root.join("captured.stderr")).unwrap_or_default()
    );
    for name in ["captured.stdout", "captured.stderr"] {
        assert_eq!(
            fs::read(f.root.join(name)).unwrap(),
            b"",
            "library unexpectedly wrote to {name}"
        );
    }
}

fn scenario(name: &str) {
    check_child(&Fixture::create(), name, None);
}

// The exact-child entry point is deliberately not prefixed portable_: every
// portable parent launches it with environment set before process startup.
#[test]
fn library_child() {
    let Ok(name) = std::env::var("GLURP_TEST_SCENARIO") else {
        return;
    };
    let f = Fixture::from_environment();
    let before_environment: std::collections::BTreeMap<_, _> = std::env::vars_os().collect();
    let before_signals = signals();
    let capture = Capture::new(&f);
    let runtime = scenarios::runtime(&name);
    if name == "seed_partial_cleanup" {
        runtime.block_on(scenarios::seed_partial_cleanup(&f));
    } else {
        runtime.block_on(scenarios::run(&name, &f));
    }
    drop(runtime);
    assert_eq!(before_environment, std::env::vars_os().collect());
    assert_eq!(
        before_signals,
        signals(),
        "collection modified caller signal handlers"
    );
    drop(capture);
}

struct Capture {
    stdout: i32,
    stderr: i32,
}
impl Capture {
    fn new(f: &Fixture) -> Self {
        std::io::stdout().flush().unwrap();
        std::io::stderr().flush().unwrap();
        let out = fs::File::create(f.root.join("captured.stdout")).unwrap();
        let err = fs::File::create(f.root.join("captured.stderr")).unwrap();
        unsafe {
            let stdout = libc::dup(libc::STDOUT_FILENO);
            let stderr = libc::dup(libc::STDERR_FILENO);
            assert!(stdout >= 0 && stderr >= 0);
            assert_eq!(
                libc::dup2(out.as_raw_fd(), libc::STDOUT_FILENO),
                libc::STDOUT_FILENO
            );
            assert_eq!(
                libc::dup2(err.as_raw_fd(), libc::STDERR_FILENO),
                libc::STDERR_FILENO
            );
            Self { stdout, stderr }
        }
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        unsafe {
            libc::dup2(self.stdout, libc::STDOUT_FILENO);
            libc::dup2(self.stderr, libc::STDERR_FILENO);
            libc::close(self.stdout);
            libc::close(self.stderr);
        }
    }
}

fn signals() -> Vec<(usize, libc::c_int, Vec<libc::c_int>)> {
    [libc::SIGINT, libc::SIGTERM]
        .into_iter()
        .map(|signal| unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            assert_eq!(libc::sigaction(signal, std::ptr::null(), &mut action), 0);
            (
                action.sa_sigaction,
                action.sa_flags,
                (1..32)
                    .map(|member| libc::sigismember(&action.sa_mask, member))
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn portable_owned_consumer_inputs_without_home_or_glurp_executable() {
    scenario("smoke");
}
#[test]
fn portable_all_harnesses_match_cli_bytes_layout_counts_and_mtimes() {
    scenario("equivalence");
}
#[test]
fn portable_literal_multiple_roots_and_selected_harnesses() {
    scenario("roots");
}
#[test]
fn portable_invalid_requests_reject_before_transport_or_transactions() {
    scenario("validation");
}
#[test]
fn portable_invalid_local_path_syntax_rejects_both_input_modes_without_prefix_directories() {
    scenario("invalid_local_paths");
}
#[test]
fn portable_non_utf8_archive_paths_return_native_reports_but_cannot_serialize_to_json() {
    scenario("non_utf8_path");
}
#[test]
fn portable_stored_snapshot_releases_config_lock_before_ssh() {
    scenario("config_lock");
}
#[test]
fn portable_current_thread_runtime_keeps_timers_responsive() {
    scenario("timer");
}
#[test]
fn portable_independent_cancellation_awaits_reaping_and_preserves_completed_data() {
    scenario("independent_cancel");
}
#[test]
fn portable_shared_parent_and_precancel_have_explicit_semantics() {
    scenario("shared_cancel");
}
#[test]
fn portable_dropped_future_cancels_only_its_worker_and_unpolled_is_inert() {
    scenario("drop");
}
#[test]
fn portable_dropped_queued_worker_never_launches_transport() {
    scenario("queued");
}
#[test]
fn portable_same_host_is_busy_across_library_and_cli_processes() {
    scenario("busy");
}
#[test]
fn portable_mixed_results_are_complete_ordered_and_distinguishable() {
    scenario("mixed");
}
#[test]
fn portable_full_and_closed_progress_channels_never_block_cleanup() {
    scenario("progress");
}
#[test]
fn portable_expected_errors_are_silent_redacted_and_signal_neutral() {
    scenario("privacy");
}
#[test]
fn portable_timeout_reaps_transport_preserves_mtimes_and_releases_lease() {
    scenario("timeout");
}

#[test]
fn portable_missing_runtime_is_an_error_without_side_effects() {
    use std::future::Future;
    use std::task::{Context, Poll, Waker};
    let f = Fixture::create();
    let request = CollectionRequest {
        source: InputSource::Explicit {
            hosts: vec![f.host("lab", "lab")],
            archive_root: f.root.join("archive"),
        },
        harnesses: vec![],
        limits: Limits::default(),
    };
    let mut future = std::pin::pin!(collect(request, CollectionControl::default()));
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        future.as_mut().poll(&mut context),
        Poll::Ready(Err(CollectionError::MissingRuntime))
    ));
    assert!(!f.root.join("archive").exists());
    assert!(!f.root.join("ssh.log").exists());
}

// LD_PRELOAD interposition is a dynamically linked Linux-only seam. All other
// library coverage above also runs on the existing musl/macOS release targets.
#[cfg(all(target_os = "linux", not(target_env = "musl")))]
#[test]
fn linux_partial_postcommit_cleanup_blocks_later_harnesses_preserves_evidence_and_continues_hosts()
{
    let f = Fixture::create();
    check_child(&f, "seed_partial_cleanup", None);
    let library = fault_shim(&f);
    check_child(&f, "partial_cleanup", Some(&library));
    assert!(
        fs::read_to_string(f.root.join("fault.log"))
            .unwrap()
            .contains("partial-cleanup-failed")
    );
}

#[cfg(all(target_os = "linux", not(target_env = "musl")))]
#[test]
fn linux_partial_postcommit_cleanup_takes_priority_over_cancellation_without_admitting_later_hosts()
{
    let f = Fixture::create();
    check_child(&f, "seed_partial_cleanup", None);
    let library = fault_shim(&f);
    check_child(&f, "partial_cleanup_cancel", Some(&library));
    assert_eq!(
        fs::read_to_string(f.root.join("fault.log")).unwrap(),
        "commit-sync-succeeded\ncleanup-paused\ncleanup-released\npartial-cleanup-failed\n"
    );
}

#[cfg(all(target_os = "linux", not(target_env = "musl")))]
#[test]
fn linux_clean_postcommit_cancellation_preserves_success_and_leaves_remaining_tasks_unstarted() {
    let f = Fixture::create();
    check_child(&f, "seed_partial_cleanup", None);
    let library = fault_shim(&f);
    check_child(&f, "clean_postcommit_cancel", Some(&library));
    assert_eq!(
        fs::read_to_string(f.root.join("fault.log")).unwrap(),
        "cleanup-paused\ncleanup-released\n"
    );
}

#[cfg(all(target_os = "linux", not(target_env = "musl")))]
#[test]
fn linux_non_contention_config_lock_errors_report_invalid_config() {
    for (scenario, event) in [
        ("config_lock_enolck", "config-lock-enolck\n"),
        ("config_lock_eio", "config-lock-eio\n"),
    ] {
        let f = Fixture::create();
        let library = fault_shim(&f);
        check_child(&f, scenario, Some(&library));
        assert_eq!(fs::read_to_string(f.root.join("fault.log")).unwrap(), event);
    }
}

#[cfg(all(target_os = "linux", not(target_env = "musl")))]
#[test]
fn linux_between_harness_barrier_proves_library_and_cli_busy_until_host_batch_finishes() {
    let f = Fixture::create();
    let library = fault_shim(&f);
    check_child(&f, "between_harnesses", Some(&library));
    assert_eq!(
        fs::read_to_string(f.root.join("fault.log")).unwrap(),
        "cleanup-paused\ncleanup-released\n"
    );
}

#[cfg(all(target_os = "linux", not(target_env = "musl")))]
fn fault_shim(f: &Fixture) -> std::path::PathBuf {
    let library = f.root.join("fs-faults.so");
    let output = Command::new("cc")
        .args(["-shared", "-fPIC", "-O2", "-o"])
        .arg(&library)
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fs_faults.c"))
        .arg("-ldl")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    library
}
