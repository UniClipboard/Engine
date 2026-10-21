use super::{backend, receive_failure_code, DieselFileTransferRepository, ENTRY_DELIVERED_BINDING};
use crate::db::models::{NewDirectoryPublishLogRow, NewFileTransferRow};
use crate::db::ports::DbExecutor;
use crate::db::repositories::entry_receive_attempt_repo::replace_terminal_attempt;
use crate::db::schema::{directory_publish_log, file_transfer};
use crate::file_transfer::persistence_cipher::TransferMetadata;
use async_trait::async_trait;
use diesel::prelude::*;
use uc_application::deps::{BeginReceiveRetryInput, BeginReceiveRetryPort, ReceiveRetryItemState};
use uc_core::ids::EntryId;
use uc_core::ports::{BeginReceiveOutcome, FileTransferProjectionError, PublishPhase};

#[async_trait]
impl<E: DbExecutor> BeginReceiveRetryPort for DieselFileTransferRepository<E> {
    async fn begin_receive_retry(
        &self,
        input: BeginReceiveRetryInput,
    ) -> Result<BeginReceiveOutcome, FileTransferProjectionError> {
        input.validate().map_err(|source| backend(source.into()))?;
        let protection = self.protection.resolve().await.map_err(backend)?;
        let mut rows = Vec::with_capacity(input.items.len());
        for item in &input.items {
            let metadata_ciphertext = protection
                .seal_metadata(
                    &item.item_id,
                    &TransferMetadata {
                        filename: item.filename.clone(),
                        // 继承的是已保存事实，不是旧文件的清理所有权。
                        cached_path: None,
                        failure_detail: None,
                    },
                )
                .await
                .map_err(|source| backend(source.into()))?;
            let file_size = i64::try_from(item.bytes).map_err(|source| backend(source.into()))?;
            let failure_code = match item.state {
                ReceiveRetryItemState::Failed(reason) => {
                    Some(receive_failure_code(reason).to_owned())
                }
                ReceiveRetryItemState::Cancelled { timed_out: true } => Some("timeout".to_owned()),
                _ => None,
            };
            rows.push(NewFileTransferRow {
                transfer_id: item.item_id.clone(),
                entry_id: Some(input.entry_id.clone()),
                file_size: Some(file_size),
                attempt_id: Some(input.attempt_id.clone()),
                binding_state: if item.state == ReceiveRetryItemState::Completed {
                    ENTRY_DELIVERED_BINDING
                } else {
                    "attempt_bound"
                }
                .into(),
                receive_item_id: Some(item.item_id.clone()),
                item_role: Some(item.role.as_str().into()),
                content_hash: None,
                status: item.state.status().as_str().into(),
                source_device: input.source_device_id.clone(),
                failure_code,
                metadata_ciphertext,
                created_at_ms: input.now_ms,
                updated_at_ms: input.now_ms,
            });
        }
        let directory = if input.directory {
            let ciphertext = self
                .directory_protection
                .seal(
                    &EntryId::from(input.entry_id.clone()),
                    &input.attempt_id,
                    &[],
                )
                .await
                .map_err(|source| backend(source.into()))?;
            Some(NewDirectoryPublishLogRow {
                entry_id: input.entry_id.clone(),
                attempt_id: input.attempt_id.clone(),
                phase: PublishPhase::Staging.to_string(),
                root_map_ciphertext: Some(ciphertext),
                partial_publication: false,
                partial_root_count: 0,
                landed: false,
                updated_at_ms: input.now_ms,
            })
        } else {
            None
        };
        self.executor
            .run(move |conn| {
                conn.transaction::<_, anyhow::Error, _>(|conn| {
                    let outcome = replace_terminal_attempt(
                        conn,
                        &input.entry_id,
                        &input.expected_attempt_id,
                        &input.attempt_id,
                        input.now_ms,
                    )?;
                    if outcome != BeginReceiveOutcome::Begun {
                        return Ok(outcome);
                    }
                    for row in &rows {
                        diesel::insert_into(file_transfer::table)
                            .values(row)
                            .execute(conn)?;
                    }
                    if let Some(directory) = &directory {
                        diesel::insert_into(directory_publish_log::table)
                            .values(directory)
                            .execute(conn)?;
                    }
                    Ok(outcome)
                })
            })
            .map_err(backend)
    }
}

#[cfg(test)]
mod tests;
