use super::{ConnectivityError, PeerConnection};
use async_trait::async_trait;
use std::sync::Arc;
use uc_core::ids::DeviceId;

#[async_trait]
pub trait AdmissionGrant: Send + Sync {
    fn peer(&self) -> DeviceId;
    /// 核对本连接原始撤销基线；缺少同步围栏时禁止跨步骤提交。
    fn check_frozen(&self) -> Result<(), ConnectivityError> {
        Err(ConnectivityError::Denied)
    }
    async fn check_current(&self) -> Result<(), ConnectivityError>;
    /// 由准入负责人读取当前权威；默认不允许跨轮保留。
    async fn can_retain(&self) -> Result<bool, ConnectivityError> {
        Ok(false)
    }
}

#[async_trait]
pub trait AdmissionAttempt: Send + Sync {
    /// 同步核对冻结的撤销基线；未提供围栏的实现不能用于跨步骤的提交保护。
    fn check_frozen(&self) -> Result<(), ConnectivityError> {
        Err(ConnectivityError::Denied)
    }
    /// 校验本请求在排队之前冻结的撤销基线；复用连接不替换该基线。
    async fn check_current(&self) -> Result<(), ConnectivityError>;
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError>;
}

/// 首帧尚未识别 peer 时冻结的撤销围栏；绑定后核对同一请求基线。
pub trait InboundAdmissionFence: Send + Sync {
    fn check_current(&self, peer: &DeviceId) -> Result<(), ConnectivityError>;
}

/// 身份解析后、成员读取之前交接准入额度；失败时不得继续读取成员或登记连接。
pub trait InboundIdentityReservation: Send + Sync {
    fn identified(&self, peer: DeviceId) -> Result<(), ConnectivityError>;
}
impl InboundIdentityReservation for Option<Box<dyn InboundAdmissionFence>> {
    fn identified(&self, peer: DeviceId) -> Result<(), ConnectivityError> {
        if let Some(fence) = self {
            fence.check_current(&peer)?;
        }
        Ok(())
    }
}

/// 复用会话唯一身份、准入和撤销登记；不向协议调用方暴露步骤。
#[async_trait]
pub trait SessionConnectionAdmissionPort: Send + Sync {
    fn inbound_fence(&self) -> Option<Box<dyn InboundAdmissionFence>> {
        None
    }
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt>;
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
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
