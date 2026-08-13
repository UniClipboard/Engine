use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;

use super::{operation_error_with_code, ProductionRuntime, ProductionSession, SessionFactory};
use crate::EngineError;

const SESSION_OPERATION_GRACE: Duration = Duration::from_secs(2);

pub(super) struct SessionSupervisor {
    session: Arc<Mutex<Option<ProductionSession>>>,
    factory: StdMutex<Option<Arc<SessionFactory>>>,
    file_transfer: Arc<uc_application::facade::FileTransferFacade>,
    lifecycle: Mutex<()>,
    operations: SessionOperationGate,
}

pub(super) struct SessionOperationLease {
    gate: Arc<SessionOperationGateInner>,
    cancellation: CancellationToken,
}

struct SessionOperationGate {
    inner: Arc<SessionOperationGateInner>,
}

struct SessionOperationGateInner {
    state: StdMutex<SessionOperationGateState>,
    changed: Notify,
}

struct SessionOperationGateState {
    open: bool,
    active: usize,
    cancellation: CancellationToken,
}

impl SessionSupervisor {
    pub(super) fn new(
        session: Arc<Mutex<Option<ProductionSession>>>,
        file_transfer: Arc<uc_application::facade::FileTransferFacade>,
    ) -> Self {
        Self {
            session,
            factory: StdMutex::new(None),
            file_transfer,
            lifecycle: Mutex::new(()),
            operations: SessionOperationGate::new_open(),
        }
    }

    pub(super) fn session(&self) -> Arc<Mutex<Option<ProductionSession>>> {
        Arc::clone(&self.session)
    }

    pub(super) fn configure_factory(&self, factory: Arc<SessionFactory>) {
        let mut slot = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(factory);
    }

    pub(super) async fn acquire_operation(&self) -> Result<SessionOperationLease, EngineError> {
        self.operations.acquire()
    }

    pub(super) async fn rebuild_session(&self) -> Result<(), EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations.close_and_wait().await;
        self.stop_current_session(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
            .await?;
        self.install_new_session().await
    }

    pub(super) async fn suspend(&self) -> Result<(), EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations.close_and_wait().await;
        self.stop_current_session(uc_core::FileTransferCancellationReason::Unknown)
            .await
    }

    pub(super) async fn resume(&self) -> Result<(), EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        if self.session.lock().await.is_some() {
            return Ok(());
        }
        self.install_new_session().await
    }

    pub(super) async fn close_file_transfers(&self) -> Result<(), EngineError> {
        self.file_transfer
            .close()
            .await
            .map_err(|error| operation_error_with_code(1104, "close file transfers", error))
    }

    async fn stop_current_session(
        &self,
        reason: uc_core::FileTransferCancellationReason,
    ) -> Result<(), EngineError> {
        let session = self.session.lock().await.take();
        if let Some(session) = session {
            session.shutdown(reason).await;
            self.file_transfer
                .cancel_active_sessions(reason)
                .await
                .map_err(|error| {
                    operation_error_with_code(1104, "cancel active file transfers", error)
                })?;
        }
        Ok(())
    }

    async fn install_new_session(&self) -> Result<(), EngineError> {
        let factory = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(super::operation_unavailable_error)?;
        let session = ProductionRuntime::build_session(&factory).await?;
        *self.session.lock().await = Some(session);
        self.operations.reopen();
        Ok(())
    }
}

#[async_trait::async_trait]
impl uc_application::facade::RebuildNetworkSessionPort for SessionSupervisor {
    async fn rebuild_network_session(
        &self,
    ) -> Result<(), uc_application::facade::RebuildNetworkSessionError> {
        self.rebuild_session().await.map_err(|error| {
            if error.is_retryable() {
                uc_application::facade::RebuildNetworkSessionError::Retryable
            } else {
                uc_application::facade::RebuildNetworkSessionError::Permanent
            }
        })
    }
}

impl SessionOperationGate {
    fn new_open() -> Self {
        Self {
            inner: Arc::new(SessionOperationGateInner {
                state: StdMutex::new(SessionOperationGateState {
                    open: true,
                    active: 0,
                    cancellation: CancellationToken::new(),
                }),
                changed: Notify::new(),
            }),
        }
    }

    fn acquire(&self) -> Result<SessionOperationLease, EngineError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.open {
            return Err(super::operation_unavailable_error());
        }
        state.active = state.active.saturating_add(1);
        Ok(SessionOperationLease {
            gate: Arc::clone(&self.inner),
            cancellation: state.cancellation.clone(),
        })
    }

    fn reopen(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.open = true;
        state.cancellation = CancellationToken::new();
        self.inner.changed.notify_waiters();
    }

    async fn close_and_wait(&self) {
        let cancellation = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.open = false;
            state.cancellation.clone()
        };
        let drained = tokio::time::timeout(SESSION_OPERATION_GRACE, self.wait_for_drain()).await;
        if drained.is_err() {
            cancellation.cancel();
            if tokio::time::timeout(SESSION_OPERATION_GRACE, self.wait_for_drain())
                .await
                .is_err()
            {
                tracing::warn!(
                    error_kind = "session_operation_drain_timeout",
                    "session operation did not stop after cancellation"
                );
            }
        }
    }

    async fn wait_for_drain(&self) {
        loop {
            let notified = self.inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let active = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .active;
            if active == 0 {
                return;
            }
            notified.await;
        }
    }
}

impl SessionOperationLease {
    pub(super) fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

impl Drop for SessionOperationLease {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.active = state.active.saturating_sub(1);
        drop(state);
        self.gate.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn closed_gate_rejects_new_operations_and_waits_for_the_existing_one() {
        let gate = Arc::new(SessionOperationGate::new_open());
        let lease = gate.acquire().expect("new gate accepts an operation");
        let closing = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move { gate.close_and_wait().await }
        });
        tokio::task::yield_now().await;
        assert!(gate.acquire().is_err());
        assert!(!closing.is_finished());

        drop(lease);
        match closing.await {
            Ok(()) => {}
            Err(error) => panic!("operation gate close task did not complete: {error}"),
        }
    }

    // 流程：会话关闭后仍有操作忽略取消信号；等待两段固定期限后关闭必须继续完成。
    #[tokio::test(start_paused = true)]
    async fn closed_gate_stops_waiting_when_an_operation_ignores_cancellation() {
        let gate = Arc::new(SessionOperationGate::new_open());
        let _lease = gate.acquire().expect("new gate accepts an operation");

        tokio::time::timeout(
            SESSION_OPERATION_GRACE.saturating_mul(2) + Duration::from_millis(1),
            gate.close_and_wait(),
        )
        .await
        .expect("gate close must remain bounded after cancellation");
    }
}
