//! 清空历史及未保存接收的完整负责人；停止、目标快照和逐项删除不交给宿主拼接。
use super::delete_entry::DeleteClipboardEntryUseCase;
use crate::transfer::receive::work::ReceiveWorkRegistry;
use anyhow::{Context, Error as SourceError, Result};
use async_trait::async_trait;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;
use uc_core::ids::EntryId;
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_id, uc_info, uc_warn,
};

#[async_trait]
pub trait HistoryDeletionTargetsPort: Send + Sync {
    /// 取得全部历史与持久接收状态对应的去重条目标识，不读取业务正文。
    async fn history_deletion_targets(&self) -> Result<Vec<EntryId>>;
}

#[derive(Debug)]
pub(crate) struct ClearHistoryResult {
    pub(crate) deleted_count: u64,
    pub(crate) failed_entries: Vec<(String, SourceError)>,
}

#[derive(Debug)]
struct ClearHistoryFailure(Vec<(String, SourceError)>);
impl Display for ClearHistoryFailure {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str("history clear could not delete its targets")
    }
}
impl Error for ClearHistoryFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.0.first().map(|(_, source)| source.as_ref())
    }
}

pub(crate) struct ClearClipboardHistoryUseCase {
    targets: Arc<dyn HistoryDeletionTargetsPort>,
    delete_uc: Arc<DeleteClipboardEntryUseCase>,
    receive_work: Arc<ReceiveWorkRegistry>,
}
impl ClearClipboardHistoryUseCase {
    pub(crate) fn new(
        targets: Arc<dyn HistoryDeletionTargetsPort>,
        delete_uc: Arc<DeleteClipboardEntryUseCase>,
        receive_work: Arc<ReceiveWorkRegistry>,
    ) -> Self {
        Self {
            targets,
            delete_uc,
            receive_work,
        }
    }

    #[tracing::instrument(name = "usecase.clear_clipboard_history.execute", skip_all)]
    pub(crate) async fn execute(&self) -> Result<ClearHistoryResult> {
        let mut scope = self.receive_work.drain_scope().await?;
        scope
            .stop()
            .await
            .context("stop receives before history clear")?;
        let entries = self
            .targets
            .history_deletion_targets()
            .await
            .context("read history clear targets")?;
        let total = entries.len() as u64;
        uc_info!(
            total_entries = total,
            "Starting bulk clipboard history deletion"
        );
        let mut deleted_count = 0;
        let mut failed_entries = Vec::new();
        for entry in entries {
            match self.delete_uc.execute(&entry).await {
                Ok(()) => deleted_count += 1,
                Err(source) => {
                    uc_warn!(
                        entry_id = log_id(&entry),
                        error_kind = "entry_delete",
                        io_error_kind = io_error_kind(source.as_ref()),
                        "Failed to delete entry during bulk clear"
                    );
                    failed_entries.push((entry.to_string(), source));
                }
            }
        }
        uc_info!(
            deleted = deleted_count,
            failed = failed_entries.len(),
            total = total,
            "Clipboard history cleared"
        );
        if failed_entries.is_empty() {
            scope.complete();
        }
        if deleted_count == 0 && !failed_entries.is_empty() {
            return Err(ClearHistoryFailure(failed_entries).into());
        }
        Ok(ClearHistoryResult {
            deleted_count,
            failed_entries,
        })
    }
}

#[cfg(test)]
mod tests;
