use super::{
    ProgressBound, ProgressReceiveError, ProgressSourceTask, ProgressTaskDescriptor,
    ProgressTerminalNotice,
};
use crate::connectivity::{ConnectionLease, StreamLease};
use async_trait::async_trait;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::ProgressIdentity;
use uc_core::ids::DeviceId;

/// 一次完整的当前任务确认；实现负责有界编解码、截止时间及请求结束检查。
#[async_trait]
pub trait CurrentProgressTaskPort: Send {
    /// 发出新挑战后，取得完整接收负责人的当前权威描述；不能返回旧任务闭包中捕获的描述。
    async fn challenge(
        &mut self,
        nonce: ProgressIdentity,
    ) -> Result<ProgressTaskDescriptor, ProgressReceiveError>;
}

/// 完整业务采用能力；连接世代来自本机当前准入上下文，不能从帧读取。
/// 调用方只有在该能力成功返回之后才允许写 Bound 或精确 TerminalAck。
#[async_trait]
pub trait ProgressReceiverPort: Send + Sync {
    async fn register(
        &self,
        peer: &DeviceId,
        generation: u64,
        source: ProgressIdentity,
        exchange: &mut dyn CurrentProgressTaskPort,
    ) -> Result<ProgressBound, ProgressReceiveError>;
    /// 成功只回显已采用的完整输入；不能归一化、替换结果或提前确认广播入队。
    async fn adopt_terminal(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressTerminalNotice,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError>;
}

/// 只查询已接受的当前接收尝试；每次新挑战都重新读取，不捕获旧任务快照。
/// None只表示本运行期未保留可授权关联，不推断任务失败或文件缺失。
pub trait ProgressTaskAuthorityPort: Send + Sync {
    fn current_task(&self, peer: &DeviceId, source: ProgressIdentity)
        -> Option<ProgressSourceTask>;
}

/// 一次已准入连接上的完整登记交换；新挑战必须实时读取authority，不使用调用前捕获的attempt。
/// 借用流，原负责人持许可至后置权威检查及实际连接保留读取结束。
#[async_trait]
pub trait ProgressRegistrationWirePort: Send + Sync {
    async fn register(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        authority: &dyn ProgressTaskAuthorityPort,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError>;
}
