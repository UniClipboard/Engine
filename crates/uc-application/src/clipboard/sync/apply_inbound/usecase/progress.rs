//! 原接收动作在文件许可内保存交付快照；根流程随后只结算批次。
use super::ApplyInboundClipboardUseCase;
use crate::clipboard::capture::InboundCaptureCommitContext;
use crate::facade::blob_transfer::BlobTransferError;
use crate::transfer::receive::materialization::{ReceiveFileProgress, ReceivedItem};
use crate::transfer::receive::work::ReceiveAction;
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::{InboundReceiveCommitError, ReceiveArtifact};
use uc_core::{SnapshotHash, SystemClipboardSnapshot};

pub(super) struct FileProgress {
    pub(super) owner: ApplyInboundClipboardUseCase,
    pub(super) action: Arc<ReceiveAction>,
    pub(super) entry: EntryId,
    pub(super) attempt: String,
    pub(super) item_ids: Vec<String>,
    pub(super) hash: String,
    pub(super) replacing: bool,
    pub(super) saved: AtomicBool,
    pub(super) from_device: DeviceId,
}
impl FileProgress {
    pub(super) fn has_saved(&self) -> bool {
        self.saved.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl ReceiveFileProgress for FileProgress {
    async fn persist_file(
        &self,
        snapshot: SystemClipboardSnapshot,
        completed: &[ReceivedItem],
        artifact: &ReceiveArtifact,
    ) -> Result<()> {
        if self.action.cancellation().is_cancelled() {
            return Err(BlobTransferError::Cancelled.into());
        }
        let delivered_item_ids = completed
            .iter()
            .map(|item| {
                self.item_ids
                    .get(item.blob_index)
                    .cloned()
                    .ok_or_else(|| anyhow!("saved receive item is missing its original identity"))
            })
            .collect::<Result<Vec<_>>>()?;
        let commit = InboundCaptureCommitContext::Progress {
            attempt_id: self.attempt.clone(),
            delivered_item_ids,
            artifact: artifact.clone(),
        };
        let hash = SnapshotHash::parse(&self.hash);
        let captured = if self.replacing || self.has_saved() {
            self.owner
                .capture
                .replace_inbound_with_identity(
                    self.entry.clone(),
                    self.from_device,
                    snapshot,
                    hash,
                    commit,
                )
                .await
        } else {
            self.owner
                .capture
                .capture_inbound_with_identity(
                    self.entry.clone(),
                    self.from_device,
                    snapshot,
                    hash,
                    commit,
                )
                .await
        };
        let captured = match captured {
            Ok(captured) => captured,
            Err(source) => {
                if source.chain().any(|error| {
                    matches!(
                        error.downcast_ref::<InboundReceiveCommitError>(),
                        Some(InboundReceiveCommitError::AttemptStateMismatch)
                    )
                }) {
                    // 持久权威已经封住原尝试，不能等外部停止通知才阻止后项启动。
                    self.action.cancellation().cancel();
                }
                return Err(source);
            }
        };
        if captured.as_ref() != Some(&self.entry) {
            return Err(anyhow!("file progress capture returned no matching entry"));
        }
        self.action.persisted(false, completed);
        self.saved.store(true, Ordering::SeqCst);
        Ok(())
    }
}
