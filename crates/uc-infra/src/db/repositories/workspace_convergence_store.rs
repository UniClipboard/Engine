//! Workspace convergence state SQLite encrypted repository (ADR-016).
//!
//! The whole persisted convergence state (membership history, peer branch
//! relationships, pending admissions, phase) is sealed with the
//! MasterKey AEAD before it is written; the only plaintext columns are the
//! space lookup token and the updated timestamp. Loading verifies that the
//! state belongs to the requested space, so a stale state from another
//! space can never be saved or read back after a space switch.

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Binary, Text};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use uc_application::deps::{
    SpaceMembershipStateRepositoryError, SpaceMembershipStateRepositoryPort,
};
use uc_core::crypto::EncryptionError;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    MemberInstanceId, MembershipHistoryRelationship, SpaceMembershipState,
    WorkspaceFailureCategory, WorkspacePhase,
};

use crate::db::ports::DbExecutor;
use crate::security::crypto_model::EncryptedBlob;
use crate::security::{v1_aead, InMemorySession, MasterKey};

use super::space_security_store::space_lookup_token;

const WORKSPACE_STATE_V4_PREFIX: &[u8] = b"uc-workspace-convergence-state-v4\0";
const WORKSPACE_STATE_V4_GUARD_PREFIX: &[u8] = b"uc-workspace-convergence-v4-guard\0";
const WORKSPACE_STATE_V4_STORAGE_VERSION: u16 = 4;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceConvergenceStateV4 {
    storage_version: u16,
    state: WorkspaceConvergenceStateV4Payload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceConvergenceStateV4Payload {
    space_lineage: String,
    own_instance: Option<MemberInstanceId>,
    peer_history_relationships:
        std::collections::BTreeMap<uc_core::DeviceId, MembershipHistoryRelationship>,
    pending_membership_history_transfers: std::collections::BTreeMap<
        uc_core::DeviceId,
        uc_core::membership::PendingMembershipHistoryTransferV2,
    >,
    phase: WorkspacePhase,
    failure_category: Option<WorkspaceFailureCategory>,
    revision: u64,
    removed: bool,
    updated_at_ms: i64,
}

impl From<SpaceMembershipState> for WorkspaceConvergenceStateV4Payload {
    fn from(state: SpaceMembershipState) -> Self {
        Self {
            space_lineage: state.space_lineage,
            own_instance: state.own_instance,
            peer_history_relationships: state.peer_history_relationships,
            pending_membership_history_transfers: state.pending_membership_history_transfers,
            phase: state.phase,
            failure_category: state.failure_category,
            revision: state.revision,
            removed: state.removed,
            updated_at_ms: state.updated_at_ms,
        }
    }
}

impl From<WorkspaceConvergenceStateV4Payload> for SpaceMembershipState {
    fn from(state: WorkspaceConvergenceStateV4Payload) -> Self {
        Self {
            space_lineage: state.space_lineage,
            own_instance: state.own_instance,
            peer_history_relationships: state.peer_history_relationships,
            pending_membership_history_transfers: state.pending_membership_history_transfers,
            phase: state.phase,
            failure_category: state.failure_category,
            revision: state.revision,
            removed: state.removed,
            updated_at_ms: state.updated_at_ms,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum WorkspaceConvergenceMigrationV3Phase {
    Staging,
    TargetVerified,
    Activated,
    CleanupPending,
}

#[derive(Serialize, Deserialize)]
struct WorkspaceConvergenceMigrationV3 {
    storage_version: u16,
    source_encrypted_payload: Vec<u8>,
    source_ciphertext_digest: [u8; 32],
    target_slot_id: String,
    phase: WorkspaceConvergenceMigrationV3Phase,
    target_plaintext_digest: Option<[u8; 32]>,
}

fn repository_error(_error: impl std::fmt::Display) -> SpaceMembershipStateRepositoryError {
    SpaceMembershipStateRepositoryError::Unavailable
}

fn session_error(error: EncryptionError) -> SpaceMembershipStateRepositoryError {
    match error {
        EncryptionError::NotInitialized | EncryptionError::Locked => {
            SpaceMembershipStateRepositoryError::Locked
        }
        error => repository_error(error),
    }
}

fn workspace_state_aad(space_id: &str) -> Vec<u8> {
    format!("uc-workspace-convergence-state-v1|{space_id}").into_bytes()
}

fn seal_prefixed_payload<T: Serialize>(
    master_key: &MasterKey,
    prefix: &[u8],
    value: &T,
    aad: &[u8],
) -> Result<Vec<u8>, SpaceMembershipStateRepositoryError> {
    let encoded = postcard::to_stdvec(value).map_err(repository_error)?;
    let mut plaintext = Vec::with_capacity(prefix.len() + encoded.len());
    plaintext.extend_from_slice(prefix);
    plaintext.extend_from_slice(&encoded);
    let encrypted =
        v1_aead::encrypt_blob_xchacha(master_key, &plaintext, aad).map_err(repository_error)?;
    serde_json::to_vec(&encrypted).map_err(repository_error)
}

fn open_workspace_plaintext(
    master_key: &MasterKey,
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>, SpaceMembershipStateRepositoryError> {
    let encrypted: EncryptedBlob = serde_json::from_slice(ciphertext)
        .map_err(|_| SpaceMembershipStateRepositoryError::Corrupt)?;
    v1_aead::decrypt_blob_xchacha(master_key, &encrypted.nonce, &encrypted.ciphertext, aad)
        .map_err(|_| SpaceMembershipStateRepositoryError::Corrupt)
}

#[derive(QueryableByName)]
struct WorkspaceStateRow {
    #[diesel(sql_type = Text, column_name = space_lookup_token)]
    _space_lookup_token: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(QueryableByName)]
struct WorkspaceV3ActiveRow {
    #[diesel(sql_type = Text)]
    slot_id: String,
    #[diesel(sql_type = BigInt)]
    generation: i64,
}

#[derive(QueryableByName)]
struct WorkspaceV3SlotRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(Debug, thiserror::Error)]
enum WorkspaceTransactionError {
    #[error(transparent)]
    Diesel(#[from] diesel::result::Error),
    #[error(transparent)]
    Workspace(SpaceMembershipStateRepositoryError),
}

fn run_workspace_transaction<T>(
    conn: &mut SqliteConnection,
    operation: impl FnOnce(&mut SqliteConnection) -> Result<T, SpaceMembershipStateRepositoryError>,
) -> Result<T, SpaceMembershipStateRepositoryError> {
    conn.immediate_transaction::<_, WorkspaceTransactionError, _>(|conn| {
        operation(conn).map_err(WorkspaceTransactionError::Workspace)
    })
    .map_err(|error| match error {
        WorkspaceTransactionError::Diesel(error) => repository_error(error),
        WorkspaceTransactionError::Workspace(error) => error,
    })
}

pub struct DieselWorkspaceConvergenceStore<E> {
    executor: E,
    session: InMemorySession,
}

impl<E> DieselWorkspaceConvergenceStore<E> {
    pub fn new(executor: E, session: InMemorySession) -> Self {
        Self { executor, session }
    }
}

impl<E: DbExecutor> DieselWorkspaceConvergenceStore<E> {
    fn master_key(&self) -> Result<MasterKey, SpaceMembershipStateRepositoryError> {
        self.session.get_master_key().map_err(session_error)
    }

    fn current_space_id(&self) -> Result<SpaceId, SpaceMembershipStateRepositoryError> {
        self.session.current_space_id().map_err(session_error)
    }
}

fn validate_current_space(
    requested_space: &SpaceId,
    current_space: &SpaceId,
) -> Result<(), SpaceMembershipStateRepositoryError> {
    if requested_space != current_space {
        return Err(SpaceMembershipStateRepositoryError::Corrupt);
    }
    Ok(())
}

fn workspace_v3_slot_aad(space_id: &str, slot_id: &str) -> Vec<u8> {
    format!("uc-workspace-convergence-v3-slot|{space_id}|{slot_id}").into_bytes()
}

fn workspace_v3_migration_aad(space_id: &str, migration_id: &str) -> Vec<u8> {
    format!("uc-workspace-convergence-v3-migration|{space_id}|{migration_id}").into_bytes()
}

fn open_v3_slot_payload(
    master_key: &MasterKey,
    space_id: &SpaceId,
    slot_id: &str,
    row: &WorkspaceV3SlotRow,
) -> Result<(SpaceMembershipState, [u8; 32]), SpaceMembershipStateRepositoryError> {
    let plaintext = open_workspace_plaintext(
        master_key,
        &row.encrypted_payload,
        &workspace_v3_slot_aad(space_id.as_ref(), slot_id),
    )?;
    let encoded = plaintext
        .strip_prefix(WORKSPACE_STATE_V4_PREFIX)
        .ok_or(SpaceMembershipStateRepositoryError::Corrupt)?;
    let stored: WorkspaceConvergenceStateV4 =
        postcard::from_bytes(encoded).map_err(|_| SpaceMembershipStateRepositoryError::Corrupt)?;
    if stored.storage_version != WORKSPACE_STATE_V4_STORAGE_VERSION {
        return Err(SpaceMembershipStateRepositoryError::Corrupt);
    }
    let state = SpaceMembershipState::from(stored.state);
    validate_loaded_state(&state, space_id, row.updated_at_ms)?;
    Ok((state, Sha256::digest(&plaintext).into()))
}

fn load_v3_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
    lookup_token: &str,
) -> Result<Option<SpaceMembershipState>, SpaceMembershipStateRepositoryError> {
    let active = sql_query(
        "SELECT slot_id, generation FROM workspace_convergence_v3_active \
         WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<WorkspaceV3ActiveRow>(conn)
    .optional()
    .map_err(repository_error)?;
    let Some(active) = active else {
        return Ok(None);
    };
    if active.generation <= 0 {
        return Err(SpaceMembershipStateRepositoryError::Corrupt);
    }
    let slot = sql_query(
        "SELECT encrypted_payload, updated_at_ms FROM workspace_convergence_v3_slots \
         WHERE space_lookup_token = ? AND slot_id = ?",
    )
    .bind::<Text, _>(lookup_token)
    .bind::<Text, _>(&active.slot_id)
    .get_result::<WorkspaceV3SlotRow>(conn)
    .optional()
    .map_err(repository_error)?
    .ok_or(SpaceMembershipStateRepositoryError::Corrupt)?;
    open_v3_slot_payload(master_key, space_id, &active.slot_id, &slot).map(|(state, _)| Some(state))
}

fn recover_v3_storage_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
    lookup_token: &str,
) -> Result<(), SpaceMembershipStateRepositoryError> {
    let active = sql_query(
        "SELECT slot_id, generation FROM workspace_convergence_v3_active \
         WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<WorkspaceV3ActiveRow>(conn)
    .optional()
    .map_err(repository_error)?;
    if let Some(active) = active {
        if active.generation <= 0 {
            return Err(SpaceMembershipStateRepositoryError::Corrupt);
        }
        let row = sql_query(
            "SELECT encrypted_payload, updated_at_ms FROM workspace_convergence_v3_slots \
             WHERE space_lookup_token = ? AND slot_id = ?",
        )
        .bind::<Text, _>(lookup_token)
        .bind::<Text, _>(&active.slot_id)
        .get_result::<WorkspaceV3SlotRow>(conn)
        .optional()
        .map_err(repository_error)?
        .ok_or(SpaceMembershipStateRepositoryError::Corrupt)?;
        let _ = open_v3_slot_payload(master_key, space_id, &active.slot_id, &row)?;
        sql_query(
            "DELETE FROM workspace_convergence_v3_slots \
             WHERE space_lookup_token = ? AND slot_id <> ?",
        )
        .bind::<Text, _>(lookup_token)
        .bind::<Text, _>(&active.slot_id)
        .execute(conn)
        .map_err(repository_error)?;
    } else {
        sql_query("DELETE FROM workspace_convergence_v3_slots WHERE space_lookup_token = ?")
            .bind::<Text, _>(lookup_token)
            .execute(conn)
            .map_err(repository_error)?;
    }
    sql_query("DELETE FROM workspace_convergence_v3_migrations WHERE space_lookup_token = ?")
        .bind::<Text, _>(lookup_token)
        .execute(conn)
        .map_err(repository_error)?;
    Ok(())
}

fn write_v3_migration_phase(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
    lookup_token: &str,
    migration_id: &str,
    migration: &WorkspaceConvergenceMigrationV3,
    updated_at_ms: i64,
) -> Result<(), SpaceMembershipStateRepositoryError> {
    let encrypted = seal_prefixed_payload(
        master_key,
        WORKSPACE_STATE_V4_PREFIX,
        migration,
        &workspace_v3_migration_aad(space_id.as_ref(), migration_id),
    )?;
    sql_query(
        "INSERT INTO workspace_convergence_v3_migrations \
         (space_lookup_token, migration_id, encrypted_payload, updated_at_ms) \
         VALUES (?, ?, ?, ?) ON CONFLICT(space_lookup_token, migration_id) DO UPDATE SET \
         encrypted_payload = excluded.encrypted_payload, updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(lookup_token)
    .bind::<Text, _>(migration_id)
    .bind::<Binary, _>(encrypted)
    .bind::<BigInt, _>(updated_at_ms)
    .execute(conn)
    .map_err(repository_error)?;
    Ok(())
}

fn save_v3_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    state: &SpaceMembershipState,
    source_encrypted_payload: Option<Vec<u8>>,
) -> Result<(), SpaceMembershipStateRepositoryError> {
    let space_id = SpaceId::from_str(state.space_lineage.as_str());
    let lookup_token = space_lookup_token(master_key, &space_id).map_err(repository_error)?;
    let slot_id = Uuid::new_v4().to_string();
    let migration_id = source_encrypted_payload
        .as_ref()
        .map(|_| Uuid::new_v4().to_string());
    let mut migration = source_encrypted_payload.map(|source| WorkspaceConvergenceMigrationV3 {
        storage_version: WORKSPACE_STATE_V4_STORAGE_VERSION,
        source_ciphertext_digest: Sha256::digest(&source).into(),
        source_encrypted_payload: source,
        target_slot_id: slot_id.clone(),
        phase: WorkspaceConvergenceMigrationV3Phase::Staging,
        target_plaintext_digest: None,
    });
    if let (Some(migration_id), Some(migration)) = (&migration_id, &migration) {
        write_v3_migration_phase(
            conn,
            master_key,
            &space_id,
            &lookup_token,
            migration_id,
            migration,
            state.updated_at_ms,
        )?;
    }

    let stored = WorkspaceConvergenceStateV4 {
        storage_version: WORKSPACE_STATE_V4_STORAGE_VERSION,
        state: state.clone().into(),
    };
    let encrypted_slot = seal_prefixed_payload(
        master_key,
        WORKSPACE_STATE_V4_PREFIX,
        &stored,
        &workspace_v3_slot_aad(space_id.as_ref(), &slot_id),
    )?;
    sql_query(
        "INSERT INTO workspace_convergence_v3_slots \
         (space_lookup_token, slot_id, encrypted_payload, updated_at_ms) VALUES (?, ?, ?, ?)",
    )
    .bind::<Text, _>(&lookup_token)
    .bind::<Text, _>(&slot_id)
    .bind::<Binary, _>(&encrypted_slot)
    .bind::<BigInt, _>(state.updated_at_ms)
    .execute(conn)
    .map_err(repository_error)?;

    let inserted = sql_query(
        "SELECT encrypted_payload, updated_at_ms FROM workspace_convergence_v3_slots \
         WHERE space_lookup_token = ? AND slot_id = ?",
    )
    .bind::<Text, _>(&lookup_token)
    .bind::<Text, _>(&slot_id)
    .get_result::<WorkspaceV3SlotRow>(conn)
    .map_err(repository_error)?;
    let (reopened, verified_digest) =
        open_v3_slot_payload(master_key, &space_id, &slot_id, &inserted)?;
    if reopened != *state {
        return Err(SpaceMembershipStateRepositoryError::Corrupt);
    }
    if let (Some(migration_id), Some(migration)) = (&migration_id, &mut migration) {
        migration.phase = WorkspaceConvergenceMigrationV3Phase::TargetVerified;
        migration.target_plaintext_digest = Some(verified_digest);
        write_v3_migration_phase(
            conn,
            master_key,
            &space_id,
            &lookup_token,
            migration_id,
            migration,
            state.updated_at_ms,
        )?;
    }

    let next_generation = sql_query(
        "SELECT slot_id, generation FROM workspace_convergence_v3_active \
         WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(&lookup_token)
    .get_result::<WorkspaceV3ActiveRow>(conn)
    .optional()
    .map_err(repository_error)?
    .map_or(Ok(1_i64), |active| {
        active
            .generation
            .checked_add(1)
            .ok_or(SpaceMembershipStateRepositoryError::Corrupt)
    })?;
    sql_query(
        "INSERT INTO workspace_convergence_v3_active (space_lookup_token, slot_id, generation) \
         VALUES (?, ?, ?) ON CONFLICT(space_lookup_token) DO UPDATE SET \
         slot_id = excluded.slot_id, generation = excluded.generation",
    )
    .bind::<Text, _>(&lookup_token)
    .bind::<Text, _>(&slot_id)
    .bind::<BigInt, _>(next_generation)
    .execute(conn)
    .map_err(repository_error)?;

    let guard = seal_prefixed_payload(
        master_key,
        WORKSPACE_STATE_V4_GUARD_PREFIX,
        &WORKSPACE_STATE_V4_STORAGE_VERSION,
        &workspace_state_aad(space_id.as_ref()),
    )?;
    sql_query(
        "INSERT INTO workspace_convergence_state \
         (space_lookup_token, encrypted_payload, updated_at_ms) VALUES (?, ?, ?) \
         ON CONFLICT(space_lookup_token) DO UPDATE SET encrypted_payload = excluded.encrypted_payload, \
         updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(&lookup_token)
    .bind::<Binary, _>(guard)
    .bind::<BigInt, _>(state.updated_at_ms)
    .execute(conn)
    .map_err(repository_error)?;

    if let (Some(migration_id), Some(migration)) = (&migration_id, &mut migration) {
        migration.phase = WorkspaceConvergenceMigrationV3Phase::Activated;
        write_v3_migration_phase(
            conn,
            master_key,
            &space_id,
            &lookup_token,
            migration_id,
            migration,
            state.updated_at_ms,
        )?;
        migration.phase = WorkspaceConvergenceMigrationV3Phase::CleanupPending;
        write_v3_migration_phase(
            conn,
            master_key,
            &space_id,
            &lookup_token,
            migration_id,
            migration,
            state.updated_at_ms,
        )?;
    }
    Ok(())
}

fn load_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
) -> Result<Option<SpaceMembershipState>, SpaceMembershipStateRepositoryError> {
    let lookup_token = space_lookup_token(master_key, space_id).map_err(repository_error)?;
    run_workspace_transaction(conn, |conn| {
        recover_v3_storage_on(conn, master_key, space_id, &lookup_token)
    })?;
    if let Some(state) = load_v3_state_on(conn, master_key, space_id, &lookup_token)? {
        return Ok(Some(state));
    }
    let row = sql_query(
        "SELECT space_lookup_token, encrypted_payload, updated_at_ms \
         FROM workspace_convergence_state WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<WorkspaceStateRow>(conn)
    .optional()
    .map_err(repository_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let _ = row;
    Err(SpaceMembershipStateRepositoryError::Corrupt)
}

fn validate_loaded_state(
    state: &SpaceMembershipState,
    space_id: &SpaceId,
    updated_at_ms: i64,
) -> Result<(), SpaceMembershipStateRepositoryError> {
    if state.space_lineage != space_id.as_ref() || state.updated_at_ms != updated_at_ms {
        return Err(SpaceMembershipStateRepositoryError::Corrupt);
    }
    Ok(())
}

fn save_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    state: &SpaceMembershipState,
) -> Result<(), SpaceMembershipStateRepositoryError> {
    save_v3_state_on(conn, master_key, state, None)
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> SpaceMembershipStateRepositoryPort
    for DieselWorkspaceConvergenceStore<E>
{
    async fn save_state(
        &self,
        state: &SpaceMembershipState,
    ) -> Result<(), SpaceMembershipStateRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = self.current_space_id()?;
        validate_current_space(&SpaceId::from_str(state.space_lineage.as_str()), &space_id)?;
        let state = state.clone();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    save_state_on(conn, &master_key, &state)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    Ok(())
                })
                .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(repository_error)
    }

    async fn load_state(
        &self,
    ) -> Result<Option<SpaceMembershipState>, SpaceMembershipStateRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = self.current_space_id()?;
        self.executor
            .run(move |conn| Ok(load_state_on(conn, &master_key, &space_id)))
            .map_err(repository_error)?
    }
}
