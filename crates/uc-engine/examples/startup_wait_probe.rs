//! 使用真实锁和受控清理验证私有启动结果等待；不代表完整启动回执。
#![allow(dead_code)]
use std::fs::File;
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use uc_engine::EngineError;
#[path = "../src/engine/startup_outcome.rs"]
mod startup_outcome;
use startup_outcome::{StartupOutcomeCell, StartupOutcomeSnapshot};

fn available(path: &std::path::Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

fn main() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let outcome = Arc::new(StartupOutcomeCell::default());
    let initial = outcome.wait(Duration::ZERO);
    outcome.update(|state| {
        state.begin();
        state.begin_cleanup();
    });
    let (started, waiting) = mpsc::channel();
    let (sent, received) = mpsc::channel();
    let long_owner = Arc::clone(&outcome);
    let waiter = std::thread::spawn(move || {
        started.send(()).expect("signal waiter");
        sent.send(long_owner.wait(Duration::from_secs(5)))
            .expect("result");
    });
    waiting
        .recv_timeout(Duration::from_secs(2))
        .expect("waiter started");
    let early = outcome.wait(Duration::ZERO);
    let still_waiting = received.recv_timeout(Duration::from_millis(100)).is_err();
    let held = !available(&path);
    drop(lease);
    outcome.update(|state| state.released(None));
    let released = received
        .recv_timeout(Duration::from_secs(5))
        .expect("released");
    waiter.join().expect("waiter");
    let repeated = outcome.wait(Duration::ZERO);
    let free = available(&path);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(std::path::Path::new(&directory).join("startup-wait.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"private production result waiter, controlled cleanup and independent file-lock probe", "initial":format!("{initial:?}"), "early":format!("{early:?}"), "long_waiter_pending":still_waiting, "lock_held_before_close":held, "released":format!("{released:?}"), "repeat_equal":released==repeated, "lock_available_after_close":free, "engine_release_receipt":"not established", "device_acceptance":"not established"})).expect("json")).expect("artifact");
    }
    assert!(matches!(initial, StartupOutcomeSnapshot::NotStarted));
    assert!(matches!(early, StartupOutcomeSnapshot::Cleaning { .. }));
    assert!(still_waiting && held && free && released == repeated);
    assert!(matches!(released, StartupOutcomeSnapshot::Released { .. }));
}
