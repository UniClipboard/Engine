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
}

/// 当前文件结算是否仍有权威；判断不改变网络结果或清理事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveFileSettlementDecision {
    Allowed,
    AttemptMismatch,
    FileMismatch,
}

/// 文件跟踪事实缺失与损坏／冲突不同；准备失败可能尚未形成网络记录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiveFileSettlementRecord {
    Missing,
    Tracked {
        network: TrackedFileTransferStatus,
        delivered: bool,
    },
    Unusable,
}

/// 本地保存失败要求网络已完成；普通清理仅接受失败、取消或尚未登记的事实。
pub fn receive_file_settlement_decision(
    attempt: Option<AttemptState>,
    file: ReceiveFileSettlementRecord,
    local_failure: Option<ReceiveFileFailure>,
) -> ReceiveFileSettlementDecision {
    if !matches!(
        attempt,
        Some(AttemptState::Receiving | AttemptState::Cancelling | AttemptState::Failing)
    ) {
        return ReceiveFileSettlementDecision::AttemptMismatch;
    }
    let (network, delivered) = match file {
        ReceiveFileSettlementRecord::Missing => {
            return if local_failure.is_none() {
                ReceiveFileSettlementDecision::Allowed
            } else {
                ReceiveFileSettlementDecision::FileMismatch
            };
        }
        ReceiveFileSettlementRecord::Tracked { network, delivered } => (network, delivered),
        ReceiveFileSettlementRecord::Unusable => {
            return ReceiveFileSettlementDecision::FileMismatch
        }
    };
    let allowed = !delivered
        && match local_failure {
            Some(_) => network == TrackedFileTransferStatus::Completed,
            None => matches!(
                network,
                TrackedFileTransferStatus::Failed | TrackedFileTransferStatus::Cancelled
            ),
        };
    if allowed {
        ReceiveFileSettlementDecision::Allowed
    } else {
        ReceiveFileSettlementDecision::FileMismatch
    }
}

pub enum InboundReceiveSettlement {
    /// 本次输出已清理，原子解除责任；保留已有失败／取消，也允许尚未登记，不补造网络记录。
    FileCleaned {
        entry_id: EntryId,
        attempt_id: String,
        artifact: ReceiveArtifact,
        now_ms: i64,
    },
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
    fn file_cleanup_requires_open_attempt_and_no_delivered_or_active_transfer() {
        for network in [
            TrackedFileTransferStatus::Pending,
            TrackedFileTransferStatus::Transferring,
            TrackedFileTransferStatus::Completed,
            TrackedFileTransferStatus::Failed,
            TrackedFileTransferStatus::Cancelled,
        ] {
            let expected = if matches!(
                network,
                TrackedFileTransferStatus::Failed | TrackedFileTransferStatus::Cancelled
            ) {
                ReceiveFileSettlementDecision::Allowed
            } else {
                ReceiveFileSettlementDecision::FileMismatch
            };
            assert_eq!(
                receive_file_settlement_decision(
                    Some(AttemptState::Receiving),
                    ReceiveFileSettlementRecord::Tracked {
                        network: network,
                        delivered: false
                    },
                    None
                ),
                expected
            );
            assert_eq!(
                receive_file_settlement_decision(
                    Some(AttemptState::Receiving),
                    ReceiveFileSettlementRecord::Tracked {
                        network: network,
                        delivered: true
                    },
                    None
                ),
                ReceiveFileSettlementDecision::FileMismatch
            );
            assert_eq!(
                receive_file_settlement_decision(
                    None,
                    ReceiveFileSettlementRecord::Tracked {
                        network: network,
                        delivered: false
                    },
                    None
                ),
                ReceiveFileSettlementDecision::AttemptMismatch
            );
            assert_eq!(
                receive_file_settlement_decision(
                    Some(AttemptState::Completed),
                    ReceiveFileSettlementRecord::Tracked {
                        network: network,
                        delivered: false
                    },
                    None
                ),
                ReceiveFileSettlementDecision::AttemptMismatch
            );
        }
        assert_eq!(
            receive_file_settlement_decision(
                Some(AttemptState::Receiving),
                ReceiveFileSettlementRecord::Missing,
                None
            ),
            ReceiveFileSettlementDecision::Allowed
        );
    }

    #[test]
    fn missing_network_record_is_allowed_only_for_cleanup_and_unusable_records_are_never_allowed() {
        for local_failure in [None, Some(ReceiveFileFailure::Cancelled)] {
            assert_eq!(
                receive_file_settlement_decision(
                    Some(AttemptState::Receiving),
                    ReceiveFileSettlementRecord::Unusable,
                    local_failure
                ),
                ReceiveFileSettlementDecision::FileMismatch
            );
            assert_eq!(
                receive_file_settlement_decision(
                    Some(AttemptState::Completed),
                    ReceiveFileSettlementRecord::Missing,
                    local_failure
                ),
                ReceiveFileSettlementDecision::AttemptMismatch
            );
        }
        assert_eq!(
            receive_file_settlement_decision(
                Some(AttemptState::Receiving),
                ReceiveFileSettlementRecord::Missing,
                Some(ReceiveFileFailure::Cancelled)
            ),
            ReceiveFileSettlementDecision::FileMismatch
        );
    }

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
            assert_eq!(
                receive_file_settlement_decision(
                    Some(AttemptState::Cancelling),
                    ReceiveFileSettlementRecord::Tracked {
                        network: TrackedFileTransferStatus::Completed,
                        delivered: false
                    },
                    Some(failure)
                ),
                ReceiveFileSettlementDecision::Allowed
            );
            for state in [
                AttemptState::Committing,
                AttemptState::Completed,
                AttemptState::Failed,
                AttemptState::Cancelled,
            ] {
                assert_eq!(
                    receive_file_settlement_decision(
                        Some(state),
                        ReceiveFileSettlementRecord::Tracked {
                            network: TrackedFileTransferStatus::Completed,
                            delivered: false
                        },
                        Some(failure)
                    ),
                    ReceiveFileSettlementDecision::AttemptMismatch
                );
            }
        }
    }
}
