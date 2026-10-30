use std::future::Future;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use uc_engine::observability::uc_warn;
use uc_engine::{EngineError, StartupLifecycle, StartupLifecycleInput, StartupReleaseOutcome};

use super::startup_delivery::DeliveryOutcome;
use super::startup_delivery::StartupDeliveryOwner;
use super::{io_error_kind, lock};
use crate::{BindingError, BindingErrorCategory};

/// 启动交付和资源清理的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum BindingStartupReleaseState {
    NotStarted,
    Pending,
    Delivered,
    Released,
    Failed,
}

/// 同一次启动的结果；宿主资源仍由宿主单独确认。
#[derive(Clone, Debug, uniffi::Record)]
pub struct BindingStartupReleaseReport {
    pub attempt_id: String,
    pub state: BindingStartupReleaseState,
    pub startup_error: Option<BindingError>,
    pub cleanup_error: Option<BindingError>,
    pub operation_error: Option<BindingError>,
    pub explicit_retry_supported: bool,
}

/// 宿主在启动调用尚未返回时转交暂停和恢复通知。
#[derive(uniffi::Object)]
pub struct MobileStartupLifecycle {
    input: Mutex<Option<StartupLifecycleInput>>,
    control: StartupLifecycle,
    preparation_failure: Arc<Mutex<Option<BindingError>>>,
    delivery: Arc<StartupDeliveryOwner>,
    preparation: Mutex<Preparation>,
    preparation_changed: Condvar,
}

#[derive(Clone)]
enum Preparation {
    NotStarted,
    Preparing,
    Accepted,
    Failed(BindingError),
}

/// 线程未接收启动 future 时，先记录错误，再让 future 执行清理。
pub(super) struct StartupLaunch<F> {
    startup: Option<F>,
    failure: Arc<Mutex<Option<BindingError>>>,
}

impl<F> StartupLaunch<F> {
    pub(super) fn new(startup: F, failure: Arc<Mutex<Option<BindingError>>>) -> Self {
        Self {
            startup: Some(startup),
            failure,
        }
    }

    pub(super) fn take(&mut self) -> Option<F> {
        self.startup.take()
    }
}

impl<F> Drop for StartupLaunch<F> {
    fn drop(&mut self) {
        if self.startup.is_some() {
            lock(&self.failure).get_or_insert(BindingError::RuntimeUnavailable);
        }
    }
}

impl MobileStartupLifecycle {
    fn retry_complete_startup_internal(
        &self,
        deadline_ms: u64,
    ) -> (
        BindingStartupReleaseState,
        Option<BindingError>,
        Option<BindingError>,
        Option<BindingError>,
        bool,
    ) {
        let started = Instant::now();
        let budget = Duration::from_millis(deadline_ms);
        match self.control.wait_release(Duration::ZERO) {
            StartupReleaseOutcome::Failed(_, _, _, true) => {
                let _ = self.control.retry_release(Duration::ZERO);
            }
            StartupReleaseOutcome::Delivered => match self.delivery.wait_internal(Duration::ZERO) {
                DeliveryOutcome::Failed {
                    explicit_retry_supported: true,
                    ..
                } => {
                    let _ = self.delivery.retry_creation();
                }
                DeliveryOutcome::Pending => {
                    let _ = self.delivery.retry_settlement();
                }
                _ => {}
            },
            _ => {}
        }
        let remaining = budget.saturating_sub(started.elapsed());
        self.complete_startup_internal(u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX))
    }

    fn complete_startup_internal(
        &self,
        deadline_ms: u64,
    ) -> (
        BindingStartupReleaseState,
        Option<BindingError>,
        Option<BindingError>,
        Option<BindingError>,
        bool,
    ) {
        let budget = Duration::from_millis(deadline_ms);
        let started = Instant::now();
        let mut phase = lock(&self.preparation);
        while matches!(*phase, Preparation::Preparing) {
            let remaining = budget.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break;
            }
            phase = self
                .preparation_changed
                .wait_timeout(phase, remaining.min(Duration::from_secs(60)))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        let phase = phase.clone();
        match phase {
            Preparation::NotStarted => {
                return (
                    BindingStartupReleaseState::NotStarted,
                    None,
                    None,
                    None,
                    false,
                )
            }
            Preparation::Preparing => {
                return (BindingStartupReleaseState::Pending, None, None, None, false)
            }
            Preparation::Failed(error) => {
                return (
                    BindingStartupReleaseState::Released,
                    Some(error),
                    None,
                    None,
                    false,
                )
            }
            Preparation::Accepted => {}
        }
        let outcome = self
            .control
            .wait_release(budget.saturating_sub(started.elapsed()));
        let preparation_failure = lock(&self.preparation_failure).clone();
        match outcome {
            StartupReleaseOutcome::NotStarted => (
                BindingStartupReleaseState::Pending,
                preparation_failure,
                None,
                None,
                false,
            ),
            StartupReleaseOutcome::InputDropped(error) => (
                BindingStartupReleaseState::Failed,
                preparation_failure.or(Some(BindingError::from(error))),
                Some(BindingError::RuntimeUnavailable),
                None,
                false,
            ),
            StartupReleaseOutcome::Pending(primary) => (
                BindingStartupReleaseState::Pending,
                preparation_failure.or_else(|| primary.map(BindingError::from)),
                None,
                None,
                false,
            ),
            StartupReleaseOutcome::Released(primary, operation) => (
                BindingStartupReleaseState::Released,
                preparation_failure.or_else(|| primary.map(BindingError::from)),
                None,
                operation.map(BindingError::from),
                false,
            ),
            StartupReleaseOutcome::Failed(primary, cleanup, operation, retry) => (
                BindingStartupReleaseState::Failed,
                preparation_failure.or_else(|| primary.map(BindingError::from)),
                Some(BindingError::from(cleanup)),
                operation.map(BindingError::from),
                retry,
            ),
            StartupReleaseOutcome::Delivered => match self
                .delivery
                .wait_internal(budget.saturating_sub(started.elapsed()))
            {
                DeliveryOutcome::NotAccepted | DeliveryOutcome::Pending => {
                    (BindingStartupReleaseState::Pending, None, None, None, false)
                }
                DeliveryOutcome::Delivered => (
                    BindingStartupReleaseState::Delivered,
                    None,
                    None,
                    None,
                    false,
                ),
                DeliveryOutcome::Released {
                    error,
                    worker_error,
                } => (
                    BindingStartupReleaseState::Released,
                    worker_error,
                    None,
                    error,
                    false,
                ),
                DeliveryOutcome::Failed {
                    error,
                    worker_error,
                    operation_error,
                    explicit_retry_supported,
                } => (
                    BindingStartupReleaseState::Failed,
                    worker_error,
                    Some(error),
                    operation_error,
                    explicit_retry_supported,
                ),
            },
        }
    }
    #[cfg(feature = "test-binding-worker-panic")]
    #[doc(hidden)]
    pub fn probe_complete_startup(
        &self,
        deadline_ms: u64,
    ) -> (
        String,
        Option<BindingError>,
        Option<BindingError>,
        Option<BindingError>,
        bool,
    ) {
        Self::probe_parts(self.complete_startup_internal(deadline_ms))
    }

    #[cfg(feature = "test-binding-worker-panic")]
    #[doc(hidden)]
    pub fn probe_retry_complete_startup(
        &self,
        deadline_ms: u64,
    ) -> (
        String,
        Option<BindingError>,
        Option<BindingError>,
        Option<BindingError>,
        bool,
    ) {
        Self::probe_parts(self.retry_complete_startup_internal(deadline_ms))
    }

    #[cfg(feature = "test-binding-worker-panic")]
    fn probe_parts(
        result: (
            BindingStartupReleaseState,
            Option<BindingError>,
            Option<BindingError>,
            Option<BindingError>,
            bool,
        ),
    ) -> (
        String,
        Option<BindingError>,
        Option<BindingError>,
        Option<BindingError>,
        bool,
    ) {
        let state = match result.0 {
            BindingStartupReleaseState::NotStarted => "not_started",
            BindingStartupReleaseState::Pending => "pending",
            BindingStartupReleaseState::Delivered => "delivered",
            BindingStartupReleaseState::Released => "released",
            BindingStartupReleaseState::Failed => "failed",
        };
        (state.into(), result.1, result.2, result.3, result.4)
    }

    #[cfg(feature = "test-binding-worker-panic")]
    #[doc(hidden)]
    pub fn probe_delivery_retained(&self) -> bool {
        self.delivery.probe_retained()
    }
    #[cfg(feature = "test-binding-worker-panic")]
    #[doc(hidden)]
    pub fn probe_delivery_release(&self, deadline_ms: u64) -> (String, bool) {
        match self
            .delivery
            .wait_internal(Duration::from_millis(deadline_ms))
        {
            DeliveryOutcome::NotAccepted => ("not_accepted".to_owned(), false),
            DeliveryOutcome::Pending => ("pending".to_owned(), false),
            DeliveryOutcome::Delivered => ("delivered".to_owned(), false),
            DeliveryOutcome::Released {
                error,
                worker_error,
            } => (
                "released".to_owned(),
                error.is_some() || worker_error.is_some(),
            ),
            DeliveryOutcome::Failed {
                error,
                worker_error,
                ..
            } => {
                let _ = (error, worker_error);
                ("failed".to_owned(), true)
            }
        }
    }
    #[cfg(feature = "test-binding-worker-panic")]
    #[doc(hidden)]
    pub fn probe_recovery_failure(&self) -> Option<(String, String, bool)> {
        self.delivery.probe_worker_failure()
    }
    #[cfg(feature = "test-binding-worker-panic")]
    #[doc(hidden)]
    pub fn probe_retry_delivery(&self) -> bool {
        self.delivery.retry_creation()
    }
    pub(super) fn delivery_owner(&self) -> Arc<StartupDeliveryOwner> {
        Arc::clone(&self.delivery)
    }

    pub(super) fn preparation_failure_owner(&self) -> Arc<Mutex<Option<BindingError>>> {
        Arc::clone(&self.preparation_failure)
    }
    pub(super) fn take_input(&self) -> Result<StartupLifecycleInput, BindingError> {
        let mut preparation = lock(&self.preparation);
        let input = lock(&self.input).take().ok_or(BindingError::Engine {
            code: 1001,
            category: BindingErrorCategory::InvalidState,
            retryable: false,
        })?;
        *preparation = Preparation::Preparing;
        self.preparation_changed.notify_all();
        Ok(input)
    }

    pub(super) fn mark_engine_accepted(&self) {
        *lock(&self.preparation) = Preparation::Accepted;
        self.preparation_changed.notify_all();
    }

    pub(super) fn record_preparation_failure(&self, error: BindingError) {
        *lock(&self.preparation_failure) = Some(error.clone());
        let mut phase = lock(&self.preparation);
        if matches!(*phase, Preparation::Preparing) {
            *phase = Preparation::Failed(error);
        }
        self.preparation_changed.notify_all();
    }

    fn report(
        &self,
        result: (
            BindingStartupReleaseState,
            Option<BindingError>,
            Option<BindingError>,
            Option<BindingError>,
            bool,
        ),
    ) -> BindingStartupReleaseReport {
        BindingStartupReleaseReport {
            attempt_id: self.control.attempt_id(),
            state: result.0,
            startup_error: result.1,
            cleanup_error: result.2,
            operation_error: result.3,
            explicit_retry_supported: result.4,
        }
    }

    fn wait(
        &self,
        future: impl Future<Output = Result<(), EngineError>>,
    ) -> Result<(), BindingError> {
        let result = wait(future);
        if matches!(result, Err(BindingError::Engine { code: 1108, .. })) {
            if let Some(error) = lock(&self.preparation_failure).clone() {
                return Err(error);
            }
        }
        result
    }
}

#[uniffi::export]
impl MobileStartupLifecycle {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        let (input, control) = StartupLifecycle::channel();
        Arc::new(Self {
            input: Mutex::new(Some(input)),
            control,
            preparation_failure: Arc::new(Mutex::new(None)),
            delivery: Arc::new(StartupDeliveryOwner::default()),
            preparation: Mutex::new(Preparation::NotStarted),
            preparation_changed: Condvar::new(),
        })
    }

    /// 等待同一启动的交付或释放结果。
    pub fn wait_release(&self, deadline_ms: u64) -> BindingStartupReleaseReport {
        self.report(self.complete_startup_internal(deadline_ms))
    }

    /// 显式重试仍由启动负责人保留的关闭能力。
    pub fn retry_release(&self, deadline_ms: u64) -> BindingStartupReleaseReport {
        self.report(self.retry_complete_startup_internal(deadline_ms))
    }

    pub fn suspend(&self) -> Result<(), BindingError> {
        self.wait(self.control.suspend())
    }

    pub fn suspend_with_deadline(&self, deadline_ms: u64) -> Result<(), BindingError> {
        self.wait(
            self.control
                .suspend_with_deadline(Duration::from_millis(deadline_ms)),
        )
    }

    pub fn resume(&self) -> Result<(), BindingError> {
        self.wait(self.control.resume())
    }
}

/// 启动期生命周期转交所需的 tokio 运行时无法创建；宿主只收到稳定错误码。
fn runtime_build_failed(error: std::io::Error) -> BindingError {
    uc_warn!(
        operation = "startup_lifecycle_runtime_build",
        error_kind = "runtime_unavailable",
        io_error_kind = io_error_kind(&error),
        "mobile operation failed"
    );
    BindingError::RuntimeUnavailable
}

fn wait(future: impl Future<Output = Result<(), EngineError>>) -> Result<(), BindingError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(runtime_build_failed)?
        .block_on(future)
        .map_err(BindingError::from)
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;
    use crate::runtime::event_recorder::EventRecorder;

    #[test]
    fn runtime_build_failure_records_the_io_kind_without_the_error_text() {
        let recorder = EventRecorder::default();
        let dispatch = tracing::Dispatch::new(recorder.clone());
        let error = tracing::dispatcher::with_default(&dispatch, || {
            runtime_build_failed(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "/secret/path",
            ))
        });

        assert_eq!(error, BindingError::RuntimeUnavailable);
        let lines = recorder.lines().join("\n");
        assert!(lines.contains("error_kind=runtime_unavailable"), "{lines}");
        assert!(lines.contains("io_error_kind=PermissionDenied"), "{lines}");
        assert!(!lines.contains("secret"), "{lines}");
    }
}
