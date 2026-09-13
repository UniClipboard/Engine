use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uc_engine::{EngineError, StartupLifecycle, StartupLifecycleInput};

use super::lock;
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

fn wait(future: impl Future<Output = Result<(), EngineError>>) -> Result<(), BindingError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| BindingError::RuntimeUnavailable)?
        .block_on(future)
        .map_err(BindingError::from)
}
