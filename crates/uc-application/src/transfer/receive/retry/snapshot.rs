//! 按原始表示次序重建基底，不能用粘贴优先排序或静默跳过非首选表示。
use super::RetryEntryReceiveError;
use crate::clipboard::active::ClipboardSnapshotDeps;
use crate::clipboard::sync::payload_codec::decode_v3_bytes_to_snapshot_blob_refs_and_file_set;
use crate::transfer::receive::work::ReceiveSource;
use anyhow::Context;
use std::sync::Arc;
use uc_core::blob::ports::BlobReaderPort;
use uc_core::ids::EntryId;
use uc_core::ports::clipboard::{
    ClipboardPayloadResolverPort, GetClipboardEntryPort, GetEntrySnapshotHashPort,
    GetRepresentationPort, PayloadResolveError, ResolvedClipboardPayload,
};
use uc_core::{ObservedClipboardRepresentation, SystemClipboardSnapshot};

pub(crate) struct RetrySnapshotReader {
    pub(crate) entry: Arc<dyn GetClipboardEntryPort>,
    pub(crate) representations: Arc<dyn GetRepresentationPort>,
    pub(crate) payload: Arc<dyn ClipboardPayloadResolverPort>,
    pub(crate) blobs: Arc<dyn BlobReaderPort>,
    pub(crate) identity: Arc<dyn GetEntrySnapshotHashPort>,
}

impl RetrySnapshotReader {
    pub(crate) fn new(
        stored: ClipboardSnapshotDeps,
        identity: Arc<dyn GetEntrySnapshotHashPort>,
    ) -> Self {
        Self {
            entry: stored.entry_repo,
            representations: stored.representation_repo,
            payload: stored.payload_resolver,
            blobs: stored.blob_store,
            identity,
        }
    }

    pub(crate) async fn read(
        &self,
        entry: &EntryId,
        source: &ReceiveSource,
    ) -> Result<Option<SystemClipboardSnapshot>, RetryEntryReceiveError> {
        if let Some(wire) = &source.original_wire {
            let (mut snapshot, _, _) = decode_v3_bytes_to_snapshot_blob_refs_and_file_set(wire)
                .context("decode original receive source")
                .map_err(RetryEntryReceiveError::Source)?;
            if snapshot.representations.len() != source.representations.len() {
                return Ok(None);
            }
            // 解码生成的是新的本地ID，原接收证明必须继续引用最初那一组ID。
            for (rep, original) in snapshot
                .representations
                .iter_mut()
                .zip(&source.representations)
            {
                rep.id = original.id.clone();
            }
            return Ok(Some(snapshot));
        }
        let identity = self
            .identity
            .get_entry_snapshot_hash(entry)
            .await
            .map_err(|source| RetryEntryReceiveError::Source(source.into()))?;
        if identity.as_deref() != Some(source.expected_hash.as_str()) {
            return Ok(None);
        }
        let Some(saved) = self
            .entry
            .get_entry(entry)
            .await
            .map_err(|source| RetryEntryReceiveError::Source(source.into()))?
        else {
            return Ok(None);
        };
        let mut representations = Vec::with_capacity(source.representations.len());
        for (index, original) in source.representations.iter().enumerate() {
            let stored = self
                .representations
                .get_representation(&saved.event_id, &original.id)
                .await
                .map_err(|source| RetryEntryReceiveError::Source(source.into()))?;
            if stored.as_ref().is_some_and(|rep| {
                rep.format_id != original.format_id || rep.mime_type != original.mime
            }) {
                return Ok(None);
            }
            let bytes = match stored {
                Some(rep) => {
                    match self
                        .payload
                        .resolve(&rep)
                        .await
                        .map_err(|source| match source {
                            unavailable @ (PayloadResolveError::Lost { .. }
                            | PayloadResolveError::Orphaned { .. }) => {
                                RetryEntryReceiveError::Unavailable(unavailable.into())
                            }
                            source => RetryEntryReceiveError::Source(source.into()),
                        })? {
                        ResolvedClipboardPayload::Inline { bytes, .. } => bytes,
                        ResolvedClipboardPayload::BlobRef { blob_id, .. } => self
                            .blobs
                            .get(&blob_id)
                            .await
                            .context("read original receive representation")
                            .map_err(RetryEntryReceiveError::Source)?,
                    }
                }
                // 失败的blob表示未曾保存，仍保留其原始位置供选中项回填。
                None if source
                    .blob_refs
                    .iter()
                    .any(|blob| blob.representation_index == Some(index as u32)) =>
                {
                    Vec::new()
                }
                None => return Ok(None),
            };
            representations.push(ObservedClipboardRepresentation::new(
                original.id.clone(),
                original.format_id.clone(),
                original.mime.clone(),
                bytes,
            ));
        }
        Ok(Some(SystemClipboardSnapshot {
            ts_ms: saved.created_at_ms,
            representations,
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        }))
    }
}
