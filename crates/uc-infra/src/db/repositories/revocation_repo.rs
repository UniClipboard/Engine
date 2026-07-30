use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};
use hmac::{Hmac, Mac};
use serde::{de::DeserializeOwned, Serialize};
use sha2::Sha256;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    BeginRevocationOutcome, BootstrapError, BootstrapId, KeyEpochError, LegacyBootstrapRecord,
    LegacyBootstrapRepositoryPort, LegacyBootstrapStage, LegacyBootstrapStatus, RevocationId,
    RevocationRecord, RevocationRepositoryPort, RevocationStage, RevocationStatus,
    SpaceKeyMaterial, SpaceSecurityMode,
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
    space_lookup_token: String,
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
struct CountRow {
    #[diesel(sql_type = BigInt)]
    count: i64,
}

#[derive(QueryableByName)]
struct LegacyBootstrapRow {
    #[diesel(sql_type = Text)]
    bootstrap_id: String,
    #[diesel(sql_type = Text)]
    space_lookup_token: String,
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
    space_lookup_token: String,
    #[diesel(sql_type = BigInt)]
    group_epoch: i64,
    #[diesel(sql_type = Text)]
    security_mode: String,
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

fn bootstrap_status_name(status: LegacyBootstrapStatus) -> &'static str {
    match status {
        LegacyBootstrapStatus::Prepared => "prepared",
        LegacyBootstrapStatus::Staged => "staged",
        LegacyBootstrapStatus::AwaitingReadmission => "awaiting_readmission",
        LegacyBootstrapStatus::Complete => "complete",
        LegacyBootstrapStatus::RecoveryRequired => "recovery_required",
    }
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

fn bootstrap_record_aad(bootstrap_id: &str, status: &str) -> Vec<u8> {
    format!("uc-legacy-bootstrap-record-v1|{bootstrap_id}|{status}").into_bytes()
}

fn bootstrap_stage_aad(bootstrap_id: &str) -> Vec<u8> {
    format!("uc-legacy-bootstrap-stage-v1|{bootstrap_id}").into_bytes()
}

fn space_aad(space_id: &str, epoch: i64) -> Vec<u8> {
    format!("uc-space-key-material-v1|{space_id}|{epoch}").into_bytes()
}

fn space_lookup_token(master_key: &MasterKey, space_id: &SpaceId) -> Result<String, KeyEpochError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(master_key.as_bytes()).map_err(backend)?;
    mac.update(b"uc-space-lookup-v1|");
    mac.update(&(space_id.as_ref().len() as u64).to_be_bytes());
    mac.update(space_id.as_ref().as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
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
        "SELECT revocation_id, space_lookup_token, previous_epoch, next_epoch, status, \
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
        || space_lookup_token(master_key, record.space_id())? != row.space_lookup_token
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

fn load_bootstrap_row(
    conn: &mut SqliteConnection,
    bootstrap_id: &str,
) -> anyhow::Result<Option<LegacyBootstrapRow>> {
    diesel::sql_query(
        "SELECT bootstrap_id, space_lookup_token, previous_epoch, next_epoch, status, \
         encrypted_record, encrypted_stage, created_at_ms, updated_at_ms \
         FROM legacy_space_bootstrap_log WHERE bootstrap_id = ?",
    )
    .bind::<Text, _>(bootstrap_id)
    .get_result(conn)
    .optional()
    .map_err(anyhow::Error::from)
}

fn decode_bootstrap_record(
    master_key: &MasterKey,
    row: &LegacyBootstrapRow,
) -> Result<LegacyBootstrapRecord, BootstrapError> {
    let record: LegacyBootstrapRecord = open(
        master_key,
        &row.encrypted_record,
        &bootstrap_record_aad(&row.bootstrap_id, &row.status),
    )
    .map_err(|error| BootstrapError::Repository(error.to_string()))?;
    if record.bootstrap_id().as_str() != row.bootstrap_id
        || space_lookup_token(master_key, record.space_id())
            .map_err(|error| BootstrapError::Repository(error.to_string()))?
            != row.space_lookup_token
        || epoch_to_i64(record.previous_epoch().value())
            .map_err(|error| BootstrapError::Repository(error.to_string()))?
            != row.previous_epoch
        || epoch_to_i64(record.next_epoch().value())
            .map_err(|error| BootstrapError::Repository(error.to_string()))?
            != row.next_epoch
        || bootstrap_status_name(record.status()) != row.status
        || record.created_at_ms() != row.created_at_ms
        || record.updated_at_ms() != row.updated_at_ms
    {
        return Err(BootstrapError::Repository(
            "legacy bootstrap row integrity mismatch".into(),
        ));
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
    let lookup_token = space_lookup_token(master_key, state.space_id())?;
    let encrypted = seal(master_key, material, &space_aad(space_id, epoch))?;
    diesel::sql_query(
        "INSERT INTO space_key_epoch_state \
         (space_lookup_token, group_epoch, security_mode, encrypted_payload, updated_at_ms) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(space_lookup_token) DO UPDATE SET \
         group_epoch = excluded.group_epoch, security_mode = excluded.security_mode, \
         encrypted_payload = excluded.encrypted_payload, updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(lookup_token)
    .bind::<BigInt, _>(epoch)
    .bind::<Text, _>(mode_name(state.mode()))
    .bind::<Binary, _>(encrypted)
    .bind::<BigInt, _>(material.updated_at_ms())
    .execute(conn)
    .map_err(backend)?;
    Ok(())
}

fn decode_space_material(
    master_key: &MasterKey,
    row: SpaceMaterialRow,
    expected_space_id: &SpaceId,
) -> Result<SpaceKeyMaterial, KeyEpochError> {
    let material: SpaceKeyMaterial = open(
        master_key,
        &row.encrypted_payload,
        &space_aad(expected_space_id.as_ref(), row.group_epoch),
    )?;
    let state = material.state();
    if state.space_id() != expected_space_id
        || space_lookup_token(master_key, state.space_id())? != row.space_lookup_token
        || epoch_to_i64(state.epoch().value())? != row.group_epoch
        || mode_name(state.mode()) != row.security_mode
        || material.updated_at_ms() != row.updated_at_ms
    {
        return Err(backend("space key material row integrity mismatch"));
    }
    Ok(material)
}

fn load_space_material_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
) -> Result<Option<SpaceKeyMaterial>, KeyEpochError> {
    let lookup_token = space_lookup_token(master_key, space_id)?;
    let row = diesel::sql_query(
        "SELECT space_lookup_token, group_epoch, security_mode, \
         encrypted_payload, updated_at_ms FROM space_key_epoch_state WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<SpaceMaterialRow>(conn)
    .optional()
    .map_err(backend)?;
    row.map(|row| decode_space_material(master_key, row, space_id))
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
        let space_id = space_id.clone();
        self.executor
            .run(move |conn| {
                load_space_material_on(conn, &master_key, &space_id)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(backend)
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
        let lookup_token = space_lookup_token(&master_key, prepared.space_id())?;
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let rows = diesel::sql_query(
                        "SELECT revocation_id, space_lookup_token, previous_epoch, next_epoch, status, \
                         encrypted_record, encrypted_stage, created_at_ms, updated_at_ms \
                         FROM member_revocation_log WHERE space_lookup_token = ? AND status <> 'complete'",
                    )
                    .bind::<Text, _>(&lookup_token)
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
                         (revocation_id, space_lookup_token, previous_epoch, next_epoch, status, \
                          encrypted_record, encrypted_stage, created_at_ms, updated_at_ms) \
                         VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?)",
                    )
                    .bind::<Text, _>(prepared.revocation_id().as_str())
                    .bind::<Text, _>(&lookup_token)
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
                    "SELECT revocation_id, space_lookup_token, previous_epoch, next_epoch, status, \
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
                    let current_material =
                        load_space_material_on(conn, &master_key, stage.record().space_id())
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

#[async_trait]
impl<E: DbExecutor> LegacyBootstrapRepositoryPort for DieselRevocationRepository<E> {
    async fn begin_legacy_bootstrap(
        &self,
        prepared: &LegacyBootstrapRecord,
    ) -> Result<LegacyBootstrapRecord, BootstrapError> {
        if prepared.status() != LegacyBootstrapStatus::Prepared {
            return Err(BootstrapError::InvalidRecord);
        }
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let prepared = prepared.clone();
        let lookup_token = space_lookup_token(&master_key, prepared.space_id())
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let encrypted = seal(
            &master_key,
            &prepared,
            &bootstrap_record_aad(
                prepared.bootstrap_id().as_str(),
                bootstrap_status_name(prepared.status()),
            ),
        )
        .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let existing = diesel::sql_query(
                        "SELECT bootstrap_id, space_lookup_token, previous_epoch, next_epoch, status, \
                         encrypted_record, encrypted_stage, created_at_ms, updated_at_ms \
                         FROM legacy_space_bootstrap_log \
                         WHERE space_lookup_token = ? AND status NOT IN ('complete', 'recovery_required')",
                    )
                    .bind::<Text, _>(&lookup_token)
                    .get_result::<LegacyBootstrapRow>(conn)
                    .optional()?;
                    if let Some(row) = existing {
                        return decode_bootstrap_record(&master_key, &row)
                            .map_err(|error| anyhow::anyhow!(error.to_string()));
                    }
                    let revocation_in_progress: i64 = diesel::sql_query(
                        "SELECT COUNT(*) AS count FROM member_revocation_log \
                         WHERE space_lookup_token = ? AND status <> 'complete'",
                    )
                    .bind::<Text, _>(&lookup_token)
                    .get_result::<CountRow>(conn)?
                    .count;
                    if revocation_in_progress != 0 {
                        return Err(anyhow::anyhow!(
                            "member revocation is already in progress for this space"
                        ));
                    }
                    let material_exists: i64 = diesel::sql_query(
                        "SELECT COUNT(*) AS count FROM space_key_epoch_state WHERE space_lookup_token = ?",
                    )
                    .bind::<Text, _>(&lookup_token)
                    .get_result::<CountRow>(conn)?
                    .count;
                    if material_exists != 0 {
                        return Err(anyhow::anyhow!(
                            "space already has key epoch material"
                        ));
                    }
                    diesel::sql_query(
                        "INSERT INTO legacy_space_bootstrap_log \
                         (bootstrap_id, space_lookup_token, previous_epoch, next_epoch, status, \
                          encrypted_record, encrypted_stage, created_at_ms, updated_at_ms) \
                         VALUES (?, ?, ?, ?, ?, ?, NULL, ?, ?)",
                    )
                    .bind::<Text, _>(prepared.bootstrap_id().as_str())
                    .bind::<Text, _>(&lookup_token)
                    .bind::<BigInt, _>(0_i64)
                    .bind::<BigInt, _>(1_i64)
                    .bind::<Text, _>(bootstrap_status_name(prepared.status()))
                    .bind::<Binary, _>(&encrypted)
                    .bind::<BigInt, _>(prepared.created_at_ms())
                    .bind::<BigInt, _>(prepared.updated_at_ms())
                    .execute(conn)?;
                    Ok(prepared)
                })
            })
            .map_err(|error| BootstrapError::Repository(error.to_string()))
    }

    async fn stage_legacy_bootstrap(
        &self,
        stage: &LegacyBootstrapStage,
    ) -> Result<(), BootstrapError> {
        let record = stage.record().clone();
        if record.status() != LegacyBootstrapStatus::Staged {
            return Err(BootstrapError::InvalidStage);
        }
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let bootstrap_id = record.bootstrap_id().as_str().to_owned();
        let encrypted_record = seal(
            &master_key,
            &record,
            &bootstrap_record_aad(&bootstrap_id, bootstrap_status_name(record.status())),
        )
        .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let encrypted_stage = seal(&master_key, stage, &bootstrap_stage_aad(&bootstrap_id))
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        self.executor
            .run(move |conn| {
                let affected = diesel::sql_query(
                    "UPDATE legacy_space_bootstrap_log SET status = ?, encrypted_record = ?, \
                     encrypted_stage = ?, updated_at_ms = ? \
                     WHERE bootstrap_id = ? AND status IN ('prepared', 'staged')",
                )
                .bind::<Text, _>(bootstrap_status_name(record.status()))
                .bind::<Binary, _>(encrypted_record)
                .bind::<Binary, _>(encrypted_stage)
                .bind::<BigInt, _>(record.updated_at_ms())
                .bind::<Text, _>(bootstrap_id)
                .execute(conn)?;
                if affected != 1 {
                    return Err(anyhow::anyhow!("legacy bootstrap cannot be staged"));
                }
                Ok(())
            })
            .map_err(|error| BootstrapError::Repository(error.to_string()))
    }

    async fn activate_legacy_bootstrap(
        &self,
        bootstrap_id: &BootstrapId,
        now_ms: i64,
    ) -> Result<LegacyBootstrapRecord, BootstrapError> {
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let bootstrap_id = bootstrap_id.as_str().to_owned();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let row = load_bootstrap_row(conn, &bootstrap_id)?
                        .ok_or_else(|| anyhow::anyhow!("legacy bootstrap not found"))?;
                    let mut record = decode_bootstrap_record(&master_key, &row)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    if matches!(
                        record.status(),
                        LegacyBootstrapStatus::AwaitingReadmission
                            | LegacyBootstrapStatus::Complete
                    ) {
                        return Ok(record);
                    }
                    if record.status() != LegacyBootstrapStatus::Staged {
                        return Err(anyhow::anyhow!("legacy bootstrap is not staged"));
                    }
                    let encrypted_stage = row
                        .encrypted_stage
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("legacy bootstrap has no staged payload"))?;
                    let stage: LegacyBootstrapStage = open(
                        &master_key,
                        encrypted_stage,
                        &bootstrap_stage_aad(&bootstrap_id),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    if stage.record().bootstrap_id() != record.bootstrap_id()
                        || stage.material().state().space_id() != record.space_id()
                    {
                        return Err(anyhow::anyhow!("legacy bootstrap stage integrity mismatch"));
                    }
                    let next_status = if record.pending_readmission().is_empty() {
                        LegacyBootstrapStatus::Complete
                    } else {
                        LegacyBootstrapStatus::AwaitingReadmission
                    };
                    record
                        .transition_to(next_status, now_ms)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    save_space_material_on(conn, &master_key, stage.material())
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_record = seal(
                        &master_key,
                        &record,
                        &bootstrap_record_aad(
                            &bootstrap_id,
                            bootstrap_status_name(record.status()),
                        ),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_stage = if record.status().is_terminal() {
                        None
                    } else {
                        Some(
                            seal(&master_key, &stage, &bootstrap_stage_aad(&bootstrap_id))
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?,
                        )
                    };
                    let affected = diesel::sql_query(
                        "UPDATE legacy_space_bootstrap_log SET status = ?, encrypted_record = ?, \
                         encrypted_stage = ?, updated_at_ms = ? \
                         WHERE bootstrap_id = ? AND status = 'staged'",
                    )
                    .bind::<Text, _>(bootstrap_status_name(record.status()))
                    .bind::<Binary, _>(encrypted_record)
                    .bind::<Nullable<Binary>, _>(encrypted_stage)
                    .bind::<BigInt, _>(record.updated_at_ms())
                    .bind::<Text, _>(&bootstrap_id)
                    .execute(conn)?;
                    if affected != 1 {
                        return Err(anyhow::anyhow!("legacy bootstrap activation was not saved"));
                    }
                    Ok(record)
                })
            })
            .map_err(|error| BootstrapError::Repository(error.to_string()))
    }

    async fn get_legacy_bootstrap(
        &self,
        bootstrap_id: &BootstrapId,
    ) -> Result<Option<LegacyBootstrapRecord>, BootstrapError> {
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let bootstrap_id = bootstrap_id.as_str().to_owned();
        self.executor
            .run(move |conn| {
                load_bootstrap_row(conn, &bootstrap_id)?
                    .map(|row| decode_bootstrap_record(&master_key, &row))
                    .transpose()
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(|error| BootstrapError::Repository(error.to_string()))
    }

    async fn list_incomplete_legacy_bootstraps(
        &self,
    ) -> Result<Vec<LegacyBootstrapRecord>, BootstrapError> {
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        self.executor
            .run(move |conn| {
                let rows = diesel::sql_query(
                    "SELECT bootstrap_id, space_lookup_token, previous_epoch, next_epoch, status, \
                     encrypted_record, encrypted_stage, created_at_ms, updated_at_ms \
                     FROM legacy_space_bootstrap_log \
                     WHERE status NOT IN ('complete', 'recovery_required')",
                )
                .load::<LegacyBootstrapRow>(conn)?;
                rows.iter()
                    .map(|row| {
                        decode_bootstrap_record(&master_key, row)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))
                    })
                    .collect()
            })
            .map_err(|error| BootstrapError::Repository(error.to_string()))
    }

    async fn acknowledge_legacy_readmission(
        &self,
        bootstrap_id: &BootstrapId,
        member: &DeviceId,
        now_ms: i64,
    ) -> Result<LegacyBootstrapRecord, BootstrapError> {
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let bootstrap_id = bootstrap_id.as_str().to_owned();
        let member = member.clone();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let row = load_bootstrap_row(conn, &bootstrap_id)?
                        .ok_or_else(|| anyhow::anyhow!("legacy bootstrap not found"))?;
                    let mut record = decode_bootstrap_record(&master_key, &row)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    if record.status() == LegacyBootstrapStatus::Complete {
                        return Ok(record);
                    }
                    record
                        .mark_readmitted(&member, now_ms)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_record = seal(
                        &master_key,
                        &record,
                        &bootstrap_record_aad(
                            &bootstrap_id,
                            bootstrap_status_name(record.status()),
                        ),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_stage = if record.status().is_terminal() {
                        None
                    } else {
                        row.encrypted_stage
                    };
                    let affected = diesel::sql_query(
                        "UPDATE legacy_space_bootstrap_log SET status = ?, encrypted_record = ?, \
                         encrypted_stage = ?, updated_at_ms = ? \
                         WHERE bootstrap_id = ? AND status = 'awaiting_readmission'",
                    )
                    .bind::<Text, _>(bootstrap_status_name(record.status()))
                    .bind::<Binary, _>(encrypted_record)
                    .bind::<Nullable<Binary>, _>(encrypted_stage)
                    .bind::<BigInt, _>(record.updated_at_ms())
                    .bind::<Text, _>(&bootstrap_id)
                    .execute(conn)?;
                    if affected != 1 {
                        return Err(anyhow::anyhow!("legacy readmission was not saved"));
                    }
                    Ok(record)
                })
            })
            .map_err(|error| BootstrapError::Repository(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use diesel::prelude::*;
    use diesel::sql_types::{Binary, Nullable, Text};
    use tempfile::{tempdir, TempDir};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        BeginRevocationOutcome, BootstrapId, ContentKeyId, GroupEpoch, LegacyBootstrapRecord,
        LegacyBootstrapRepositoryPort, LegacyBootstrapStage, LegacyBootstrapStatus,
        PendingGroupUpdate, RevocationId, RevocationOutboxMessage, RevocationRecord,
        RevocationRepositoryPort, RevocationStage, RevocationStatus, SpaceKeyMaterial,
        SpaceKeyState,
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

    fn staged_legacy_bootstrap() -> LegacyBootstrapStage {
        let mut record = LegacyBootstrapRecord::prepare(
            BootstrapId::from_string("bootstrap-sensitive").unwrap(),
            SpaceId::from_str("space-sensitive"),
            DeviceId::new("sponsor-sensitive"),
            vec![DeviceId::new("retained-device-sensitive")],
            100,
        )
        .unwrap();
        record
            .transition_to(LegacyBootstrapStatus::Staged, 110)
            .unwrap();
        LegacyBootstrapStage::new(
            record,
            SpaceKeyMaterial::new(
                ready_state(),
                b"mls-group-state-sensitive".to_vec(),
                b"key-catalog-sensitive".to_vec(),
                110,
            ),
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
        #[diesel(sql_type = Text)]
        space_lookup_token: String,
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
        let space = diesel::sql_query(
            "SELECT space_lookup_token, encrypted_payload FROM space_key_epoch_state LIMIT 1",
        )
        .get_result::<RawSpaceCiphertext>(&mut conn)
        .unwrap();
        let mut persisted = row.encrypted_record;
        persisted.extend(row.encrypted_stage.unwrap());
        persisted.extend(space.space_lookup_token.as_bytes());
        persisted.extend(space.encrypted_payload);

        for plaintext in [
            "space-sensitive",
            "content-key-current",
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
            "SELECT space_lookup_token, encrypted_payload FROM space_key_epoch_state LIMIT 1",
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
    async fn legacy_bootstrap_activation_is_atomic_and_waits_for_readmission() {
        let (repo, pool, _tempdir) = make_repo();
        let stage = staged_legacy_bootstrap();
        let bootstrap_id = stage.record().bootstrap_id().clone();

        assert_eq!(
            repo.begin_legacy_bootstrap(
                &LegacyBootstrapRecord::prepare(
                    bootstrap_id.clone(),
                    SpaceId::from_str("space-sensitive"),
                    DeviceId::new("sponsor-sensitive"),
                    vec![DeviceId::new("retained-device-sensitive")],
                    100,
                )
                .unwrap()
            )
            .await
            .unwrap()
            .status(),
            LegacyBootstrapStatus::Prepared
        );
        repo.stage_legacy_bootstrap(&stage).await.unwrap();

        let mut conn = pool.get().unwrap();
        let row = diesel::sql_query(
            "SELECT encrypted_record, encrypted_stage FROM legacy_space_bootstrap_log LIMIT 1",
        )
        .get_result::<RawCiphertexts>(&mut conn)
        .unwrap();
        let mut persisted = row.encrypted_record;
        persisted.extend(row.encrypted_stage.unwrap());
        for plaintext in [
            "space-sensitive",
            "sponsor-sensitive",
            "retained-device-sensitive",
            "mls-group-state-sensitive",
            "key-catalog-sensitive",
        ] {
            assert!(
                !persisted
                    .windows(plaintext.len())
                    .any(|window| window == plaintext.as_bytes()),
                "plaintext leaked into bootstrap database row: {plaintext}"
            );
        }
        drop(conn);

        let activated = repo
            .activate_legacy_bootstrap(&bootstrap_id, 120)
            .await
            .unwrap();
        assert_eq!(
            activated.status(),
            LegacyBootstrapStatus::AwaitingReadmission
        );
        assert_eq!(
            repo.load_space_material(&SpaceId::from_str("space-sensitive"))
                .await
                .unwrap()
                .unwrap(),
            stage.material().clone()
        );
        assert_eq!(
            repo.list_incomplete_legacy_bootstraps().await.unwrap(),
            vec![activated.clone()]
        );

        let completed = repo
            .acknowledge_legacy_readmission(
                &bootstrap_id,
                &DeviceId::new("retained-device-sensitive"),
                130,
            )
            .await
            .unwrap();
        assert_eq!(completed.status(), LegacyBootstrapStatus::Complete);
        assert!(repo
            .list_incomplete_legacy_bootstraps()
            .await
            .unwrap()
            .is_empty());
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
