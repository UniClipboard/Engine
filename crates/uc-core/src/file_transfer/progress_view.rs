//! 供宿主读取的完整业务事实；不暴露登记、producer、binding或线上序号。
use super::{
    FileTransferCancellationReason, ProgressBatchSummary, ProgressFailureReason,
    ProgressFreshnessView, ProgressItem, ProgressPhase,
};
use crate::ids::{DeviceId, EntryId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferProgressItemStatus {
    Waiting,
    Transferring,
    Processing,
    Retrying,
    Completed,
    Failed,
    Cancelled,
    Unconfirmed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgressItemView {
    pub item: ProgressItem,
    pub status: TransferProgressItemStatus,
    /// 未确认时仍保留最后已知阶段，宿主不需要自行从协议推断。
    pub last_known_phase: Option<ProgressPhase>,
    pub round: Option<u32>,
    pub bytes: Option<u64>,
    pub total: Option<u64>,
    pub failure: Option<ProgressFailureReason>,
    pub cancellation: Option<FileTransferCancellationReason>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgressView {
    pub peer: DeviceId,
    pub source_entry: EntryId,
    pub attempt: Option<String>,
    pub revision: u64,
    pub freshness: ProgressFreshnessView,
    /// 是否能提供可信联系视图；已确认成果不依赖系统时钟仍然可用。
    pub freshness_available: bool,
    pub summary: ProgressBatchSummary,
    pub items: Vec<TransferProgressItemView>,
}
