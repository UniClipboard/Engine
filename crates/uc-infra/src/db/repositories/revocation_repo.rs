use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};
use serde::{de::DeserializeOwned, Serialize};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    BeginRevocationOutcome, KeyEpochError, RevocationId, RevocationRecord,
    RevocationRepositoryPort, RevocationStage, RevocationStatus, SpaceKeyMaterial,
    SpaceSecurityMode,
};

use crate::db::ports::DbExecutor;
use crate::security::crypto_model::EncryptedBlob;
use crate::security::{v1_aead, InMemorySession, MasterKey};

pub struct DieselRevocationRepository<E> {
    executor: E,
    session: InMemorySession,
}

impl<E> DieselRevocationRepository<E> {
    pub fn new(executor: E, session: InMemorySession) -> Self {
        Self { executor, session }
    }
}

#[derive(QueryableByName)]
struct RevocationRow {
    #[diesel(sql_type = Text)]
    revocation_id: String,
    #[diesel(sql_type = Text)]
    space_id: String,
    #[diesel(sql_type = BigInt)]
    previous_epoch: i64,
    #[diesel(sql_type = BigInt)]
    next_epoch: i64,
    #[diesel(sql_type = Text)]
    status: String,
    #[diesel(sql_type = Binary)]
    encrypted_record: Vec<u8>,
    #[diesel(sql_type = Nullable<Binary>)]
    encrypted_stage: Option<Vec<u8>>,
    #[diesel(sql_type = BigInt)]
    created_at_ms: i64,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(QueryableByName)]
struct SpaceMaterialRow {
    #[diesel(sql_type = Text)]
    space_id: String,
    #[diesel(sql_type = BigInt)]
    group_epoch: i64,
    #[diesel(sql_type = Text)]
    security_mode: String,
    #[diesel(sql_type = Text)]
    current_content_key_id: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

fn backend(error: impl std::fmt::Display) -> KeyEpochError {
    KeyEpochError::Repository(error.to_string())
}

fn epoch_to_i64(epoch: u64) -> Result<i64, KeyEpochError> {
    i64::try_from(epoch).map_err(|_| backend("group epoch exceeds SQLite range"))
}

fn status_name(status: RevocationStatus) -> &'static str {
    match status {
        RevocationStatus::Prepared => "prepared",
        RevocationStatus::Staged => "staged",
        RevocationStatus::Activated => "activated",
        RevocationStatus::Distributing => "distributing",
        RevocationStatus::Complete => "complete",
        RevocationStatus::RecoveryRequired => "recovery_required",
    }
}

fn mode_name(mode: SpaceSecurityMode) -> &'static str {
    match mode {
        SpaceSecurityMode::Legacy => "legacy",
        SpaceSecurityMode::Migrating => "migrating",
        SpaceSecurityMode::Ready => "ready",
    }
}

fn record_aad(revocation_id: &str, status: &str) -> Vec<u8> {
    format!("uc-revocation-record-v1|{revocation_id}|{status}").into_bytes()
}

fn stage_aad(revocation_id: &str) -> Vec<u8> {
    format!("uc-revocation-stage-v1|{revocation_id}").into_bytes()
}

fn space_aad(space_id: &str, epoch: i64) -> Vec<u8> {
    format!("uc-space-key-material-v1|{space_id}|{epoch}").into_bytes()
}

fn seal<T: Serialize>(
    master_key: &MasterKey,
    value: &T,
    aad: &[u8],
) -> Result<Vec<u8>, KeyEpochError> {
    let plaintext = serde_json::to_vec(value).map_err(backend)?;
    let encrypted = v1_aead::encrypt_blob_xchacha(master_key, &plaintext, aad).map_err(backend)?;
    serde_json::to_vec(&encrypted).map_err(backend)
}

fn open<T: DeserializeOwned>(
    master_key: &MasterKey,
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<T, KeyEpochError> {
    let encrypted: EncryptedBlob = serde_json::from_slice(ciphertext).map_err(backend)?;
    let plaintext =
        v1_aead::decrypt_blob_xchacha(master_key, &encrypted.nonce, &encrypted.ciphertext, aad)
            .map_err(backend)?;
    serde_json::from_slice(&plaintext).map_err(backend)
}

fn load_revocation_row(
    conn: &mut SqliteConnection,
    revocation_id: &str,
) -> anyhow::Result<Option<RevocationRow>> {
    diesel::sql_query(
        "SELECT revocation_id, space_id, previous_epoch, next_epoch, status, \
         encrypted_record, encrypted_stage, created_at_ms, updated_at_ms \
         FROM member_revocation_log WHERE revocation_id = ?",
    )
    .bind::<Text, _>(revocation_id)
    .get_result(conn)
    .optional()
    .map_err(anyhow::Error::from)
}

fn decode_record(
    master_key: &MasterKey,
    row: &RevocationRow,
) -> Result<RevocationRecord, KeyEpochError> {
    let record: RevocationRecord = open(
        master_key,
        &row.encrypted_record,
        &record_aad(&row.revocation_id, &row.status),
    )?;
    let previous_epoch = epoch_to_i64(record.previous_epoch().value())?;
    let next_epoch = epoch_to_i64(record.next_epoch().value())?;
    if record.revocation_id().as_str() != row.revocation_id
        || record.space_id().as_ref() != row.space_id
        || previous_epoch != row.previous_epoch
        || next_epoch != row.next_epoch
        || status_name(record.status()) != row.status
        || record.created_at_ms() != row.created_at_ms
        || record.updated_at_ms() != row.updated_at_ms
    {
        return Err(backend("revocation row integrity mismatch"));
    }
    Ok(record)
}

fn save_space_material_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    material: &SpaceKeyMaterial,
) -> Result<(), KeyEpochError> {
    let state = material.state();
    let epoch = epoch_to_i64(state.epoch().value())?;
    let space_id = state.space_id().as_ref();
    let encrypted = seal(master_key, material, &space_aad(space_id, epoch))?;
    diesel::sql_query(
        "INSERT INTO space_key_epoch_state \
         (space_id, group_epoch, security_mode, current_content_key_id, encrypted_payload, updated_at_ms) \
         VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(space_id) DO UPDATE SET \
         group_epoch = excluded.group_epoch, security_mode = excluded.security_mode, \
         current_content_key_id = excluded.current_content_key_id, \
         encrypted_payload = excluded.encrypted_payload, updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(space_id)
    .bind::<BigInt, _>(epoch)
    .bind::<Text, _>(mode_name(state.mode()))
    .bind::<Text, _>(state.current_content_key_id().as_str())
    .bind::<Binary, _>(encrypted)
    .bind::<BigInt, _>(material.updated_at_ms())
    .execute(conn)
    .map_err(backend)?;
    Ok(())
}

fn decode_space_material(
    master_key: &MasterKey,
    row: SpaceMaterialRow,
) -> Result<SpaceKeyMaterial, KeyEpochError> {
    let material: SpaceKeyMaterial = open(
        master_key,
        &row.encrypted_payload,
        &space_aad(&row.space_id, row.group_epoch),
    )?;
    let state = material.state();
    if state.space_id().as_ref() != row.space_id
        || epoch_to_i64(state.epoch().value())? != row.group_epoch
        || mode_name(state.mode()) != row.security_mode
        || state.current_content_key_id().as_str() != row.current_content_key_id
        || material.updated_at_ms() != row.updated_at_ms
    {
        return Err(backend("space key material row integrity mismatch"));
    }
    Ok(material)
}

fn load_space_material_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &str,
) -> Result<Option<SpaceKeyMaterial>, KeyEpochError> {
    let row = diesel::sql_query(
        "SELECT space_id, group_epoch, security_mode, current_content_key_id, \
         encrypted_payload, updated_at_ms FROM space_key_epoch_state WHERE space_id = ?",
    )
    .bind::<Text, _>(space_id)
    .get_result::<SpaceMaterialRow>(conn)
    .optional()
    .map_err(backend)?;
    row.map(|row| decode_space_material(master_key, row))
        .transpose()
}

#[async_trait]
impl<E: DbExecutor> RevocationRepositoryPort for DieselRevocationRepository<E> {
    async fn save_space_material(&self, material: &SpaceKeyMaterial) -> Result<(), KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        self.executor
            .run(|conn| {
                save_space_material_on(conn, &master_key, material)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(backend)
    }

    async fn load_space_material(
        &self,
        space_id: &SpaceId,
    ) -> Result<Option<SpaceKeyMaterial>, KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let space_id = space_id.as_ref().to_owned();
        let row = self
            .executor
            .run(move |conn| {
                diesel::sql_query(
                    "SELECT space_id, group_epoch, security_mode, current_content_key_id, \
                     encrypted_payload, updated_at_ms FROM space_key_epoch_state WHERE space_id = ?",
                )
                .bind::<Text, _>(&space_id)
                .get_result::<SpaceMaterialRow>(conn)
                .optional()
                .map_err(anyhow::Error::from)
            })
            .map_err(backend)?;
        row.map(|row| decode_space_material(&master_key, row))
            .transpose()
    }

    async fn begin_revocation(
        &self,
        prepared: &RevocationRecord,
    ) -> Result<BeginRevocationOutcome, KeyEpochError> {
        if prepared.status() != RevocationStatus::Prepared {
            return Err(backend("begin revocation requires prepared status"));
        }
        let master_key = self.session.get_master_key().map_err(backend)?;
        let encrypted = seal(
            &master_key,
            prepared,
            &record_aad(
                prepared.revocation_id().as_str(),
                status_name(prepared.status()),
            ),
        )?;
        let prepared = prepared.clone();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let rows = diesel::sql_query(
                        "SELECT revocation_id, space_id, previous_epoch, next_epoch, status, \
                         encrypted_record, encrypted_stage, created_at_ms, updated_at_ms \
                         FROM member_revocation_log WHERE space_id = ? AND status <> 'complete'",
                    )
                    .bind::<Text, _>(prepared.space_id().as_ref())
                    .load::<RevocationRow>(conn)?;
                    let has_incomplete = !rows.is_empty();
                    for row in rows {
                        let existing = decode_record(&master_key, &row)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                        if existing.target_device_id() == prepared.target_device_id() {
                            return Ok(BeginRevocationOutcome::Existing(existing));
                        }
                    }
                    if has_incomplete {
                        return Err(anyhow::anyhow!(
                            "another member revocation is already in progress"
                        ));
                    }
                    diesel::sql_query(
                        "INSERT INTO member_revocation_log \
                         (revocation_id, space_id, previous_epoch, next_epoch, status, \
                          encrypted_record, encrypted_stage, created_at_ms, updated_at_ms) \
                         VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?)",
                    )
                    .bind::<Text, _>(prepared.revocation_id().as_str())
                    .bind::<Text, _>(prepared.space_id().as_ref())
                    .bind::<BigInt, _>(
                        epoch_to_i64(prepared.previous_epoch().value())
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                    )
                    .bind::<BigInt, _>(
                        epoch_to_i64(prepared.next_epoch().value())
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                    )
                    .bind::<Text, _>(status_name(prepared.status()))
                    .bind::<Binary, _>(&encrypted)
                    .bind::<BigInt, _>(prepared.created_at_ms())
                    .bind::<BigInt, _>(prepared.updated_at_ms())
                    .execute(conn)?;
                    Ok(BeginRevocationOutcome::Begun(prepared))
                })
            })
            .map_err(backend)
    }

    async fn get_revocation(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Option<RevocationRecord>, KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let revocation_id = revocation_id.as_str().to_owned();
        let row = self
            .executor
            .run(move |conn| load_revocation_row(conn, &revocation_id))
            .map_err(backend)?;
        row.map(|row| decode_record(&master_key, &row)).transpose()
    }

    async fn list_incomplete_revocations(&self) -> Result<Vec<RevocationRecord>, KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let rows = self
            .executor
            .run(|conn| {
                diesel::sql_query(
                    "SELECT revocation_id, space_id, previous_epoch, next_epoch, status, \
                     encrypted_record, encrypted_stage, created_at_ms, updated_at_ms \
                     FROM member_revocation_log WHERE status <> 'complete' ORDER BY created_at_ms",
                )
                .load::<RevocationRow>(conn)
                .map_err(anyhow::Error::from)
            })
            .map_err(backend)?;
        rows.iter()
            .map(|row| decode_record(&master_key, row))
            .collect()
    }

    async fn stage_revocation(&self, stage: &RevocationStage) -> Result<(), KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let record = stage.record();
        let encrypted_record = seal(
            &master_key,
            record,
            &record_aad(
                record.revocation_id().as_str(),
                status_name(record.status()),
            ),
        )?;
        let encrypted_stage = seal(
            &master_key,
            stage,
            &stage_aad(record.revocation_id().as_str()),
        )?;
        let affected = self
            .executor
            .run(|conn| {
                diesel::sql_query(
                    "UPDATE member_revocation_log SET status = ?, encrypted_record = ?, \
                     encrypted_stage = ?, updated_at_ms = ? \
                     WHERE revocation_id = ? AND status = 'prepared'",
                )
                .bind::<Text, _>(status_name(record.status()))
                .bind::<Binary, _>(encrypted_record)
                .bind::<Binary, _>(encrypted_stage)
                .bind::<BigInt, _>(record.updated_at_ms())
                .bind::<Text, _>(record.revocation_id().as_str())
                .execute(conn)
                .map_err(anyhow::Error::from)
            })
            .map_err(backend)?;
        if affected != 1 {
            return Err(backend("revocation is not prepared"));
        }
        Ok(())
    }

    async fn load_staged_revocation(
        &self,
        revocation_id: &RevocationId,
    ) -> Result<Option<RevocationStage>, KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let revocation_id_value = revocation_id.as_str().to_owned();
        let row = self
            .executor
            .run(move |conn| load_revocation_row(conn, &revocation_id_value))
            .map_err(backend)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let Some(encrypted_stage) = row.encrypted_stage else {
            return Ok(None);
        };
        let stage: RevocationStage = open(
            &master_key,
            &encrypted_stage,
            &stage_aad(revocation_id.as_str()),
        )?;
        if stage.record().revocation_id() != revocation_id {
            return Err(backend("staged revocation integrity mismatch"));
        }
        Ok(Some(stage))
    }

    async fn activate_revocation(
        &self,
        revocation_id: &RevocationId,
        now_ms: i64,
    ) -> Result<RevocationRecord, KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let revocation_id = revocation_id.as_str().to_owned();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let row = load_revocation_row(conn, &revocation_id)?
                        .ok_or_else(|| anyhow::anyhow!("revocation not found"))?;
                    let encrypted_stage = row
                        .encrypted_stage
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("revocation has no staged payload"))?;
                    let stage: RevocationStage =
                        open(&master_key, encrypted_stage, &stage_aad(&revocation_id))
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let mut record = decode_record(&master_key, &row)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    record
                        .transition_to(RevocationStatus::Activated, now_ms)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let mut stage = stage;
                    stage
                        .transition_to(RevocationStatus::Activated, now_ms)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let current_material = load_space_material_on(
                        conn,
                        &master_key,
                        stage.record().space_id().as_ref(),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?
                    .ok_or_else(|| anyhow::anyhow!("space key material not found"))?;
                    let material = SpaceKeyMaterial::new(
                        stage.next_space_state().clone(),
                        stage.group_state().to_vec(),
                        stage.key_catalog().to_vec(),
                        now_ms,
                    )
                    .with_pending_group_updates_from_excluding(
                        &current_material,
                        stage.record().target_device_id(),
                    );
                    save_space_material_on(conn, &master_key, &material)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_record = seal(
                        &master_key,
                        &record,
                        &record_aad(&revocation_id, status_name(record.status())),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_stage = seal(&master_key, &stage, &stage_aad(&revocation_id))
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let affected = diesel::sql_query(
                        "UPDATE member_revocation_log SET status = ?, encrypted_record = ?, \
                         encrypted_stage = ?, updated_at_ms = ? \
                         WHERE revocation_id = ? AND status = 'staged'",
                    )
                    .bind::<Text, _>(status_name(record.status()))
                    .bind::<Binary, _>(encrypted_record)
                    .bind::<Binary, _>(encrypted_stage)
                    .bind::<BigInt, _>(record.updated_at_ms())
                    .bind::<Text, _>(&revocation_id)
                    .execute(conn)?;
                    if affected != 1 {
                        return Err(anyhow::anyhow!("revocation is not staged"));
                    }
                    Ok(record)
                })
            })
            .map_err(backend)
    }

    async fn start_distribution(
        &self,
        revocation_id: &RevocationId,
        now_ms: i64,
    ) -> Result<RevocationRecord, KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let revocation_id = revocation_id.as_str().to_owned();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let row = load_revocation_row(conn, &revocation_id)?
                        .ok_or_else(|| anyhow::anyhow!("revocation not found"))?;
                    let mut record = decode_record(&master_key, &row)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    if matches!(
                        record.status(),
                        RevocationStatus::Distributing | RevocationStatus::Complete
                    ) {
                        return Ok(record);
                    }
                    if record.status() != RevocationStatus::Activated {
                        return Err(anyhow::anyhow!("revocation is not activated"));
                    }
                    let encrypted_stage = row
                        .encrypted_stage
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("revocation has no staged payload"))?;
                    let mut stage: RevocationStage =
                        open(&master_key, encrypted_stage, &stage_aad(&revocation_id))
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    record
                        .transition_to(RevocationStatus::Distributing, now_ms)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    stage
                        .transition_to(RevocationStatus::Distributing, now_ms)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    if stage.all_recipients_confirmed() {
                        record
                            .transition_to(RevocationStatus::Complete, now_ms)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                        stage
                            .transition_to(RevocationStatus::Complete, now_ms)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    }
                    let encrypted_record = seal(
                        &master_key,
                        &record,
                        &record_aad(&revocation_id, status_name(record.status())),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_stage = if record.status() == RevocationStatus::Complete {
                        None
                    } else {
                        Some(
                            seal(&master_key, &stage, &stage_aad(&revocation_id))
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                        )
                    };
                    let affected = diesel::sql_query(
                        "UPDATE member_revocation_log SET status = ?, encrypted_record = ?, \
                         encrypted_stage = ?, updated_at_ms = ? \
                         WHERE revocation_id = ? AND status = 'activated'",
                    )
                    .bind::<Text, _>(status_name(record.status()))
                    .bind::<Binary, _>(encrypted_record)
                    .bind::<Nullable<Binary>, _>(encrypted_stage)
                    .bind::<BigInt, _>(record.updated_at_ms())
                    .bind::<Text, _>(&revocation_id)
                    .execute(conn)?;
                    if affected != 1 {
                        return Err(anyhow::anyhow!("revocation distribution did not start"));
                    }
                    Ok(record)
                })
            })
            .map_err(backend)
    }

    async fn acknowledge_recipient(
        &self,
        revocation_id: &RevocationId,
        recipient: &DeviceId,
        now_ms: i64,
    ) -> Result<RevocationRecord, KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let revocation_id = revocation_id.as_str().to_owned();
        let recipient = recipient.clone();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let row = load_revocation_row(conn, &revocation_id)?
                        .ok_or_else(|| anyhow::anyhow!("revocation not found"))?;
                    let mut record = decode_record(&master_key, &row)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    if record.status() == RevocationStatus::Complete {
                        return Ok(record);
                    }
                    if record.status() != RevocationStatus::Distributing {
                        return Err(anyhow::anyhow!("revocation is not distributing"));
                    }
                    let encrypted_stage = row
                        .encrypted_stage
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("revocation has no staged payload"))?;
                    let mut stage: RevocationStage =
                        open(&master_key, encrypted_stage, &stage_aad(&revocation_id))
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    stage
                        .acknowledge_recipient(&recipient, now_ms)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    if stage.all_recipients_confirmed() {
                        record
                            .transition_to(RevocationStatus::Complete, now_ms)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                        stage
                            .transition_to(RevocationStatus::Complete, now_ms)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    }
                    let encrypted_record = seal(
                        &master_key,
                        &record,
                        &record_aad(&revocation_id, status_name(record.status())),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_stage = if record.status() == RevocationStatus::Complete {
                        None
                    } else {
                        Some(
                            seal(&master_key, &stage, &stage_aad(&revocation_id))
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                        )
                    };
                    let affected = diesel::sql_query(
                        "UPDATE member_revocation_log SET status = ?, encrypted_record = ?, \
                         encrypted_stage = ?, updated_at_ms = ? \
                         WHERE revocation_id = ? AND status = 'distributing'",
                    )
                    .bind::<Text, _>(status_name(record.status()))
                    .bind::<Binary, _>(encrypted_record)
                    .bind::<Nullable<Binary>, _>(encrypted_stage)
                    .bind::<BigInt, _>(record.updated_at_ms())
                    .bind::<Text, _>(&revocation_id)
                    .execute(conn)?;
                    if affected != 1 {
                        return Err(anyhow::anyhow!("revocation acknowledgement was not saved"));
                    }
                    Ok(record)
                })
            })
            .map_err(backend)
    }
}

#[cfg(test)]
mod tests {
    use diesel::prelude::*;
    use diesel::sql_types::{Binary, Nullable};
    use tempfile::{tempdir, TempDir};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        BeginRevocationOutcome, ContentKeyId, GroupEpoch, PendingGroupUpdate, RevocationId,
        RevocationOutboxMessage, RevocationRecord, RevocationRepositoryPort, RevocationStage,
        RevocationStatus, SpaceKeyMaterial, SpaceKeyState,
    };

    use super::DieselRevocationRepository;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::{init_db_pool, DbPool};
    use crate::security::{InMemorySession, MasterKey};

    fn make_repo() -> (
        DieselRevocationRepository<DieselSqliteExecutor>,
        DbPool,
        TempDir,
    ) {
        let tempdir = tempdir().unwrap();
        let database_url = tempdir.path().join("revocation.sqlite");
        let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
        let session = InMemorySession::new();
        session.set_master_key(MasterKey::from_bytes(&[0x5a; 32]).unwrap());
        let repo =
            DieselRevocationRepository::new(DieselSqliteExecutor::new(pool.clone()), session);
        (repo, pool, tempdir)
    }

    fn reopen_repo(pool: &DbPool) -> DieselRevocationRepository<DieselSqliteExecutor> {
        let session = InMemorySession::new();
        session.set_master_key(MasterKey::from_bytes(&[0x5a; 32]).unwrap());
        DieselRevocationRepository::new(DieselSqliteExecutor::new(pool.clone()), session)
    }

    fn ready_state() -> SpaceKeyState {
        let mut state = SpaceKeyState::legacy(SpaceId::from_str("space-sensitive"));
        state.mark_migrating().unwrap();
        state
            .mark_ready(ContentKeyId::from_string("content-key-current").unwrap())
            .unwrap();
        state
    }

    fn prepared(id: &str) -> RevocationRecord {
        RevocationRecord::prepare(
            RevocationId::from_string(id).unwrap(),
            SpaceId::from_str("space-sensitive"),
            DeviceId::new("removed-device-sensitive"),
            GroupEpoch::new(1),
            100,
        )
        .unwrap()
    }

    fn staged(mut record: RevocationRecord) -> RevocationStage {
        record.transition_to(RevocationStatus::Staged, 110).unwrap();
        let mut next_state = ready_state();
        next_state
            .rotate(ContentKeyId::from_string("content-key-next").unwrap())
            .unwrap();
        RevocationStage::new(
            record,
            next_state,
            b"group-state-sensitive".to_vec(),
            b"key-catalog-sensitive".to_vec(),
            vec![
                RevocationOutboxMessage::new(
                    DeviceId::new("retained-device-sensitive"),
                    b"commit-sensitive".to_vec(),
                ),
                RevocationOutboxMessage::new(
                    DeviceId::new("second-retained-device-sensitive"),
                    b"second-commit-sensitive".to_vec(),
                ),
            ],
        )
        .unwrap()
    }

    async fn seed_current_space(
        repo: &DieselRevocationRepository<DieselSqliteExecutor>,
    ) -> SpaceKeyMaterial {
        let material = SpaceKeyMaterial::new(
            ready_state(),
            b"old-group-state-sensitive".to_vec(),
            b"old-key-catalog-sensitive".to_vec(),
            90,
        );
        repo.save_space_material(&material).await.unwrap();
        material
    }

    #[tokio::test]
    async fn begin_is_idempotent_for_the_same_space_and_target() {
        let (repo, _pool, _tempdir) = make_repo();
        seed_current_space(&repo).await;
        let first = prepared("revocation-first");
        let duplicate = prepared("revocation-duplicate");

        assert_eq!(
            repo.begin_revocation(&first).await.unwrap(),
            BeginRevocationOutcome::Begun(first.clone())
        );
        assert_eq!(
            repo.begin_revocation(&duplicate).await.unwrap(),
            BeginRevocationOutcome::Existing(first)
        );
    }

    #[tokio::test]
    async fn begin_rejects_a_concurrent_revocation_for_another_member() {
        let (repo, _pool, _tempdir) = make_repo();
        seed_current_space(&repo).await;
        let first = prepared("revocation-first-target");
        repo.begin_revocation(&first).await.unwrap();
        let second = RevocationRecord::prepare(
            RevocationId::from_string("revocation-second-target").unwrap(),
            SpaceId::from_str("space-sensitive"),
            DeviceId::new("another-removed-device"),
            GroupEpoch::new(1),
            101,
        )
        .unwrap();

        assert!(repo.begin_revocation(&second).await.is_err());
    }

    #[derive(QueryableByName)]
    struct RawCiphertexts {
        #[diesel(sql_type = Binary)]
        encrypted_record: Vec<u8>,
        #[diesel(sql_type = Nullable<Binary>)]
        encrypted_stage: Option<Vec<u8>>,
    }

    #[derive(QueryableByName)]
    struct RawSpaceCiphertext {
        #[diesel(sql_type = Binary)]
        encrypted_payload: Vec<u8>,
    }

    #[tokio::test]
    async fn all_sensitive_revocation_payloads_are_ciphertext_at_rest() {
        let (repo, pool, _tempdir) = make_repo();
        seed_current_space(&repo).await;
        let prepared = prepared("revocation-encrypted");
        repo.begin_revocation(&prepared).await.unwrap();
        repo.stage_revocation(&staged(prepared)).await.unwrap();

        let mut conn = pool.get().unwrap();
        let row = diesel::sql_query(
            "SELECT encrypted_record, encrypted_stage FROM member_revocation_log LIMIT 1",
        )
        .get_result::<RawCiphertexts>(&mut conn)
        .unwrap();
        let space =
            diesel::sql_query("SELECT encrypted_payload FROM space_key_epoch_state LIMIT 1")
                .get_result::<RawSpaceCiphertext>(&mut conn)
                .unwrap();
        let mut persisted = row.encrypted_record;
        persisted.extend(row.encrypted_stage.unwrap());
        persisted.extend(space.encrypted_payload);

        for plaintext in [
            "removed-device-sensitive",
            "retained-device-sensitive",
            "group-state-sensitive",
            "key-catalog-sensitive",
            "commit-sensitive",
            "old-group-state-sensitive",
            "old-key-catalog-sensitive",
        ] {
            assert!(
                !persisted
                    .windows(plaintext.len())
                    .any(|window| window == plaintext.as_bytes()),
                "plaintext leaked into database: {plaintext}"
            );
        }
    }

    #[tokio::test]
    async fn pending_group_update_is_encrypted_and_survives_restart_until_acknowledged() {
        let (repo, pool, _tempdir) = make_repo();
        let mut material = seed_current_space(&repo).await;
        let update = PendingGroupUpdate::persistent(
            DeviceId::new("pending-recipient-sensitive"),
            b"pending-commit-sensitive".to_vec(),
        );
        let update_id = update.update_id().to_string();
        material.add_pending_group_updates([update.clone()], 100);
        repo.save_space_material(&material).await.unwrap();

        let mut conn = pool.get().unwrap();
        let row = diesel::sql_query(
            "SELECT encrypted_payload FROM space_key_epoch_state WHERE space_id = 'space-sensitive'",
        )
        .get_result::<RawSpaceCiphertext>(&mut conn)
        .unwrap();
        drop(conn);
        for plaintext in ["pending-recipient-sensitive", "pending-commit-sensitive"] {
            assert!(
                !row.encrypted_payload
                    .windows(plaintext.len())
                    .any(|window| window == plaintext.as_bytes()),
                "plaintext leaked into database: {plaintext}"
            );
        }

        drop(repo);
        let reopened = reopen_repo(&pool);
        let mut restored = reopened
            .load_space_material(&SpaceId::from_str("space-sensitive"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.pending_group_updates(), &[update]);

        assert!(restored.acknowledge_group_update(&update_id, 110));
        reopened.save_space_material(&restored).await.unwrap();
        assert!(reopen_repo(&pool)
            .load_space_material(&SpaceId::from_str("space-sensitive"))
            .await
            .unwrap()
            .unwrap()
            .pending_group_updates()
            .is_empty());
    }

    #[tokio::test]
    async fn activation_atomically_publishes_the_staged_space() {
        let (repo, _pool, _tempdir) = make_repo();
        seed_current_space(&repo).await;
        let prepared = prepared("revocation-activate");
        repo.begin_revocation(&prepared).await.unwrap();
        let stage = staged(prepared);
        repo.stage_revocation(&stage).await.unwrap();

        let activated = repo
            .activate_revocation(stage.record().revocation_id(), 120)
            .await
            .unwrap();
        let loaded = repo
            .load_space_material(stage.record().space_id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(activated.status(), RevocationStatus::Activated);
        assert_eq!(loaded.state(), stage.next_space_state());
        assert_eq!(loaded.group_state(), stage.group_state());
        assert_eq!(loaded.key_catalog(), stage.key_catalog());
    }

    #[tokio::test]
    async fn activation_preserves_pending_admission_updates() {
        let (repo, _pool, _tempdir) = make_repo();
        let mut current = seed_current_space(&repo).await;
        let pending = PendingGroupUpdate::persistent(
            DeviceId::new("offline-retained-member"),
            b"pending-admission-update".to_vec(),
        );
        let removed_member_pending = PendingGroupUpdate::persistent(
            DeviceId::new("removed-device-sensitive"),
            b"obsolete-target-update".to_vec(),
        );
        current.add_pending_group_updates([pending.clone(), removed_member_pending], 100);
        repo.save_space_material(&current).await.unwrap();
        let prepared = prepared("revocation-preserve-admission");
        repo.begin_revocation(&prepared).await.unwrap();
        let stage = staged(prepared);
        repo.stage_revocation(&stage).await.unwrap();

        repo.activate_revocation(stage.record().revocation_id(), 120)
            .await
            .unwrap();

        assert_eq!(
            repo.load_space_material(stage.record().space_id())
                .await
                .unwrap()
                .unwrap()
                .pending_group_updates(),
            &[pending]
        );
    }

    #[tokio::test]
    async fn staged_revocation_survives_repository_restart() {
        let (repo, pool, _tempdir) = make_repo();
        seed_current_space(&repo).await;
        let prepared = prepared("revocation-restart-stage");
        repo.begin_revocation(&prepared).await.unwrap();
        let stage = staged(prepared);
        repo.stage_revocation(&stage).await.unwrap();
        drop(repo);

        let reopened = reopen_repo(&pool);
        assert_eq!(
            reopened
                .load_staged_revocation(stage.record().revocation_id())
                .await
                .unwrap(),
            Some(stage)
        );
    }

    #[tokio::test]
    async fn incomplete_revocations_are_discovered_after_restart() {
        let (repo, pool, _tempdir) = make_repo();
        seed_current_space(&repo).await;
        let prepared = prepared("revocation-restart-list");
        repo.begin_revocation(&prepared).await.unwrap();
        let stage = staged(prepared);
        repo.stage_revocation(&stage).await.unwrap();
        drop(repo);

        let reopened = reopen_repo(&pool);
        assert_eq!(
            reopened.list_incomplete_revocations().await.unwrap(),
            vec![stage.record().clone()]
        );
    }

    #[tokio::test]
    async fn failed_activation_rolls_back_epoch_and_revocation_status() {
        let (repo, pool, _tempdir) = make_repo();
        let original = seed_current_space(&repo).await;
        let prepared = prepared("revocation-rollback");
        repo.begin_revocation(&prepared).await.unwrap();
        let stage = staged(prepared);
        repo.stage_revocation(&stage).await.unwrap();
        let mut conn = pool.get().unwrap();
        diesel::sql_query(
            "CREATE TRIGGER fail_space_activation BEFORE UPDATE ON space_key_epoch_state \
             BEGIN SELECT RAISE(ABORT, 'forced activation failure'); END",
        )
        .execute(&mut conn)
        .unwrap();
        drop(conn);

        assert!(repo
            .activate_revocation(stage.record().revocation_id(), 120)
            .await
            .is_err());
        assert_eq!(
            repo.get_revocation(stage.record().revocation_id())
                .await
                .unwrap()
                .unwrap()
                .status(),
            RevocationStatus::Staged
        );
        assert_eq!(
            repo.load_space_material(stage.record().space_id())
                .await
                .unwrap()
                .unwrap(),
            original
        );
    }

    #[tokio::test]
    async fn distribution_progress_survives_restart_and_completes_after_all_confirmations() {
        let (repo, pool, _tempdir) = make_repo();
        seed_current_space(&repo).await;
        let prepared = prepared("revocation-distribution");
        repo.begin_revocation(&prepared).await.unwrap();
        let stage = staged(prepared);
        let revocation_id = stage.record().revocation_id().clone();
        repo.stage_revocation(&stage).await.unwrap();
        repo.activate_revocation(&revocation_id, 120).await.unwrap();

        let distributing = repo.start_distribution(&revocation_id, 130).await.unwrap();
        assert_eq!(distributing.status(), RevocationStatus::Distributing);
        let waiting = repo
            .acknowledge_recipient(
                &revocation_id,
                &DeviceId::new("retained-device-sensitive"),
                140,
            )
            .await
            .unwrap();
        assert_eq!(waiting.status(), RevocationStatus::Distributing);
        drop(repo);

        let reopened = reopen_repo(&pool);
        let resumed = reopened
            .load_staged_revocation(&revocation_id)
            .await
            .unwrap()
            .unwrap();
        assert!(resumed.outbox()[0].is_confirmed());
        assert!(!resumed.outbox()[1].is_confirmed());

        let complete = reopened
            .acknowledge_recipient(
                &revocation_id,
                &DeviceId::new("second-retained-device-sensitive"),
                150,
            )
            .await
            .unwrap();
        assert_eq!(complete.status(), RevocationStatus::Complete);
        assert!(reopened
            .load_staged_revocation(&revocation_id)
            .await
            .unwrap()
            .is_none());
        assert!(reopened
            .list_incomplete_revocations()
            .await
            .unwrap()
            .is_empty());
    }
}
