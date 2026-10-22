//! 中间快照只包含已保存／本次待保存的真实成果，其余文件保留缺失项。
use super::workflow::{finalize_partial, rewrite_file_display_metadata};
use super::{MaterializeOutcome, MissingFileRef, ReceiveSelection, ReceivedItem, ReceivedOutput};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use anyhow::{anyhow, Result};
use std::collections::BTreeMap;
use uc_core::ids::DeviceId;
use uc_core::SystemClipboardSnapshot;

pub(super) struct FileProgressSnapshot {
    pub(super) snapshot: SystemClipboardSnapshot,
    pub(super) completed: Vec<ReceivedItem>,
}
impl FileProgressSnapshot {
    pub(super) fn new(
        snapshot: &SystemClipboardSnapshot,
        incomplete_representations: &[usize],
        refs: &[V3BlobRef],
        indices: &[usize],
        completed: &[ReceivedItem],
        selection: Option<&ReceiveSelection>,
        from_device: &DeviceId,
    ) -> Result<Self> {
        let mut saved = BTreeMap::new();
        if let Some(selection) = selection {
            for (index, item) in &selection.landed {
                saved.insert(*index, item.clone());
            }
        }
        saved.extend(completed.iter().map(|item| (item.blob_index, item.clone())));
        let mut paths = Vec::new();
        let mut digests = Vec::new();
        let mut file_refs = Vec::new();
        let mut missing = Vec::new();
        for (blob, index) in refs.iter().zip(indices) {
            if let Some(item) = saved.get(index) {
                let ReceivedOutput::File(artifact) = &item.output else {
                    return Err(anyhow!("file progress contains a representation output"));
                };
                paths.push(artifact.final_path.clone());
                digests.push(item.plaintext_hash);
                file_refs.push(blob.clone());
            } else {
                missing.push(MissingFileRef {
                    filename: blob
                        .filename
                        .clone()
                        .unwrap_or_else(|| format!("blob-{}", blob.entry_id.as_ref())),
                    size_bytes: blob.size_bytes,
                });
            }
        }
        let mut snapshot = snapshot.clone();
        snapshot.file_content_digests = digests;
        rewrite_file_display_metadata(&mut snapshot, &paths, &file_refs)?;
        let completed = saved.into_values().collect::<Vec<_>>();
        let result = finalize_partial(
            snapshot,
            incomplete_representations,
            paths,
            missing,
            from_device,
            MaterializeOutcome::PartialFailed,
            false,
            Vec::new(),
            completed.clone(),
        );
        Ok(Self {
            snapshot: result.snapshot,
            completed,
        })
    }
}

#[derive(Debug, thiserror::Error)]
#[error("receive file progress commit failed")]
pub(crate) struct FileProgressCommitError(#[source] pub(crate) anyhow::Error);

/// 保存失败后的持久结算也失败；两个来源都归完整接收负责人。
#[derive(Debug, thiserror::Error)]
#[error("receive file failure settlement remains incomplete")]
pub(crate) struct FileFailureSettlementError {
    #[source]
    pub(crate) cause: anyhow::Error,
    pub(crate) settlement: anyhow::Error,
}
