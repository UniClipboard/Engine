use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uc_engine::observability::uc_warn;
use uc_engine::{EngineError, StartupLifecycle, StartupLifecycleInput};

use super::{io_error_kind, lock};
use crate::{BindingError, BindingErrorCategory};

/// 宿主在启动调用尚未返回时转交暂停和恢复通知。
#[derive(uniffi::Object)]
pub struct MobileStartupLifecycle {
    input: Mutex<Option<StartupLifecycleInput>>,
    control: StartupLifecycle,
}

impl MobileStartupLifecycle {
    pub(super) fn take_input(&self) -> Result<StartupLifecycleInput, BindingError> {
        lock(&self.input).take().ok_or(BindingError::Engine {
            code: 1001,
            category: BindingErrorCategory::InvalidState,
            retryable: false,
        })
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
        })
    }

    pub fn suspend(&self) -> Result<(), BindingError> {
        wait(self.control.suspend())
    }

    pub fn suspend_with_deadline(&self, deadline_ms: u64) -> Result<(), BindingError> {
        wait(
            self.control
                .suspend_with_deadline(Duration::from_millis(deadline_ms)),
        )
    }

    pub fn resume(&self) -> Result<(), BindingError> {
        wait(self.control.resume())
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
