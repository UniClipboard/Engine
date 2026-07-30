use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    BootstrapError, BootstrapId, LegacyBootstrapRecord, LegacyBootstrapRepositoryPort,
    LegacyBootstrapStage, LegacyBootstrapStatus,
};

use crate::db::ports::DbExecutor;
use crate::security::MasterKey;

use super::encrypted_payload::{open, seal, space_lookup_token};
use super::space_material::save_space_material_on;
use super::{epoch_to_i64, DieselSpaceSecurityStore};

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

fn bootstrap_status_name(status: LegacyBootstrapStatus) -> &'static str {
    match status {
        LegacyBootstrapStatus::Prepared => "prepared",
        LegacyBootstrapStatus::Staged => "staged",
        LegacyBootstrapStatus::AwaitingReadmission => "awaiting_readmission",
        LegacyBootstrapStatus::Complete => "complete",
        LegacyBootstrapStatus::RecoveryRequired => "recovery_required",
    }
}

fn bootstrap_record_aad(bootstrap_id: &str, status: &str) -> Vec<u8> {
    format!("uc-legacy-bootstrap-record-v1|{bootstrap_id}|{status}").into_bytes()
}

fn bootstrap_stage_aad(bootstrap_id: &str) -> Vec<u8> {
    format!("uc-legacy-bootstrap-stage-v1|{bootstrap_id}").into_bytes()
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

#[async_trait]
impl<E: DbExecutor> LegacyBootstrapRepositoryPort for DieselSpaceSecurityStore<E> {
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

    async fn load_legacy_bootstrap_stage(
        &self,
        bootstrap_id: &BootstrapId,
    ) -> Result<Option<LegacyBootstrapStage>, BootstrapError> {
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        let bootstrap_id = bootstrap_id.as_str().to_owned();
        self.executor
            .run(move |conn| {
                let Some(row) = load_bootstrap_row(conn, &bootstrap_id)? else {
                    return Ok(None);
                };
                let Some(encrypted_stage) = row.encrypted_stage else {
                    return Ok(None);
                };
                let stage = open(
                    &master_key,
                    &encrypted_stage,
                    &bootstrap_stage_aad(&bootstrap_id),
                )
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                Ok(Some(stage))
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

    async fn list_legacy_bootstraps(&self) -> Result<Vec<LegacyBootstrapRecord>, BootstrapError> {
        let master_key = self
            .session
            .get_master_key()
            .map_err(|error| BootstrapError::Repository(error.to_string()))?;
        self.executor
            .run(move |conn| {
                let rows = diesel::sql_query(
                    "SELECT bootstrap_id, space_lookup_token, previous_epoch, next_epoch, status, \
                     encrypted_record, encrypted_stage, created_at_ms, updated_at_ms \
                     FROM legacy_space_bootstrap_log ORDER BY updated_at_ms DESC",
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
