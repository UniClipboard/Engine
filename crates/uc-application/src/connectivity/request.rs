//! 在协议自己的队列之前冻结准入；网络重试复用同一请求，不重新授予旧任务资格。
use super::checks::checked;
use super::{
    AdmissionAttempt, AdmissionGrant, ConnectionLease, ConnectivityError, ConnectivityManager,
    InboundAdmissionFence, PeerConnection,
};
use async_trait::async_trait;
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
    pub(super) address_hint: Option<Vec<u8>>,
}

/// 来源身份尚需异步解析时，先冻结整个请求的会话及撤销基线。
pub(crate) struct ConnectionRequestScope {
    pub(super) manager: ConnectivityManager,
    pub(super) session: CancellationToken,
    pub(super) fence: Option<Box<dyn InboundAdmissionFence>>,
}
struct ScopedAttempt {
    attempt: Arc<dyn AdmissionAttempt>,
    fence: Option<Box<dyn InboundAdmissionFence>>,
    peer: DeviceId,
}
#[async_trait]
impl AdmissionAttempt for ScopedAttempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        if let Some(fence) = &self.fence {
            fence.check_current(&self.peer)?;
        }
        self.attempt.check_current().await?;
        if let Some(fence) = &self.fence {
            fence.check_current(&self.peer)?;
        }
        Ok(())
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.check_current().await?;
        let grant = self.attempt.adopt(connection).await?;
        self.check_current().await?;
        Ok(grant)
    }
}
impl ConnectionRequestScope {
    pub fn bind(
        self,
        target: DeviceId,
        address_hint: Vec<u8>,
    ) -> Result<ConnectionRequest, ConnectivityError> {
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if let Some(fence) = &self.fence {
            fence.check_current(&target)?;
        }
        let attempt = Arc::new(ScopedAttempt {
            attempt: self.manager.request(target).attempt,
            fence: self.fence,
            peer: target,
        });
        Ok(ConnectionRequest {
            manager: self.manager,
            target,
            attempt,
            session: self.session,
            address_hint: Some(address_hint),
        })
    }
}
impl ConnectionRequest {
    pub async fn connect(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<ConnectionLease, ConnectivityError> {
        self.validate(deadline, cancel).await?;
        let lease = self.manager.acquire_request(self, deadline, cancel).await?;
        if self.session.is_cancelled() || cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if Instant::now() >= deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        Ok(lease)
    }
    pub(crate) async fn validate(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<(), ConnectivityError> {
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
        if Instant::now() >= deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        Ok(())
    }
}
