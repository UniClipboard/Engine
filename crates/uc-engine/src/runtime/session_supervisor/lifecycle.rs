use std::sync::atomic::Ordering;
use std::sync::Weak;

use async_trait::async_trait;
use uc_application::facade::{LifecycleError, RuntimeLifecyclePort, TransitionContext};
use uc_core::FileTransferCancellationReason;

use super::SessionSupervisor;
use crate::runtime::operation_unavailable_error;
use crate::{EngineError, EngineErrorCategory};

pub(super) struct SessionWork(pub(super) Weak<SessionSupervisor>);

pub(super) fn lifecycle_error(error: LifecycleError) -> EngineError {
    if error.is_stopped() {
        return EngineError::new(1001, EngineErrorCategory::InvalidState, false);
    }
    error
        .primary
        .downcast_ref::<EngineError>()
        .cloned()
        .unwrap_or_else(|| EngineError::new(1108, EngineErrorCategory::Internal, true))
}

#[async_trait]
impl RuntimeLifecyclePort for SessionWork {
    async fn suspend(&self, _context: &TransitionContext) -> anyhow::Result<()> {
        let owner = self.0.upgrade().ok_or_else(operation_unavailable_error)?;
        let _lifecycle = owner.lifecycle.lock().await;
        owner.suspended.store(true, Ordering::Release);
        owner.operations.close_and_wait(None).await?;
        owner
            .stop_current_session(FileTransferCancellationReason::Unknown)
            .await?;
        Ok(())
    }

    async fn resume(&self, _context: &TransitionContext) -> anyhow::Result<()> {
        let owner = self.0.upgrade().ok_or_else(operation_unavailable_error)?;
        let _lifecycle = owner.lifecycle.lock().await;
        owner.install_new_session(false).await?;
        owner.suspended.store(false, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{lifecycle_error, EngineError, EngineErrorCategory, LifecycleError};

    #[test]
    fn participant_failure_keeps_the_engine_category_and_retry_policy() {
        let original = EngineError::new(1106, EngineErrorCategory::DeadlineExceeded, true);
        let error = LifecycleError {
            primary: anyhow::Error::new(original.clone()).context("stop session work"),
            additional: Vec::new(),
        };
        assert_eq!(lifecycle_error(error), original);
    }
}
