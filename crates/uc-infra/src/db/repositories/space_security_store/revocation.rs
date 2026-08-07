use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    BeginRevocationOutcome, KeyEpochError, PreparedRevocationResolution, RevocationId,
    RevocationRecord, RevocationRepositoryPort, RevocationStage, RevocationStatus,
    SpaceKeyMaterial, SpaceSecurityStateResetError, SpaceSecurityStateResetPort,
};

use crate::db::ports::DbExecutor;
use crate::security::MasterKey;

use super::encrypted_payload::{open, seal, space_lookup_token};
use super::space_material::{load_space_material_on, save_space_material_on};
use super::{backend, epoch_to_i64, DieselSpaceSecurityStore};

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

fn record_aad(revocation_id: &str, status: &str) -> Vec<u8> {
    format!("uc-revocation-record-v1|{revocation_id}|{status}").into_bytes()
}

fn stage_aad(revocation_id: &str) -> Vec<u8> {
    format!("uc-revocation-stage-v1|{revocation_id}").into_bytes()
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
        return Err(KeyEpochError::PersistedStateIntegrityFailed);
    }
    Ok(record)
}

#[async_trait]
impl<E: DbExecutor> RevocationRepositoryPort for DieselSpaceSecurityStore<E> {
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
                    let mut has_incomplete = false;
                    for row in rows {
                        let existing = decode_record(&master_key, &row)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                        if existing.status() == RevocationStatus::Prepared
                            && existing.previous_epoch() < prepared.previous_epoch()
                        {
                            let affected = diesel::sql_query(
                                "DELETE FROM member_revocation_log \
                                 WHERE revocation_id = ? AND status = 'prepared' \
                                 AND previous_epoch = ?",
                            )
                            .bind::<Text, _>(existing.revocation_id().as_str())
                            .bind::<BigInt, _>(epoch_to_i64(existing.previous_epoch().value())?)
                            .execute(conn)?;
                            if affected != 1 {
                                return Err(anyhow::anyhow!(
                                    "obsolete prepared revocation could not be replaced"
                                ));
                            }
                            tracing::warn!(
                                event = "member_revocation.obsolete_prepared_replaced",
                                previous_epoch = existing.previous_epoch().value(),
                                current_epoch = prepared.previous_epoch().value(),
                                "obsolete prepared member revocation was replaced"
                            );
                            continue;
                        }
                        has_incomplete = true;
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

    async fn resolve_prepared_revocation(
        &self,
        revocation_id: &RevocationId,
        resolution: PreparedRevocationResolution,
        now_ms: i64,
    ) -> Result<RevocationRecord, KeyEpochError> {
        let master_key = self.session.get_master_key().map_err(backend)?;
        let revocation_id = revocation_id.as_str().to_owned();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let row = load_revocation_row(conn, &revocation_id)?
                        .ok_or_else(|| anyhow::anyhow!("revocation not found"))?;
                    if row.encrypted_stage.is_some() {
                        return Err(anyhow::anyhow!(
                            "prepared revocation already has a staged payload"
                        ));
                    }
                    let mut record = decode_record(&master_key, &row)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    if record.status() != RevocationStatus::Prepared {
                        return Err(anyhow::anyhow!("revocation is not prepared"));
                    }
                    let (status, verified_material, staged_payload) = match resolution {
                        PreparedRevocationResolution::TargetAbsent(material) => {
                            (RevocationStatus::Complete, Some(material), None)
                        }
                        PreparedRevocationResolution::TargetPresent {
                            current_material,
                            stage,
                        } => (RevocationStatus::Staged, Some(current_material), Some(stage)),
                        PreparedRevocationResolution::RecoveryRequired(material) => {
                            (RevocationStatus::RecoveryRequired, material, None)
                        }
                    };
                    if let Some(verified_material) = verified_material {
                        if verified_material.state().space_id() != record.space_id()
                            || verified_material.state().epoch() < record.previous_epoch()
                        {
                            return Err(anyhow::anyhow!(
                                "prepared revocation verification epoch mismatch"
                            ));
                        }
                        let persisted_material =
                            load_space_material_on(conn, &master_key, record.space_id())
                                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                        if persisted_material.as_ref() != Some(&verified_material) {
                            return Err(anyhow::anyhow!(
                                "prepared revocation verification state changed"
                            ));
                        }
                        if let Some(stage) = staged_payload.as_ref() {
                            let staged_record = stage.record();
                            if staged_record.status() != RevocationStatus::Staged
                                || staged_record.revocation_id() != record.revocation_id()
                                || staged_record.space_id() != record.space_id()
                                || staged_record.target_device_id() != record.target_device_id()
                                || staged_record.retained_recipients()
                                    != record.retained_recipients()
                                || staged_record.previous_epoch()
                                    != verified_material.state().epoch()
                                || staged_record.next_epoch()
                                    != verified_material.state().epoch().next().map_err(
                                        |error| anyhow::anyhow!(error.to_string()),
                                    )?
                                || stage.next_space_state().space_id() != record.space_id()
                                || stage.next_space_state().epoch() != staged_record.next_epoch()
                            {
                                return Err(anyhow::anyhow!(
                                    "prepared revocation restage validation failed"
                                ));
                            }
                            record = staged_record.clone();
                        }
                    } else if status != RevocationStatus::RecoveryRequired {
                        return Err(anyhow::anyhow!(
                            "prepared revocation completion requires verified material"
                        ));
                    }
                    if staged_payload.is_none() {
                        record
                            .transition_to(status, now_ms)
                            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    }
                    let encrypted_record = seal(
                        &master_key,
                        &record,
                        &record_aad(&revocation_id, status_name(status)),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let encrypted_stage = staged_payload
                        .as_ref()
                        .map(|stage| seal(&master_key, stage, &stage_aad(&revocation_id)))
                        .transpose()
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let previous_epoch = epoch_to_i64(record.previous_epoch().value())
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let next_epoch = epoch_to_i64(record.next_epoch().value())
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let affected = diesel::sql_query(
                        "UPDATE member_revocation_log SET previous_epoch = ?, next_epoch = ?, \
                         status = ?, encrypted_record = ?, encrypted_stage = ?, updated_at_ms = ? \
                         WHERE revocation_id = ? AND status = 'prepared' AND encrypted_stage IS NULL",
                    )
                    .bind::<BigInt, _>(previous_epoch)
                    .bind::<BigInt, _>(next_epoch)
                    .bind::<Text, _>(status_name(status))
                    .bind::<Binary, _>(encrypted_record)
                    .bind::<Nullable<Binary>, _>(encrypted_stage)
                    .bind::<BigInt, _>(now_ms)
                    .bind::<Text, _>(&revocation_id)
                    .execute(conn)?;
                    if affected != 1 {
                        return Err(anyhow::anyhow!(
                            "prepared revocation resolution lost atomic race"
                        ));
                    }
                    Ok(record)
                })
            })
            .map_err(backend)
    }

    async fn commit_revocation_recovery(
        &self,
        stage: &RevocationStage,
        material: &SpaceKeyMaterial,
    ) -> Result<RevocationRecord, KeyEpochError> {
        let record = stage.record();
        if !matches!(
            record.status(),
            RevocationStatus::Distributing
                | RevocationStatus::Complete
                | RevocationStatus::RecoveryRequired
        ) || (record.status() != RevocationStatus::RecoveryRequired
            && (material.state().space_id() != record.space_id()
                || material.state().epoch() != record.next_epoch()
                || stage.next_space_state() != material.state()
                || stage.group_state() != material.group_state()
                || stage.key_catalog() != material.key_catalog()))
        {
            return Err(backend("invalid revocation recovery payload"));
        }
        let master_key = self.session.get_master_key().map_err(backend)?;
        let encrypted_record = seal(
            &master_key,
            record,
            &record_aad(
                record.revocation_id().as_str(),
                status_name(record.status()),
            ),
        )?;
        let encrypted_stage = if record.status() == RevocationStatus::Complete {
            None
        } else {
            Some(seal(
                &master_key,
                stage,
                &stage_aad(record.revocation_id().as_str()),
            )?)
        };
        let revocation_id = record.revocation_id().as_str().to_owned();
        let previous_epoch = epoch_to_i64(record.previous_epoch().value())?;
        let next_epoch = epoch_to_i64(record.next_epoch().value())?;
        let status = status_name(record.status()).to_owned();
        let updated_at_ms = record.updated_at_ms();
        let record = record.clone();
        let stage = stage.clone();
        let material = material.clone();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let row = load_revocation_row(conn, &revocation_id)?
                        .ok_or_else(|| anyhow::anyhow!("revocation not found"))?;
                    let existing_record = decode_record(&master_key, &row)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let existing_stage: RevocationStage = open(
                        &master_key,
                        row.encrypted_stage
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("revocation has no staged payload"))?,
                        &stage_aad(&revocation_id),
                    )
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let appends_generation = existing_stage.generation_count()
                        < stage.generation_count()
                        && existing_record.next_epoch() < record.next_epoch();
                    let finishes_without_generation = matches!(
                        record.status(),
                        RevocationStatus::Complete | RevocationStatus::RecoveryRequired
                    ) && existing_stage.generation_count()
                        == stage.generation_count()
                        && existing_record.next_epoch() == record.next_epoch();
                    if existing_record.status() != RevocationStatus::Distributing
                        || !(appends_generation || finishes_without_generation)
                    {
                        return Err(anyhow::anyhow!("revocation recovery is not append-only"));
                    }
                    save_space_material_on(conn, &master_key, &material)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    let affected = diesel::sql_query(
                        "UPDATE member_revocation_log SET previous_epoch = ?, next_epoch = ?, \
                         status = ?, encrypted_record = ?, encrypted_stage = ?, updated_at_ms = ? \
                         WHERE revocation_id = ? AND status = 'distributing'",
                    )
                    .bind::<BigInt, _>(previous_epoch)
                    .bind::<BigInt, _>(next_epoch)
                    .bind::<Text, _>(&status)
                    .bind::<Binary, _>(&encrypted_record)
                    .bind::<Nullable<Binary>, _>(&encrypted_stage)
                    .bind::<BigInt, _>(updated_at_ms)
                    .bind::<Text, _>(&revocation_id)
                    .execute(conn)?;
                    if affected != 1 {
                        return Err(anyhow::anyhow!("revocation recovery was not saved"));
                    }
                    Ok(record)
                })
            })
            .map_err(backend)
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
impl<E: DbExecutor> SpaceSecurityStateResetPort for DieselSpaceSecurityStore<E> {
    async fn clear_space_security_state_except(
        &self,
        active_space_id: &SpaceId,
    ) -> Result<(), SpaceSecurityStateResetError> {
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| SpaceSecurityStateResetError::Repository(error.to_string()))?;
        let active_space_lookup_token = space_lookup_token(&master_key, active_space_id)
            .map_err(|error| SpaceSecurityStateResetError::Repository(error.to_string()))?;
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    diesel::sql_query(
                        "DELETE FROM member_revocation_log WHERE space_lookup_token <> ?",
                    )
                    .bind::<Text, _>(&active_space_lookup_token)
                    .execute(conn)?;
                    diesel::sql_query(
                        "DELETE FROM legacy_space_bootstrap_log WHERE space_lookup_token <> ?",
                    )
                    .bind::<Text, _>(&active_space_lookup_token)
                    .execute(conn)?;
                    diesel::sql_query(
                        "DELETE FROM space_key_epoch_state WHERE space_lookup_token <> ?",
                    )
                    .bind::<Text, _>(&active_space_lookup_token)
                    .execute(conn)?;
                    Ok(())
                })
            })
            .map_err(|error| SpaceSecurityStateResetError::Repository(error.to_string()))
    }
}
