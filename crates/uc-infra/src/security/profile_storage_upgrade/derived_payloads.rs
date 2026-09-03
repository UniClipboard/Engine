//! 专用持久字段与搜索派生数据的一次性 V3 转换边界。
//!
//! 本模块只编排 owner codec，不拥有字段序列化、路径编码或实体 AAD。转换从
//! 不可变 `v3-primary` 复制出完整候选目录，全部回读验证后才原子发布。

use std::path::Path;
use std::sync::Arc;

use diesel::connection::SimpleConnection as _;
use diesel::{Connection as _, RunQueryDsl as _};
use uc_core::ids::{EntryId, ProfileId};

use crate::db::repositories::active_clipboard_register_cipher::{
    ActiveClipboardRegisterCipher, V3ActiveClipboardRegisterCipher,
};
use crate::db::repositories::directory_publish_log_cipher::{
    DirectoryPublishLogCipher, V3DirectoryPublishLogCipher,
};
use crate::db::repositories::entry_file_set_cipher::{
    EntryFileSetPathCipher, V3EntryFileSetPathCipher,
};
use crate::db::repositories::receive_artifact_cipher::{
    ReceiveArtifactCipher, V3ReceiveArtifactCipher,
};
use crate::file_transfer::persistence_cipher::{
    TransferPersistenceCipher, V3TransferPersistenceCipher,
};
use crate::search::{RenderPayloadCodec, SearchGroupRef, V3SearchProtection};
use crate::security::{ContentProtection, ProfileContentKeyVault};
use crate::space::InMemorySession;

use super::journal::UpgradeJournalV1;
use super::primary_payloads::{blob_tree_digest, compact_database, sync_directory};
use super::target::{file_digest, TargetGenerationStager};
use super::ProfileStorageUpgradeError;

const DATABASE_FILE: &str = "profile.sqlite";
const BLOB_DIRECTORY: &str = "blobs";
const V3_SEARCH_INDEX_VERSION: &str = "search-v12";

pub(super) struct DerivedPayloadConverter {
    profile_id: ProfileId,
    source_session: Arc<InMemorySession>,
    content_protection: Arc<ContentProtection>,
    search_protection: V3SearchProtection,
}

pub(super) struct ConvertedDerivedPayloads {
    pub(super) profile_database_digest: [u8; 32],
    pub(super) blob_tree_digest: [u8; 32],
    pub(super) derived_count: u64,
    pub(super) search_document_count: u64,
}

impl DerivedPayloadConverter {
    pub(super) fn new(
        profile_id: ProfileId,
        source_session: Arc<InMemorySession>,
        vault: Arc<ProfileContentKeyVault>,
    ) -> Self {
        Self {
            profile_id,
            content_protection: Arc::new(ContentProtection::for_content(
                Arc::clone(&source_session),
                Arc::clone(&vault),
            )),
            search_protection: V3SearchProtection::new(Arc::clone(&source_session), vault),
            source_session,
        }
    }

    pub(super) async fn convert(
        &self,
        journal: &UpgradeJournalV1,
        target: &TargetGenerationStager,
    ) -> Result<ConvertedDerivedPayloads, ProfileStorageUpgradeError> {
        target.verify_separated(journal)?;
        let paths = target.paths(journal);
        if paths.payload_output.is_dir() {
            let converted = self.inspect_output(&paths.payload_output).await?;
            target.verify_source_revision(journal)?;
            return Ok(converted);
        }
        if paths.payload_output.exists() {
            return Err(corrupt(anyhow::anyhow!(
                "profile upgrade payload output has an invalid type"
            )));
        }
        let parent = paths
            .payload_output
            .parent()
            .ok_or_else(|| storage(anyhow::anyhow!("payload output parent is missing")))?;
        let work = parent.join(format!(".v3-payloads-{}.tmp", uuid::Uuid::new_v4()));
        let result = self
            .build_output(&paths.primary_output, &paths.payload_output, &work)
            .await;
        let converted = match result {
            Ok(converted) => converted,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&work);
                return Err(error);
            }
        };
        std::fs::rename(&work, &paths.payload_output).map_err(io_storage)?;
        sync_directory(parent).map_err(io_storage)?;
        target.verify_source_revision(journal)?;
        Ok(converted)
    }

    pub(super) async fn verify(
        &self,
        journal: &UpgradeJournalV1,
        target: &TargetGenerationStager,
    ) -> Result<(), ProfileStorageUpgradeError> {
        target.verify_separated(journal)?;
        let converted = self
            .inspect_output(&target.paths(journal).payload_output)
            .await?;
        if journal.payload_profile_database_digest() != Some(converted.profile_database_digest)
            || journal.payload_blob_tree_digest() != Some(converted.blob_tree_digest)
            || journal.converted_derived_count() != Some(converted.derived_count)
            || journal.converted_search_document_count() != Some(converted.search_document_count)
        {
            return Err(corrupt(anyhow::anyhow!(
                "profile upgrade payload output digest or count mismatch"
            )));
        }
        target.verify_source_revision(journal)
    }

    async fn build_output(
        &self,
        primary_output: &Path,
        final_output: &Path,
        work: &Path,
    ) -> Result<ConvertedDerivedPayloads, ProfileStorageUpgradeError> {
        copy_directory(primary_output, work)?;
        let database = work.join(DATABASE_FILE);
        let rows = load_rows(&database)?;
        let converted = self.convert_rows(rows).await?;
        save_rows(&database, &converted)?;
        compact_database(&database)?;
        let inspected = self.inspect_output(work).await?;
        if final_output.exists() {
            return Err(corrupt(anyhow::anyhow!(
                "profile upgrade payload output appeared during conversion"
            )));
        }
        sync_directory(work).map_err(io_storage)?;
        Ok(inspected)
    }

    async fn convert_rows(
        &self,
        rows: LoadedRows,
    ) -> Result<ConvertedRows, ProfileStorageUpgradeError> {
        let legacy_file_set = (!rows.file_sets.is_empty())
            .then(|| {
                EntryFileSetPathCipher::legacy_for_upgrade(&self.source_session, &self.profile_id)
            })
            .transpose()
            .map_err(security)?;
        let legacy_transfer = (!rows.transfers.is_empty() || !rows.transfer_events.is_empty())
            .then(|| {
                TransferPersistenceCipher::legacy_for_upgrade(
                    &self.source_session,
                    &self.profile_id,
                )
            })
            .transpose()
            .map_err(security)?;
        let legacy_render = (!rows.search_documents.is_empty())
            .then(|| RenderPayloadCodec::legacy_for_upgrade(&self.source_session, &self.profile_id))
            .transpose()
            .map_err(security)?;
        let legacy_active = (!rows.active_registers.is_empty())
            .then(|| {
                ActiveClipboardRegisterCipher::legacy_for_upgrade(
                    &self.source_session,
                    &self.profile_id,
                )
            })
            .transpose()
            .map_err(security)?;
        let legacy_publish = (!rows.publish_logs.is_empty())
            .then(|| {
                DirectoryPublishLogCipher::legacy_for_upgrade(
                    &self.source_session,
                    &self.profile_id,
                )
            })
            .transpose()
            .map_err(security)?;
        let legacy_artifact = (!rows.receive_artifacts.is_empty())
            .then(|| {
                ReceiveArtifactCipher::legacy_for_upgrade(&self.source_session, &self.profile_id)
            })
            .transpose()
            .map_err(security)?;

        let v3_file_set = V3EntryFileSetPathCipher::new(Arc::clone(&self.content_protection));
        let v3_transfer = V3TransferPersistenceCipher::new(Arc::clone(&self.content_protection));
        let v3_active = V3ActiveClipboardRegisterCipher::new(Arc::clone(&self.content_protection));
        let v3_publish = V3DirectoryPublishLogCipher::new(Arc::clone(&self.content_protection));
        let v3_artifact = V3ReceiveArtifactCipher::new(Arc::clone(&self.content_protection));

        let mut file_sets = Vec::with_capacity(rows.file_sets.len());
        for row in rows.file_sets {
            let legacy = legacy_file_set.as_ref().ok_or_else(missing_legacy)?;
            let entry_id = EntryId::from(row.entry_id.as_str());
            let original_text_ct = match row.original_text_ct {
                Some(ciphertext) => {
                    let plaintext = legacy
                        .open_original_text(&entry_id, row.line_index, &ciphertext)
                        .map_err(owner_security)?;
                    Some(
                        v3_file_set
                            .seal_original_text(&entry_id, row.line_index, &plaintext)
                            .await
                            .map_err(owner_security)?,
                    )
                }
                None => None,
            };
            let relative_path_ct = match row.relative_path_ct {
                Some(ciphertext) => {
                    let plaintext = legacy
                        .open_relative_path(&entry_id, row.line_index, &ciphertext)
                        .map_err(owner_security)?;
                    Some(
                        v3_file_set
                            .seal_relative_path(&entry_id, row.line_index, &plaintext)
                            .await
                            .map_err(owner_security)?,
                    )
                }
                None => None,
            };
            let root_name_ct = match row.root_name_ct {
                Some(ciphertext) => {
                    let plaintext = legacy
                        .open_root_name(&entry_id, row.line_index, &ciphertext)
                        .map_err(owner_security)?;
                    Some(
                        v3_file_set
                            .seal_root_name(&entry_id, row.line_index, &plaintext)
                            .await
                            .map_err(owner_security)?,
                    )
                }
                None => None,
            };
            file_sets.push(ConvertedFileSetRow {
                entry_id: row.entry_id,
                line_index: row.line_index,
                original_text_ct,
                relative_path_ct,
                root_name_ct,
            });
        }

        let mut transfers = Vec::with_capacity(rows.transfers.len());
        for row in rows.transfers {
            let legacy = legacy_transfer.as_ref().ok_or_else(missing_legacy)?;
            let metadata = legacy
                .open_metadata(&row.transfer_id, &row.ciphertext)
                .map_err(owner_security)?;
            let ciphertext = v3_transfer
                .seal_metadata(&row.transfer_id, &metadata)
                .await
                .map_err(owner_security)?;
            transfers.push((row.transfer_id, ciphertext));
        }

        let mut transfer_events = Vec::with_capacity(rows.transfer_events.len());
        for row in rows.transfer_events {
            let legacy = legacy_transfer.as_ref().ok_or_else(missing_legacy)?;
            let event = legacy
                .open_event(
                    &row.transfer_id,
                    row.sequence,
                    &row.event_type,
                    &row.ciphertext,
                )
                .map_err(owner_security)?;
            let ciphertext = v3_transfer
                .seal_event(&row.transfer_id, row.sequence, &row.event_type, &event)
                .await
                .map_err(owner_security)?;
            transfer_events.push((row.id, ciphertext));
        }

        let search_group_ref = if rows.search_documents.is_empty() {
            None
        } else {
            Some(
                self.search_protection
                    .index_terms(&[])
                    .await
                    .map_err(owner_security)?
                    .group_ref()
                    .as_bytes()
                    .to_vec(),
            )
        };
        let mut search_documents = Vec::with_capacity(rows.search_documents.len());
        for row in rows.search_documents {
            let legacy = legacy_render.as_ref().ok_or_else(missing_legacy)?;
            let entry_id = EntryId::from(row.entry_id.as_str());
            let fields = legacy
                .decrypt(&entry_id, &row.ciphertext)
                .map_err(owner_security)?;
            let ciphertext = self
                .search_protection
                .seal_render(&entry_id, &fields)
                .await
                .map_err(owner_security)?;
            search_documents.push((
                row.profile_id,
                row.entry_id,
                ciphertext,
                search_group_ref.clone().ok_or_else(missing_legacy)?,
            ));
        }

        let mut active_registers = Vec::with_capacity(rows.active_registers.len());
        for row in rows.active_registers {
            let reference = legacy_active
                .as_ref()
                .ok_or_else(missing_legacy)?
                .open(&row.ciphertext)
                .map_err(owner_security)?;
            active_registers.push((
                row.id,
                v3_active.seal(&reference).await.map_err(owner_security)?,
            ));
        }
        let mut publish_logs = Vec::with_capacity(rows.publish_logs.len());
        for row in rows.publish_logs {
            let entry_id = EntryId::from(row.entry_id.as_str());
            let roots = legacy_publish
                .as_ref()
                .ok_or_else(missing_legacy)?
                .open(&entry_id, &row.attempt_id, &row.ciphertext)
                .map_err(owner_security)?;
            let ciphertext = v3_publish
                .seal(&entry_id, &row.attempt_id, &roots)
                .await
                .map_err(owner_security)?;
            publish_logs.push((row.entry_id, row.attempt_id, ciphertext));
        }
        let mut receive_artifacts = Vec::with_capacity(rows.receive_artifacts.len());
        for row in rows.receive_artifacts {
            let artifacts = legacy_artifact
                .as_ref()
                .ok_or_else(missing_legacy)?
                .open(&row.entry_id, &row.attempt_id, &row.ciphertext)
                .map_err(owner_security)?;
            let ciphertext = v3_artifact
                .seal(&row.entry_id, &row.attempt_id, &artifacts)
                .await
                .map_err(owner_security)?;
            receive_artifacts.push((row.entry_id, row.attempt_id, ciphertext));
        }
        Ok(ConvertedRows {
            file_sets,
            transfers,
            transfer_events,
            search_documents,
            active_registers,
            publish_logs,
            receive_artifacts,
        })
    }

    async fn inspect_output(
        &self,
        output: &Path,
    ) -> Result<ConvertedDerivedPayloads, ProfileStorageUpgradeError> {
        if !output.is_dir() {
            return Err(corrupt(anyhow::anyhow!(
                "profile payload output is missing"
            )));
        }
        let database = output.join(DATABASE_FILE);
        let rows = load_v3_rows(&database)?;
        let file_set = V3EntryFileSetPathCipher::new(Arc::clone(&self.content_protection));
        for row in &rows.file_sets {
            let entry_id = EntryId::from(row.entry_id.as_str());
            if let Some(ciphertext) = &row.original_text_ct {
                file_set
                    .open_original_text(&entry_id, row.line_index, ciphertext)
                    .await
                    .map_err(owner_corrupt)?;
            }
            if let Some(ciphertext) = &row.relative_path_ct {
                file_set
                    .open_relative_path(&entry_id, row.line_index, ciphertext)
                    .await
                    .map_err(owner_corrupt)?;
            }
            if let Some(ciphertext) = &row.root_name_ct {
                file_set
                    .open_root_name(&entry_id, row.line_index, ciphertext)
                    .await
                    .map_err(owner_corrupt)?;
            }
        }
        let transfer = V3TransferPersistenceCipher::new(Arc::clone(&self.content_protection));
        for row in &rows.transfers {
            transfer
                .open_metadata(&row.transfer_id, &row.ciphertext)
                .await
                .map_err(owner_corrupt)?;
        }
        for row in &rows.transfer_events {
            transfer
                .open_event(
                    &row.transfer_id,
                    row.sequence,
                    &row.event_type,
                    &row.ciphertext,
                )
                .await
                .map_err(owner_corrupt)?;
        }
        for row in &rows.search_documents {
            SearchGroupRef::from_bytes(&row.group_ref).map_err(owner_corrupt)?;
            self.search_protection
                .open_render(&EntryId::from(row.entry_id.as_str()), &row.ciphertext)
                .await
                .map_err(owner_corrupt)?;
        }
        let active = V3ActiveClipboardRegisterCipher::new(Arc::clone(&self.content_protection));
        for row in &rows.active_registers {
            active.open(&row.ciphertext).await.map_err(owner_corrupt)?;
        }
        let publish = V3DirectoryPublishLogCipher::new(Arc::clone(&self.content_protection));
        for row in &rows.publish_logs {
            publish
                .open(
                    &EntryId::from(row.entry_id.as_str()),
                    &row.attempt_id,
                    &row.ciphertext,
                )
                .await
                .map_err(owner_corrupt)?;
        }
        let artifact = V3ReceiveArtifactCipher::new(Arc::clone(&self.content_protection));
        for row in &rows.receive_artifacts {
            artifact
                .open(&row.entry_id, &row.attempt_id, &row.ciphertext)
                .await
                .map_err(owner_corrupt)?;
        }
        verify_search_rebuild_gate(&database)?;
        Ok(ConvertedDerivedPayloads {
            profile_database_digest: file_digest(&database)?,
            blob_tree_digest: blob_tree_digest(&output.join(BLOB_DIRECTORY))?,
            derived_count: rows.derived_count(),
            search_document_count: rows.search_documents.len() as u64,
        })
    }
}

#[derive(diesel::QueryableByName)]
struct FileSetRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    entry_id: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    line_index: i64,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
    original_text_ct: Option<Vec<u8>>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
    relative_path_ct: Option<Vec<u8>>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Binary>)]
    root_name_ct: Option<Vec<u8>>,
}
#[derive(diesel::QueryableByName)]
struct TransferRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    transfer_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    ciphertext: Vec<u8>,
}
#[derive(diesel::QueryableByName)]
struct TransferEventRow {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    id: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    transfer_id: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    sequence: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    event_type: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    ciphertext: Vec<u8>,
}
#[derive(diesel::QueryableByName)]
struct SearchRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    profile_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    entry_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    ciphertext: Vec<u8>,
}
#[derive(diesel::QueryableByName)]
struct V3SearchRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    entry_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    ciphertext: Vec<u8>,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    group_ref: Vec<u8>,
}
#[derive(diesel::QueryableByName)]
struct IdRow {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    id: i32,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    ciphertext: Vec<u8>,
}
#[derive(diesel::QueryableByName)]
struct AttemptRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    entry_id: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    attempt_id: String,
    #[diesel(sql_type = diesel::sql_types::Binary)]
    ciphertext: Vec<u8>,
}

struct LoadedRows {
    file_sets: Vec<FileSetRow>,
    transfers: Vec<TransferRow>,
    transfer_events: Vec<TransferEventRow>,
    search_documents: Vec<SearchRow>,
    active_registers: Vec<IdRow>,
    publish_logs: Vec<AttemptRow>,
    receive_artifacts: Vec<AttemptRow>,
}
struct LoadedV3Rows {
    file_sets: Vec<FileSetRow>,
    transfers: Vec<TransferRow>,
    transfer_events: Vec<TransferEventRow>,
    search_documents: Vec<V3SearchRow>,
    active_registers: Vec<IdRow>,
    publish_logs: Vec<AttemptRow>,
    receive_artifacts: Vec<AttemptRow>,
}
impl LoadedV3Rows {
    fn derived_count(&self) -> u64 {
        (self.file_sets.len()
            + self.transfers.len()
            + self.transfer_events.len()
            + self.search_documents.len()
            + self.active_registers.len()
            + self.publish_logs.len()
            + self.receive_artifacts.len()) as u64
    }
}
struct ConvertedFileSetRow {
    entry_id: String,
    line_index: i64,
    original_text_ct: Option<Vec<u8>>,
    relative_path_ct: Option<Vec<u8>>,
    root_name_ct: Option<Vec<u8>>,
}
struct ConvertedRows {
    file_sets: Vec<ConvertedFileSetRow>,
    transfers: Vec<(String, Vec<u8>)>,
    transfer_events: Vec<(i32, Vec<u8>)>,
    search_documents: Vec<(String, String, Vec<u8>, Vec<u8>)>,
    active_registers: Vec<(i32, Vec<u8>)>,
    publish_logs: Vec<(String, String, Vec<u8>)>,
    receive_artifacts: Vec<(String, String, Vec<u8>)>,
}

fn load_rows(path: &Path) -> Result<LoadedRows, ProfileStorageUpgradeError> {
    let mut connection = open_connection(path)?;
    Ok(LoadedRows {
        file_sets: diesel::sql_query("SELECT entry_id, line_index, original_text_ct, relative_path_ct, root_name_ct FROM entry_file_set ORDER BY entry_id, line_index").load(&mut connection).map_err(database_storage)?,
        transfers: diesel::sql_query("SELECT transfer_id, metadata_ciphertext AS ciphertext FROM file_transfer ORDER BY transfer_id").load(&mut connection).map_err(database_storage)?,
        transfer_events: diesel::sql_query("SELECT id, transfer_id, sequence, event_type, payload_ciphertext AS ciphertext FROM file_transfer_events ORDER BY id").load(&mut connection).map_err(database_storage)?,
        search_documents: diesel::sql_query("SELECT profile_id, entry_id, render_payload AS ciphertext FROM search_document WHERE render_payload IS NOT NULL ORDER BY profile_id, entry_id").load(&mut connection).map_err(database_storage)?,
        active_registers: diesel::sql_query("SELECT id, consumable_ref_ciphertext AS ciphertext FROM active_clipboard_register WHERE consumable_ref_ciphertext IS NOT NULL ORDER BY id").load(&mut connection).map_err(database_storage)?,
        publish_logs: diesel::sql_query("SELECT entry_id, attempt_id, root_map_ciphertext AS ciphertext FROM directory_publish_log WHERE root_map_ciphertext IS NOT NULL ORDER BY entry_id, attempt_id").load(&mut connection).map_err(database_storage)?,
        receive_artifacts: diesel::sql_query("SELECT entry_id, attempt_id, artifact_ciphertext AS ciphertext FROM receive_artifact_log ORDER BY entry_id, attempt_id").load(&mut connection).map_err(database_storage)?,
    })
}

fn load_v3_rows(path: &Path) -> Result<LoadedV3Rows, ProfileStorageUpgradeError> {
    let mut connection = open_connection(path)?;
    Ok(LoadedV3Rows {
        file_sets: diesel::sql_query("SELECT entry_id, line_index, original_text_ct, relative_path_ct, root_name_ct FROM entry_file_set ORDER BY entry_id, line_index").load(&mut connection).map_err(database_storage)?,
        transfers: diesel::sql_query("SELECT transfer_id, metadata_ciphertext AS ciphertext FROM file_transfer ORDER BY transfer_id").load(&mut connection).map_err(database_storage)?,
        transfer_events: diesel::sql_query("SELECT id, transfer_id, sequence, event_type, payload_ciphertext AS ciphertext FROM file_transfer_events ORDER BY id").load(&mut connection).map_err(database_storage)?,
        search_documents: diesel::sql_query("SELECT entry_id, render_payload AS ciphertext, protection_group_ref AS group_ref FROM search_document WHERE render_payload IS NOT NULL ORDER BY profile_id, entry_id").load(&mut connection).map_err(database_storage)?,
        active_registers: diesel::sql_query("SELECT id, consumable_ref_ciphertext AS ciphertext FROM active_clipboard_register WHERE consumable_ref_ciphertext IS NOT NULL ORDER BY id").load(&mut connection).map_err(database_storage)?,
        publish_logs: diesel::sql_query("SELECT entry_id, attempt_id, root_map_ciphertext AS ciphertext FROM directory_publish_log WHERE root_map_ciphertext IS NOT NULL ORDER BY entry_id, attempt_id").load(&mut connection).map_err(database_storage)?,
        receive_artifacts: diesel::sql_query("SELECT entry_id, attempt_id, artifact_ciphertext AS ciphertext FROM receive_artifact_log ORDER BY entry_id, attempt_id").load(&mut connection).map_err(database_storage)?,
    })
}

fn save_rows(path: &Path, rows: &ConvertedRows) -> Result<(), ProfileStorageUpgradeError> {
    let mut connection = open_connection(path)?;
    let group_ref_column = diesel::sql_query(
        "SELECT COUNT(*) AS count FROM pragma_table_info('search_document') WHERE name = 'protection_group_ref'",
    )
    .get_result::<ColumnCount>(&mut connection)
    .map_err(database_storage)?;
    if group_ref_column.count == 0 {
        connection
            .batch_execute("ALTER TABLE search_document ADD COLUMN protection_group_ref BLOB;")
            .map_err(database_storage)?;
    }
    connection.transaction::<_, diesel::result::Error, _>(|connection| {
        for row in &rows.file_sets { diesel::sql_query("UPDATE entry_file_set SET original_text_ct = ?, relative_path_ct = ?, root_name_ct = ? WHERE entry_id = ? AND line_index = ?").bind::<diesel::sql_types::Nullable<diesel::sql_types::Binary>, _>(row.original_text_ct.as_deref()).bind::<diesel::sql_types::Nullable<diesel::sql_types::Binary>, _>(row.relative_path_ct.as_deref()).bind::<diesel::sql_types::Nullable<diesel::sql_types::Binary>, _>(row.root_name_ct.as_deref()).bind::<diesel::sql_types::Text, _>(&row.entry_id).bind::<diesel::sql_types::BigInt, _>(row.line_index).execute(connection)?; }
        for (id, ciphertext) in &rows.transfers { diesel::sql_query("UPDATE file_transfer SET metadata_ciphertext = ? WHERE transfer_id = ?").bind::<diesel::sql_types::Binary, _>(ciphertext).bind::<diesel::sql_types::Text, _>(id).execute(connection)?; }
        for (id, ciphertext) in &rows.transfer_events { diesel::sql_query("UPDATE file_transfer_events SET payload_ciphertext = ? WHERE id = ?").bind::<diesel::sql_types::Binary, _>(ciphertext).bind::<diesel::sql_types::Integer, _>(id).execute(connection)?; }
        for (profile_id, entry_id, ciphertext, group_ref) in &rows.search_documents { diesel::sql_query("UPDATE search_document SET render_payload = ?, protection_group_ref = ?, index_version = ? WHERE profile_id = ? AND entry_id = ?").bind::<diesel::sql_types::Binary, _>(ciphertext).bind::<diesel::sql_types::Binary, _>(group_ref).bind::<diesel::sql_types::Text, _>(V3_SEARCH_INDEX_VERSION).bind::<diesel::sql_types::Text, _>(profile_id).bind::<diesel::sql_types::Text, _>(entry_id).execute(connection)?; }
        for (id, ciphertext) in &rows.active_registers { diesel::sql_query("UPDATE active_clipboard_register SET consumable_ref_ciphertext = ? WHERE id = ?").bind::<diesel::sql_types::Binary, _>(ciphertext).bind::<diesel::sql_types::Integer, _>(id).execute(connection)?; }
        for (entry_id, attempt_id, ciphertext) in &rows.publish_logs { diesel::sql_query("UPDATE directory_publish_log SET root_map_ciphertext = ? WHERE entry_id = ? AND attempt_id = ?").bind::<diesel::sql_types::Binary, _>(ciphertext).bind::<diesel::sql_types::Text, _>(entry_id).bind::<diesel::sql_types::Text, _>(attempt_id).execute(connection)?; }
        for (entry_id, attempt_id, ciphertext) in &rows.receive_artifacts { diesel::sql_query("UPDATE receive_artifact_log SET artifact_ciphertext = ? WHERE entry_id = ? AND attempt_id = ?").bind::<diesel::sql_types::Binary, _>(ciphertext).bind::<diesel::sql_types::Text, _>(entry_id).bind::<diesel::sql_types::Text, _>(attempt_id).execute(connection)?; }
        diesel::sql_query("DELETE FROM search_posting").execute(connection)?;
        diesel::sql_query("DELETE FROM search_entry_tag").execute(connection)?;
        diesel::sql_query("UPDATE search_index_meta SET index_version = ?, search_blocked = 1, last_rebuild_started_at_ms = NULL, last_rebuild_completed_at_ms = NULL").bind::<diesel::sql_types::Text, _>(V3_SEARCH_INDEX_VERSION).execute(connection)?;
        Ok(())
    }).map_err(database_storage)
}

#[derive(diesel::QueryableByName)]
struct ColumnCount {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

#[derive(diesel::QueryableByName)]
struct SearchGateRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    posting_count: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    tag_count: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    unblocked_count: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    invalid_group_count: i64,
}
fn verify_search_rebuild_gate(path: &Path) -> Result<(), ProfileStorageUpgradeError> {
    let mut connection = open_connection(path)?;
    let row = diesel::sql_query("SELECT (SELECT COUNT(*) FROM search_posting) AS posting_count, (SELECT COUNT(*) FROM search_entry_tag) AS tag_count, (SELECT COUNT(*) FROM search_index_meta WHERE search_blocked != 1 OR index_version != 'search-v12') AS unblocked_count, (SELECT COUNT(*) FROM search_document WHERE render_payload IS NOT NULL AND (protection_group_ref IS NULL OR length(protection_group_ref) != 32)) AS invalid_group_count").get_result::<SearchGateRow>(&mut connection).map_err(database_storage)?;
    if row.posting_count != 0
        || row.tag_count != 0
        || row.unblocked_count != 0
        || row.invalid_group_count != 0
    {
        return Err(corrupt(anyhow::anyhow!(
            "V3 search rebuild gate is invalid"
        )));
    }
    Ok(())
}

fn open_connection(
    path: &Path,
) -> Result<diesel::sqlite::SqliteConnection, ProfileStorageUpgradeError> {
    let database = path
        .to_str()
        .ok_or_else(|| storage(anyhow::anyhow!("payload database path is invalid")))?;
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(database).map_err(|source| {
            storage(anyhow::Error::new(source).context("open derived payload database"))
        })?;
    connection
        .batch_execute("PRAGMA busy_timeout = 5000; PRAGMA foreign_keys = ON;")
        .map_err(database_storage)?;
    Ok(connection)
}

fn copy_directory(source: &Path, destination: &Path) -> Result<(), ProfileStorageUpgradeError> {
    if !source.is_dir() {
        return Err(corrupt(anyhow::anyhow!(
            "primary payload output is missing"
        )));
    }
    std::fs::create_dir(destination).map_err(io_storage)?;
    for entry in std::fs::read_dir(source).map_err(io_storage)? {
        let entry = entry.map_err(io_storage)?;
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(io_storage)?;
        if kind.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target).map_err(io_storage)?;
            std::fs::File::open(&target)
                .and_then(|file| file.sync_all())
                .map_err(io_storage)?;
        } else {
            return Err(corrupt(anyhow::anyhow!(
                "primary payload output contains an unsupported entry"
            )));
        }
    }
    sync_directory(destination).map_err(io_storage)
}

fn missing_legacy() -> ProfileStorageUpgradeError {
    corrupt(anyhow::anyhow!("legacy owner codec is missing"))
}
fn owner_security(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProfileStorageUpgradeError {
    security(anyhow::Error::new(source).context("convert owner-defined payload"))
}
fn owner_corrupt(
    source: impl std::error::Error + Send + Sync + 'static,
) -> ProfileStorageUpgradeError {
    corrupt(anyhow::Error::new(source).context("verify owner-defined V3 payload"))
}
fn database_storage(source: diesel::result::Error) -> ProfileStorageUpgradeError {
    storage(anyhow::Error::new(source).context("update derived payload database"))
}
fn io_storage(source: std::io::Error) -> ProfileStorageUpgradeError {
    storage(anyhow::Error::new(source).context("persist derived payload output"))
}
fn storage(source: anyhow::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Storage { source }
}
fn security(source: anyhow::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Security { source }
}
fn corrupt(source: anyhow::Error) -> ProfileStorageUpgradeError {
    ProfileStorageUpgradeError::Corrupt { source }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use uc_core::clipboard::MobileConsumableRef;
    use uc_core::file_transfer::FileTransferEvent;
    use uc_core::ids::SpaceId;
    use uc_core::ports::{
        ReceiveArtifact, ReceiveArtifactOwnership, SecureStorageError, SecureStoragePort,
    };

    use super::*;
    use crate::db::pool::init_db_pool;
    use crate::file_transfer::persistence_cipher::TransferMetadata;
    use crate::search::RenderFields;
    use crate::security::MasterKey;

    #[derive(Default)]
    struct MemorySecureStorage(Mutex<BTreeMap<String, Vec<u8>>>);

    impl SecureStoragePort for MemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(key)
                .cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(key);
            Ok(())
        }
    }

    #[tokio::test]
    async fn complete_derived_output_is_v3_only_and_search_is_rebuild_gated() {
        let directory = tempfile::tempdir().unwrap();
        let profile_id = ProfileId::from("default");
        let session = Arc::new(InMemorySession::new());
        let space_id = SpaceId::from_str("derived-source-space");
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0xD1; 32]).unwrap(),
        );
        let material = session
            .create_migrated_space_material(&space_id, 1)
            .unwrap();
        session.install_space_material(&material).unwrap();
        let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
        let vault = Arc::new(ProfileContentKeyVault::new(
            directory.path().join("vault"),
            secure_storage,
            [0xD2; 16],
        ));
        vault
            .install_verified_space_material(&material)
            .await
            .unwrap();

        let primary = directory.path().join("v3-primary");
        std::fs::create_dir_all(primary.join(BLOB_DIRECTORY)).unwrap();
        let database = primary.join(DATABASE_FILE);
        let pool = init_db_pool(database.to_str().unwrap()).unwrap();
        let entry_id = EntryId::from("entry-derived");

        let file_set = EntryFileSetPathCipher::legacy_for_upgrade(&session, &profile_id).unwrap();
        let original = file_set
            .seal_original_text(&entry_id, 0, "/secret/original.txt")
            .unwrap();
        let relative = file_set
            .seal_relative_path(&entry_id, 0, "secret/relative.txt")
            .unwrap();
        let root_name = file_set
            .seal_root_name(&entry_id, 0, "secret-root")
            .unwrap();
        let transfer =
            TransferPersistenceCipher::legacy_for_upgrade(&session, &profile_id).unwrap();
        let metadata = transfer
            .seal_metadata(
                "transfer-derived",
                &TransferMetadata {
                    filename: "secret-transfer.txt".to_owned(),
                    cached_path: Some("/secret/cache".to_owned()),
                    failure_detail: None,
                },
            )
            .unwrap();
        let transfer_event = FileTransferEvent::started(
            "transfer-derived",
            "peer-derived",
            "secret-transfer.txt",
            Some(1),
        );
        let event = transfer
            .seal_event("transfer-derived", 1, "started", &transfer_event)
            .unwrap();
        let active =
            ActiveClipboardRegisterCipher::legacy_for_upgrade(&session, &profile_id).unwrap();
        let active_ref = active
            .seal(&MobileConsumableRef::new("hash-derived", entry_id.clone()))
            .unwrap();
        let publish = DirectoryPublishLogCipher::legacy_for_upgrade(&session, &profile_id).unwrap();
        let root_map = vec![(
            PathBuf::from("/secret/staging"),
            PathBuf::from("/secret/final"),
        )];
        let published = publish
            .seal(&entry_id, "attempt-derived", &root_map)
            .unwrap();
        let artifact = ReceiveArtifactCipher::legacy_for_upgrade(&session, &profile_id).unwrap();
        let artifacts = vec![ReceiveArtifact {
            item_id: "item-derived".to_owned(),
            staged_path: PathBuf::from("/secret/receive-staging"),
            final_path: PathBuf::from("/secret/receive-final"),
            ownership: ReceiveArtifactOwnership::ManagedStaging,
        }];
        let artifact_payload = artifact
            .seal(entry_id.as_ref(), "attempt-derived", &artifacts)
            .unwrap();
        let render = RenderPayloadCodec::legacy_for_upgrade(&session, &profile_id).unwrap();
        let render_payload = render
            .encrypt(
                &entry_id,
                &RenderFields::new(
                    Some("secret preview".to_owned()),
                    vec!["secret-render.txt".to_owned()],
                    Vec::new(),
                    vec!["/secret/render/path".to_owned()],
                    Some(14),
                ),
            )
            .unwrap();

        let mut connection = pool.get().unwrap();
        diesel::sql_query("INSERT INTO clipboard_event (event_id, captured_at_ms, source_device, snapshot_hash) VALUES ('event-derived', 1, 'device-derived', 'snapshot-derived')").execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO clipboard_entry (entry_id, event_id, created_at_ms, active_time_ms, total_size, pinned, deleted_at_ms, delivery_tracked, is_favorited, content_category) VALUES (?, 'event-derived', 1, 1, 1, 0, NULL, 0, 0, 'files')").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO entry_file_set (entry_id, line_index, kind, original_text_ct, root_index, relative_path_ct, kind_tag, root_name_ct) VALUES (?, 0, 'file', ?, 0, ?, 'file', ?)").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).bind::<diesel::sql_types::Binary, _>(original).bind::<diesel::sql_types::Binary, _>(relative).bind::<diesel::sql_types::Binary, _>(root_name).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO file_transfer (transfer_id, entry_id, file_size, attempt_id, binding_state, receive_item_id, item_role, content_hash, status, source_device, failure_code, metadata_ciphertext, created_at_ms, updated_at_ms) VALUES ('transfer-derived', ?, 1, NULL, 'legacy', NULL, NULL, NULL, 'pending', 'device-derived', NULL, ?, 1, 1)").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).bind::<diesel::sql_types::Binary, _>(metadata).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO file_transfer_events (transfer_id, sequence, event_type, payload_ciphertext, occurred_at_ms) VALUES ('transfer-derived', 1, 'started', ?, 1)").bind::<diesel::sql_types::Binary, _>(event).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO active_clipboard_register (id, snapshot_hash, entry_id, activated_at_ms, activated_by, consumable_ref_ciphertext) VALUES (1, 'hash-derived', ?, 1, 'device-derived', ?)").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).bind::<diesel::sql_types::Binary, _>(active_ref).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO directory_publish_log (entry_id, attempt_id, phase, root_map_ciphertext, partial_publication, partial_root_count, landed, updated_at_ms) VALUES (?, 'attempt-derived', 'publishing', ?, 0, 0, 0, 1)").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).bind::<diesel::sql_types::Binary, _>(published).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO receive_artifact_log (entry_id, attempt_id, phase, resolution, artifact_ciphertext, updated_at_ms) VALUES (?, 'attempt-derived', 'publishing', 'pending', ?, 1)").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).bind::<diesel::sql_types::Binary, _>(artifact_payload).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO search_document (profile_id, entry_id, event_id, active_time_ms, captured_at_ms, file_type, file_extensions, mime_type, indexed_at_ms, index_version, source_device, payload_state, render_payload) VALUES ('default', ?, 'event-derived', 1, 1, 'files', '[]', 'text/plain', 1, 'search-v11', 'device-derived', NULL, ?)").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).bind::<diesel::sql_types::Binary, _>(render_payload).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO search_posting (profile_id, term_tag, entry_id, field_mask, term_freq) VALUES ('default', zeroblob(32), ?, 1, 1)").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO search_entry_tag (profile_id, entry_id, tag_id) VALUES ('default', ?, 'derived-tag')").bind::<diesel::sql_types::Text, _>(entry_id.as_ref()).execute(&mut connection).unwrap();
        diesel::sql_query("INSERT INTO search_index_meta (profile_id, index_version, search_blocked, last_rebuild_started_at_ms, last_rebuild_completed_at_ms, plaintext_purge_done_ms) VALUES ('default', 'search-v11', 0, 1, 1, 1)").execute(&mut connection).unwrap();
        drop(connection);
        drop(pool);
        compact_database(&database).unwrap();
        let primary_database_before = std::fs::read(&database).unwrap();

        let converter = DerivedPayloadConverter::new(profile_id, Arc::clone(&session), vault);
        let final_output = directory.path().join("v3-payloads");
        let work = directory.path().join("v3-payloads-work");
        let converted = converter
            .build_output(&primary, &final_output, &work)
            .await
            .unwrap();

        assert_eq!(converted.derived_count, 7);
        assert_eq!(converted.search_document_count, 1);
        assert_eq!(std::fs::read(&database).unwrap(), primary_database_before);
        session.clear();
        let reopened = converter.inspect_output(&work).await.unwrap();
        assert_eq!(reopened.derived_count, 7);
        verify_search_rebuild_gate(&work.join(DATABASE_FILE)).unwrap();
        let bytes = std::fs::read(work.join(DATABASE_FILE)).unwrap();
        for secret in [
            b"secret/original.txt".as_slice(),
            b"secret-transfer.txt",
            b"secret preview",
            b"/secret/receive-final",
        ] {
            assert!(!bytes.windows(secret.len()).any(|window| window == secret));
        }
    }
}
