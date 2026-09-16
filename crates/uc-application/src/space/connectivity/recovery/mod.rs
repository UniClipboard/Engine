use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;

use async_trait::async_trait;
use futures::future::{BoxFuture, FutureExt, Shared};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const RETRY_DELAYS: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkRecoveryPhase {
    Idle,
    Recovering,
    RetryScheduled,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetworkRecoveryStatus {
    pub phase: NetworkRecoveryPhase,
    pub retryable: bool,
    pub next_retry_in: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkRecoveryEvent {
    Started,
    RetryScheduled { delay: Duration },
    Succeeded,
    Failed { retryable: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RebuildNetworkSessionError {
    #[error("network session rebuild can be retried")]
    Retryable,
    #[error("network session rebuild cannot be retried")]
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NetworkRecoveryRequestError {
    #[error("network recovery runtime is stopped")]
    Stopped,
    #[error(transparent)]
    Rebuild(#[from] RebuildNetworkSessionError),
}

/// The Engine owns the complete replacement of a running network session.
/// The application layer only decides when that action is needed and how it
/// is retried.
#[async_trait]
pub trait RebuildNetworkSessionPort: Send + Sync {
    async fn rebuild_network_session(&self) -> Result<(), RebuildNetworkSessionError>;
}

#[derive(Clone)]
pub struct NetworkRecoveryFacade {
    inner: Arc<NetworkRecoveryInner>,
}

struct NetworkRecoveryInner {
    port: Arc<dyn RebuildNetworkSessionPort>,
    cancel: CancellationToken,
    manual_wake: tokio::sync::Notify,
    events: tokio::sync::broadcast::Sender<NetworkRecoveryEvent>,
    state: Mutex<RecoveryState>,
}

struct RecoveryState {
    phase: NetworkRecoveryPhase,
    next_retry_at: Option<Instant>,
    in_flight: Option<Shared<BoxFuture<'static, Result<(), NetworkRecoveryRequestError>>>>,
}

impl NetworkRecoveryFacade {
    pub fn new(port: Arc<dyn RebuildNetworkSessionPort>) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(16);
        Self {
            inner: Arc::new(NetworkRecoveryInner {
                port,
                cancel: CancellationToken::new(),
                manual_wake: tokio::sync::Notify::new(),
                events,
                state: Mutex::new(RecoveryState {
                    phase: NetworkRecoveryPhase::Idle,
                    next_retry_at: None,
                    in_flight: None,
                }),
            }),
        }
    }

    pub async fn status(&self) -> NetworkRecoveryStatus {
        let state = self.inner.state.lock().await;
        NetworkRecoveryStatus {
            phase: state.phase,
            retryable: matches!(
                state.phase,
                NetworkRecoveryPhase::RetryScheduled | NetworkRecoveryPhase::Failed
            ),
            next_retry_in: state
                .next_retry_at
                .and_then(|at| at.checked_duration_since(Instant::now())),
        }
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<NetworkRecoveryEvent> {
        self.inner.events.subscribe()
    }

    /// Starts or joins the single current recovery cycle. A manual request can
    /// use this without needing a network-change observation.
    pub async fn request_recovery(&self) -> Result<(), NetworkRecoveryRequestError> {
        let (future, wake_retry) = self.start_recovery().await?;
        if wake_retry {
            self.inner.manual_wake.notify_one();
        }
        future.await
    }

    pub async fn shutdown(&self) {
        self.inner.cancel.cancel();
        let mut state = self.inner.state.lock().await;
        state.phase = NetworkRecoveryPhase::Stopped;
        state.next_retry_at = None;
        state.in_flight = None;
    }

    async fn start_recovery(
        &self,
    ) -> Result<
        (
            Shared<BoxFuture<'static, Result<(), NetworkRecoveryRequestError>>>,
            bool,
        ),
        NetworkRecoveryRequestError,
    > {
        let mut state = self.inner.state.lock().await;
        if state.phase == NetworkRecoveryPhase::Stopped || self.inner.cancel.is_cancelled() {
            return Err(NetworkRecoveryRequestError::Stopped);
        }
        if let Some(in_flight) = state.in_flight.clone() {
            return Ok((
                in_flight,
                state.phase == NetworkRecoveryPhase::RetryScheduled,
            ));
        }

        state.phase = NetworkRecoveryPhase::Recovering;
        state.next_retry_at = None;
        let inner = Arc::clone(&self.inner);
        let future = async move { run_recovery_cycle(inner).await }
            .boxed()
            .shared();
        state.in_flight = Some(future.clone());
        let _ = self.inner.events.send(NetworkRecoveryEvent::Started);
        Ok((future, false))
    }
}

async fn run_recovery_cycle(
    inner: Arc<NetworkRecoveryInner>,
) -> Result<(), NetworkRecoveryRequestError> {
    let mut last_error = RebuildNetworkSessionError::Retryable;
    for attempt in 0..=RETRY_DELAYS.len() {
        if attempt > 0 {
            let delay = RETRY_DELAYS[attempt - 1];
            {
                let mut state = inner.state.lock().await;
                if state.phase == NetworkRecoveryPhase::Stopped {
                    return Err(NetworkRecoveryRequestError::Stopped);
                }
                state.phase = NetworkRecoveryPhase::RetryScheduled;
                state.next_retry_at = Some(Instant::now() + delay);
            }
            let _ = inner
                .events
                .send(NetworkRecoveryEvent::RetryScheduled { delay });
            tokio::select! {
                _ = inner.cancel.cancelled() => return Err(NetworkRecoveryRequestError::Stopped),
                _ = inner.manual_wake.notified() => {}
                _ = tokio::time::sleep(delay) => {}
            }
        }

        let resumed_from_retry = {
            let mut state = inner.state.lock().await;
            if state.phase == NetworkRecoveryPhase::Stopped {
                return Err(NetworkRecoveryRequestError::Stopped);
            }
            let resumed_from_retry = state.phase == NetworkRecoveryPhase::RetryScheduled;
            state.phase = NetworkRecoveryPhase::Recovering;
            state.next_retry_at = None;
            resumed_from_retry
        };
        if resumed_from_retry {
            let _ = inner.events.send(NetworkRecoveryEvent::Started);
        }
        let result = tokio::select! {
            _ = inner.cancel.cancelled() => Err(NetworkRecoveryRequestError::Stopped),
            result = inner.port.rebuild_network_session() => result.map_err(NetworkRecoveryRequestError::from),
        };
        match result {
            Ok(()) => {
                finish_cycle(&inner, NetworkRecoveryPhase::Idle, None).await;
                let _ = inner.events.send(NetworkRecoveryEvent::Succeeded);
                return Ok(());
            }
            Err(NetworkRecoveryRequestError::Rebuild(RebuildNetworkSessionError::Retryable)) => {
                last_error = RebuildNetworkSessionError::Retryable;
            }
            Err(error) => {
                finish_cycle(&inner, NetworkRecoveryPhase::Failed, None).await;
                let _ = inner
                    .events
                    .send(NetworkRecoveryEvent::Failed { retryable: false });
                return Err(error);
            }
        }
    }
    finish_cycle(&inner, NetworkRecoveryPhase::Failed, None).await;
    let _ = inner
        .events
        .send(NetworkRecoveryEvent::Failed { retryable: true });
    Err(NetworkRecoveryRequestError::Rebuild(last_error))
}

async fn finish_cycle(
    inner: &NetworkRecoveryInner,
    phase: NetworkRecoveryPhase,
    next_retry_at: Option<Instant>,
) {
    let mut state = inner.state.lock().await;
    if state.phase != NetworkRecoveryPhase::Stopped {
        state.phase = phase;
        state.next_retry_at = next_retry_at;
    }
    state.in_flight = None;
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use tokio::sync::Notify;

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
            Err(RebuildNetworkSessionError::Retryable),
            Err(RebuildNetworkSessionError::Retryable),
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

    #[tokio::test(start_paused = true)]
    async fn manual_request_wakes_a_scheduled_retry_without_starting_another_cycle() {
        let rebuilder = Arc::new(RecordingRebuilder::new([
            Err(RebuildNetworkSessionError::Retryable),
            Ok(()),
        ]));
        let recovery = NetworkRecoveryFacade::new(rebuilder.clone());
        let task = tokio::spawn({
            let recovery = recovery.clone();
            async move { recovery.request_recovery().await }
        });
        tokio::task::yield_now().await;
        assert_eq!(
            recovery.status().await.phase,
            NetworkRecoveryPhase::RetryScheduled
        );

        assert_eq!(recovery.request_recovery().await, Ok(()));
        assert_eq!(task.await.unwrap(), Ok(()));
        assert_eq!(rebuilder.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn shutdown_prevents_new_recovery_requests() {
        let recovery = NetworkRecoveryFacade::new(Arc::new(RecordingRebuilder::new([])));
        recovery.shutdown().await;
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
}
