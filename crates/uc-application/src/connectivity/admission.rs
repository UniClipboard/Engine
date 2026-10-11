use super::{ConnectivityError, PeerConnection};
use async_trait::async_trait;
use std::sync::Arc;
use uc_core::ids::DeviceId;

#[async_trait]
pub trait AdmissionGrant: Send + Sync {
    fn peer(&self) -> DeviceId;
    async fn check_current(&self) -> Result<(), ConnectivityError>;
}

#[async_trait]
pub trait AdmissionAttempt: Send + Sync {
    /// 校验本请求在排队之前冻结的撤销基线；复用连接不替换该基线。
    async fn check_current(&self) -> Result<(), ConnectivityError>;
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError>;
}

/// 复用会话唯一身份、准入和撤销登记；不向协议调用方暴露步骤。
#[async_trait]
pub trait SessionConnectionAdmissionPort: Send + Sync {
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt>;
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError>;
}

/// 入站拒绝仍持有有限处理额度，直到有界拒绝回执结算；过载直接关闭。
pub struct InboundConnectionError {
    pub(super) cause: ConnectivityError,
    pub(super) _capacity: Option<tokio::sync::OwnedSemaphorePermit>,
}
impl InboundConnectionError {
    pub fn is_busy(&self) -> bool {
        matches!(self.cause, ConnectivityError::Busy(_))
    }
}
impl std::fmt::Display for InboundConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.cause, f)
    }
}
impl std::fmt::Debug for InboundConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}
impl std::error::Error for InboundConnectionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}
