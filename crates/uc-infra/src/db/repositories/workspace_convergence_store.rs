//! Workspace convergence state SQLite encrypted repository (ADR-016).
//!
//! The whole persisted convergence state (change chain, confirmations,
//! pending handoff records, waiting members, phase) is sealed with the
//! MasterKey AEAD before it is written; the only plaintext columns are the
//! space lookup token and the updated timestamp. Loading verifies that the
//! state belongs to the requested space, so a stale state from another
//! space can never be saved or read back after a space switch.

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Binary, Text};
use serde::{de::DeserializeOwned, Serialize};

use uc_core::crypto::EncryptionError;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    WorkspaceConvergenceRepositoryError, WorkspaceConvergenceRepositoryPort,
    WorkspaceConvergenceState,
};

use crate::db::ports::DbExecutor;
use crate::security::crypto_model::EncryptedBlob;
use crate::security::{v1_aead, InMemorySession, MasterKey};

use super::space_security_store::space_lookup_token;

fn repository_error(error: impl std::fmt::Display) -> WorkspaceConvergenceRepositoryError {
    WorkspaceConvergenceRepositoryError::Repository(error.to_string())
}

fn session_error(error: EncryptionError) -> WorkspaceConvergenceRepositoryError {
    match error {
        EncryptionError::NotInitialized | EncryptionError::Locked => {
            WorkspaceConvergenceRepositoryError::Locked
        }
        error => repository_error(error),
    }
}

fn workspace_state_aad(space_id: &str) -> Vec<u8> {
    format!("uc-workspace-convergence-state-v1|{space_id}").into_bytes()
}

fn seal_workspace_payload<T: Serialize>(
    master_key: &MasterKey,
    value: &T,
    aad: &[u8],
) -> Result<Vec<u8>, WorkspaceConvergenceRepositoryError> {
    let plaintext = postcard::to_stdvec(value).map_err(repository_error)?;
    let encrypted =
        v1_aead::encrypt_blob_xchacha(master_key, &plaintext, aad).map_err(repository_error)?;
    serde_json::to_vec(&encrypted).map_err(repository_error)
}

fn open_workspace_payload<T: DeserializeOwned>(
    master_key: &MasterKey,
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<T, WorkspaceConvergenceRepositoryError> {
    let encrypted: EncryptedBlob = serde_json::from_slice(ciphertext)
        .map_err(|_| WorkspaceConvergenceRepositoryError::Corrupt)?;
    let plaintext =
        v1_aead::decrypt_blob_xchacha(master_key, &encrypted.nonce, &encrypted.ciphertext, aad)
            .map_err(|_| WorkspaceConvergenceRepositoryError::Corrupt)?;
    postcard::from_bytes(&plaintext).map_err(|_| WorkspaceConvergenceRepositoryError::Corrupt)
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
    fn master_key(&self) -> Result<MasterKey, WorkspaceConvergenceRepositoryError> {
        self.session.get_master_key().map_err(session_error)
    }

    fn current_space_id(&self) -> Result<SpaceId, WorkspaceConvergenceRepositoryError> {
        self.session.current_space_id().map_err(session_error)
    }
}

fn validate_current_space(
    requested_space: &SpaceId,
    current_space: &SpaceId,
) -> Result<(), WorkspaceConvergenceRepositoryError> {
    if requested_space != current_space {
        return Err(WorkspaceConvergenceRepositoryError::Repository(
            "workspace convergence state belongs to a different space".to_owned(),
        ));
    }
    Ok(())
}

fn load_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
    let lookup_token = space_lookup_token(master_key, space_id).map_err(repository_error)?;
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
    let state: WorkspaceConvergenceState = open_workspace_payload(
        master_key,
        &row.encrypted_payload,
        &workspace_state_aad(space_id.as_ref()),
    )
    .map_err(repository_error)?;
    if state.space_lineage != space_id.as_ref() || state.updated_at_ms != row.updated_at_ms {
        return Err(repository_error(
            "workspace convergence state row integrity mismatch",
        ));
    }
    Ok(Some(state))
}

fn save_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    state: &WorkspaceConvergenceState,
) -> Result<(), WorkspaceConvergenceRepositoryError> {
    let space_id = SpaceId::from_str(state.space_lineage.as_str());
    let lookup_token = space_lookup_token(master_key, &space_id).map_err(repository_error)?;
    let encrypted =
        seal_workspace_payload(master_key, state, &workspace_state_aad(space_id.as_ref()))?;
    sql_query(
        "INSERT INTO workspace_convergence_state \
         (space_lookup_token, encrypted_payload, updated_at_ms) VALUES (?, ?, ?) \
         ON CONFLICT(space_lookup_token) DO UPDATE SET \
         encrypted_payload = excluded.encrypted_payload, \
         updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(lookup_token)
    .bind::<Binary, _>(encrypted)
    .bind::<BigInt, _>(state.updated_at_ms)
    .execute(conn)
    .map_err(repository_error)?;
    Ok(())
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> WorkspaceConvergenceRepositoryPort
    for DieselWorkspaceConvergenceStore<E>
{
    async fn save_state(
        &self,
        state: &WorkspaceConvergenceState,
    ) -> Result<(), WorkspaceConvergenceRepositoryError> {
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
    ) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = self.current_space_id()?;
        self.executor
            .run(move |conn| {
                load_state_on(conn, &master_key, &space_id)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(repository_error)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use diesel::prelude::*;
    use diesel::sql_types::Binary;
    use diesel::QueryableByName;
    use tempfile::{tempdir, TempDir};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        MemberInstanceId, RemovalIntentId, WorkspaceConvergenceRepositoryPort,
        WorkspaceConvergenceState, WorkspacePhase,
    };

    use super::DieselWorkspaceConvergenceStore;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::{init_db_pool, DbPool};
    use crate::security::{InMemorySession, MasterKey};

    const SPACE: &str = "workspace-space";
    const SENSITIVE_MARKER: &[u8] = b"workspace-state-sensitive-marker";

    #[derive(QueryableByName)]
    struct EncryptedPayloadRow {
        #[diesel(sql_type = Binary)]
        encrypted_payload: Vec<u8>,
    }

    fn session() -> InMemorySession {
        let session = InMemorySession::new();
        session.set_master_key_for_space(
            SpaceId::from_str(SPACE),
            MasterKey::from_bytes(&[0x57; 32]).unwrap(),
        );
        session
    }

    fn reopen_store(pool: DbPool) -> DieselWorkspaceConvergenceStore<DieselSqliteExecutor> {
        DieselWorkspaceConvergenceStore::new(DieselSqliteExecutor::new(pool), session())
    }

    fn persisted_state() -> WorkspaceConvergenceState {
        let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 123);
        state.removal_intents = BTreeSet::from([RemovalIntentId::from_bytes([0x42; 32])]);
        state.phase = WorkspacePhase::Converging;
        state.updated_at_ms = 123;
        state
    }

    fn make_store() -> (
        DieselWorkspaceConvergenceStore<DieselSqliteExecutor>,
        DbPool,
        TempDir,
    ) {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("workspace.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        (reopen_store(pool.clone()), pool, directory)
    }

    #[tokio::test]
    async fn stale_space_state_cannot_be_saved_after_switching_spaces() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("workspace.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let session = session();
        let store = DieselWorkspaceConvergenceStore::new(
            DieselSqliteExecutor::new(pool.clone()),
            session.clone(),
        );
        let state = persisted_state();

        session.set_master_key_for_space(
            SpaceId::from_str("another-space"),
            MasterKey::from_bytes(&[0x57; 32]).unwrap(),
        );

        assert!(store.save_state(&state).await.is_err());
        assert_eq!(store.load_state().await.unwrap(), None);
    }

    #[tokio::test]
    async fn state_survives_a_new_session_without_plaintext_on_disk() {
        let (store, pool, directory) = make_store();
        let state = persisted_state();

        store.save_state(&state).await.unwrap();

        let reopened = reopen_store(pool.clone());
        assert_eq!(reopened.load_state().await.unwrap(), Some(state.clone()));

        let mut connection = pool.get().unwrap();
        let rows = diesel::sql_query("SELECT encrypted_payload FROM workspace_convergence_state")
            .load::<EncryptedPayloadRow>(&mut connection)
            .unwrap();
        assert!(rows.iter().all(|row| {
            !row.encrypted_payload
                .windows(SENSITIVE_MARKER.len())
                .any(|window| window == SENSITIVE_MARKER)
        }));

        let markers: [&[u8]; 2] = [SENSITIVE_MARKER, b"workspace-space"];
        let files = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        assert!(files.iter().any(|path| path.ends_with("workspace.sqlite")));
        for path in files {
            let bytes = fs::read(&path).unwrap();
            for marker in markers {
                assert!(
                    !bytes.windows(marker.len()).any(|window| window == marker),
                    "sensitive marker persisted in database auxiliary file"
                );
            }
        }
    }

    #[tokio::test]
    async fn chain_order_and_revision_survive_a_round_trip() {
        let (store, pool, _directory) = make_store();
        let mut state = persisted_state();
        let a = MemberInstanceId::from_bytes([0x0a; 32]);
        let digest = [0x11; 32];
        let change = uc_core::membership::WorkspaceChange {
            space_lineage: SPACE.to_owned(),
            kind: uc_core::membership::WorkspaceChangeKind::Admission,
            previous_epoch: 0,
            next_epoch: 1,
            previous_digest: digest,
            digest,
            security_updates: Vec::new(),
            admission: Some(uc_core::membership::AdmissionChangeFacts {
                member_instance: a,
                device_id: DeviceId::new("sensitive-device-name"),
                device_name: "sensitive-device-name".to_owned(),
                identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .unwrap(),
                transport_public_key: vec![1; 32],
                transport_address_blob: vec![2; 16],
                identity_signature: vec![3; 64],
            }),
            removal: None,
            created_at_ms: 55,
        };
        state.changes.push(change);
        state.revision = 7;
        store.save_state(&state).await.unwrap();
        let reopened = reopen_store(pool.clone());
        let loaded = reopened.load_state().await.unwrap().unwrap();
        assert_eq!(loaded.changes.len(), 1);
        assert_eq!(loaded.changes[0].change_id(), state.changes[0].change_id());
        assert_eq!(loaded.revision, 7);
    }
}
