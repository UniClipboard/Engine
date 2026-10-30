//! 限制阻塞线程验证排队中的登记结算；不代表完整启动回执。
#![allow(dead_code)]
#[path = "../src/engine/log_activity.rs"]
mod log_activity;

use log_activity::LogActivity;
use std::fs::File;
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use uc_engine::{EngineError, EngineErrorCategory};

fn operation_cancelled_error() -> EngineError {
    EngineError::new(1002, EngineErrorCategory::DeadlineExceeded, true)
}

fn available(path: &std::path::Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

fn main() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("blocker.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    let _blocker = runtime.spawn_blocking(move || {
        entered.send(()).expect("entered");
        released.recv().expect("release");
        drop(lease);
    });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("blocker running");
    let activity = LogActivity::default();
    let owner = activity.clone();
    let starter = runtime.spawn(async move { owner.begin().await });
    let (settled, settlement) = mpsc::channel();
    runtime.block_on(async {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let sender = settled.clone();
            if activity.on_beginning_settled(move || {
                let _ = sender.send(());
            }) {
                break;
            }
            assert!(Instant::now() < until, "registration was not queued");
            tokio::task::yield_now().await;
        }
    });
    drop(settled);
    starter.abort();
    let held = !available(&path);
    runtime.shutdown_timeout(Duration::from_millis(50));
    let before_release = settlement.recv_timeout(Duration::from_millis(50)).is_ok();
    release.send(()).expect("release blocker");
    let settled_after = before_release || settlement.recv_timeout(Duration::from_secs(5)).is_ok();
    let duplicate = settlement.try_recv().is_ok();
    let until = Instant::now() + Duration::from_secs(5);
    while !available(&path) {
        assert!(Instant::now() < until, "blocker lock remained held");
        std::thread::yield_now();
    }
    let late_observer =
        activity.on_beginning_settled(|| panic!("settled registration cannot accept observer"));
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(std::path::Path::new(&directory).join("log-registration-queued.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production log activity, single controlled blocking worker, independent file-lock probe", "blocker_lock_held":held, "settled_before_blocker_release":before_release, "settled":settled_after, "duplicate_settlement":duplicate, "late_observer_registered":late_observer, "blocker_lock_released":true, "complete_startup_release":"not established"})).expect("json")).expect("artifact");
    }
    assert!(held && settled_after && !duplicate && !late_observer);
}
