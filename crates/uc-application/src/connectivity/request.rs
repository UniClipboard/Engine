//! 在协议自己的队列之前冻结准入；网络重试复用同一请求，不重新授予旧任务资格。
use super::checks::checked;
use super::{
    AdmissionAttempt, AdmissionGrant, ConnectionKey, ConnectionLease, ConnectivityError,
    ConnectivityManager, InboundAdmissionFence, PeerConnection,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

#[derive(Clone)]
pub struct ConnectionRequest<K: ConnectionKey = DeviceId> {
    pub(super) manager: ConnectivityManager<K>,
    pub(super) target: K,
    pub(super) attempt: Arc<dyn AdmissionAttempt<K>>,
    pub(super) session: CancellationToken,
    pub(super) address_hint: Option<Vec<u8>>,
}

/// 来源身份尚需异步解析时，先冻结整个请求的会话及撤销基线。
pub(crate) struct ConnectionRequestScope<K: ConnectionKey = DeviceId> {
    pub(super) manager: ConnectivityManager<K>,
    pub(super) session: CancellationToken,
    pub(super) fence: Option<Box<dyn InboundAdmissionFence<K>>>,
}
struct ScopedAttempt<K: ConnectionKey> {
    attempt: Arc<dyn AdmissionAttempt<K>>,
    fence: Option<Box<dyn InboundAdmissionFence<K>>>,
    peer: K,
}
#[async_trait]
impl<K: ConnectionKey> AdmissionAttempt<K> for ScopedAttempt<K> {
    fn check_frozen(&self) -> Result<(), ConnectivityError> {
        self.fence
            .as_ref()
            .ok_or(ConnectivityError::Denied)?
            .check_current(&self.peer)
    }
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
    ) -> Result<Box<dyn AdmissionGrant<K>>, ConnectivityError> {
        self.check_current().await?;
        let grant = self.attempt.adopt(connection).await?;
        self.check_current().await?;
        Ok(grant)
    }
}
impl<K: ConnectionKey> ConnectionRequestScope<K> {
    pub fn bind(
        self,
        target: K,
        address_hint: Vec<u8>,
    ) -> Result<ConnectionRequest<K>, ConnectivityError> {
        self.bind_inner(target, Some(address_hint))
    }

    pub(crate) fn bind_peer(self, target: K) -> Result<ConnectionRequest<K>, ConnectivityError> {
        self.bind_inner(target, None)
    }

    fn bind_inner(
        self,
        target: K,
        address_hint: Option<Vec<u8>>,
    ) -> Result<ConnectionRequest<K>, ConnectivityError> {
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
            address_hint,
        })
    }
}
impl<K: ConnectionKey> ConnectionRequest<K> {
    /// 地址只供本请求拨号；backend 必须校验它与目标身份一致。
    pub fn with_address_hint(mut self, address_hint: Vec<u8>) -> Self {
        self.address_hint = Some(address_hint);
        self
    }
    pub(crate) fn check_frozen(&self) -> Result<(), ConnectivityError> {
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        self.attempt.check_frozen()?;
        if self.session.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        Ok(())
    }
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.session.child_token()
    }

    pub async fn connect(
        &self,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<ConnectionLease<K>, ConnectivityError> {
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
