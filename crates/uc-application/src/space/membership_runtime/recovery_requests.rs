use std::sync::Arc;

/// 成员持久状态已提交后，请求后台尽快执行一次可恢复工作。
#[derive(Clone)]
pub(crate) struct MembershipRecoveryRequests {
    notify: Arc<tokio::sync::Notify>,
}

impl MembershipRecoveryRequests {
    pub(crate) fn new() -> Self {
        Self {
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    pub(crate) fn request(&self) {
        self.notify.notify_one();
    }

    pub(crate) async fn notified(&self) {
        self.notify.notified().await;
    }
}
