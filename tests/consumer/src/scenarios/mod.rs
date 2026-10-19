//! Public-API-only fixtures shared with the repository integration tests.
//!
//! All environment changes occur on Command before launching a disposable
//! process. Several collect calls deliberately overlap inside that process.
mod cancellation;
mod collection;
mod fixture;
mod recovery;
mod validation;

pub use fixture::Fixture;
pub use recovery::seed_partial_cleanup;

pub async fn run(name: &str, f: &Fixture) {
    f.assert_no_executable();
    match name {
        "smoke" => collection::smoke(f).await,
        "equivalence" => collection::equivalence(f).await,
        "roots" => collection::roots(f).await,
        "validation" => validation::validation(f).await,
        "invalid_local_paths" => validation::invalid_local_paths(f).await,
        "non_utf8_path" => validation::non_utf8_path(f).await,
        "config_lock" => validation::config_lock(f).await,
        "config_lock_enolck" | "config_lock_eio" => validation::config_lock_error(f).await,
        "timer" => cancellation::timer(f).await,
        "independent_cancel" => cancellation::independent_cancel(f).await,
        "shared_cancel" => cancellation::shared_cancel(f).await,
        "drop" => cancellation::dropped(f).await,
        "queued" => cancellation::queued(f).await,
        "busy" => collection::busy(f).await,
        "mixed" => collection::mixed(f).await,
        "progress" => collection::progress(f).await,
        "privacy" => validation::privacy(f).await,
        "timeout" => cancellation::timeout(f).await,
        "partial_cleanup" => recovery::partial_cleanup(f).await,
        "partial_cleanup_cancel" => recovery::postcommit_cancel(f, true).await,
        "clean_postcommit_cancel" => recovery::postcommit_cancel(f, false).await,
        "between_harnesses" => recovery::between_harnesses(f).await,
        "seed_partial_cleanup" => seed_partial_cleanup(f).await,
        _ => panic!("unknown fixture scenario"),
    }
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
