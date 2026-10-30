//! 隔离验证进程登记门；不代表完整进程释放回执。

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
    use std::sync::Arc;
    use std::time::Duration;
    pub(crate) struct Network;
    impl Network {
        pub(crate) fn unpark(&self, _: Duration) -> SignalResult {
            SignalResult::Completed
        }
        pub(crate) fn park(&self, _: Duration) -> SignalResult {
            SignalResult::Completed
        }
    }
    pub(crate) fn installed_network_log() -> Option<Arc<Network>> {
        None
    }
    pub(crate) fn notify_engine_closer_changed() {}
}

use engine_activity::{fence_engines, resume_engines, wait_fenced_idle, EngineLogActivity};
use std::sync::Arc;
use std::time::Duration;
use uc_observability_runtime::SignalResult;

fn main() {
    let (mut first, _) = EngineLogActivity::begin(Duration::from_secs(1));
    assert!(first.is_admitted());
    let fence = fence_engines();
    let same_fence = fence_engines();
    let pending = !wait_fenced_idle(&fence, Duration::from_millis(20));
    let (rejected, result) = EngineLogActivity::begin(Duration::from_millis(20));
    let rejected_at_fence = !rejected.is_admitted() && result == Some(SignalResult::TimedOut);
    drop(rejected);
    let waited_fence = Arc::clone(&fence);
    let waiter =
        std::thread::spawn(move || wait_fenced_idle(&waited_fence, Duration::from_secs(2)));
    let blocked_begin = std::thread::spawn(|| EngineLogActivity::begin(Duration::from_secs(2)).0);
    std::thread::sleep(Duration::from_millis(20));
    first.end(Duration::from_secs(1));
    let drained = wait_fenced_idle(&fence, Duration::from_secs(1));
    let waiter_woke = waiter.join().expect("release waiter");
    let resumed = resume_engines(&fence);
    let mut next = blocked_begin.join().expect("blocked registration");
    let next_admitted = next.is_admitted();
    let new_fence = fence_engines();
    let old_rejected = !resume_engines(&fence);
    let new_pending = !wait_fenced_idle(&new_fence, Duration::from_millis(20));
    next.end(Duration::from_secs(1));
    let new_drained = wait_fenced_idle(&new_fence, Duration::from_secs(1));
    let new_resumed = resume_engines(&new_fence);
    let (abandoned, _) = EngineLogActivity::begin(Duration::from_secs(1));
    let (mut survivor, _) = EngineLogActivity::begin(Duration::from_secs(1));
    let abandoned_fence = fence_engines();
    let waiting_fence = Arc::clone(&abandoned_fence);
    let (started, ready) = std::sync::mpsc::channel();
    let abandoned_waiter = std::thread::spawn(move || {
        started.send(()).expect("waiter started");
        wait_fenced_idle(&waiting_fence, Duration::from_secs(2))
    });
    ready.recv().expect("waiter ready");
    drop(abandoned);
    let abandoned_detected = !abandoned_waiter.join().expect("abandoned waiter");
    survivor.end(Duration::from_secs(1));
    let abandoned_stable = !wait_fenced_idle(&abandoned_fence, Duration::ZERO);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-registration-fence.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"scope":"isolated production registration coordinator, controlled absent network", "same_fence":Arc::ptr_eq(&fence,&same_fence), "pending_while_active":pending, "registration_rejected_at_deadline":rejected_at_fence, "drained":drained, "waiter_woke_after_last_registration":waiter_woke, "resume":resumed, "blocked_registration_admitted_after_resume":next_admitted, "old_fence_rejected":old_rejected, "new_pending":new_pending, "new_drained":new_drained, "new_resumed":new_resumed, "abandoned_waiter_detected":abandoned_detected, "abandoned_after_other_end":abandoned_stable, "complete_process_release":"not established"})).expect("serialize")).expect("write evidence");
    }
    assert!(Arc::ptr_eq(&fence, &same_fence));
    assert!(pending && rejected_at_fence && drained && waiter_woke && resumed && next_admitted);
    assert!(old_rejected && new_pending && new_drained && new_resumed);
    assert!(abandoned_detected && abandoned_stable);
}
