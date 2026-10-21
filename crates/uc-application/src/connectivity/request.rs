//! 在协议自己的队列之前冻结准入；网络重试复用同一请求，不重新授予旧任务资格。
use super::checks::checked;
use super::{AdmissionAttempt, ConnectionLease, ConnectivityError, ConnectivityManager};
use std::sync::Arc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

#[derive(Clone)]
pub struct ConnectionRequest {
    pub(super) manager: ConnectivityManager,
    pub(super) target: DeviceId,
    pub(super) attempt: Arc<dyn AdmissionAttempt>,
    pub(super) session: CancellationToken,
}
impl ConnectionRequest {
    pub async fn connect(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<ConnectionLease, ConnectivityError> {
        if self.session.is_cancelled() || cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        checked(
            self.attempt.check_current(),
            self.manager.policy().wait_for_local_work(),
            deadline,
            cancel,
        )
        .await?;
        if self.session.is_cancelled() || cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        let lease = self.manager.acquire_request(self, deadline, cancel).await?;
        if self.session.is_cancelled() || cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if Instant::now() >= deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        Ok(lease)
    }
}
