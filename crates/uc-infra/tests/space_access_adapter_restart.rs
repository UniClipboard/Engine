//! `RuntimeSpaceAccessAdapter`（`uc-infra-security`）读取真实 SQLite 仓储
//! （`uc-infra` 的 `DieselSpaceSecurityStore`）持久化结果的跨 crate 集成验证。
//!
//! 这条场景原本随 `RuntimeSpaceAccessAdapter` 的内部单测存在；security 拆成
//! 独立 crate 后，真实数据库仓储仍留在 `uc-infra`（storage 未拆分前），
//! 两者不能再共享同一个内部测试模块，因此迁到这里作为 `uc-infra` 的集成测试：
//! `uc-infra` 本就依赖 `uc-infra-security`，这个方向不产生循环依赖。
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tempfile::tempdir;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    BootstrapError, BootstrapId, GroupEpoch, GroupRevocationPort, LegacyBootstrapRecord,
    LegacyBootstrapRepositoryPort, LegacyBootstrapStage, LegacyBootstrapStatus, RevocationId,
    RevocationRecord, RevocationRepositoryPort,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use uc_infra::db::executor::DieselSqliteExecutor;
use uc_infra::db::pool::init_db_pool;
use uc_infra::db::repositories::DieselSpaceSecurityStore;
use uc_infra_crypto::secrets::MasterKey;
use uc_infra_security::key_slot_store::JsonKeySlotStore;
use uc_infra_security::{
    DefaultCurrentProfile, InMemorySession, KeyMaterialStore, ProfileContentKeyVault,
    RuntimeSpaceAccessAdapter,
};

#[derive(Default)]
struct MemorySecureStorage(Mutex<std::collections::BTreeMap<String, Vec<u8>>>);

impl SecureStoragePort for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

#[derive(Default)]
struct MemoryLegacyBootstrapRepository {
    record: Mutex<Option<LegacyBootstrapRecord>>,
    stage: Mutex<Option<LegacyBootstrapStage>>,
}

#[async_trait]
impl LegacyBootstrapRepositoryPort for MemoryLegacyBootstrapRepository {
    async fn begin_legacy_bootstrap(
        &self,
        prepared: &LegacyBootstrapRecord,
    ) -> Result<LegacyBootstrapRecord, BootstrapError> {
        let mut record = self.record.lock().unwrap();
        if let Some(existing) = record.as_ref() {
            return Ok(existing.clone());
        }
        *record = Some(prepared.clone());
        Ok(prepared.clone())
    }

    async fn stage_legacy_bootstrap(
        &self,
        stage: &LegacyBootstrapStage,
    ) -> Result<(), BootstrapError> {
        *self.record.lock().unwrap() = Some(stage.record().clone());
        *self.stage.lock().unwrap() = Some(stage.clone());
        Ok(())
    }

    async fn activate_legacy_bootstrap(
        &self,
        bootstrap_id: &BootstrapId,
        now_ms: i64,
    ) -> Result<LegacyBootstrapRecord, BootstrapError> {
        let mut record = self.record.lock().unwrap();
        let current = record.as_mut().ok_or(BootstrapError::InvalidRecord)?;
        if current.bootstrap_id() != bootstrap_id {
            return Err(BootstrapError::InvalidRecord);
        }
        if current.status() == LegacyBootstrapStatus::Staged {
            let status = if current.pending_readmission().is_empty() {
                LegacyBootstrapStatus::Complete
            } else {
                LegacyBootstrapStatus::AwaitingReadmission
            };
            current.transition_to(status, now_ms)?;
        }
        Ok(current.clone())
    }

    async fn load_legacy_bootstrap_stage(
        &self,
        bootstrap_id: &BootstrapId,
    ) -> Result<Option<LegacyBootstrapStage>, BootstrapError> {
        Ok(self
            .stage
            .lock()
            .unwrap()
            .as_ref()
            .filter(|stage| stage.record().bootstrap_id() == bootstrap_id)
            .cloned())
    }

    async fn get_legacy_bootstrap(
        &self,
        bootstrap_id: &BootstrapId,
    ) -> Result<Option<LegacyBootstrapRecord>, BootstrapError> {
        Ok(self
            .record
            .lock()
            .unwrap()
            .as_ref()
            .filter(|record| record.bootstrap_id() == bootstrap_id)
            .cloned())
    }

    async fn list_incomplete_legacy_bootstraps_for_space(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<LegacyBootstrapRecord>, BootstrapError> {
        Ok(self
            .record
            .lock()
            .unwrap()
            .iter()
            .filter(|record| record.space_id() == space_id && !record.status().is_terminal())
            .cloned()
            .collect())
    }

    async fn list_non_complete_legacy_bootstraps_for_space(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<LegacyBootstrapRecord>, BootstrapError> {
        Ok(self
            .record
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                record.space_id() == space_id && record.status() != LegacyBootstrapStatus::Complete
            })
            .cloned()
            .collect())
    }

    async fn acknowledge_legacy_readmission(
        &self,
        bootstrap_id: &BootstrapId,
        member: &DeviceId,
        now_ms: i64,
    ) -> Result<LegacyBootstrapRecord, BootstrapError> {
        let mut record = self.record.lock().unwrap();
        let current = record.as_mut().ok_or(BootstrapError::InvalidRecord)?;
        if current.bootstrap_id() != bootstrap_id {
            return Err(BootstrapError::InvalidRecord);
        }
        current.mark_readmitted(member, now_ms)?;
        Ok(current.clone())
    }
}

#[tokio::test]
async fn current_revocation_snapshot_survives_repository_restart() {
    let directory = tempdir().unwrap();
    let database_url = directory.path().join("current-revocation-restart.sqlite");
    let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
    let session = Arc::new(InMemorySession::new());
    let space_id = SpaceId::from("space-revocation-restart");
    session.set_master_key_for_space(
        space_id.clone(),
        MasterKey::from_bytes(&[0x41; 32]).unwrap(),
    );
    let repository = Arc::new(DieselSpaceSecurityStore::new(
        DieselSqliteExecutor::new(pool.clone()),
        session.as_ref().clone(),
    ));
    let record = RevocationRecord::prepare_with_recipients(
        RevocationId::from_string("revocation-restart").unwrap(),
        space_id,
        DeviceId::new("dev-removed"),
        vec![DeviceId::new("dev-c"), DeviceId::new("dev-d")],
        GroupEpoch::new(1),
        123,
    )
    .unwrap();
    repository.begin_revocation(&record).await.unwrap();
    drop(repository);

    let reopened: Arc<dyn RevocationRepositoryPort> = Arc::new(DieselSpaceSecurityStore::new(
        DieselSqliteExecutor::new(pool),
        session.as_ref().clone(),
    ));
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let key_material = Arc::new(KeyMaterialStore::new(
        Arc::clone(&secure_storage) as Arc<dyn SecureStoragePort>,
        Arc::new(JsonKeySlotStore::new(directory.path().to_path_buf())),
    ));
    let vault = Arc::new(ProfileContentKeyVault::new(
        directory.path().join("profile-content-vault"),
        secure_storage,
        [0x61; 16],
    ));
    let restarted = RuntimeSpaceAccessAdapter::new(
        key_material,
        Arc::new(DefaultCurrentProfile::new()),
        session,
        reopened,
        Arc::new(MemoryLegacyBootstrapRepository::default()),
        vault,
    );

    let current = restarted.current_group_revocation().await.unwrap().unwrap();

    assert_eq!(
        current.revocation_id().map(RevocationId::as_str),
        Some("revocation-restart")
    );
    assert_eq!(current.removed_device_ids(), [DeviceId::new("dev-removed")]);
    assert_eq!(
        current.pending_recipient_device_ids(),
        [DeviceId::new("dev-c"), DeviceId::new("dev-d")]
    );
    assert_eq!(current.pending_recipients(), 2);
    assert_eq!(current.updated_at_ms(), 123);
}
