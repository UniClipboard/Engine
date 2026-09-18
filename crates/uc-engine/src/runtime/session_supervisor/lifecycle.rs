use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Weak;

use async_trait::async_trait;
use tokio::task::JoinHandle;
use uc_application::deps::{LifecycleError, RuntimeLifecyclePort, TransitionContext};
use uc_application::facade::NetworkRecoveryRequestError;
use uc_core::{FileTransferCancellationReason, TaskShutdownReport};

use super::SessionSupervisor;
use crate::runtime::operation_unavailable_error;
use crate::{EngineError, EngineErrorCategory};

pub(super) struct SessionWork(pub(super) Weak<SessionSupervisor>);

pub(in super::super) fn lifecycle_error(error: LifecycleError) -> EngineError {
    let source = if error.is_stopped() || error.is_superseded() {
        match error.additional.first() {
            Some(source) => source,
            None => return EngineError::new(1001, EngineErrorCategory::InvalidState, false),
        }
    } else {
        &error.primary
    };
    if source.chain().any(|source| {
        matches!(
            source.downcast_ref::<NetworkRecoveryRequestError>(),
            Some(NetworkRecoveryRequestError::Task(_))
        )
    }) {
        return EngineError::new(1108, EngineErrorCategory::Internal, false);
    }
    if source
        .chain()
        .filter_map(|source| source.downcast_ref::<TaskShutdownReport>())
        .any(|report| report.timed_out_count > 0)
    {
        return EngineError::new(1106, EngineErrorCategory::DeadlineExceeded, true);
    }
    source
        .chain()
        .find_map(|source| source.downcast_ref::<EngineError>())
        .cloned()
        .unwrap_or_else(|| EngineError::new(1108, EngineErrorCategory::Internal, true))
}

#[async_trait]
impl RuntimeLifecyclePort for SessionWork {
    async fn suspend(&self, context: &TransitionContext) -> anyhow::Result<()> {
        let owner = self.0.upgrade().ok_or_else(operation_unavailable_error)?;
        let context = context.clone();
        // 共同期限可以结束调用方等待，但已经取得的会话必须由原负责人完整交接。
        // 外层期限取消本次等待时，独立任务继续持有 owner 和生命周期锁；后继重试会在同一锁后接续。
        join_owned(tokio::spawn(
            async move { suspend_owned(owner, context).await },
        ))
        .await
    }

    async fn resume(&self, _context: &TransitionContext) -> anyhow::Result<()> {
        let owner = self.0.upgrade().ok_or_else(operation_unavailable_error)?;
        let _lifecycle = owner.lifecycle.lock().await;
        owner.install_new_session(false).await?;
        owner
            .session_recovery_enabled
            .store(true, Ordering::Release);
        Ok(())
    }
}

async fn join_owned(task: JoinHandle<anyhow::Result<()>>) -> anyhow::Result<()> {
    task.await
        .map_err(|_| EngineError::new(1108, EngineErrorCategory::Internal, true))?
}

async fn suspend_owned(
    owner: std::sync::Arc<SessionSupervisor>,
    context: TransitionContext,
) -> anyhow::Result<()> {
    let _lifecycle = owner.lifecycle.lock().await;
    owner
        .session_recovery_enabled
        .store(false, Ordering::Release);
    let mut errors = Vec::new();
    if let Err(error) = drain_operations_and_stop_session(
        owner.operations.close_and_wait(None, context.deadline()),
        owner.stop_current_session(FileTransferCancellationReason::Unknown, context.deadline()),
    )
    .await
    {
        errors.push(error.context("stop current session work"));
    }
    if let Err(error) = owner.shutdown_network(context.deadline()).await {
        errors.push(error.into());
    }
    LifecycleError::from_errors(errors).map_err(Into::into)
}

async fn drain_operations_and_stop_session<Drain, Stop>(
    drain_operations: Drain,
    stop_session: Stop,
) -> anyhow::Result<()>
where
    Drain: Future<Output = Result<(), EngineError>>,
    Stop: Future<Output = Result<(), LifecycleError>>,
{
    drain_operations.await.map_err(anyhow::Error::new)?;
    let mut errors = Vec::new();
    if let Err(error) = stop_session.await {
        errors.push(error.primary.context("stop current session"));
        errors.extend(
            error
                .additional
                .into_iter()
                .map(|error| error.context("stop current session")),
        );
    }
    LifecycleError::from_errors(errors).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{
        lifecycle_error, EngineError, EngineErrorCategory, LifecycleError,
        NetworkRecoveryRequestError,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use uc_application::facade::RebuildNetworkSessionError;
    use uc_core::TaskRegistry;

    #[tokio::test]
    async fn operation_drain_failure_keeps_the_session_available_to_in_flight_work() {
        let stop_called = AtomicBool::new(false);
        let result = super::drain_operations_and_stop_session(
            async {
                Err(EngineError::new(
                    1106,
                    EngineErrorCategory::DeadlineExceeded,
                    true,
                ))
            },
            async {
                stop_called.store(true, Ordering::SeqCst);
                Err(LifecycleError {
                    primary: std::io::Error::other("session stop failed").into(),
                    additional: vec![std::io::Error::other("transfer stop failed").into()],
                })
            },
        )
        .await
        .unwrap_err();

        assert!(!stop_called.load(Ordering::SeqCst));
        assert_eq!(
            result.downcast_ref::<EngineError>().unwrap().category(),
            EngineErrorCategory::DeadlineExceeded
        );
    }

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

    #[test]
    fn rebuild_failure_keeps_its_original_classification_through_shutdown() {
        let original = EngineError::new(1106, EngineErrorCategory::DeadlineExceeded, false);
        let failure = NetworkRecoveryRequestError::Rebuild(RebuildNetworkSessionError::new(
            original.clone(),
            false,
        ));
        let error = LifecycleError {
            primary: anyhow::Error::new(failure).context("stop network recovery"),
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

    #[tokio::test]
    async fn owned_suspend_task_failure_is_a_stable_internal_error() {
        let error = super::join_owned(tokio::spawn(async {
            panic!("private session lifecycle failure");
        }))
        .await
        .unwrap_err();
        let error = error.downcast_ref::<EngineError>().unwrap();
        assert_eq!(error.code(), 1108);
        assert_eq!(error.category(), EngineErrorCategory::Internal);
        assert!(error.is_retryable());
    }
}
