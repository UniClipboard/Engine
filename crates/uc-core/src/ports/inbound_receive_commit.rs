use async_trait::async_trait;

use super::{
    ReceiveArtifact, ReceiveArtifactPhase, ReceiveArtifactRecord, ReceiveArtifactResolution,
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

pub enum InboundReceiveSettlement {
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

/// 已交付独立文件只解除自己的清理责任，其他产物仍归原尝试。
pub fn receive_artifacts_after_delivery(
    record: &ReceiveArtifactRecord,
    delivered: &ReceiveArtifact,
    delivered_item_ids: &[String],
) -> Result<Vec<ReceiveArtifact>, InboundReceiveCommitError> {
    if !delivered_item_ids.contains(&delivered.item_id)
        || record.artifacts.iter().any(|item| {
            item.item_id != delivered.item_id && delivered_item_ids.contains(&item.item_id)
        })
        || record.phase != ReceiveArtifactPhase::Preparing
        || record.resolution != ReceiveArtifactResolution::Pending
        || record
            .artifacts
            .iter()
            .filter(|item| item.item_id == delivered.item_id)
            .count()
            != 1
        || !record.artifacts.contains(delivered)
    {
        return Err(InboundReceiveCommitError::ArtifactStateMismatch);
    }
    Ok(record
        .artifacts
        .iter()
        .filter(|item| item.item_id != delivered.item_id)
        .cloned()
        .collect())
}
