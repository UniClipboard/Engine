use super::super::{ProgressExchangeDeadline, ProgressReceiveError, ProgressSourceTask};
use super::{ProgressPageReply, ProgressPageRequest, ProgressSnapshotRead};
use crate::connectivity::{ConnectionLease, StreamLease};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

/// 已获得原监督额度的一轮完整读取；不得另外建立查询循环、队列或重起预算。
#[async_trait]
pub trait ProgressSnapshotRoundPort: Send + Sync {
    async fn read_snapshot(
        &self,
        peer: &DeviceId,
        expected: ProgressSourceTask,
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
