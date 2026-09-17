use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify};

use super::{
    NetworkRecoveryEvent, NetworkRecoveryFacade, NetworkRecoveryPhase, NetworkRecoveryRequestError,
    RebuildNetworkSessionError, RebuildNetworkSessionPort,
};

mod shutdown;
mod sources;

fn retryable_failure() -> RebuildNetworkSessionError {
    RebuildNetworkSessionError::new(io::Error::other("test rebuild failure"), true)
}

struct RecordingRebuilder {
    calls: AtomicUsize,
    results: Mutex<VecDeque<Result<(), RebuildNetworkSessionError>>>,
}

impl RecordingRebuilder {
    fn new(results: impl IntoIterator<Item = Result<(), RebuildNetworkSessionError>>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            results: Mutex::new(results.into_iter().collect()),
        }
    }
}

#[async_trait]
impl RebuildNetworkSessionPort for RecordingRebuilder {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.results.lock().await.pop_front().unwrap_or(Ok(()))
    }
}

struct BlockingRebuilder {
    calls: AtomicUsize,
    started: Notify,
    release: Notify,
}

impl BlockingRebuilder {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            started: Notify::new(),
            release: Notify::new(),
        }
    }
}

#[async_trait]
impl RebuildNetworkSessionPort for BlockingRebuilder {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_waiters();
        self.release.notified().await;
        Ok(())
    }
}

#[tokio::test]
async fn simultaneous_manual_requests_share_one_rebuild() {
    let rebuilder = Arc::new(BlockingRebuilder::new());
    let recovery = NetworkRecoveryFacade::new(rebuilder.clone());
    let first = tokio::spawn({
        let recovery = recovery.clone();
        async move { recovery.request_recovery().await }
    });
    rebuilder.started.notified().await;
    let second = tokio::spawn({
        let recovery = recovery.clone();
        async move { recovery.request_recovery().await }
    });
    tokio::task::yield_now().await;
    assert_eq!(rebuilder.calls.load(Ordering::SeqCst), 1);
    rebuilder.release.notify_waiters();

    for result in [first.await, second.await] {
        match result {
            Ok(outcome) => assert_eq!(outcome, Ok(())),
            Err(error) => panic!("recovery task did not complete: {error}"),
        }
    }
    assert_eq!(rebuilder.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn retryable_failures_use_the_bounded_retry_ladder() {
    let rebuilder = Arc::new(RecordingRebuilder::new([
        Err(retryable_failure()),
        Err(retryable_failure()),
        Ok(()),
    ]));
    let recovery = NetworkRecoveryFacade::new(rebuilder.clone());
    let task = tokio::spawn({
        let recovery = recovery.clone();
        async move { recovery.request_recovery().await }
    });
    tokio::task::yield_now().await;
    assert_eq!(rebuilder.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        recovery.status().await.phase,
        NetworkRecoveryPhase::RetryScheduled
    );

    tokio::time::advance(Duration::from_secs(1)).await;
    tokio::task::yield_now().await;
    assert_eq!(rebuilder.calls.load(Ordering::SeqCst), 2);
    tokio::time::advance(Duration::from_secs(2)).await;
    match task.await {
        Ok(result) => assert_eq!(result, Ok(())),
        Err(error) => panic!("recovery task did not complete: {error}"),
    }
    assert_eq!(rebuilder.calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn shutdown_prevents_new_recovery_requests() {
    let recovery = NetworkRecoveryFacade::new(Arc::new(RecordingRebuilder::new([])));
    recovery.shutdown().await.unwrap();
    assert_eq!(
        recovery.request_recovery().await,
        Err(NetworkRecoveryRequestError::Stopped)
    );
    assert_eq!(recovery.status().await.phase, NetworkRecoveryPhase::Stopped);
}

#[tokio::test]
async fn recovery_publishes_started_and_succeeded_events() {
    let recovery = NetworkRecoveryFacade::new(Arc::new(RecordingRebuilder::new([Ok(())])));
    let mut events = recovery.subscribe();

    assert_eq!(recovery.request_recovery().await, Ok(()));
    assert_eq!(events.recv().await, Ok(NetworkRecoveryEvent::Started));
    assert_eq!(events.recv().await, Ok(NetworkRecoveryEvent::Succeeded));
}
