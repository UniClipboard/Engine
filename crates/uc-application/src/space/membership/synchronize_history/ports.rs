use async_trait::async_trait;
use uc_core::ids::DeviceId;

/// 在成员历史完成验证后，刷新已认证成员的可复用网络地址。
#[async_trait]
pub trait RefreshVerifiedPeerAddressPort: Send + Sync {
    async fn refresh_verified_peer_address(&self, peer: &DeviceId);
}
