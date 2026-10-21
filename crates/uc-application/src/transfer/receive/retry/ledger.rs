//! 接受新尝试及其完整项集合；复用结果不转移旧文件的清理所有权。
use async_trait::async_trait;
use std::{collections::HashSet, fmt};
use uc_core::ports::{
    BeginReceiveOutcome, FileTransferProjectionError, ReceiveItemFailureReason, ReceiveItemRole,
    TrackedFileTransferStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveRetryItemState {
    Pending,
    Completed,
    Failed(ReceiveItemFailureReason),
    Cancelled { timed_out: bool },
}

impl ReceiveRetryItemState {
    pub fn status(self) -> TrackedFileTransferStatus {
        match self {
            Self::Pending => TrackedFileTransferStatus::Pending,
            Self::Completed => TrackedFileTransferStatus::Completed,
            Self::Failed(_) => TrackedFileTransferStatus::Failed,
            Self::Cancelled { .. } => TrackedFileTransferStatus::Cancelled,
        }
    }
}

#[derive(Clone)]
pub struct ReceiveRetryItem {
    pub item_id: String,
    pub role: ReceiveItemRole,
    pub filename: String,
    pub bytes: u64,
    pub state: ReceiveRetryItemState,
}

/// 只传递实际保存或旧终态事实；新尝试没有继承旧输出的可删除路径。
#[derive(Clone)]
pub struct BeginReceiveRetryInput {
    pub entry_id: String,
    pub expected_attempt_id: String,
    pub attempt_id: String,
    pub source_device_id: String,
    pub directory: bool,
    pub now_ms: i64,
    pub items: Vec<ReceiveRetryItem>,
}

impl fmt::Debug for BeginReceiveRetryInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BeginReceiveRetryInput")
            .field("items", &self.items.len())
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("receive retry ledger plan is invalid")]
pub struct InvalidReceiveRetryPlan;

impl BeginReceiveRetryInput {
    pub fn validate(&self) -> Result<(), InvalidReceiveRetryPlan> {
        let mut ids = HashSet::new();
        if self.entry_id.is_empty()
            || self.expected_attempt_id.is_empty()
            || self.attempt_id.is_empty()
            || self.source_device_id.is_empty()
            || self.attempt_id == self.expected_attempt_id
            || (!self.directory
                && !self
                    .items
                    .iter()
                    .any(|item| item.state == ReceiveRetryItemState::Pending))
            || self
                .items
                .iter()
                .try_fold(0_u64, |total, item| total.checked_add(item.bytes))
                .is_none_or(|total| total > i64::MAX as u64)
            || self.items.iter().any(|item| {
                item.item_id.is_empty()
                    || !ids.insert(&item.item_id)
                    || (self.directory && item.state != ReceiveRetryItemState::Pending)
            })
        {
            return Err(InvalidReceiveRetryPlan);
        }
        Ok(())
    }
}

#[async_trait]
pub trait BeginReceiveRetryPort: Send + Sync {
    /// 精确旧尝试CAS与全部新项在同一事务可见；任何项失败则全部回滚。
    /// 成功继承仅表示既有交付事实，不创建下载事件或新产物清理记录。
    async fn begin_receive_retry(
        &self,
        input: BeginReceiveRetryInput,
    ) -> Result<BeginReceiveOutcome, FileTransferProjectionError>;
}
