//! 移除意图与收敛状态的 SQLite 加密仓储。
//!
//! 敏感负载(意图、因果证明、传播进度、恢复执行状态、备用 key package 私钥
//! 状态)全部经 MasterKey AEAD 加密后落库;明文列只有空间查询令牌与时间戳。
//! 意图与其引起的本机安全限制在同一事务中落库,崩溃恢复后不会出现
//! "意图存在但本机重新信任目标"的状态。

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{BigInt, Binary, Text};
use serde::{de::DeserializeOwned, Serialize};

use uc_core::crypto::EncryptionError;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    RemovalIntentRepositoryError, RemovalIntentRepositoryPort, RemovalPendingJoinStorePort,
    RemovalPersistedState, SignedRemovalIntent,
};

use crate::db::ports::DbExecutor;
use crate::security::crypto_model::EncryptedBlob;
use crate::security::{v1_aead, InMemorySession, MasterKey};

use super::space_security_store::space_lookup_token;

fn removal_error(error: impl std::fmt::Display) -> RemovalIntentRepositoryError {
    RemovalIntentRepositoryError::Repository(error.to_string())
}

fn session_error(error: EncryptionError) -> RemovalIntentRepositoryError {
    match error {
        EncryptionError::NotInitialized | EncryptionError::Locked => {
            RemovalIntentRepositoryError::Locked
        }
        error => removal_error(error),
    }
}

fn removal_state_aad(space_id: &str) -> Vec<u8> {
    format!("uc-removal-convergence-state-v1|{space_id}").into_bytes()
}

fn removal_pending_aad(space_id: &str) -> Vec<u8> {
    format!("uc-removal-pending-join-v1|{space_id}").into_bytes()
}

fn seal_removal_payload<T: Serialize>(
    master_key: &MasterKey,
    value: &T,
    aad: &[u8],
) -> Result<Vec<u8>, RemovalIntentRepositoryError> {
    let plaintext = postcard::to_stdvec(value).map_err(removal_error)?;
    let encrypted =
        v1_aead::encrypt_blob_xchacha(master_key, &plaintext, aad).map_err(removal_error)?;
    serde_json::to_vec(&encrypted).map_err(removal_error)
}

fn open_removal_payload<T: DeserializeOwned>(
    master_key: &MasterKey,
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<T, RemovalIntentRepositoryError> {
    let encrypted: EncryptedBlob =
        serde_json::from_slice(ciphertext).map_err(|_| RemovalIntentRepositoryError::Corrupt)?;
    let plaintext =
        v1_aead::decrypt_blob_xchacha(master_key, &encrypted.nonce, &encrypted.ciphertext, aad)
            .map_err(|_| RemovalIntentRepositoryError::Corrupt)?;
    postcard::from_bytes(&plaintext).map_err(|_| RemovalIntentRepositoryError::Corrupt)
}

#[derive(QueryableByName)]
struct RemovalStateRow {
    #[diesel(sql_type = Text, column_name = space_lookup_token)]
    _space_lookup_token: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(QueryableByName)]
struct RemovalPendingRow {
    #[diesel(sql_type = Text, column_name = space_lookup_token)]
    _space_lookup_token: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

pub struct DieselRemovalIntentStore<E> {
    executor: E,
    session: InMemorySession,
}

impl<E> DieselRemovalIntentStore<E> {
    pub fn new(executor: E, session: InMemorySession) -> Self {
        Self { executor, session }
    }
}

impl<E: DbExecutor> DieselRemovalIntentStore<E> {
    fn master_key(&self) -> Result<MasterKey, RemovalIntentRepositoryError> {
        self.session.get_master_key().map_err(session_error)
    }

    fn current_space_id(&self) -> Result<SpaceId, RemovalIntentRepositoryError> {
        self.session.current_space_id().map_err(session_error)
    }
}

fn load_persisted_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
) -> Result<Option<RemovalPersistedState>, RemovalIntentRepositoryError> {
    let lookup_token = space_lookup_token(master_key, space_id).map_err(removal_error)?;
    let row = sql_query(
        "SELECT space_lookup_token, encrypted_payload, updated_at_ms \
         FROM removal_convergence_state WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<RemovalStateRow>(conn)
    .optional()
    .map_err(removal_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let state: RemovalPersistedState = open_removal_payload(
        master_key,
        &row.encrypted_payload,
        &removal_state_aad(space_id.as_ref()),
    )
    .map_err(removal_error)?;
    if state.space_lineage != space_id.as_ref() || state.updated_at_ms != row.updated_at_ms {
        return Err(removal_error(
            "removal convergence state row integrity mismatch",
        ));
    }
    Ok(Some(state))
}

fn save_persisted_state_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    state: &RemovalPersistedState,
) -> Result<(), RemovalIntentRepositoryError> {
    let space_id = SpaceId::from_str(state.space_lineage.as_str());
    let lookup_token = space_lookup_token(master_key, &space_id).map_err(removal_error)?;
    let encrypted = seal_removal_payload(master_key, state, &removal_state_aad(space_id.as_ref()))?;
    sql_query(
        "INSERT INTO removal_convergence_state \
         (space_lookup_token, encrypted_payload, updated_at_ms) VALUES (?, ?, ?) \
         ON CONFLICT(space_lookup_token) DO UPDATE SET \
         encrypted_payload = excluded.encrypted_payload, \
         updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(lookup_token)
    .bind::<Binary, _>(encrypted)
    .bind::<BigInt, _>(state.updated_at_ms)
    .execute(conn)
    .map_err(removal_error)?;
    Ok(())
}

fn validate_persisted_state_for_space(
    state: &RemovalPersistedState,
    space_id: &SpaceId,
) -> Result<(), RemovalIntentRepositoryError> {
    if state.space_lineage != space_id.as_ref() {
        return Err(RemovalIntentRepositoryError::Repository(
            "removal state belongs to a different space".to_owned(),
        ));
    }
    if state
        .intents
        .iter()
        .any(|intent| !state.locally_removed.contains(&intent.content.target))
    {
        return Err(RemovalIntentRepositoryError::Repository(
            "removal state is missing a local removal restriction".to_owned(),
        ));
    }
    Ok(())
}

fn validate_current_space(
    requested_space: &SpaceId,
    current_space: &SpaceId,
) -> Result<(), RemovalIntentRepositoryError> {
    if requested_space != current_space {
        return Err(RemovalIntentRepositoryError::Repository(
            "removal state belongs to a different space".to_owned(),
        ));
    }
    Ok(())
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> RemovalIntentRepositoryPort for DieselRemovalIntentStore<E> {
    async fn current_space_lineage(&self) -> Result<String, RemovalIntentRepositoryError> {
        Ok(self.current_space_id()?.as_ref().to_owned())
    }

    async fn save_new_intent_state(
        &self,
        intent: &SignedRemovalIntent,
        state: &RemovalPersistedState,
    ) -> Result<bool, RemovalIntentRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = self.current_space_id()?;
        validate_persisted_state_for_space(state, &space_id)?;
        if !state
            .intents
            .iter()
            .any(|known| known.intent_id == intent.intent_id)
        {
            return Err(RemovalIntentRepositoryError::Repository(
                "removal state does not include the new intent".to_owned(),
            ));
        }
        let intent = intent.clone();
        let state = state.clone();
        let executor = &self.executor;
        let saved = executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let current = load_persisted_state_on(conn, &master_key, &space_id)?;
                    if current.as_ref().is_some_and(|current| {
                        current
                            .intents
                            .iter()
                            .any(|known| known.intent_id == intent.intent_id)
                    }) {
                        return Ok(false);
                    }
                    save_persisted_state_on(conn, &master_key, &state)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    Ok(true)
                })
                .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(removal_error)?;
        Ok(saved)
    }

    async fn save_state(
        &self,
        state: &RemovalPersistedState,
    ) -> Result<(), RemovalIntentRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = self.current_space_id()?;
        validate_persisted_state_for_space(state, &space_id)?;
        let state = state.clone();
        self.executor
            .run(move |conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    save_persisted_state_on(conn, &master_key, &state)
                        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                    Ok(())
                })
                .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(removal_error)
    }

    async fn load_state(
        &self,
    ) -> Result<Option<RemovalPersistedState>, RemovalIntentRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = self.current_space_id()?;
        self.executor
            .run(move |conn| {
                load_persisted_state_on(conn, &master_key, &space_id)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(removal_error)
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> RemovalPendingJoinStorePort for DieselRemovalIntentStore<E> {
    async fn save(
        &self,
        space_lineage: &str,
        pending: Vec<u8>,
    ) -> Result<(), RemovalIntentRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = SpaceId::from_str(space_lineage);
        validate_current_space(&space_id, &self.current_space_id()?)?;
        self.executor
            .run(move |conn| {
                let lookup_token = space_lookup_token(&master_key, &space_id)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                let encrypted = seal_removal_payload(
                    &master_key,
                    &pending,
                    &removal_pending_aad(space_id.as_ref()),
                )
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                let now_ms = chrono::Utc::now().timestamp_millis();
                sql_query(
                    "INSERT INTO removal_pending_join \
                     (space_lookup_token, encrypted_payload, updated_at_ms) VALUES (?, ?, ?) \
                     ON CONFLICT(space_lookup_token) DO UPDATE SET \
                     encrypted_payload = excluded.encrypted_payload, \
                     updated_at_ms = excluded.updated_at_ms",
                )
                .bind::<Text, _>(lookup_token)
                .bind::<Binary, _>(encrypted)
                .bind::<BigInt, _>(now_ms)
                .execute(conn)?;
                Ok(())
            })
            .map_err(removal_error)
    }

    async fn load(
        &self,
        space_lineage: &str,
    ) -> Result<Option<Vec<u8>>, RemovalIntentRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = SpaceId::from_str(space_lineage);
        validate_current_space(&space_id, &self.current_space_id()?)?;
        self.executor
            .run(move |conn| {
                let lookup_token = space_lookup_token(&master_key, &space_id)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                let row = sql_query(
                    "SELECT space_lookup_token, encrypted_payload FROM removal_pending_join \
                     WHERE space_lookup_token = ?",
                )
                .bind::<Text, _>(lookup_token)
                .get_result::<RemovalPendingRow>(conn)
                .optional()?;
                let Some(row) = row else {
                    return Ok(None);
                };
                let pending: Vec<u8> = open_removal_payload(
                    &master_key,
                    &row.encrypted_payload,
                    &removal_pending_aad(space_id.as_ref()),
                )
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                Ok(Some(pending))
            })
            .map_err(removal_error)
    }

    async fn clear(&self, space_lineage: &str) -> Result<(), RemovalIntentRepositoryError> {
        let master_key = self.master_key()?;
        let space_id = SpaceId::from_str(space_lineage);
        validate_current_space(&space_id, &self.current_space_id()?)?;
        self.executor
            .run(move |conn| {
                let lookup_token = space_lookup_token(&master_key, &space_id)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                sql_query("DELETE FROM removal_pending_join WHERE space_lookup_token = ?")
                    .bind::<Text, _>(lookup_token)
                    .execute(conn)?;
                Ok(())
            })
            .map_err(removal_error)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;

    use diesel::prelude::*;
    use diesel::sql_types::Binary;
    use diesel::QueryableByName;
    use tempfile::{tempdir, TempDir};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        MemberInstanceId, RemovalCausalCheckpoint, RemovalCausalProof, RemovalCausalProofMember,
        RemovalIntentContent, RemovalIntentRepositoryPort, RemovalPendingJoinStorePort,
        RemovalPersistedState, RemovalPhase, SignedRemovalIntent,
    };

    use super::DieselRemovalIntentStore;
    use crate::db::executor::DieselSqliteExecutor;
    use crate::db::pool::{init_db_pool, DbPool};
    use crate::security::{InMemorySession, MasterKey};

    const SPACE: &str = "removal-space";
    const SENSITIVE_MARKER: &[u8] = b"removal-state-sensitive-marker";

    #[derive(QueryableByName)]
    struct EncryptedPayloadRow {
        #[diesel(sql_type = Binary)]
        encrypted_payload: Vec<u8>,
    }

    fn instance(device: &str, key: u8) -> MemberInstanceId {
        MemberInstanceId::derive(device, &[key; 32])
    }

    fn session() -> InMemorySession {
        let session = InMemorySession::new();
        session.set_master_key_for_space(
            SpaceId::from_str(SPACE),
            MasterKey::from_bytes(&[0x4d; 32]).unwrap(),
        );
        session
    }

    fn reopen_store(pool: DbPool) -> DieselRemovalIntentStore<DieselSqliteExecutor> {
        DieselRemovalIntentStore::new(DieselSqliteExecutor::new(pool), session())
    }

    fn persisted_state() -> RemovalPersistedState {
        let alice = instance("alice-sensitive-device", 1);
        let bob = instance("removed-sensitive-device", 2);
        let proof = RemovalCausalProof::new(
            1,
            vec![
                RemovalCausalProofMember {
                    device_id: DeviceId::new("alice-sensitive-device"),
                    instance: alice,
                    signing_public_key: vec![1; 32],
                },
                RemovalCausalProofMember {
                    device_id: DeviceId::new("removed-sensitive-device"),
                    instance: bob,
                    signing_public_key: vec![2; 32],
                },
            ],
        );
        let intent = SignedRemovalIntent::new(
            RemovalIntentContent {
                space_lineage: SPACE.to_owned(),
                view_epoch: 1,
                view_members: vec![alice, bob],
                initiator: alice,
                target: bob,
            },
            SENSITIVE_MARKER.to_vec(),
            proof,
        );
        let causal_checkpoint = RemovalCausalCheckpoint::from_intent(&intent);
        RemovalPersistedState {
            space_lineage: SPACE.to_owned(),
            intents: vec![intent],
            locally_removed: BTreeSet::from([bob]),
            locally_removed_devices: BTreeSet::from([DeviceId::new("removed-sensitive-device")]),
            member_devices: BTreeMap::from([
                (alice, DeviceId::new("alice-sensitive-device")),
                (bob, DeviceId::new("removed-sensitive-device")),
            ]),
            retired_members: BTreeSet::new(),
            causal_history: vec![causal_checkpoint],
            peer_exchanges: BTreeMap::new(),
            recovery: None,
            applied_digest: None,
            completed_member_count: None,
            admission_generation: 1,
            phase: RemovalPhase::Converging,
            updated_at_ms: 123,
            self_removed: None,
            self_removed_target: None,
            notified_removals: BTreeSet::new(),
            view_signing_keys: BTreeMap::new(),
        }
    }

    fn make_store() -> (
        DieselRemovalIntentStore<DieselSqliteExecutor>,
        DbPool,
        TempDir,
    ) {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("removal.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        (reopen_store(pool.clone()), pool, directory)
    }

    #[tokio::test]
    async fn stale_space_state_cannot_be_saved_after_switching_spaces() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("removal.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let session = session();
        let store =
            DieselRemovalIntentStore::new(DieselSqliteExecutor::new(pool.clone()), session.clone());
        let state = persisted_state();

        session.set_master_key_for_space(
            SpaceId::from_str("another-space"),
            MasterKey::from_bytes(&[0x4d; 32]).unwrap(),
        );

        assert!(store.save_state(&state).await.is_err());
        assert_eq!(store.load_state().await.unwrap(), None);
    }

    #[tokio::test]
    async fn state_cannot_drop_a_saved_intents_local_removal_restriction() {
        let (store, _pool, _directory) = make_store();
        let mut state = persisted_state();
        let target = state.intents[0].content.target;
        state.locally_removed.remove(&target);

        assert!(store.save_state(&state).await.is_err());
        assert_eq!(store.load_state().await.unwrap(), None);
    }

    #[tokio::test]
    async fn stale_space_cannot_save_pending_recovery_join_data_after_a_switch() {
        let directory = tempdir().unwrap();
        let database_path = directory.path().join("removal.sqlite");
        let pool = init_db_pool(database_path.to_str().unwrap()).unwrap();
        let session = session();
        let store = DieselRemovalIntentStore::new(DieselSqliteExecutor::new(pool), session.clone());

        session.set_master_key_for_space(
            SpaceId::from_str("another-space"),
            MasterKey::from_bytes(&[0x4d; 32]).unwrap(),
        );

        assert!(store
            .save(SPACE, b"stale-pending-join".to_vec())
            .await
            .is_err());
        assert_eq!(store.load("another-space").await.unwrap(), None);
    }

    #[tokio::test]
    async fn state_and_pending_join_survive_a_new_session_without_plaintext_on_disk() {
        let (store, pool, directory) = make_store();
        let state = persisted_state();
        let pending = b"pending-join-sensitive-marker".to_vec();

        store.save_state(&state).await.unwrap();
        store.save(SPACE, pending.clone()).await.unwrap();

        let reopened = reopen_store(pool.clone());
        assert_eq!(reopened.load_state().await.unwrap(), Some(state));
        assert_eq!(reopened.load(SPACE).await.unwrap(), Some(pending.clone()));

        let mut connection = pool.get().unwrap();
        let state_rows =
            diesel::sql_query("SELECT encrypted_payload FROM removal_convergence_state")
                .load::<EncryptedPayloadRow>(&mut connection)
                .unwrap();
        let pending_rows = diesel::sql_query("SELECT encrypted_payload FROM removal_pending_join")
            .load::<EncryptedPayloadRow>(&mut connection)
            .unwrap();
        let payloads = state_rows
            .into_iter()
            .chain(pending_rows)
            .map(|row| row.encrypted_payload)
            .collect::<Vec<_>>();
        assert!(payloads.iter().all(|payload| {
            !payload
                .windows(SENSITIVE_MARKER.len())
                .any(|window| window == SENSITIVE_MARKER)
                && !payload
                    .windows(pending.len())
                    .any(|window| window == pending.as_slice())
        }));

        // P05: WAL 和共享内存文件与主库同样不能留下业务原文。数据库工作目录
        // 中的每个文件都由真实 SQLite 写入产生，不能只检查逻辑表的密文列。
        let markers: [&[u8]; 4] = [
            SENSITIVE_MARKER,
            pending.as_slice(),
            b"alice-sensitive-device",
            b"removed-sensitive-device",
        ];
        let files = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        assert!(files.iter().any(|path| path.ends_with("removal.sqlite")));
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
}
