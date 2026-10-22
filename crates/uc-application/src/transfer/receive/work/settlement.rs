//! 未完成文件结算的原始责任；不放入错误对象，也不另建资源池。
use super::ReceiveAction;
use std::sync::PoisonError;
use uc_core::ids::EntryId;
use uc_core::ports::{ReceiveArtifact, ReceiveFileFailure};

pub(crate) struct UnsettledFile {
    pub(crate) entry: EntryId,
    pub(crate) attempt: String,
    pub(crate) artifact: ReceiveArtifact,
    pub(crate) failure: ReceiveFileFailure,
    pub(crate) remaining: Vec<ReceiveArtifact>,
}

impl ReceiveAction {
    pub(crate) fn defer_file_settlement(&self, pending: UnsettledFile) {
        self.file_settlement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(pending);
    }
    pub(crate) fn take_file_settlement(&self) -> Option<UnsettledFile> {
        self.file_settlement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}
