use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::engine::EngineRuntime;
use crate::{Operation, OperationResult, StartupProgress, StartupState};

struct HeldShutdown {
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
    fail: bool,
}

#[async_trait]
impl EngineRuntime for HeldShutdown {
    async fn execute(
        &self,
        _: Operation,
        _: CancellationToken,
    ) -> Result<OperationResult, EngineError> {
        unreachable!("unclaimed startup cannot accept operations");
    }

    async fn suspend(&self, _: Option<Instant>) -> Result<(), EngineError> {
        unreachable!("unclaimed startup cannot suspend");
    }

    async fn resume(&self, _cancellation: CancellationToken) -> Result<(), EngineError> {
        unreachable!("unclaimed startup cannot resume");
    }

    async fn shutdown(&self, _: Option<Instant>) -> Result<(), EngineError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.notified().await;
        if self.fail {
            Err(EngineError::new(1108, EngineErrorCategory::Internal, false))
        } else {
            Ok(())
        }
    }
}

fn prepared(fail: bool) -> (StartupHandoff, StartupProgress, Arc<HeldShutdown>) {
    let runtime = Arc::new(HeldShutdown {
        entered: Notify::new(),
        release: Notify::new(),
        calls: AtomicUsize::new(0),
        fail,
    });
    let parts = Engine::from_runtime(Arc::clone(&runtime), 16);
    let (input, progress) = StartupProgress::channel();
    input.store.starting_services();
    (StartupHandoff::new(parts, input), progress, runtime)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_startup_dropped_outside_runtime_keeps_cleanup_and_retry_permission_owned() {
    for delivered in [false, true] {
        let (handoff, mut progress, runtime) = prepared(false);
        let (sender, receiver) = oneshot::channel();
        if delivered {
            assert!(sender.send(handoff).is_ok());
            std::thread::spawn(move || drop(receiver)).join().unwrap();
        } else {
            drop(receiver);
            assert!(sender.send(handoff).is_err());
        }
        timeout(Duration::from_secs(1), runtime.entered.notified())
            .await
            .unwrap();
        assert_eq!(progress.snapshot().state, StartupState::StartingServices);
        assert!(!progress.snapshot().allowed_actions.retry);
        runtime.release.notify_one();
        timeout(Duration::from_secs(1), async {
            while !progress.snapshot().state.is_terminal() {
                progress.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        assert_eq!(progress.snapshot().state, StartupState::Interrupted);
        assert!(progress.snapshot().allowed_actions.retry);
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        assert_eq!(Arc::strong_count(&runtime), 1);
    }
}

#[tokio::test]
async fn abandoned_startup_cleanup_failure_is_visible_to_the_progress_observer() {
    let (handoff, mut progress, runtime) = prepared(true);
    drop(handoff);
    timeout(Duration::from_secs(1), runtime.entered.notified())
        .await
        .unwrap();
    runtime.release.notify_one();
    timeout(Duration::from_secs(1), async {
        while !progress.snapshot().state.is_terminal() {
            progress.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert_eq!(progress.snapshot().state, StartupState::Failed);
    assert!(!progress.snapshot().allowed_actions.retry);
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn claimed_startup_becomes_ready_and_leaves_shutdown_to_the_caller() {
    let (handoff, progress, runtime) = prepared(false);
    let (engine, _events) = handoff.claim().unwrap();
    assert_eq!(progress.snapshot().state, StartupState::Ready);
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 0);
    runtime.release.notify_one();
    engine.shutdown(Duration::from_secs(1)).await.unwrap();
    assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
}
