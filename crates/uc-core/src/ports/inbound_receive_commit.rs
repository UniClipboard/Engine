use async_trait::async_trait;

use super::{
    AttemptState, ReceiveArtifact, ReceiveArtifactPhase, ReceiveArtifactRecord,
    ReceiveArtifactResolution, ReceiveItemFailureReason, TrackedFileTransferStatus,
};
use crate::clipboard::{
    ClipboardEntry, ClipboardEntryContentCategory, ClipboardEvent, ClipboardSelectionDecision,
    EntryFileSet, PersistedClipboardRepresentation,
};
use crate::ids::EntryId;

#[derive(Clone)]
pub enum InboundReceiveRecord {
    Create {
        entry: ClipboardEntry,
        event: ClipboardEvent,
        representations: Vec<PersistedClipboardRepresentation>,
        selection: ClipboardSelectionDecision,
    },
    Replace {
        entry_id: EntryId,
        new_event: ClipboardEvent,
        new_representations: Vec<PersistedClipboardRepresentation>,
        new_selection: ClipboardSelectionDecision,
        new_total_size: i64,
        new_content_category: ClipboardEntryContentCategory,
    },
}

impl InboundReceiveRecord {
    pub fn entry_id(&self) -> &EntryId {
        match self {
            Self::Create { entry, .. } => &entry.entry_id,
            Self::Replace { entry_id, .. } => entry_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletedReceiveArtifacts {
    None,
    Landed,
    DirectoryPublished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialReceiveArtifacts {
    None,
    Landed,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoEntryReceiveArtifacts {
    None,
    /// The attempt no longer owns any cleanup work. Managed staging has been
    /// removed, while user-destination final paths were deliberately preserved
    /// and relinquished according to their ownership contract.
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialReceiveTerminal {
    Cancelled,
    Failed,
}

/// 网络完成后，本地交付的失败事实；不改变网络会话终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveFileFailure {
    Failed(ReceiveItemFailureReason),
    Cancelled,
}

impl ReceiveFileFailure {
    /// 已交付事实优先；本地失败只覆盖尚未交付的网络完成结果。
    pub fn project(
        self,
        network: TrackedFileTransferStatus,
        delivered: bool,
    ) -> (TrackedFileTransferStatus, Option<ReceiveItemFailureReason>) {
        if network != TrackedFileTransferStatus::Completed || delivered {
            return (network, None);
        }
        match self {
            Self::Failed(reason) => (TrackedFileTransferStatus::Failed, Some(reason)),
            Self::Cancelled => (TrackedFileTransferStatus::Cancelled, None),
        }
    }

    pub fn accepts_attempt(self, state: AttemptState) -> bool {
        matches!(
            state,
            AttemptState::Receiving | AttemptState::Cancelling | AttemptState::Failing
        )
    }
}

pub enum InboundReceiveSettlement {
    /// 输出已按所有权完成清理，原子保存失败事实并解除本项清理责任。
    FileFailed {
        entry_id: EntryId,
        attempt_id: String,
        artifact: ReceiveArtifact,
        failure: ReceiveFileFailure,
        now_ms: i64,
    },
    /// 独立文件的持久交付；保持接收中，原子解除该项的待清理责任。
    Progress {
        /// 当前保存快照中的成功项；新文件只解除下述单项产物责任。
        delivered_item_ids: Vec<String>,
        record: InboundReceiveRecord,
        attempt_id: String,
        artifact: ReceiveArtifact,
        now_ms: i64,
    },
    Complete {
        record: InboundReceiveRecord,
        attempt_id: String,
        /// 随记录一起保存的逐项成果；只有提交成功才能成为交付证明。
        delivered_item_ids: Vec<String>,
        file_set: Option<EntryFileSet>,
        artifacts: CompletedReceiveArtifacts,
        now_ms: i64,
    },
    Partial {
        record: InboundReceiveRecord,
        attempt_id: String,
        /// 只包含实际进入本次保存记录的成功项，不包含仅下载完成的项。
        delivered_item_ids: Vec<String>,
        terminal: PartialReceiveTerminal,
        file_set: Option<EntryFileSet>,
        artifacts: PartialReceiveArtifacts,
        now_ms: i64,
    },
    NoEntry {
        entry_id: EntryId,
        attempt_id: String,
        terminal: PartialReceiveTerminal,
        artifacts: NoEntryReceiveArtifacts,
        now_ms: i64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum InboundReceiveCommitError {
    #[error("receive attempt is not in the required settlement state")]
    AttemptStateMismatch,
    #[error("required receive artifact record is missing")]
    ArtifactRecordMissing,
    #[error("receive artifact ownership changed before commit")]
    ArtifactStateMismatch,
    #[error("required directory publication record is missing")]
    DirectoryPublishRecordMissing,
    #[error("inbound receive commit store error")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait]
pub trait CommitInboundReceivePort: Send + Sync {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> Result<(), InboundReceiveCommitError>;
}

/// 只解除实际已交付或已清理的本项，其他产物仍归原尝试。
pub enum ReceiveArtifactSettlement<'a> {
    Delivered(&'a [String]),
    Cleaned,
}

pub fn receive_artifacts_after_settlement(
    record: &ReceiveArtifactRecord,
    settled: &ReceiveArtifact,
    settlement: ReceiveArtifactSettlement<'_>,
) -> Result<Vec<ReceiveArtifact>, InboundReceiveCommitError> {
    let valid_delivery = match settlement {
        ReceiveArtifactSettlement::Delivered(ids) => {
            ids.contains(&settled.item_id)
                && !record
                    .artifacts
                    .iter()
                    .any(|item| item.item_id != settled.item_id && ids.contains(&item.item_id))
        }
        ReceiveArtifactSettlement::Cleaned => true,
    };
    if !valid_delivery
        || record.phase != ReceiveArtifactPhase::Preparing
        || record.resolution != ReceiveArtifactResolution::Pending
        || record
            .artifacts
            .iter()
            .filter(|item| item.item_id == settled.item_id)
            .count()
            != 1
        || !record.artifacts.contains(settled)
    {
        return Err(InboundReceiveCommitError::ArtifactStateMismatch);
    }
    Ok(record
        .artifacts
        .iter()
        .filter(|item| item.item_id != settled.item_id)
        .cloned()
        .collect())
}

#[cfg(test)]
mod file_failure_tests {
    use super::*;

    #[test]
    fn local_failure_never_retracts_delivered_or_rewrites_network_failure() {
        for failure in [
            ReceiveFileFailure::Failed(ReceiveItemFailureReason::StorageUnavailable),
            ReceiveFileFailure::Cancelled,
        ] {
            assert_eq!(
                failure.project(TrackedFileTransferStatus::Completed, true),
                (TrackedFileTransferStatus::Completed, None)
            );
            assert_eq!(
                failure.project(TrackedFileTransferStatus::Failed, false),
                (TrackedFileTransferStatus::Failed, None)
            );
            assert!(failure.accepts_attempt(AttemptState::Cancelling));
            for state in [
                AttemptState::Committing,
                AttemptState::Completed,
                AttemptState::Failed,
                AttemptState::Cancelled,
            ] {
                assert!(!failure.accepts_attempt(state));
            }
        }
    }
}
