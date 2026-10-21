use anyhow::{Context, Result};
use std::sync::Arc;
use tracing::{info_span, Instrument};
use uc_core::ports::clipboard::ListClipboardEntriesPort;
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_id, uc_info, uc_warn,
};

use super::delete_entry::DeleteClipboardEntryUseCase;

#[derive(Debug, Clone)]
pub(crate) struct ClearHistoryResult {
    pub(crate) deleted_count: u64,
    pub(crate) failed_entries: Vec<(String, String)>,
}

/// Use case for clearing all clipboard history entries via paginated listing
/// and per-entry deletion.
pub(crate) struct ClearClipboardHistoryUseCase {
    list_entries: Arc<dyn ListClipboardEntriesPort>,
    delete_uc: Arc<DeleteClipboardEntryUseCase>,
}

const BATCH_SIZE: usize = 1000;

impl ClearClipboardHistoryUseCase {
    pub(crate) fn new(
        list_entries: Arc<dyn ListClipboardEntriesPort>,
        delete_uc: Arc<DeleteClipboardEntryUseCase>,
    ) -> Self {
        Self {
            list_entries,
            delete_uc,
        }
    }

    #[tracing::instrument(name = "usecase.clear_clipboard_history.execute", skip_all)]
    pub(crate) async fn execute(&self) -> Result<ClearHistoryResult> {
        let entries = self.collect_all_entries().await?;

        let total = entries.len() as u64;
        uc_info!(
            total_entries = total,
            "Starting bulk clipboard history deletion"
        );

        if total == 0 {
            return Ok(ClearHistoryResult {
                deleted_count: 0,
                failed_entries: Vec::new(),
            });
        }

        let mut deleted_count = 0u64;
        let mut failed_entries: Vec<(String, String)> = Vec::new();

        for entry in &entries {
            let entry_id_str = entry.entry_id.inner().to_string();
            match self.delete_uc.execute(&entry.entry_id).await {
                Ok(()) => deleted_count += 1,
                Err(e) => {
                    uc_warn!(
                        entry_id = log_id(&entry.entry_id),
                        error_kind = "entry_delete",
                        io_error_kind = io_error_kind(e.as_ref()),
                        "Failed to delete entry during bulk clear"
                    );
                    failed_entries.push((entry_id_str, e.to_string()));
                }
            }
        }

        uc_info!(
            deleted = deleted_count,
            failed = failed_entries.len(),
            total = total,
            "Clipboard history cleared"
        );

        if deleted_count == 0 && !failed_entries.is_empty() {
            return Err(anyhow::anyhow!(
                "All {} entries failed to delete",
                failed_entries.len()
            ));
        }

        Ok(ClearHistoryResult {
            deleted_count,
            failed_entries,
        })
    }

    async fn collect_all_entries(&self) -> Result<Vec<uc_core::clipboard::ClipboardEntry>> {
        let mut entries = Vec::new();
        let mut offset = 0usize;

        loop {
            let batch = self
                .list_entries
                .list_entries(BATCH_SIZE, offset)
                .instrument(info_span!(
                    "list_entries_batch",
                    batch_size = BATCH_SIZE,
                    offset = offset
                ))
                .await
                .context("Failed to list entries for bulk delete")?;

            if batch.is_empty() {
                break;
            }

            let batch_len = batch.len();
            entries.extend(batch);
            offset += batch_len;

            if batch_len < BATCH_SIZE {
                break;
            }
        }

        Ok(entries)
    }
}
