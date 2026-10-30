//! 使用真实执行器和独立文件锁验证停止结果，不代表 Engine 释放。
#[path = "../src/engine/owned_executor.rs"]
mod owned_executor;

use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use owned_executor::{ExecutorStop, OwnedExecutor};

fn available(path: &Path) -> bool {
    let result = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("probe");
    assert!(matches!(result.code(), Some(0 | 1)));
    result.success()
}

fn reservation_probe() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("construction.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let executor = Arc::new(OwnedExecutor::start().expect("executor"));
    let (entered, waiting) = mpsc::channel();
    let (completed, completion) = mpsc::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    let task = executor
        .spawn(async move {
            entered.send(()).expect("entered");
            gate.await.expect("release");
            drop(lease);
            completed.send(()).expect("completed");
        })
        .unwrap_or_else(|_| panic!("task rejected"));
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("started");
    let initial = executor.stop(Duration::from_millis(200));
    let held = !available(&path);
    let blocked_reservation = executor.spawn(async {}).is_err();
    let pending_task = completion.try_recv().is_err();
    release.send(()).expect("release");
    let task_completed = completion.recv_timeout(Duration::from_secs(5)).is_ok();
    drop(task);
    let stopped = executor.stop(Duration::from_secs(5));
    let repeated = executor.stop(Duration::ZERO);
    let free = available(&path);
    let valid = initial == ExecutorStop::Pending
        && held
        && blocked_reservation
        && pending_task
        && task_completed
        && stopped == ExecutorStop::Stopped
        && repeated == stopped
        && free;
    let artifact = serde_json::json!({"scope":"standalone candidate executor, accepted async construction and independent lock", "initial":format!("{initial:?}"), "held":held, "later_reservation_rejected":blocked_reservation, "task_pending":pending_task, "task_completed":task_completed, "stopped":format!("{stopped:?}"), "repeat_equal":stopped==repeated, "lock_free":free, "engine_release":"not established", "valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join("owned-executor-reservation.json"),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(valid);
}

fn main() {
    if std::env::args().any(|arg| arg == "--reservation") {
        reservation_probe();
        return;
    }
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("worker.lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let executor = Arc::new(OwnedExecutor::start().expect("executor"));
    let parent = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("parent");
    let (entered, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let runtime = Arc::clone(&executor);
    parent.block_on(async move {
        runtime.handle().spawn_blocking(move || {
            entered.send(()).expect("entered");
            gate.recv().expect("release");
            drop(lease);
        });
    });
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("worker entered");
    drop(parent);
    let held_before = !available(&path);
    #[cfg(feature = "dev-tools")]
    let panic = std::env::args().any(|arg| arg == "--panic");
    #[cfg(not(feature = "dev-tools"))]
    let panic = false;
    #[cfg(feature = "dev-tools")]
    if panic {
        executor.probe_panic();
    }
    let first = executor.stop(Duration::ZERO);
    let second = executor.stop(Duration::from_millis(100));
    let held_after = !available(&path);
    let first_waiter = {
        let executor = Arc::clone(&executor);
        std::thread::spawn(move || executor.stop(Duration::from_secs(10)))
    };
    let second_waiter = {
        let executor = Arc::clone(&executor);
        std::thread::spawn(move || executor.stop(Duration::from_secs(10)))
    };
    let waiting_both = !first_waiter.is_finished() && !second_waiter.is_finished();
    release.send(()).expect("release");
    let stopped = executor.stop(Duration::from_secs(5));
    let repeated = executor.stop(Duration::ZERO);
    let concurrent_equal = first_waiter.join().expect("first waiter") == stopped
        && second_waiter.join().expect("second waiter") == stopped;
    let free = available(&path);
    let valid = held_before
        && held_after
        && first == ExecutorStop::Pending
        && second == first
        && stopped
            == if panic {
                ExecutorStop::Failed
            } else {
                ExecutorStop::Stopped
            }
        && repeated == stopped
        && waiting_both
        && concurrent_equal
        && free;
    let artifact = serde_json::json!({"scope":"standalone candidate executor, actual blocking work and independent lock", "parent_executor_dropped":true, "worker_panicked":panic, "held_before":held_before, "first":format!("{first:?}"), "second":format!("{second:?}"), "held_after":held_after, "stopped":format!("{stopped:?}"), "repeat_equal":stopped==repeated, "waiters_pending":waiting_both, "concurrent_equal":concurrent_equal, "lock_free":free, "engine_release":"not established", "valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join(if panic {
                "owned-executor-panic.json"
            } else {
                "owned-executor.json"
            }),
            serde_json::to_vec_pretty(&artifact).expect("json"),
        )
        .expect("artifact");
    }
    assert!(valid);
}
