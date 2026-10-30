//! 下载负责人提供实际轮次；本轮展示与原有效数据高水位分别交接。
use async_trait::async_trait;

#[async_trait]
pub(in super::super) trait BlobProgressSink: Send + Sync {
    /// 无阻塞记录本轮已验证位置；重传可低于历史高水位，不延长原下载期限。
    fn observe_round(&self, round: u32, bytes: u64, total: Option<u64>);
    /// 实际进入网络恢复；不修改原有效数据高水位和期限。
    fn observe_retrying(&self, round: u32);
    /// 已取得实际受保护内容，后续解密与保存仍未完成；可来自准确本地缓存。
    fn observe_obtained(&self, round: u32, verified_size: u64);
    /// 原本地接收会话继续使用单调累计高水位。
    async fn report(&self, bytes: u64, total: Option<u64>);
}
