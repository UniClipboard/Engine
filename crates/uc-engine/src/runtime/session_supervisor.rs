use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::{operation_error_with_code, ProductionRuntime, ProductionSession, SessionFactory};
use crate::{EngineError, OperationResult};

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

    pub(super) fn clear_factory(&self) {
        let mut slot = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = None;
    }

    pub(super) async fn acquire_operation(&self) -> Result<SessionOperationLease, EngineError> {
        self.operations.acquire()
    }

    pub(super) async fn rebuild_session(&self) -> Result<(), EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations.close_and_wait(None).await?;
        self.stop_current_session(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
            .await?;
        self.install_new_session(false).await
    }

    pub(super) async fn transition_session(
        &self,
        current_operation: SessionOperationLease,
    ) -> Result<uc_application::facade::CurrentJoinStatus, EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations
            .close_and_wait(Some(current_operation))
            .await?;
        let session = self
            .session
            .lock()
            .await
            .take()
            .ok_or_else(super::operation_unavailable_error)?;
        let facade = Arc::clone(&session.facade);
        session
            .shutdown(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
            .await;
        let status = facade
            .complete_pending_space_transition()
            .await
            .map_err(|error| operation_error_with_code(1103, "recover space transition", error))?;
        self.install_new_session(true).await?;
        Ok(status)
    }

    pub(super) async fn reset_space(
        &self,
        current_operation: SessionOperationLease,
    ) -> Result<OperationResult, EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations
            .close_and_wait(Some(current_operation))
            .await?;
        let session = self
            .session
            .lock()
            .await
            .take()
            .ok_or_else(super::operation_unavailable_error)?;
        let facade = Arc::clone(&session.facade);
        session
            .shutdown(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
            .await;
        let transfer_result = self
            .file_transfer
            .cancel_active_sessions(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
            .await
            .map_err(|error| {
                operation_error_with_code(1104, "cancel active file transfers", error)
            });
        if let Err(error) = transfer_result {
            return match self.install_new_session(false).await {
                Ok(()) => Err(error),
                Err(install_error) => Err(install_error),
            };
        }

        let result = crate::operations::space::reset_space::execute_reset_space(&facade).await;
        let install_result = self.install_new_session(result.is_ok()).await;
        match (result, install_result) {
            (Ok(result), Ok(())) => Ok(result),
            (Err(error), Ok(())) => {
                let facade = self
                    .session
                    .lock()
                    .await
                    .as_ref()
                    .map(|session| Arc::clone(&session.facade))
                    .ok_or_else(super::operation_unavailable_error)?;
                match facade.has_committed_device_management_reset().await {
                    Ok(true) => {
                        crate::operations::space::reset_space::execute_reset_space(&facade).await
                    }
                    Ok(false) | Err(_) => Err(error),
                }
            }
            (_, Err(error)) => Err(error),
        }
    }

    pub(super) async fn suspend(&self) -> Result<(), EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        self.operations.close_and_wait(None).await?;
        self.stop_current_session(uc_core::FileTransferCancellationReason::Unknown)
            .await
    }

    pub(super) async fn resume(&self) -> Result<(), EngineError> {
        let _lifecycle = self.lifecycle.lock().await;
        if self.session.lock().await.is_some() {
            return Ok(());
        }
        self.install_new_session(false).await
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

    async fn install_new_session(&self, resume_space_activities: bool) -> Result<(), EngineError> {
        let factory = self
            .factory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .ok_or_else(super::operation_unavailable_error)?;
        let mut session = ProductionRuntime::build_session(&factory).await?;
        let mut resume_space_activities = resume_space_activities;
        if session
            .facade
            .has_pending_space_transition()
            .await
            .map_err(|error| {
                operation_error_with_code(1103, "inspect pending space transition", error)
            })?
        {
            let facade = Arc::clone(&session.facade);
            session
                .shutdown(uc_core::FileTransferCancellationReason::ConnectivityRecovery)
                .await;
            facade
                .complete_pending_space_transition()
                .await
                .map_err(|error| {
                    operation_error_with_code(1103, "recover pending space transition", error)
                })?;
            session = ProductionRuntime::build_session(&factory).await?;
            resume_space_activities = true;
        }
        if resume_space_activities {
            let recovered = session
                .facade
                .recover_space_session()
                .await
                .map_err(|error| {
                    operation_error_with_code(1103, "activate transitioned space session", error)
                })?;
            if !recovered.unlocked {
                return Err(operation_error_with_code(
                    1103,
                    "activate transitioned space session",
                    "the transitioned space could not be unlocked",
                ));
            }
        }
        if let Some(pending) = factory
            .space_join
            .recover_completion()
            .await
            .map_err(|error| {
                operation_error_with_code(1103, "rebuild join completion acknowledgment", error)
            })?
        {
            if let Err(error) = session.facade.deliver_join_completion_ack(pending).await {
                warn!(error = %error, "join completion acknowledgment delivery deferred");
            }
        }
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

    async fn close_and_wait(
        &self,
        current_operation: Option<SessionOperationLease>,
    ) -> Result<(), EngineError> {
        if current_operation
            .as_ref()
            .is_some_and(|lease| !Arc::ptr_eq(&lease.gate, &self.inner))
        {
            return Err(super::operation_unavailable_error());
        }
        let cancellation = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.open = false;
            state.cancellation.clone()
        };
        drop(current_operation);
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
        Ok(())
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
            async move { gate.close_and_wait(None).await }
        });
        tokio::task::yield_now().await;
        assert!(gate.acquire().is_err());
        assert!(!closing.is_finished());

        drop(lease);
        match closing.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("operation gate close failed: {error}"),
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
            gate.close_and_wait(None),
        )
        .await
        .expect("gate close must remain bounded after cancellation")
        .expect("gate close must accept no current operation");
    }

    #[tokio::test]
    async fn transition_operation_excludes_itself_but_waits_for_other_operations() {
        let gate = Arc::new(SessionOperationGate::new_open());
        let transition = gate.acquire().expect("transition acquires ordinary lease");
        let other = gate.acquire().expect("concurrent operation acquires lease");
        let closing = tokio::spawn({
            let gate = Arc::clone(&gate);
            async move { gate.close_and_wait(Some(transition)).await }
        });

        tokio::task::yield_now().await;
        assert!(gate.acquire().is_err());
        assert!(!closing.is_finished());

        drop(other);
        closing.await.unwrap().unwrap();
    }
}
