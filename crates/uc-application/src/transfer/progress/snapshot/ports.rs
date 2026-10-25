use super::super::{ProgressExchangeDeadline, ProgressReceiveError, ProgressSnapshotTarget};
use super::{ProgressPageReply, ProgressPageRequest, ProgressSnapshotQuery, ProgressSnapshotRead};
use crate::connectivity::{ConnectionLease, StreamLease};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

/// 同一监督额度内的完整读取；尚未确认的描述只能在原计划完整核验后返回，不重起预算。
#[async_trait]
pub trait ProgressSnapshotRoundPort: Send + Sync {
    async fn read_snapshot(
        &self,
        peer: &DeviceId,
        target: ProgressSnapshotTarget,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressSnapshotRead, ProgressReceiveError>;
}
/// 单页交换只负责有界IO、真实连接准入、精确挑战和完整请求／响应结束。
#[async_trait]
pub trait ProgressSnapshotWirePort: Send + Sync {
    async fn read_page(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        request: ProgressPageRequest,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressPageReply, ProgressReceiveError>;
}

/// 原接收负责人生成当前实际状态的一页；完整计划、内容修订与退休由同一负责人管理。
#[async_trait]
pub trait ProgressSnapshotProviderPort: Send + Sync {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError>;
}
