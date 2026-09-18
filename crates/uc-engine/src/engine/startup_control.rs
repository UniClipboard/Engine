use std::sync::Arc;
use std::time::Duration;

use super::lifecycle::{submit, Request, TransitionQueue};
use crate::{EngineError, EngineErrorCategory};

#[cfg(test)]
mod tests;

/// 启动前即可接受暂停和恢复；实例交出后仍指向同一次启动的生命周期。
#[derive(Clone)]
pub struct StartupLifecycle {
    requests: Arc<TransitionQueue>,
}

/// 只能交给一次完整启动；未使用的输入被丢弃后，等待中的通知明确失败。
pub struct StartupLifecycleInput {
    pub(super) requests: Arc<TransitionQueue>,
}

impl StartupLifecycle {
    pub fn channel() -> (StartupLifecycleInput, Self) {
        let requests = Arc::new(TransitionQueue::default());
        (
            StartupLifecycleInput {
                requests: Arc::clone(&requests),
            },
            Self { requests },
        )
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
        self.requests
            .fail_startup(EngineError::new(1108, EngineErrorCategory::Internal, true));
    }
}
