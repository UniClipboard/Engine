//! 复用只读已保存输出；重新核对内容与所有权，不把旧成功文件交给新尝试清理。
use super::{ReceiveSelection, ReceivedItem, ReceivedOutput};
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::facade::blob_transfer::BlobTransferError;
use anyhow::{Context, Result};
use std::path::Path;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::ports::ReceiveArtifactOwnership;
use uc_core::SystemClipboardSnapshot;
use uc_observability_contract::wire_enum;

wire_enum! {
    #[derive(Debug, Clone, Copy, thiserror::Error)]
    pub(crate) enum ReceiveReuseRefusal: &'static str {
        #[error("receive selection is invalid")]
        InvalidSelection => "receive_selection_invalid",
        #[error("directory retry must select the original whole unit")]
        DirectoryUnitRequired => "receive_directory_unit_required",
        #[error("received output ownership has changed")]
        OwnershipChanged => "receive_reuse_ownership_changed",
        #[error("received output content has changed")]
        ContentChanged => "receive_reuse_content_changed",
    }
}
impl ErrorClass for ReceiveReuseRefusal {
    fn class(&self) -> &'static str {
        // 错误分类接口借用self，生成的唯一词表按值读取。
        Self::wire(*self)
    }
}

pub(super) async fn validate_selection(
    selection: &ReceiveSelection,
    refs: &[V3BlobRef],
    directory: bool,
    snapshot: &SystemClipboardSnapshot,
    cache_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<()> {
    if directory {
        if !selection.landed.is_empty()
            || selection.selected.len() != refs.len()
            || (0..refs.len()).any(|index| !selection.selected.contains(&index))
        {
            return Err(ReceiveReuseRefusal::DirectoryUnitRequired.into());
        }
        return Ok(());
    }
    if selection.selected.is_empty()
        || selection
            .selected
            .iter()
            .any(|index| *index >= refs.len() || selection.landed.contains_key(index))
    {
        return Err(ReceiveReuseRefusal::InvalidSelection.into());
    }
    for (index, item) in &selection.landed {
        let blob = refs
            .get(*index)
            .ok_or(ReceiveReuseRefusal::InvalidSelection)?;
        if item.blob_index != *index {
            return Err(ReceiveReuseRefusal::InvalidSelection.into());
        }
        match (&item.output, blob.representation_index) {
            (ReceivedOutput::File(_), None) => validate_file(item, cache_dir, cancellation).await?,
            (ReceivedOutput::Representation(id), Some(index)) => {
                let rep = snapshot
                    .representations
                    .get(index as usize)
                    .ok_or(ReceiveReuseRefusal::OwnershipChanged)?;
                let bytes = rep
                    .inline_bytes()
                    .ok_or(ReceiveReuseRefusal::OwnershipChanged)?;
                if rep.id != *id {
                    return Err(ReceiveReuseRefusal::OwnershipChanged.into());
                }
                if bytes.len() as u64 != item.bytes
                    || blake3::hash(bytes).as_bytes() != &item.plaintext_hash
                {
                    return Err(ReceiveReuseRefusal::ContentChanged.into());
                }
            }
            _ => return Err(ReceiveReuseRefusal::InvalidSelection.into()),
        }
    }
    Ok(())
}
async fn validate_file(
    item: &ReceivedItem,
    cache_dir: &Path,
    cancellation: &CancellationToken,
) -> Result<()> {
    let ReceivedOutput::File(artifact) = &item.output else {
        return Err(ReceiveReuseRefusal::InvalidSelection.into());
    };
    if artifact.final_path != artifact.staged_path {
        return Err(ReceiveReuseRefusal::OwnershipChanged.into());
    }
    let metadata = tokio::fs::symlink_metadata(&artifact.final_path)
        .await
        .context("inspect previously received output")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(ReceiveReuseRefusal::OwnershipChanged.into());
    }
    if artifact.ownership == ReceiveArtifactOwnership::ManagedStaging {
        let root = tokio::fs::canonicalize(cache_dir.join("iroh-blobs"))
            .await
            .context("resolve managed receive root")?;
        let current = tokio::fs::canonicalize(&artifact.final_path)
            .await
            .context("resolve received output ownership")?;
        if !current.starts_with(root) {
            return Err(ReceiveReuseRefusal::OwnershipChanged.into());
        }
    }
    if metadata.len() != item.bytes {
        return Err(ReceiveReuseRefusal::ContentChanged.into());
    }
    let mut file = tokio::fs::File::open(&artifact.final_path)
        .await
        .context("open previously received output")?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if cancellation.is_cancelled() {
            return Err(BlobTransferError::Cancelled.into());
        }
        let read = file
            .read(&mut buffer)
            .await
            .context("verify previously received output")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if hasher.finalize().as_bytes() != &item.plaintext_hash {
        return Err(ReceiveReuseRefusal::ContentChanged.into());
    }
    Ok(())
}
