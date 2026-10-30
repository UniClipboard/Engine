//! 隔离验证登记封口与进行中的网络转换；不代表完整进程释放回执。
#![allow(dead_code)]

#[path = "../src/engine_activity.rs"]
mod engine_activity;

mod local_file {
    use std::sync::{Mutex, MutexGuard, PoisonError};
    pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

mod runtime {
    use crate::SignalResult;
    use std::sync::{mpsc, Arc, Mutex, OnceLock};
    use std::time::Duration;
    pub(crate) struct Network {
        pub(crate) reopening: mpsc::Sender<()>,
        pub(crate) reopen_release: Mutex<mpsc::Receiver<()>>,
        pub(crate) parking: mpsc::Sender<()>,
        pub(crate) park_release: Mutex<mpsc::Receiver<()>>,
    }
    pub(crate) static NETWORK: OnceLock<Arc<Network>> = OnceLock::new();
    impl Network {
        pub(crate) fn unpark(&self, _: Duration) -> SignalResult {
            self.reopening.send(()).expect("reopening signal");
            self.reopen_release
                .lock()
                .expect("reopen lock")
                .recv()
                .expect("release reopening");
            SignalResult::Completed
        }
        pub(crate) fn park(&self, _: Duration) -> SignalResult {
            self.parking.send(()).expect("parking signal");
            self.park_release
                .lock()
                .expect("park lock")
                .recv()
                .expect("release parking");
            SignalResult::Completed
        }
    }
    pub(crate) fn installed_network_log() -> Option<Arc<Network>> {
        NETWORK.get().cloned()
    }
    pub(crate) fn notify_engine_closer_changed() {}
}

use engine_activity::{fence_engines, resume_engines, wait_fenced_idle, EngineLogActivity};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use uc_observability_runtime::SignalResult;

fn main() {
    let (reopening_tx, reopening_rx) = mpsc::channel();
    let (reopen_release_tx, reopen_release_rx) = mpsc::channel();
    let (parking_tx, parking_rx) = mpsc::channel();
    let (park_release_tx, park_release_rx) = mpsc::channel();
    runtime::NETWORK
        .set(Arc::new(runtime::Network {
            reopening: reopening_tx,
            reopen_release: Mutex::new(reopen_release_rx),
            parking: parking_tx,
            park_release: Mutex::new(park_release_rx),
        }))
        .unwrap_or_else(|_| panic!("network setup"));
    let begin = std::thread::spawn(|| EngineLogActivity::begin(Duration::from_secs(5)).0);
    reopening_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("reopening entered");
    let (fence_tx, fence_rx) = mpsc::channel();
    let fence_thread =
        std::thread::spawn(move || fence_tx.send(fence_engines()).expect("fence result"));
    let early_fence = fence_rx.recv_timeout(Duration::from_millis(150));
    let accepted_while_reopening = early_fence.is_ok();
    reopen_release_tx.send(()).expect("release reopening");
    let fence = early_fence.unwrap_or_else(|_| {
        fence_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("late fence")
    });
    fence_thread.join().expect("fence thread");
    let mut activity = begin.join().expect("registration thread");
    let pending_active = !wait_fenced_idle(&fence, Duration::from_millis(20));
    let end = std::thread::spawn(move || activity.end(Duration::from_secs(5)));
    parking_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("parking entered");
    let (idle_tx, idle_rx) = mpsc::channel();
    let pending_fence = Arc::clone(&fence);
    let idle = std::thread::spawn(move || {
        idle_tx
            .send(wait_fenced_idle(&pending_fence, Duration::from_millis(100)))
            .expect("idle result")
    });
    let pending_park = idle_rx.recv_timeout(Duration::from_millis(250));
    let parking_wait_bounded = matches!(pending_park, Ok(false));
    park_release_tx.send(()).expect("release parking");
    if pending_park.is_err() {
        let _ = idle_rx.recv_timeout(Duration::from_secs(2));
    }
    idle.join().expect("idle waiter");
    let ended = end.join().expect("end thread") == Some(SignalResult::Completed);
    let drained = wait_fenced_idle(&fence, Duration::from_secs(1));
    let resumed = resume_engines(&fence);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-fence-inflight.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"scope":"isolated production registration coordinator with controlled network transitions", "fence_accepted_while_reopening":accepted_while_reopening, "pending_while_registered":pending_active, "parking_wait_bounded_and_pending":parking_wait_bounded, "parking_completed":ended, "drained_after_parking":drained, "resumed":resumed, "complete_process_release":"not established"})).expect("serialize")).expect("write evidence");
    }
    assert!(
        accepted_while_reopening,
        "fence acceptance must not wait for network reopening"
    );
    assert!(pending_active && parking_wait_bounded && ended && drained && resumed);
}
