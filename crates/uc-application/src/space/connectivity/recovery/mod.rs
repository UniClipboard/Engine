mod cycle;
mod error;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::{BoxFuture, Shared};
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::{CancellationToken, DropGuard};

use cycle::start_cycle;
pub use error::{NetworkRecoveryRequestError, RebuildNetworkSessionError};

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
    _stop_on_drop: Arc<DropGuard>,
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
    retryable: bool,
    next_retry_at: Option<Instant>,
    in_flight: Option<RecoveryCompletion>,
    failure: Option<NetworkRecoveryRequestError>,
}

type RecoveryCompletion = Shared<BoxFuture<'static, Result<(), NetworkRecoveryRequestError>>>;

impl NetworkRecoveryFacade {
    pub fn new(port: Arc<dyn RebuildNetworkSessionPort>) -> Self {
        let (events, _) = tokio::sync::broadcast::channel(16);
        let cancel = CancellationToken::new();
        Self {
            _stop_on_drop: Arc::new(cancel.clone().drop_guard()),
            inner: Arc::new(NetworkRecoveryInner {
                port,
                cancel,
                manual_wake: tokio::sync::Notify::new(),
                events,
                state: Mutex::new(RecoveryState {
                    phase: NetworkRecoveryPhase::Idle,
                    retryable: false,
                    next_retry_at: None,
                    in_flight: None,
                    failure: None,
                }),
            }),
        }
    }

    pub async fn status(&self) -> NetworkRecoveryStatus {
        let state = self.inner.state.lock().await;
        NetworkRecoveryStatus {
            phase: state.phase,
            retryable: !self.inner.cancel.is_cancelled()
                && state.failure.is_none()
                && (state.phase == NetworkRecoveryPhase::RetryScheduled
                    || (state.phase == NetworkRecoveryPhase::Failed && state.retryable)),
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

    pub async fn shutdown(&self) -> Result<(), NetworkRecoveryRequestError> {
        let in_flight = {
            let mut state = self.inner.state.lock().await;
            self.inner.cancel.cancel();
            state.next_retry_at = None;
            state.in_flight.clone()
        };
        let result = if let Some(in_flight) = in_flight {
            match in_flight.await {
                Ok(()) | Err(NetworkRecoveryRequestError::Stopped) => Ok(()),
                Err(error) => Err(error),
            }
        } else {
            Ok(())
        };
        let mut state = self.inner.state.lock().await;
        state.phase = NetworkRecoveryPhase::Stopped;
        match state.failure.clone() {
            Some(error) => Err(error),
            None => result,
        }
    }

    async fn start_recovery(
        &self,
    ) -> Result<(RecoveryCompletion, bool), NetworkRecoveryRequestError> {
        let mut state = self.inner.state.lock().await;
        if state.phase == NetworkRecoveryPhase::Stopped || self.inner.cancel.is_cancelled() {
            return Err(NetworkRecoveryRequestError::Stopped);
        }
        if let Some(error) = state.failure.clone() {
            return Err(error);
        }
        if let Some(in_flight) = state.in_flight.clone() {
            return Ok((
                in_flight,
                state.phase == NetworkRecoveryPhase::RetryScheduled,
            ));
        }

        state.phase = NetworkRecoveryPhase::Recovering;
        state.next_retry_at = None;
        let future = start_cycle(Arc::clone(&self.inner));
        state.in_flight = Some(future.clone());
        let _ = self.inner.events.send(NetworkRecoveryEvent::Started);
        Ok((future, false))
    }
}

#[cfg(test)]
mod tests;
