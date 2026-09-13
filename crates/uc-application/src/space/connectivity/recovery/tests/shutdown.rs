use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{oneshot, Notify};
use tokio::time::timeout;

use super::{
    retryable_failure, NetworkRecoveryEvent, NetworkRecoveryFacade, NetworkRecoveryPhase,
    NetworkRecoveryRequestError, RebuildNetworkSessionError, RebuildNetworkSessionPort,
    RecordingRebuilder,
};

struct DiskRebuilder {
    entered: Notify,
    release: std::sync::Mutex<Option<oneshot::Receiver<()>>>,
    completed: AtomicBool,
    fail: AtomicBool,
}

#[async_trait]
impl RebuildNetworkSessionPort for DiskRebuilder {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError> {
        let release = self.release.lock().unwrap().take().unwrap();
        let disk = tokio::task::spawn_blocking(move || release.blocking_recv().unwrap());
        self.entered.notify_one();
        disk.await.unwrap();
        self.completed.store(true, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            Err(retryable_failure())
        } else {
            Ok(())
        }
    }
}

fn fixture() -> (
    Arc<DiskRebuilder>,
    oneshot::Sender<()>,
    NetworkRecoveryFacade,
) {
    let (release, wait) = oneshot::channel();
    let port = Arc::new(DiskRebuilder {
        entered: Notify::new(),
        release: std::sync::Mutex::new(Some(wait)),
        completed: AtomicBool::new(false),
        fail: AtomicBool::new(false),
    });
    let recovery = NetworkRecoveryFacade::new(port.clone());
    (port, release, recovery)
}

#[tokio::test]
async fn shutdown_waits_for_the_started_disk_action_after_the_requester_leaves() {
    let (port, release, recovery) = fixture();
    let mut events = recovery.subscribe();
    let caller = tokio::spawn({
        let recovery = recovery.clone();
        async move { recovery.request_recovery().await }
    });
    port.entered.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let mut closing = tokio::spawn({
        let recovery = recovery.clone();
        async move { recovery.shutdown().await }
    });
    let premature = timeout(Duration::from_millis(20), &mut closing).await;
    let phase_before_release = recovery.status().await.phase;
    // 先放行真实阻塞线程，断言失败时也不让测试运行期卡在销毁。
    release.send(()).unwrap();
    assert!(
        premature.is_err(),
        "shutdown returned while disk work remained active"
    );
    assert_ne!(phase_before_release, NetworkRecoveryPhase::Stopped);
    closing.await.unwrap().unwrap();
    assert!(port.completed.load(Ordering::SeqCst));
    assert_eq!(recovery.status().await.phase, NetworkRecoveryPhase::Stopped);
    assert_eq!(
        recovery.request_recovery().await,
        Err(NetworkRecoveryRequestError::Stopped)
    );
    assert_eq!(events.try_recv().unwrap(), NetworkRecoveryEvent::Started);
    assert!(
        events.try_recv().is_err(),
        "stopping must not publish a late recovery success"
    );
}

#[tokio::test]
async fn abandoned_manual_request_still_completes_without_another_polling_caller() {
    let (port, release, recovery) = fixture();
    let caller = tokio::spawn({
        let recovery = recovery.clone();
        async move { recovery.request_recovery().await }
    });
    port.entered.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    timeout(Duration::from_secs(1), async {
        while !port.completed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    recovery.shutdown().await.unwrap();
}

#[tokio::test]
async fn repeated_shutdown_waits_even_after_the_first_shutdown_caller_leaves() {
    let (port, release, recovery) = fixture();
    let caller = tokio::spawn({
        let recovery = recovery.clone();
        async move { recovery.request_recovery().await }
    });
    port.entered.notified().await;
    let mut first = Box::pin(recovery.shutdown());
    assert!(timeout(Duration::from_millis(20), first.as_mut())
        .await
        .is_err());
    drop(first);
    let mut second = Box::pin(recovery.shutdown());
    let premature = timeout(Duration::from_millis(20), second.as_mut()).await;
    release.send(()).unwrap();
    assert!(premature.is_err());
    second.await.unwrap();
    assert_eq!(
        caller.await.unwrap(),
        Err(NetworkRecoveryRequestError::Stopped)
    );
    assert!(port.completed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn shutdown_before_a_queued_cycle_starts_does_not_begin_a_rebuild() {
    let port = Arc::new(RecordingRebuilder::new([]));
    let recovery = NetworkRecoveryFacade::new(port.clone());
    let (request, _) = recovery.start_recovery().await.unwrap();
    recovery.shutdown().await.unwrap();
    assert_eq!(request.await, Err(NetworkRecoveryRequestError::Stopped));
    assert_eq!(port.calls.load(Ordering::SeqCst), 0);
}

struct PanickingRebuilder;

#[async_trait]
impl RebuildNetworkSessionPort for PanickingRebuilder {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError> {
        panic!("sensitive recovery panic payload");
    }
}

#[tokio::test]
async fn worker_panic_is_retained_for_requests_and_repeated_shutdown() {
    let recovery = NetworkRecoveryFacade::new(Arc::new(PanickingRebuilder));
    let original = recovery.request_recovery().await.unwrap_err();
    let NetworkRecoveryRequestError::Task(source) = &original else {
        panic!("expected original task failure");
    };
    assert!(source.is_panic());
    assert!(original.source().is_some());
    assert!(!format!("{original:?}").contains("sensitive"));
    assert!(!format!("{original}").contains("sensitive"));
    assert!(!recovery.status().await.retryable);
    assert_eq!(recovery.request_recovery().await.unwrap_err(), original);
    assert_eq!(recovery.shutdown().await.unwrap_err(), original);
    assert_eq!(recovery.shutdown().await.unwrap_err(), original);
}

#[tokio::test(start_paused = true)]
async fn final_stop_wins_over_a_ready_manual_retry() {
    let port = Arc::new(RecordingRebuilder::new([Err(retryable_failure()), Ok(())]));
    let recovery = NetworkRecoveryFacade::new(port.clone());
    let (result, _) = recovery.start_recovery().await.unwrap();
    tokio::task::yield_now().await;
    assert_eq!(
        recovery.status().await.phase,
        NetworkRecoveryPhase::RetryScheduled
    );

    recovery.inner.cancel.cancel();
    recovery.inner.manual_wake.notify_one();

    assert_eq!(result.await, Err(NetworkRecoveryRequestError::Stopped));
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn releasing_the_last_facade_stops_scheduled_retries() {
    let port = Arc::new(RecordingRebuilder::new([Err(retryable_failure()), Ok(())]));
    let recovery = NetworkRecoveryFacade::new(port.clone());
    let mut events = recovery.subscribe();
    let (result, _) = recovery.start_recovery().await.unwrap();
    assert_eq!(events.recv().await.unwrap(), NetworkRecoveryEvent::Started);
    assert!(matches!(
        events.recv().await.unwrap(),
        NetworkRecoveryEvent::RetryScheduled { .. }
    ));
    drop(recovery);
    assert_eq!(result.await, Err(NetworkRecoveryRequestError::Stopped));
    assert_eq!(port.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_rebuild_failure_during_shutdown_is_retained_for_repeated_confirmation() {
    let (port, release, recovery) = fixture();
    port.fail.store(true, Ordering::SeqCst);
    let (request, _) = recovery.start_recovery().await.unwrap();
    port.entered.notified().await;
    let mut closing = Box::pin(recovery.shutdown());
    let premature = timeout(Duration::from_millis(20), closing.as_mut()).await;
    release.send(()).unwrap();
    assert!(premature.is_err());
    let error = closing.await.unwrap_err();
    assert!(error
        .source()
        .unwrap()
        .downcast_ref::<io::Error>()
        .is_some());
    assert_eq!(request.await.unwrap_err(), error);
    assert_eq!(recovery.shutdown().await.unwrap_err(), error);
}

#[tokio::test]
async fn shutdown_queued_before_completion_suppresses_the_stale_success_event() {
    let (port, release, recovery) = fixture();
    let mut events = recovery.subscribe();
    let (request, _) = recovery.start_recovery().await.unwrap();
    port.entered.notified().await;
    let state = recovery.inner.state.lock().await;
    let mut closing = Box::pin(recovery.shutdown());
    assert!(timeout(Duration::from_millis(20), closing.as_mut())
        .await
        .is_err());
    release.send(()).unwrap();
    timeout(Duration::from_secs(1), async {
        while !port.completed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(state);
    closing.await.unwrap();
    assert_eq!(request.await, Err(NetworkRecoveryRequestError::Stopped));
    assert_eq!(events.try_recv().unwrap(), NetworkRecoveryEvent::Started);
    assert!(events.try_recv().is_err());
}
