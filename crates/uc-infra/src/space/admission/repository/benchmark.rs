use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use diesel::RunQueryDsl;
use uc_application::deps::{
    JoinerActivationStatePort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerError,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::persisted::{PersistedSpaceAdmissionRepositoryV2, StoredSpaceAdmissionV1};
use super::SqliteSpaceAdmissionState;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::{init_db_pool, DbPool};
use crate::security::{ActiveSpaceGenerationManifestStore, AdmissionKeyManager};

#[derive(Default)]
struct MemoryStorage {
    values: Mutex<HashMap<String, Vec<u8>>>,
    locked: AtomicBool,
}

impl SecureStoragePort for MemoryStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(SecureStorageError::PermissionDenied("locked".to_owned()));
        }
        let values = self
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(values.get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        let mut values = self
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        values.insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        let mut values = self
            .values
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        values.remove(key);
        Ok(())
    }
}

struct UnusedMembership;

#[async_trait]
impl LoadMembershipLedgerPort for UnusedMembership {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Err(MembershipLedgerError::Unavailable)
    }
}

/// 仅供正式性能套件创建不含用户资料的准入仓储场景。
pub struct AdmissionRepositoryBenchmark {
    _directory: tempfile::TempDir,
    pool: DbPool,
    keys: Arc<AdmissionKeyManager>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    membership: Arc<dyn LoadMembershipLedgerPort>,
}

impl AdmissionRepositoryBenchmark {
    pub fn without_current_join(unrelated_record_bytes: usize) -> anyhow::Result<Self> {
        let directory = tempfile::tempdir()?;
        let database_path = directory.path().join("profile.sqlite");
        let database_url = database_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("benchmark database path is invalid"))?;
        let pool = init_db_pool(database_url)?;
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemoryStorage::default()),
            [0x31; 16],
        ));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            directory.path().join("vault"),
            Arc::clone(&keys),
        ));
        let membership: Arc<dyn LoadMembershipLedgerPort> = Arc::new(UnusedMembership);
        let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
        if unrelated_record_bytes > 0 {
            state.records.insert(
                [0x41; 32],
                StoredSpaceAdmissionV1 {
                    wrapped_data_key: keys.create_wrapped_attempt_key([0x41; 32])?,
                    encrypted_payload: vec![0x51; unrelated_record_bytes].into(),
                },
            );
        }
        let plaintext = postcard::to_stdvec(&state)?;
        let encrypted = keys.seal_profile_payload(b"space-admission-repository-v1", &plaintext)?;
        diesel::sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?)",
        )
        .bind::<diesel::sql_types::Binary, _>(encrypted)
        .execute(&mut pool.get()?)?;

        Ok(Self {
            _directory: directory,
            pool,
            keys,
            manifests,
            membership,
        })
    }

    pub async fn load_without_current_join(&self) -> anyhow::Result<()> {
        let repository = SqliteSpaceAdmissionState::new(
            DieselSqliteExecutor::new(self.pool.clone()),
            Arc::clone(&self.keys),
            Arc::clone(&self.manifests),
            Arc::clone(&self.membership),
        );
        let loaded = JoinerActivationStatePort::load(&repository)
            .await
            .map_err(anyhow::Error::new)?;
        if loaded.is_some() {
            return Err(anyhow::anyhow!(
                "benchmark without a current join returned an activation"
            ));
        }
        Ok(())
    }
}
