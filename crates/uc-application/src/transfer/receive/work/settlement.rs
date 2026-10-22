//! 未完成接收结算的原始责任；不放入错误对象，也不另建资源池。
use super::ReceiveAction;
use crate::transfer::receive::materialization::{DirectoryPublication, ReceiveOutputFailure};
use std::sync::PoisonError;
use uc_core::ids::EntryId;
use uc_core::ports::ReceiveArtifact;

pub(crate) struct UnsettledFile {
    pub(crate) entry: EntryId,
    pub(crate) attempt: String,
    pub(crate) artifact: ReceiveArtifact,
    pub(crate) failure: ReceiveOutputFailure,
    pub(crate) remaining: Vec<ReceiveArtifact>,
    pub(crate) output_cleanup_pending: bool,
    pub(crate) has_artifact_journal: bool,
}

pub(crate) struct UnsettledRepresentations {
    pub(crate) entry: EntryId,
    pub(crate) attempt: String,
    pub(crate) cancelled: bool,
}

pub(crate) struct UnsettledDirectory {
    pub(crate) entry: EntryId,
    pub(crate) attempt: Option<String>,
    pub(crate) cancelled: bool,
    pub(crate) publication: DirectoryPublication,
    /// 成员准备失败的原终态通知只在NoEntry真正交接后发出。
    pub(crate) notify_failure: bool,
}

pub(crate) enum UnsettledReceive {
    File(UnsettledFile),
    Representations(UnsettledRepresentations),
    Directory(UnsettledDirectory),
}

impl ReceiveAction {
    pub(crate) fn defer_file_settlement(&self, pending: UnsettledFile) {
        self.settlement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(UnsettledReceive::File(pending));
    }
    pub(crate) fn defer_representation_settlement(&self, pending: UnsettledRepresentations) {
        self.settlement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(UnsettledReceive::Representations(pending));
    }
    pub(crate) fn defer_directory_settlement(&self, pending: UnsettledDirectory) {
        self.settlement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(UnsettledReceive::Directory(pending));
    }
    pub(crate) fn take_settlement(&self) -> Option<UnsettledReceive> {
        self.settlement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}
