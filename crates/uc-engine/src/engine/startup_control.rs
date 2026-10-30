#[cfg(feature = "dev-tools")]
use std::io::ErrorKind;
use std::sync::Arc;
use std::time::Duration;

use super::lifecycle::{submit, Request, TransitionQueue};
use super::startup_owner::StartupCleanupOwner;
use super::StartupOutcomeSnapshot;
#[cfg(feature = "dev-tools")]
use crate::HostCapabilityErrorCategory;
use crate::{EngineError, EngineErrorCategory};

#[cfg(test)]
mod tests;

/// 启动前即可接受暂停和恢复；实例交出后仍指向同一次启动的生命周期。
#[derive(Clone)]
pub struct StartupLifecycle {
    requests: Arc<TransitionQueue>,
    _cleanup: Arc<StartupCleanupOwner>,
    attempt_id: String,
}

/// 只能交给一次完整启动；未使用的输入被丢弃后，等待中的通知明确失败。
pub struct StartupLifecycleInput {
    pub(super) requests: Arc<TransitionQueue>,
    pub(super) cleanup: Arc<StartupCleanupOwner>,
}

/// 一次启动及其清理的结果；错误不包含宿主路径或用户内容。
pub enum StartupReleaseOutcome {
    NotStarted,
    InputDropped(EngineError),
    Pending(Option<EngineError>),
    Delivered,
    Released(Option<EngineError>, Option<EngineError>),
    Failed(Option<EngineError>, EngineError, Option<EngineError>, bool),
}

impl From<StartupOutcomeSnapshot> for StartupReleaseOutcome {
    fn from(outcome: StartupOutcomeSnapshot) -> Self {
        match outcome {
            StartupOutcomeSnapshot::NotStarted => StartupReleaseOutcome::NotStarted,
            StartupOutcomeSnapshot::InputDropped { startup_error } => {
                StartupReleaseOutcome::InputDropped(startup_error)
            }
            StartupOutcomeSnapshot::Constructing { startup_error }
            | StartupOutcomeSnapshot::Cleaning { startup_error } => {
                StartupReleaseOutcome::Pending(startup_error)
            }
            StartupOutcomeSnapshot::Delivered => StartupReleaseOutcome::Delivered,
            StartupOutcomeSnapshot::Released {
                startup_error,
                operation_error,
            } => StartupReleaseOutcome::Released(startup_error, operation_error),
            StartupOutcomeSnapshot::Failed {
                startup_error,
                cleanup_error,
                operation_error,
                explicit_retry_supported,
            } => StartupReleaseOutcome::Failed(
                startup_error,
                cleanup_error,
                operation_error,
                explicit_retry_supported,
            ),
        }
    }
}

#[cfg(feature = "test-binding-worker-panic")]
#[doc(hidden)]
pub type StartupProbeOutcome = StartupReleaseOutcome;

impl StartupLifecycle {
    /// 同一启动负责人的不透明身份。
    pub fn attempt_id(&self) -> String {
        self.attempt_id.clone()
    }

    /// 等待此启动的交付或资源释放结果。期限不取消已接受的工作。
    pub fn wait_release(&self, deadline: Duration) -> StartupReleaseOutcome {
        self.wait_release_internal(deadline).into()
    }

    /// 仅在负责人持有可重试的关闭器时发起显式重试。
    pub fn retry_release(&self, deadline: Duration) -> StartupReleaseOutcome {
        self.retry_host_internal(deadline).into()
    }

    #[cfg(feature = "test-binding-worker-panic")]
    #[doc(hidden)]
    pub fn probe_startup_outcome(&self, deadline: Duration) -> StartupProbeOutcome {
        self.wait_release_internal(deadline).into()
    }

    #[cfg(feature = "test-binding-worker-panic")]
    #[doc(hidden)]
    pub fn probe_retry_startup_outcome(&self, deadline: Duration) -> StartupProbeOutcome {
        self.retry_host_internal(deadline).into()
    }
    pub fn channel() -> (StartupLifecycleInput, Self) {
        let requests = Arc::new(TransitionQueue::default());
        let cleanup = Arc::new(StartupCleanupOwner::default());
        (
            StartupLifecycleInput {
                requests: Arc::clone(&requests),
                cleanup: Arc::clone(&cleanup),
            },
            Self {
                requests,
                _cleanup: cleanup,
                attempt_id: uuid::Uuid::new_v4().to_string(),
            },
        )
    }

    /// 在启动负责人内等待结果。
    pub(crate) fn wait_release_internal(&self, deadline: Duration) -> StartupOutcomeSnapshot {
        self._cleanup.wait_internal(deadline)
    }

    /// 重试负责人保留的宿主变化流关闭。
    pub(crate) fn retry_host_internal(&self, deadline: Duration) -> StartupOutcomeSnapshot {
        self._cleanup.retry_host_internal(deadline)
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_rollback_task_panic(&self) -> bool {
        self._cleanup.probe_rollback_task_panic()
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_owned_start_failure(&self) -> Option<(&'static str, std::io::ErrorKind)> {
        self._cleanup.probe_owned_start_failure()
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_cleanup_worker_failure(
        &self,
    ) -> Option<(&'static str, std::io::ErrorKind)> {
        self._cleanup.probe_cleanup_worker_failure()
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_retry_executor_panic(&self) -> bool {
        self._cleanup.probe_retry_executor_panic()
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_retry_executor_drop_panic(&self) -> bool {
        self._cleanup.probe_retry_executor_drop_panic()
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_retry_callback_and_drop_panic(&self) -> bool {
        self._cleanup.probe_retry_callback_and_drop_panic()
    }

    #[cfg(feature = "dev-tools")]
    #[allow(dead_code, reason = "Used by source-including probes")]
    pub(crate) fn probe_host_cleanup_errors(
        &self,
    ) -> Vec<(HostCapabilityErrorCategory, Option<ErrorKind>)> {
        self._cleanup.probe_host_cleanup_errors()
    }

    pub async fn suspend(&self) -> Result<(), EngineError> {
        submit(&self.requests, Request::Suspend(None)).await
    }

    /// 期限包含启动等待；返回超时不代表暂停成功，也不撤销已接受的请求。
    pub async fn suspend_with_deadline(&self, deadline: Duration) -> Result<(), EngineError> {
        submit(&self.requests, Request::suspend_with_deadline(deadline)?).await
    }

    pub async fn resume(&self) -> Result<(), EngineError> {
        submit(&self.requests, Request::Resume).await
    }
}

impl Drop for StartupLifecycleInput {
    fn drop(&mut self) {
        let error = self
            .cleanup
            .primary_error()
            .unwrap_or_else(|| EngineError::new(1108, EngineErrorCategory::Internal, true));
        self.cleanup.input_dropped(error.clone());
        self.requests.fail_startup(error);
    }
}
