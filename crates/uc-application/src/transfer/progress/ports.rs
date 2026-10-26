use super::{
    ProgressBound, ProgressClaimRefusal, ProgressContactNotice, ProgressExchangeDeadline,
    ProgressReceiveError, ProgressSourceTask, ProgressTaskDescriptor, ProgressTerminalNotice,
    ProgressUpdateNotice,
};
use crate::connectivity::{ConnectionLease, PeerSendStream, StreamLease, UniStreamLease};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    OutboundProgressSummary, ProgressActive, ProgressIdentity, ProgressItem, ProgressTerminal,
};
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
    async fn adopt_contact(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressContactNotice,
    ) -> Result<(), ProgressReceiveError>;
    async fn adopt_update(
        &self,
        peer: &DeviceId,
        generation: u64,
        notice: ProgressUpdateNotice,
    ) -> Result<(), ProgressReceiveError>;
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

/// 缺少尝试关联的完整通知动作；只更新既有未确认意图，不采用字节或结果。
/// 原意图、已确认任务及退休围栏由同一投影负责人统一核对。
#[async_trait]
pub trait ProgressUnattributedReceiverPort: Send + Sync {
    async fn observe_unattributed(
        &self,
        peer: &DeviceId,
        source: ProgressIdentity,
    ) -> Result<(), ProgressReceiveError>;
}

/// 只查询已接受的当前接收尝试；原始计划在接受时已完整核验且同一描述下不可变。
/// 每次新挑战都重新读取，不捕获旧任务快照。
/// None只表示本运行期未保留可授权关联，不推断任务失败或文件缺失。
pub trait ProgressTaskAuthorityPort: Send + Sync {
    fn current_task(&self, peer: &DeviceId, source: ProgressIdentity)
        -> Option<ProgressSourceTask>;
    /// 原结算后按完整冻结描述原子领取；同一当前尝试/项至多成功一次。
    /// producer、修订、源、attempt及计划范围任一变化都拒绝，不把旧结果重新标记为当前任务。
    fn claim_terminal(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        item: ProgressItem,
    ) -> Result<ProgressSourceTask, ProgressClaimRefusal>;
}

/// 一次已准入连接上的完整登记交换；新挑战必须实时读取authority，不使用调用前捕获的attempt。
/// 借用流，原负责人持许可至后置权威检查及实际连接保留读取结束。
#[async_trait]
pub trait ProgressWirePort: Send + Sync {
    /// 粗粒度通知经已准入单向流完整发送；传输结束不代表业务结果采用。
    async fn summary(
        &self,
        stream: &mut UniStreamLease<Box<dyn PeerSendStream>>,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        summary: OutboundProgressSummary,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError>;
    async fn contact(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressContactNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError>;
    async fn update(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressUpdateNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError>;
    async fn register(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        authority: &dyn ProgressTaskAuthorityPort,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError>;
    /// 有界终态交换；成功必须回显完全相同的已采用终态。
    async fn terminal(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressTerminalNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError>;
}

/// 一轮完整终态通知；原绝对截止由完整监督负责人创建，不得在实现里重建五秒。
#[async_trait]
pub trait ProgressTerminalRoundPort: Send + Sync {
    async fn report_settled(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        result: ProgressTerminal,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError>;
}

/// 已获得统一交换额度的一轮普通活动或联系通知，不产生业务ACK或文件成功结论。
#[async_trait]
pub trait ProgressLiveRoundPort: Send + Sync {
    async fn report_contact(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        sequence: u64,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError>;
    async fn report_live(
        &self,
        peer: &DeviceId,
        expected: ProgressTaskDescriptor,
        update: ProgressActive,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError>;
}
