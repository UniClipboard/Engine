//! 接收业务查询的完整负责人；门面不枚举 attempt 或拼接逐条读取。
use super::cancellation::CancelEntryReceiveError;
use anyhow::Error as SourceError;
use std::sync::Arc;
use uc_core::ids::EntryId;
use uc_core::ports::{
    EntryReceiveProgress, GetEntryReceiveProgressPort, ListNonTerminalAttemptsPort,
};

pub(crate) struct ReceiveProgressQuery {
    progress: Arc<dyn GetEntryReceiveProgressPort>,
    attempts: Arc<dyn ListNonTerminalAttemptsPort>,
}
impl ReceiveProgressQuery {
    pub(crate) fn new(
        progress: Arc<dyn GetEntryReceiveProgressPort>,
        attempts: Arc<dyn ListNonTerminalAttemptsPort>,
    ) -> Self {
        Self { progress, attempts }
    }
    pub(crate) async fn get(
        &self,
        entry: &EntryId,
    ) -> Result<Option<EntryReceiveProgress>, CancelEntryReceiveError> {
        self.progress
            .get_entry_receive_progress(entry.as_ref())
            .await
            .map_err(|source| {
                CancelEntryReceiveError::Attempt(
                    SourceError::from(source).context("read entry receive progress"),
                )
            })
    }
    pub(crate) async fn list(&self) -> Result<Vec<EntryReceiveProgress>, CancelEntryReceiveError> {
        let attempts = self
            .attempts
            .list_non_terminal_attempts()
            .await
            .map_err(|source| {
                CancelEntryReceiveError::Attempt(
                    SourceError::from(source).context("list receive attempts"),
                )
            })?;
        let mut progress = Vec::with_capacity(attempts.len());
        for attempt in attempts {
            if let Some(current) = self.get(&EntryId::from(attempt.entry_id)).await? {
                progress.push(current);
            }
        }
        Ok(progress)
    }
}
