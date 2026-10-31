mod file_failure;
mod progress;

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use diesel::prelude::*;
use uc_application::deps::BlobReclaimPort;
use uc_core::file_transfer::receive_no_entry_confirmation;
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::space::DeriveSpaceSubkeyPort;
use uc_core::ports::AttemptState;
use uc_core::ports::{
    CommitInboundReceivePort, CompletedReceiveArtifacts, InboundReceiveCommitError,
    InboundReceiveRecord, InboundReceiveSettlement, NoEntryReceiveArtifacts,
    PartialReceiveArtifacts, PartialReceiveTerminal, ReceiveArtifactPhase,
    ReceiveArtifactResolution,
};
use uc_core::BlobId;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

use super::clipboard_entry_repo::insert_entry_and_selection_rows;
use super::clipboard_event_repo::insert_clipboard_event_rows;
use super::entry_file_set_repo::{replace_entry_file_set_rows, EntryFileSetProtection};
use super::entry_receive_attempt_repo::to_attempt;
use super::entry_replace_repo::{
    prepare_entry_replacement, replace_entry_content_rows, PreparedEntryReplacement,
};
use super::file_transfer_repo::ENTRY_DELIVERED_BINDING;
use super::receive_artifact_cipher::V3ReceiveArtifactCipher;
use super::receive_artifact_log_repo::ReceiveArtifactProtection;
use crate::db::mappers::clipboard_entry_mapper::ClipboardEntryRowMapper;
use crate::db::mappers::clipboard_event_mapper::ClipboardEventRowMapper;
use crate::db::mappers::clipboard_selection_mapper::ClipboardSelectionRowMapper;
use crate::db::mappers::snapshot_representation_mapper::RepresentationRowMapper;
use crate::db::models::snapshot_representation::NewSnapshotRepresentationRow;
use crate::db::models::{
    EntryReceiveAttemptRow, NewClipboardEntryRow, NewClipboardEventRow, NewClipboardSelectionRow,
};
use crate::db::ports::{DbExecutor, InsertMapper};
use crate::db::schema::{
    directory_publish_log, entry_receive_attempt, file_transfer, receive_artifact_log,
};
use progress::PreparedFileArtifacts;
use uc_core::ports::inbound_receive_commit::ReceiveArtifactSettlement;
use uc_infra_security::ContentProtection;

pub struct DieselInboundReceiveCommitRepository<E> {
    executor: E,
    file_set_protection: EntryFileSetProtection,
    artifact_protection: ReceiveArtifactProtection,
    /// 生产装配必须调用 [`Self::with_blob_reclaim`]。测试构造器保持原样，
    /// 未注入时提交仍然成功，只是不回收被替换的 blob。
    blob_reclaim: Option<Arc<dyn BlobReclaimPort>>,
}

impl<E> DieselInboundReceiveCommitRepository<E> {
    pub fn new(
        executor: E,
        derive_subkey: Arc<dyn DeriveSpaceSubkeyPort>,
        current_profile: Arc<dyn CurrentProfilePort>,
    ) -> Self {
        Self {
            executor,
            file_set_protection: EntryFileSetProtection::legacy(
                derive_subkey.clone(),
                current_profile.clone(),
            ),
            artifact_protection: ReceiveArtifactProtection::Legacy {
                derive_subkey,
                current_profile,
            },
            blob_reclaim: None,
        }
    }

    pub fn new_v3(executor: E, protection: Arc<ContentProtection>) -> Self {
        Self {
            executor,
            file_set_protection: EntryFileSetProtection::v3(protection.clone()),
            artifact_protection: ReceiveArtifactProtection::V3(V3ReceiveArtifactCipher::new(
                protection,
            )),
            blob_reclaim: None,
        }
    }

    /// 事务提交成功后再回收被替换的 blob。回收失败只记警告，不让提交失败。
    pub fn with_blob_reclaim(mut self, blob_reclaim: Arc<dyn BlobReclaimPort>) -> Self {
        self.blob_reclaim = Some(blob_reclaim);
        self
    }
}

enum PreparedRecord {
    Create {
        entry: NewClipboardEntryRow,
        event: NewClipboardEventRow,
        representations: Vec<NewSnapshotRepresentationRow>,
        selection: NewClipboardSelectionRow,
    },
    Replace(PreparedEntryReplacement),
}

#[derive(Clone, Copy)]
enum ArtifactAction {
    None,
    Landed,
    RolledBack,
    DirectoryPublished,
}

#[derive(Debug, thiserror::Error)]
enum CommitInvariantError {
    #[error("attempt state mismatch")]
    AttemptStateMismatch,
    #[error("artifact record missing")]
    ArtifactRecordMissing,
    #[error("artifact ownership changed")]
    ArtifactStateMismatch,
    #[error("directory publish record missing")]
    DirectoryPublishRecordMissing,
    #[error("receive file outcome does not match its settlement")]
    DeliveryItemMismatch,
}

fn map_error(error: anyhow::Error) -> InboundReceiveCommitError {
    let error = match error.downcast::<InboundReceiveCommitError>() {
        Ok(error) => return error,
        Err(error) => error,
    };
    match error.downcast_ref::<CommitInvariantError>() {
        Some(CommitInvariantError::AttemptStateMismatch) => {
            InboundReceiveCommitError::AttemptStateMismatch
        }
        Some(CommitInvariantError::ArtifactRecordMissing) => {
            InboundReceiveCommitError::ArtifactRecordMissing
        }
        Some(CommitInvariantError::ArtifactStateMismatch) => {
            InboundReceiveCommitError::ArtifactStateMismatch
        }
        Some(CommitInvariantError::DirectoryPublishRecordMissing) => {
            InboundReceiveCommitError::DirectoryPublishRecordMissing
        }
        Some(CommitInvariantError::DeliveryItemMismatch) | None => {
            InboundReceiveCommitError::Backend(error.context("commit inbound receive").into())
        }
    }
}

fn prepare_record(record: &InboundReceiveRecord) -> Result<PreparedRecord> {
    match record {
        InboundReceiveRecord::Create {
            entry,
            event,
            representations,
            selection,
        } => Ok(PreparedRecord::Create {
            entry: ClipboardEntryRowMapper.to_row(entry)?,
            event: ClipboardEventRowMapper.to_row(event)?,
            representations: representations
                .iter()
                .map(|representation| {
                    RepresentationRowMapper.to_row(&(representation, &event.event_id))
                })
                .collect::<Result<Vec<_>>>()?,
            selection: ClipboardSelectionRowMapper.to_row(selection)?,
        }),
        InboundReceiveRecord::Replace {
            entry_id,
            new_event,
            new_representations,
            new_selection,
            new_total_size,
            new_content_category,
        } => Ok(PreparedRecord::Replace(prepare_entry_replacement(
            entry_id,
            new_event,
            new_representations,
            new_selection,
            *new_total_size,
            *new_content_category,
        )?)),
    }
}

#[async_trait]
impl<E: DbExecutor> CommitInboundReceivePort for DieselInboundReceiveCommitRepository<E> {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> Result<(), InboundReceiveCommitError> {
        if let InboundReceiveSettlement::FileFailed {
            entry_id,
            attempt_id,
            artifact,
            failure,
            now_ms,
        } = settlement
        {
            return self
                .commit_failed_file_artifacts(
                    entry_id,
                    attempt_id,
                    artifact,
                    Some(*failure),
                    *now_ms,
                )
                .await;
        }
        if let InboundReceiveSettlement::FileCleaned {
            entry_id,
            attempt_id,
            artifact,
            now_ms,
        } = settlement
        {
            return self
                .commit_failed_file_artifacts(entry_id, attempt_id, artifact, None, *now_ms)
                .await;
        }
        let no_entry_replay = match settlement {
            InboundReceiveSettlement::NoEntry {
                terminal,
                artifacts,
                ..
            } => Some((*terminal, *artifacts)),
            _ => None,
        };
        let progress_artifacts = match settlement {
            InboundReceiveSettlement::Progress {
                delivered_item_ids,
                record,
                attempt_id,
                artifact,
                ..
            } => Some(
                PreparedFileArtifacts::prepare(
                    &self.executor,
                    &self.artifact_protection,
                    record.entry_id().as_ref(),
                    attempt_id,
                    artifact,
                    ReceiveArtifactSettlement::Delivered(delivered_item_ids),
                )
                .await?,
            ),
            _ => None,
        };
        let delivered_item_ids = match settlement {
            InboundReceiveSettlement::Progress {
                delivered_item_ids, ..
            }
            | InboundReceiveSettlement::Complete {
                delivered_item_ids, ..
            }
            | InboundReceiveSettlement::Partial {
                delivered_item_ids, ..
            } => delivered_item_ids.clone(),
            InboundReceiveSettlement::NoEntry { .. } => Vec::new(),
            InboundReceiveSettlement::FileFailed { .. }
            | InboundReceiveSettlement::FileCleaned { .. } => {
                return Err(InboundReceiveCommitError::AttemptStateMismatch)
            }
        };
        let (record, entry_id, attempt_id, file_set, expected, target, artifacts, now_ms) =
            match settlement {
                InboundReceiveSettlement::FileFailed { .. }
                | InboundReceiveSettlement::FileCleaned { .. } => {
                    return Err(InboundReceiveCommitError::AttemptStateMismatch)
                }
                InboundReceiveSettlement::Progress {
                    record,
                    attempt_id,
                    now_ms,
                    ..
                } => (
                    Some(record),
                    record.entry_id().to_string(),
                    attempt_id.clone(),
                    None,
                    AttemptState::Receiving,
                    AttemptState::Receiving,
                    ArtifactAction::None,
                    *now_ms,
                ),
                InboundReceiveSettlement::Complete {
                    record,
                    attempt_id,
                    delivered_item_ids: _,
                    file_set,
                    artifacts,
                    now_ms,
                } => (
                    Some(record),
                    record.entry_id().to_string(),
                    attempt_id.clone(),
                    file_set.as_ref(),
                    AttemptState::Committing,
                    AttemptState::Completed,
                    match artifacts {
                        CompletedReceiveArtifacts::None => ArtifactAction::None,
                        CompletedReceiveArtifacts::Landed => ArtifactAction::Landed,
                        CompletedReceiveArtifacts::DirectoryPublished => {
                            ArtifactAction::DirectoryPublished
                        }
                    },
                    *now_ms,
                ),
                InboundReceiveSettlement::Partial {
                    record,
                    attempt_id,
                    delivered_item_ids: _,
                    terminal,
                    file_set,
                    artifacts,
                    now_ms,
                } => (
                    Some(record),
                    record.entry_id().to_string(),
                    attempt_id.clone(),
                    file_set.as_ref(),
                    terminal_expected(*terminal),
                    terminal_target(*terminal),
                    match artifacts {
                        PartialReceiveArtifacts::None => ArtifactAction::None,
                        PartialReceiveArtifacts::Landed => ArtifactAction::Landed,
                        PartialReceiveArtifacts::RolledBack => ArtifactAction::RolledBack,
                    },
                    *now_ms,
                ),
                InboundReceiveSettlement::NoEntry {
                    entry_id,
                    attempt_id,
                    terminal,
                    artifacts,
                    now_ms,
                } => (
                    None,
                    entry_id.to_string(),
                    attempt_id.clone(),
                    None,
                    terminal_expected(*terminal),
                    terminal_target(*terminal),
                    match artifacts {
                        NoEntryReceiveArtifacts::None => ArtifactAction::None,
                        NoEntryReceiveArtifacts::RolledBack => ArtifactAction::RolledBack,
                    },
                    *now_ms,
                ),
            };

        let prepared_record = record.map(prepare_record).transpose().map_err(|error| {
            InboundReceiveCommitError::Backend(
                error.context("prepare inbound receive record").into(),
            )
        })?;
        let file_set_rows = if let Some(file_set) = file_set {
            Some(
                self.file_set_protection
                    .encode_rows(&uc_core::ids::EntryId::from(entry_id.as_str()), file_set)
                    .await
                    .map_err(|error| InboundReceiveCommitError::Backend(Box::new(error)))?,
            )
        } else {
            None
        };

        let replaced_blob_ids = self
            .executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut replaced_blob_ids = Vec::new();
                    if let Some((terminal, artifacts)) = no_entry_replay {
                        if confirm_no_entry_replay(
                            conn,
                            &entry_id,
                            &attempt_id,
                            terminal,
                            artifacts,
                        )? {
                            return Ok(replaced_blob_ids);
                        }
                    }
                    if let Some(prepared_record) = &prepared_record {
                        match prepared_record {
                            PreparedRecord::Create {
                                entry,
                                event,
                                representations,
                                selection,
                            } => {
                                insert_clipboard_event_rows(conn, event, representations)?;
                                insert_entry_and_selection_rows(conn, entry, selection)?;
                            }
                            PreparedRecord::Replace(prepared) => {
                                replaced_blob_ids
                                    .extend(replace_entry_content_rows(conn, prepared, true)?);
                            }
                        }
                    }
                    if let Some(file_set_rows) = &file_set_rows {
                        replaced_blob_ids.extend(replace_entry_file_set_rows(
                            conn,
                            &entry_id,
                            file_set_rows,
                        )?);
                    }

                    let attempt_updated = diesel::update(
                        entry_receive_attempt::table
                            .filter(entry_receive_attempt::entry_id.eq(&entry_id))
                            .filter(entry_receive_attempt::current_attempt_id.eq(&attempt_id))
                            .filter(entry_receive_attempt::attempt_state.eq(expected.to_string())),
                    )
                    .set((
                        entry_receive_attempt::attempt_state.eq(target.to_string()),
                        entry_receive_attempt::updated_at_ms.eq(now_ms),
                    ))
                    .execute(conn)?;
                    if attempt_updated != 1 {
                        return Err(CommitInvariantError::AttemptStateMismatch.into());
                    }

                    if let Some(progress) = &progress_artifacts {
                        progress.commit(conn, &entry_id, &attempt_id, now_ms)?;
                    }
                    settle_artifacts(conn, &entry_id, &attempt_id, artifacts, now_ms)?;
                    for item_id in &delivered_item_ids {
                        let updated = diesel::update(
                            file_transfer::table
                                .filter(file_transfer::entry_id.eq(&entry_id))
                                .filter(file_transfer::attempt_id.eq(&attempt_id))
                                .filter(file_transfer::receive_item_id.eq(item_id))
                                .filter(file_transfer::status.eq("completed"))
                                .filter(file_transfer::failure_code.is_null()),
                        )
                        .set((
                            file_transfer::binding_state.eq(ENTRY_DELIVERED_BINDING),
                            file_transfer::updated_at_ms.eq(now_ms),
                        ))
                        .execute(conn)?;
                        if updated != 1 {
                            return Err(CommitInvariantError::DeliveryItemMismatch.into());
                        }
                    }
                    Ok(replaced_blob_ids)
                })
            })
            .map_err(map_error)?;
        self.reclaim_replaced_blobs(replaced_blob_ids).await;
        Ok(())
    }
}

impl<E> DieselInboundReceiveCommitRepository<E> {
    async fn reclaim_replaced_blobs(&self, blob_ids: Vec<String>) {
        let Some(reclaim) = &self.blob_reclaim else {
            return;
        };
        if blob_ids.is_empty() {
            return;
        }
        let blob_ids: Vec<BlobId> = blob_ids.into_iter().map(BlobId::from).collect();
        match reclaim.reclaim_unreferenced(&blob_ids).await {
            Ok(report) if report.failures == 0 => {}
            Ok(report) => {
                uc_warn!(
                    error_kind = "blob_reclaim",
                    errors = report.failures,
                    deleted = report.rows_deleted,
                    bytes_reclaimed = report.bytes_freed,
                    "unreferenced blob reclaim left failures after inbound receive commit"
                );
            }
            Err(error) => {
                uc_warn!(
                    error_kind = "blob_reclaim",
                    io_error_kind = io_error_kind(&error),
                    "unreferenced blob reclaim failed after inbound receive commit"
                );
            }
        }
    }
}

/// 回执丢失后的确认没有持久副作用，不能以重放为由重新移交产物。
fn confirm_no_entry_replay(
    conn: &mut diesel::sqlite::SqliteConnection,
    entry_id: &str,
    attempt_id: &str,
    terminal: PartialReceiveTerminal,
    artifacts: NoEntryReceiveArtifacts,
) -> Result<bool> {
    let current = entry_receive_attempt::table
        .filter(entry_receive_attempt::entry_id.eq(entry_id))
        .select(EntryReceiveAttemptRow::as_select())
        .first(conn)
        .optional()?
        .map(to_attempt)
        .transpose()?;
    let record = if artifacts == NoEntryReceiveArtifacts::RolledBack {
        receive_artifact_log::table
            .filter(receive_artifact_log::entry_id.eq(entry_id))
            .filter(receive_artifact_log::attempt_id.eq(attempt_id))
            .select((
                receive_artifact_log::phase,
                receive_artifact_log::resolution,
            ))
            .first::<(String, String)>(conn)
            .optional()?
            .map(|(phase, resolution)| {
                Ok::<_, anyhow::Error>((
                    ReceiveArtifactPhase::parse(&phase)?,
                    ReceiveArtifactResolution::parse(&resolution)?,
                ))
            })
            .transpose()?
    } else {
        None
    };
    receive_no_entry_confirmation(
        entry_id,
        attempt_id,
        current.as_ref(),
        terminal,
        artifacts,
        record,
    )
    .map_err(anyhow::Error::new)
}

fn terminal_expected(terminal: PartialReceiveTerminal) -> AttemptState {
    match terminal {
        PartialReceiveTerminal::Cancelled => AttemptState::Cancelling,
        PartialReceiveTerminal::Failed => AttemptState::Failing,
    }
}

fn terminal_target(terminal: PartialReceiveTerminal) -> AttemptState {
    match terminal {
        PartialReceiveTerminal::Cancelled => AttemptState::Cancelled,
        PartialReceiveTerminal::Failed => AttemptState::Failed,
    }
}

fn settle_artifacts(
    conn: &mut diesel::sqlite::SqliteConnection,
    entry_id: &str,
    attempt_id: &str,
    action: ArtifactAction,
    now_ms: i64,
) -> Result<()> {
    match action {
        ArtifactAction::None => Ok(()),
        ArtifactAction::Landed => {
            settle_generic_artifacts(conn, entry_id, attempt_id, "landed", now_ms)
        }
        ArtifactAction::RolledBack => {
            settle_generic_artifacts(conn, entry_id, attempt_id, "rolled_back", now_ms)
        }
        ArtifactAction::DirectoryPublished => {
            let updated = diesel::update(
                directory_publish_log::table
                    .filter(directory_publish_log::entry_id.eq(entry_id))
                    .filter(directory_publish_log::attempt_id.eq(attempt_id)),
            )
            .set((
                directory_publish_log::phase.eq("landed"),
                directory_publish_log::landed.eq(true),
                directory_publish_log::updated_at_ms.eq(now_ms),
            ))
            .execute(conn)?;
            if updated != 1 {
                return Err(CommitInvariantError::DirectoryPublishRecordMissing.into());
            }
            Ok(())
        }
    }
}

fn settle_generic_artifacts(
    conn: &mut diesel::sqlite::SqliteConnection,
    entry_id: &str,
    attempt_id: &str,
    resolution: &str,
    now_ms: i64,
) -> Result<()> {
    let updated = diesel::update(
        receive_artifact_log::table
            .filter(receive_artifact_log::entry_id.eq(entry_id))
            .filter(receive_artifact_log::attempt_id.eq(attempt_id)),
    )
    .set((
        receive_artifact_log::phase.eq("settled"),
        receive_artifact_log::resolution.eq(resolution),
        receive_artifact_log::updated_at_ms.eq(now_ms),
    ))
    .execute(conn)?;
    if updated != 1 {
        return Err(CommitInvariantError::ArtifactRecordMissing.into());
    }
    Ok(())
}
