use crate::clipboard::history::resource_snapshot::ReadEntryResourceSnapshotPort;
use anyhow::Result;
use std::sync::Arc;
use uc_core::{
    ids::EntryId, ports::clipboard::ResolvedClipboardPayload, ports::ClipboardPayloadResolverPort,
    BlobId,
};

/// Get clipboard entry resource metadata (blob reference only).
pub(crate) struct GetEntryResourceUseCase {
    snapshot_reader: Arc<dyn ReadEntryResourceSnapshotPort>,
    payload_resolver: Arc<dyn ClipboardPayloadResolverPort>,
}

#[derive(Debug, Clone)]
pub(crate) struct EntryResourceResult {
    pub(crate) blob_id: Option<BlobId>,
    pub(crate) mime_type: Option<String>,
    pub(crate) size_bytes: i64,
    pub(crate) url: Option<String>,
    pub(crate) inline_data: Option<Vec<u8>>,
}

impl GetEntryResourceUseCase {
    pub(crate) fn new(
        snapshot_reader: Arc<dyn ReadEntryResourceSnapshotPort>,
        payload_resolver: Arc<dyn ClipboardPayloadResolverPort>,
    ) -> Self {
        Self {
            snapshot_reader,
            payload_resolver,
        }
    }

    pub(crate) async fn execute(&self, entry_id: &EntryId) -> Result<EntryResourceResult> {
        let snapshot = self
            .snapshot_reader
            .read_entry_resource_snapshot(entry_id)
            .await?
            .ok_or(anyhow::anyhow!("Entry not found"))?;
        let preview = snapshot
            .preview_representation
            .ok_or(anyhow::anyhow!("Selection not found"))?;
        // 图像文件的预览选择可能指向 URI 清单，仍优先使用同一快照中的图像表示。
        let target_rep = snapshot
            .representations
            .iter()
            .find(|rep| rep.mime_type.as_ref().is_some_and(|mime| mime.is_image()))
            .or_else(|| {
                snapshot
                    .representations
                    .iter()
                    .find(|rep| rep.id == preview)
            })
            .ok_or(anyhow::anyhow!("Preview representation not found"))?;

        let payload = self.payload_resolver.resolve(target_rep).await?;

        match payload {
            ResolvedClipboardPayload::Inline { mime, bytes } => Ok(EntryResourceResult {
                blob_id: None,
                mime_type: Some(mime),
                size_bytes: target_rep.size_bytes,
                url: None,
                inline_data: Some(bytes),
            }),
            ResolvedClipboardPayload::BlobRef { mime, blob_id } => {
                let blob_id_clone = blob_id.clone();
                Ok(EntryResourceResult {
                    blob_id: Some(blob_id),
                    mime_type: Some(mime),
                    size_bytes: target_rep.size_bytes,
                    url: Some(format!("/clipboard/blobs/{}", blob_id_clone)),
                    inline_data: None,
                })
            }
        }
    }
}
