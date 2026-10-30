//! 使用受控清理边界与真实锁验证私有启动结果；不代表完整启动回执。
#![allow(dead_code)]
use std::fs::File;
use std::process::Command;
use std::sync::{mpsc, Arc, Mutex};
use uc_engine::{EngineError, EngineErrorCategory};
#[path = "../src/engine/startup_outcome.rs"]
mod startup_outcome;
use startup_outcome::{StartupOutcome, StartupOutcomeSnapshot};

fn available(path: &std::path::Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}
fn main() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let outcome = Arc::new(Mutex::new(StartupOutcome::default()));
    let initial = outcome.lock().expect("state").snapshot();
    let primary = EngineError::new(42, EngineErrorCategory::Unavailable, true);
    outcome.lock().expect("state").begin();
    outcome
        .lock()
        .expect("state")
        .record_primary(primary.clone());
    outcome.lock().expect("state").begin_cleanup();
    let pending = outcome.lock().expect("state").snapshot();
    let held = !available(&path);
    let (go, proceed) = mpsc::channel();
    let worker_outcome = Arc::clone(&outcome);
    let worker = std::thread::spawn(move || {
        proceed.recv().expect("release");
        drop(lease);
        worker_outcome.lock().expect("state").released(None);
    });
    assert!(matches!(pending, StartupOutcomeSnapshot::Cleaning { .. }));
    go.send(()).expect("finish");
    worker.join().expect("worker");
    let released = outcome.lock().expect("state").snapshot();
    let released_again = outcome.lock().expect("state").snapshot();
    let free = available(&path);
    let mut failed = StartupOutcome::default();
    failed.begin();
    failed.record_primary(primary.clone());
    failed.begin_cleanup();
    let cleanup = EngineError::new(1108, EngineErrorCategory::Internal, false);
    failed.failed(cleanup.clone(), false);
    failed.released(None);
    failed.record_primary(EngineError::new(99, EngineErrorCategory::Internal, false));
    let failed_result = failed.snapshot();
    let mut with_operation_error = StartupOutcome::default();
    with_operation_error.begin();
    with_operation_error.record_primary(primary.clone());
    with_operation_error.begin_cleanup();
    let operation = EngineError::new(77, EngineErrorCategory::Unavailable, true);
    with_operation_error.released(Some(operation.clone()));
    let released_with_error = with_operation_error.snapshot();
    let mut recorded_before_release = StartupOutcome::default();
    recorded_before_release.begin();
    recorded_before_release.record_primary(primary.clone());
    recorded_before_release.begin_cleanup();
    recorded_before_release.record_operation_error(operation.clone());
    recorded_before_release.released(None);
    let previously_recorded = recorded_before_release.snapshot();
    let mut delivered = StartupOutcome::default();
    delivered.begin();
    delivered.delivered();
    let delivered_result = delivered.snapshot();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(std::path::Path::new(&directory).join("startup-outcome.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"private production result state with controlled transitions and real independent lock probe", "initial":format!("{initial:?}"), "pending":format!("{pending:?}"), "lock_held_while_pending":held, "released":format!("{released:?}"), "repeat_equal":released==released_again, "lock_available_after_close":free, "failed":format!("{failed_result:?}"), "released_with_operation_error":format!("{released_with_error:?}"), "previously_recorded_operation_error":format!("{previously_recorded:?}"), "delivered":format!("{delivered_result:?}"), "engine_release_receipt":"not established", "device_acceptance":"not established"})).expect("json")).expect("artifact");
    }
    assert!(matches!(initial, StartupOutcomeSnapshot::NotStarted));
    assert!(held && free && released == released_again);
    assert!(
        matches!(released, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), .. } if *error == primary)
    );
    assert!(
        matches!(failed_result, StartupOutcomeSnapshot::Failed { startup_error: Some(ref error), cleanup_error: ref actual, .. } if *error == primary && *actual == cleanup)
    );
    assert!(
        matches!(released_with_error, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), operation_error: Some(ref op) } if *error == primary && *op == operation)
    );
    assert!(
        matches!(previously_recorded, StartupOutcomeSnapshot::Released { startup_error: Some(ref error), operation_error: Some(ref op) } if *error == primary && *op == operation)
    );
    assert!(matches!(
        delivered_result,
        StartupOutcomeSnapshot::Delivered
    ));
    let _ = outcome.lock().expect("state").can_complete();
}
