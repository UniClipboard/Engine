use std::sync::Arc;

use async_trait::async_trait;
use tokio::time::Instant;
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;

use crate::connectivity::{ConnectivityError, PeerReceiveStream, PeerSendStream};

#[derive(Debug, thiserror::Error)]
pub enum PeerReachabilityWireError {
    #[error("presence network IO failed")]
    Io(#[source] std::io::Error),
    #[error("presence connection operation failed")]
    Connection(#[source] ConnectivityError),
    #[error("presence network deadline elapsed")]
    TimedOut(#[source] tokio::time::error::Elapsed),
    #[error("presence network deadline expired")]
    DeadlineExpired,
    #[error("presence frame is invalid")]
    InvalidFrame(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait]
pub trait PeerReachabilityWirePort: Send + Sync {
    async fn read_admission(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError>;
    async fn write_admission(
        &self,
        send: &mut dyn PeerSendStream,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError>;
    async fn read_confirmation(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<bool, PeerReachabilityWireError>;
    async fn write_confirmation(
        &self,
        send: &mut dyn PeerSendStream,
        admitted: bool,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError>;
    async fn read_challenge(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<[u8; 16], PeerReachabilityWireError>;
    async fn write_challenge(
        &self,
        send: &mut dyn PeerSendStream,
        nonce: &[u8; 16],
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError>;
    async fn read_reply(
        &self,
        receive: &mut dyn PeerReceiveStream,
        nonce: &[u8; 16],
        deadline: Instant,
    ) -> Result<bool, PeerReachabilityWireError>;
    async fn write_reply(
        &self,
        send: &mut dyn PeerSendStream,
        nonce: &[u8; 16],
        admitted: bool,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError>;
}

/// 复用认证身份目录、统一资源撤销及原网络恢复通知，不保存在线证据。
#[async_trait]
pub trait PeerReachabilityEnvironmentPort: Send + Sync {
    async fn identify(&self, authenticated_key: &[u8; 32]) -> Result<DeviceId, ConnectivityError>;
    async fn recover_for_demand(&self);
    fn forget(&self, peer: &DeviceId);
    fn disconnect_all(&self);
    fn communication_failed(&self, peer: DeviceId);
}

pub struct PeerReachabilityTransportPorts {
    pub wire: Arc<dyn PeerReachabilityWirePort>,
    pub environment: Arc<dyn PeerReachabilityEnvironmentPort>,
}

impl ErrorClass for PeerReachabilityWireError {
    fn class(&self) -> &'static str {
        match self {
            Self::Io(_) => "presence_wire_io",
            Self::Connection(source) => source.class(),
            Self::TimedOut(_) | Self::DeadlineExpired => "presence_wire_timeout",
            Self::InvalidFrame(_) => "presence_wire_invalid_frame",
        }
    }
}
