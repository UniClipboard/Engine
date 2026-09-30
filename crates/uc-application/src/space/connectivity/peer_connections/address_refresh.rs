use async_trait::async_trait;
use uc_core::ids::DeviceId;

/// 与当前成员的连接已建立并通过准入后，刷新该成员的可复用网络地址。
///
/// 调用方（`PeerConnectionCoordinator`）只决定时机：收到当前成员范围内设备的 `Online` 通知，
/// 即传输层已验证身份、该身份是当前成员且连接已通过准入。实现根据本次连接判定哪些路径可保存。
/// 刷新尽力而为，失败不改变连接结果，也不产生需要调用方处理的重试责任。
#[async_trait]
pub trait RefreshVerifiedPeerAddressPort: Send + Sync {
    async fn refresh_verified_peer_address(&self, peer: &DeviceId);
}
