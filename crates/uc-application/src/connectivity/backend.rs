use super::{ConnectivityError, StreamPair};
use async_trait::async_trait;
use std::sync::Arc;
use uc_core::ids::DeviceId;
use uc_core::ports::ConnectionChannel;

/// 仅由本条连接实际协商的协议能力决定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionMode {
    SingleExchange,
    Multiplexed,
}

/// 本连接实际协商的有限协议能力，不包含传输库协议字节。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionProtocol {
    Unsupported,
    ActiveClipboardState,
    ActiveClipboardPull,
    Clipboard,
    MultiplexedClipboard,
    GroupUpdate,
    LegacyGroupUpdate,
    MembershipHistory,
    MultiplexedMembershipHistory,
}

#[async_trait]
pub trait ConnectivityBackend: Send + Sync {
    async fn connect(&self, target: DeviceId)
        -> Result<Arc<dyn PeerConnection>, ConnectivityError>;
}

/// 已完成密码握手的传输事实；不代表成员准入。
#[async_trait]
pub trait PeerConnection: Send + Sync {
    fn id(&self) -> usize;
    fn authenticated_key(&self) -> [u8; 32];
    fn mode(&self) -> ConnectionMode;
    fn protocol(&self) -> ConnectionProtocol;
    fn channel(&self) -> ConnectionChannel;
    fn is_closed(&self) -> bool;
    /// 明确的连接级不可达证据；本地授权/任务失败不能冒充离线。
    fn is_unreachable(&self) -> bool {
        false
    }
    fn close(&self);
    async fn closed(&self);
    async fn open_bi(&self) -> Result<StreamPair, ConnectivityError>;
    async fn accept_bi(&self) -> Result<StreamPair, ConnectivityError>;
}
