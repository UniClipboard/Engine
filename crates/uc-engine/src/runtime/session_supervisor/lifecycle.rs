use std::sync::atomic::Ordering;
use std::sync::Weak;

use async_trait::async_trait;
use uc_application::facade::{
    LifecycleError, NetworkRecoveryRequestError, RuntimeLifecyclePort, TransitionContext,
};
use uc_core::{FileTransferCancellationReason, TaskShutdownReport};

use super::SessionSupervisor;
use crate::runtime::operation_unavailable_error;
use crate::{EngineError, EngineErrorCategory};

pub(super) struct SessionWork(pub(super) Weak<SessionSupervisor>);

pub(in super::super) fn lifecycle_error(error: LifecycleError) -> EngineError {
    if error.is_stopped() {
        return EngineError::new(1001, EngineErrorCategory::InvalidState, false);
    }
    if error.primary.chain().any(|source| {
        matches!(
            source.downcast_ref::<NetworkRecoveryRequestError>(),
            Some(NetworkRecoveryRequestError::Task(_))
        )
    }) {
        return EngineError::new(1108, EngineErrorCategory::Internal, false);
    }
    if error
        .primary
        .chain()
        .filter_map(|source| source.downcast_ref::<TaskShutdownReport>())
        .any(|report| report.timed_out_count > 0)
    {
        return EngineError::new(1106, EngineErrorCategory::DeadlineExceeded, true);
    }
    error
        .primary
        .downcast_ref::<EngineError>()
        .cloned()
        .unwrap_or_else(|| EngineError::new(1108, EngineErrorCategory::Internal, true))
}

#[async_trait]
impl RuntimeLifecyclePort for SessionWork {
    async fn suspend(&self, context: &TransitionContext) -> anyhow::Result<()> {
        let owner = self.0.upgrade().ok_or_else(operation_unavailable_error)?;
        let _lifecycle = owner.lifecycle.lock().await;
        owner.suspended.store(true, Ordering::Release);
        owner
            .operations
            .close_and_wait(None, context.deadline())
            .await?;
        owner
            .stop_current_session(FileTransferCancellationReason::Unknown, context.deadline())
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
    use super::{
        lifecycle_error, EngineError, EngineErrorCategory, LifecycleError,
        NetworkRecoveryRequestError,
    };
    use std::sync::Arc;
    use std::time::Duration;
    use uc_core::TaskRegistry;

    #[tokio::test]
    async fn task_timeout_keeps_its_category_through_nested_shutdown_reports() {
        let registry = TaskRegistry::new();
        assert!(registry.spawn(|_| std::future::pending()).await);
        let tasks = registry
            .shutdown(Duration::ZERO)
            .await
            .into_result()
            .unwrap_err();
        let inner = LifecycleError {
            primary: tasks.into(),
            additional: Vec::new(),
        };
        let outer = LifecycleError {
            primary: anyhow::Error::new(inner).context("stop session work"),
            additional: Vec::new(),
        };
        assert_eq!(
            lifecycle_error(outer),
            EngineError::new(1106, EngineErrorCategory::DeadlineExceeded, true)
        );
    }

    #[test]
    fn participant_failure_keeps_the_engine_category_and_retry_policy() {
        let original = EngineError::new(1106, EngineErrorCategory::DeadlineExceeded, true);
        let error = LifecycleError {
            primary: anyhow::Error::new(original.clone()).context("stop session work"),
            additional: Vec::new(),
        };
        assert_eq!(lifecycle_error(error), original);
    }

    #[tokio::test]
    async fn network_recovery_panic_keeps_its_non_retryable_classification_during_shutdown() {
        let source = tokio::spawn(async { panic!("private recovery failure") })
            .await
            .unwrap_err();
        let failure = NetworkRecoveryRequestError::Task(Arc::new(source));
        let error = LifecycleError {
            primary: anyhow::Error::new(failure).context("stop network recovery"),
            additional: Vec::new(),
        };
        assert_eq!(
            lifecycle_error(error),
            EngineError::new(1108, EngineErrorCategory::Internal, false)
        );
    }
}
