use super::{ProgressBound, ProgressReceiveError, ProgressTaskDescriptor, ProgressTerminalNotice};
use async_trait::async_trait;
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
