use super::super::{ProgressExchangeDeadline, ProgressReceiveError, ProgressSnapshotTarget};
use super::{ProgressPageReply, ProgressPageRequest, ProgressSnapshotQuery, ProgressSnapshotRead};
use crate::connectivity::{ConnectionLease, StreamLease};
use async_trait::async_trait;
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;

/// 入队时同步冻结原会话；返回的完整读取只在取得实际额度后poll，不提前启动IO。
pub type ProgressSnapshotRound =
    BoxFuture<'static, Result<ProgressSnapshotRead, ProgressReceiveError>>;

/// 尚未确认的描述只能在原计划完整核验后返回；入队和执行共用原截止与取消，不重取会话或预算。
pub trait ProgressSnapshotRoundPort: Send + Sync + 'static {
    fn read_snapshot(
        self: Arc<Self>,
        peer: DeviceId,
        target: ProgressSnapshotTarget,
        deadline: Arc<ProgressExchangeDeadline>,
        cancel: CancellationToken,
    ) -> ProgressSnapshotRound;
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
