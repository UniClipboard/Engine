mod cache_files;

use self::cache_files::CacheFiles;
use crate::transfer::progress::ProgressProjectionRegistry;
use crate::transfer::receive::work::ReceiveWorkRegistry;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{info_span, Instrument};
use uc_core::clipboard::ClipboardRepositoryError;
use uc_core::ids::{BlobId, EntryId};
use uc_core::ports::blob::{BlobTransferPort, TagReason};
use uc_core::ports::clipboard::{
    DeleteClipboardEntryWithReceiveStatePort, GetClipboardEntryPort,
    ListRepresentationsForEventPort,
};

use super::blob_reclaim::BlobReclaimPort;
use uc_core::ports::SearchIndexPort;
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_id, uc_info, uc_warn,
};

/// Use case for deleting clipboard entries with all associated data.
pub(crate) struct DeleteClipboardEntryUseCase {
    receive_work: Arc<ReceiveWorkRegistry>,
    progress: ProgressProjectionRegistry,
    get_entry: Arc<dyn GetClipboardEntryPort>,
    delete: Arc<dyn DeleteClipboardEntryWithReceiveStatePort>,
    representation_repo: Arc<dyn ListRepresentationsForEventPort>,
    file_cache_dir: Option<PathBuf>,
    search_index: Option<Arc<dyn SearchIndexPort>>,
    blob_transfer: Option<Arc<dyn BlobTransferPort>>,
    blob_reclaim: Arc<dyn BlobReclaimPort>,
}

impl DeleteClipboardEntryUseCase {
    pub(crate) fn from_ports(
        get_entry: Arc<dyn GetClipboardEntryPort>,
        delete: Arc<dyn DeleteClipboardEntryWithReceiveStatePort>,
        representation_repo: Arc<dyn ListRepresentationsForEventPort>,
        receive_work: Arc<ReceiveWorkRegistry>,
        progress: ProgressProjectionRegistry,
        blob_reclaim: Arc<dyn BlobReclaimPort>,
    ) -> Self {
        Self {
            receive_work,
            progress,
            get_entry,
            delete,
            representation_repo,
            file_cache_dir: None,
            search_index: None,
            blob_transfer: None,
            blob_reclaim,
        }
    }

    pub(crate) fn with_file_cache_dir(mut self, dir: PathBuf) -> Self {
        self.file_cache_dir = Some(dir);
        self
    }

    pub(crate) fn with_search_index(mut self, search_index: Arc<dyn SearchIndexPort>) -> Self {
        self.search_index = Some(search_index);
        self
    }

    pub(crate) fn with_blob_transfer(mut self, blob_transfer: Arc<dyn BlobTransferPort>) -> Self {
        self.blob_transfer = Some(blob_transfer);
        self
    }

    /// Deletes a clipboard entry and its associated selection, event, and snapshot
    /// representations in the required order. For file entries (text/uri-list),
    /// also deletes the cache files from disk when they live inside the managed
    /// file-cache directory.
    #[tracing::instrument(
        name = "usecase.delete_clipboard_entry.execute",
        skip(self),
        fields(entry_id = %entry_id)
    )]
    pub(crate) async fn execute(&self, entry_id: &EntryId) -> Result<()> {
        let _progress_scope = self.progress.deleting_entry(entry_id);
        let _receive_scope = self.receive_work.delete_scope(entry_id).await?;
        let entry = self
            .get_entry
            .get_entry(entry_id)
            .instrument(info_span!("fetch_entry", entry_id = %entry_id))
            .await
            .context("read entry before deletion")?;
        let representations = match entry {
            Some(entry) => self
                .representation_repo
                .get_representations_for_event(&entry.event_id)
                .await
                .context("read representations before deletion")?,
            None => Vec::new(),
        };
        let cache_files = match &self.file_cache_dir {
            Some(root) => CacheFiles::verify(&representations, root)
                .await
                .context("verify managed cache ownership before entry deletion")?,
            None => None,
        };
        // 仓储在同一原子边界解析实际条目与NoEntry状态，缺失不能伪报删除成功。
        let removal = self
            .delete
            .delete_entry_with_receive_state(entry_id)
            .await
            .context("delete entry and settled receive state")?;
        if !removal.removed {
            return Err(ClipboardRepositoryError::NotFound("clipboard entry".into()).into());
        }
        // 回收失败不改变删除结果。文件删不掉时行已经提交，留给后续扫描。
        reclaim_after_delete(self.blob_reclaim.as_ref(), &removal.candidate_blob_ids).await;

        if let Some(blob_transfer) = self.blob_transfer.as_ref() {
            async {
                if let Err(e) = blob_transfer
                    .untag(TagReason::ClipboardEntry(entry_id.clone()))
                    .await
                {
                    uc_warn!(
                        entry_id = log_id(&entry_id),
                        error_kind = "blob_untag",
                        io_error_kind = io_error_kind(&e),
                        "blob untag failed during entry delete; iroh-blobs GC will reclaim metadata on its next sweep"
                    );
                }
            }
            .instrument(info_span!("release_blob_tag", entry_id = %entry_id))
            .await;
        }

        async {
            let Some(cache_files) = cache_files else {
                return;
            };
            for path in cache_files.paths {
                if let Err(source) = tokio::fs::remove_file(&path).await {
                    uc_warn!(
                        error_kind = "cache_file_remove",
                        io_error_kind = io_error_kind(&source),
                        "Failed to delete cache file during entry cleanup"
                    );
                } else {
                    uc_info!("Deleted cache file during entry cleanup");
                    if let Some(parent) = path.parent() {
                        if parent != cache_files.root && parent.starts_with(&cache_files.root) {
                            let _ = tokio::fs::remove_dir(parent).await;
                        }
                    }
                }
            }
        }
        .instrument(info_span!("cleanup_cache_files", entry_id = %entry_id))
        .await;

        if let Some(search_index) = self.search_index.as_ref() {
            async {
                if let Err(e) = search_index.remove_entry(entry_id).await {
                    uc_warn!(
                        error_kind = "search_index_remove",
                        io_error_kind = io_error_kind(&e),
                        entry_id = log_id(&entry_id),
                        "search index cleanup failed, continuing delete"
                    );
                }
            }
            .instrument(info_span!("cleanup_search_index", entry_id = %entry_id))
            .await;
        }

        uc_info!(
            entry_id = log_id(&entry_id),
            "Deleted clipboard entry successfully"
        );
        Ok(())
    }
}

async fn reclaim_after_delete(reclaim: &dyn BlobReclaimPort, blob_ids: &[BlobId]) {
    if blob_ids.is_empty() {
        return;
    }
    match reclaim.reclaim_unreferenced(blob_ids).await {
        Ok(report) if report.failures == 0 => {}
        Ok(report) => {
            uc_warn!(
                error_kind = "blob_reclaim",
                errors = report.failures,
                deleted = report.rows_deleted,
                bytes_reclaimed = report.bytes_freed,
                "unreferenced blob reclaim left failures after entry deletion"
            );
        }
        Err(error) => {
            uc_warn!(
                error_kind = "blob_reclaim",
                io_error_kind = io_error_kind(&error),
                "unreferenced blob reclaim failed after entry deletion"
            );
        }
    }
}

#[cfg(test)]
mod tests;
