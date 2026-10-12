//! `RuntimeSpaceAccessAdapter`（`uc-infra-security`）读取真实 SQLite 仓储
//! （`uc-infra-storage` 的 `DieselSpaceSecurityStore`）持久化结果的跨 crate 集成验证。
//!
//! 这条场景原本随 `RuntimeSpaceAccessAdapter` 的内部单测存在；真实数据库仓储
//! 位于 `uc-infra-storage`，而 `uc-infra-security` 不能反向依赖它，两者不能
//! 共享同一个内部测试模块，因此放在这里作为 `uc-infra-profile` 的集成测试：
//! 本 crate 同时依赖 `uc-infra-security` 与 `uc-infra-storage`，这个方向不产生
//! 循环依赖。
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tempfile::{tempdir, TempDir};
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    BootstrapError, BootstrapId, GroupEpoch, GroupRevocationPort, LegacyBootstrapRecord,
    LegacyBootstrapRepositoryPort, LegacyBootstrapStage, LegacyBootstrapStatus, RevocationId,
    RevocationRecord, RevocationRepositoryPort,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use uc_infra_crypto::mls_group::{MlsClientState, MlsGroupEngine};
use uc_infra_crypto::secrets::MasterKey;
use uc_infra_security::key_slot_store::JsonKeySlotStore;
use uc_infra_security::{
    group_update_apply_verdict, DefaultCurrentProfile, GroupUpdateVerdict, InMemorySession,
    KeyMaterialStore, ProfileContentKeyVault, RuntimeSpaceAccessAdapter,
};
use uc_infra_storage::db::executor::DieselSqliteExecutor;
use uc_infra_storage::db::pool::init_db_pool;
use uc_infra_storage::db::repositories::DieselSpaceSecurityStore;

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

struct GroupFixture {
    directory: TempDir,
    storage: Arc<MemorySecureStorage>,
    session: Arc<InMemorySession>,
    repository: Arc<DieselSpaceSecurityStore<DieselSqliteExecutor>>,
    vault: Arc<ProfileContentKeyVault>,
    access: RuntimeSpaceAccessAdapter,
}

impl GroupFixture {
    fn crash_at_update_boundary(
        self,
        space_id: &SpaceId,
        payload: &[u8],
        fail_install: bool,
    ) -> Self {
        let master = self.session.get_master_key().unwrap();
        let input = CrashInput {
            directory: self.directory.path().to_path_buf(),
            space_id: space_id.as_ref().to_owned(),
            master: master.as_bytes().to_vec(),
            storage: self.storage.0.lock().unwrap().clone(),
            payload: payload.to_vec(),
            fail_install,
        };
        let Self {
            directory,
            storage,
            session,
            repository,
            vault,
            access,
        } = self;
        vault.close();
        drop(access);
        drop(repository);
        drop(vault);
        drop(session);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_group_update_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("UC_GROUP_UPDATE_CRASH_CHILD", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        // 宿主密钥只经匿名 stdin 管道交给本地测试进程，不写明文文件或输出。
        let mut stdin = child.stdin.take().unwrap();
        serde_json::to_writer(&mut stdin, &input).unwrap();
        writeln!(stdin).unwrap();
        drop(stdin);
        let mut marker = false;
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            if line.unwrap().ends_with("GROUP_UPDATE_CRASH_BOUNDARY") {
                marker = true;
                break;
            }
        }
        if !marker {
            let status = child.wait().unwrap();
            panic!("child did not reach durable boundary: {status}");
        }
        // 真实强杀，不运行 Runtime、vault 或仓储的 Drop/关闭流程。
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(space_id.clone(), master);
        Self::open(directory, storage, session)
    }

    fn new(space_id: &SpaceId) -> Self {
        let directory = tempdir().unwrap();
        let storage = Arc::new(MemorySecureStorage::default());
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            space_id.clone(),
            MasterKey::from_bytes(&[0x51; 32]).unwrap(),
        );
        Self::open(directory, storage, session)
    }

    fn open(
        directory: TempDir,
        storage: Arc<MemorySecureStorage>,
        session: Arc<InMemorySession>,
    ) -> Self {
        let pool = init_db_pool(directory.path().join("group.sqlite").to_str().unwrap()).unwrap();
        let repository = Arc::new(DieselSpaceSecurityStore::new(
            DieselSqliteExecutor::new(pool),
            session.as_ref().clone(),
        ));
        let vault = Arc::new(ProfileContentKeyVault::new(
            directory.path().join("vault"),
            storage.clone(),
            [0x61; 16],
        ));
        let access = RuntimeSpaceAccessAdapter::new(
            Arc::new(KeyMaterialStore::new(
                storage.clone(),
                Arc::new(JsonKeySlotStore::new(directory.path().to_path_buf())),
            )),
            Arc::new(DefaultCurrentProfile::new()),
            session.clone(),
            repository.clone(),
            Arc::new(MemoryLegacyBootstrapRepository::default()),
            vault.clone(),
        );
        Self {
            directory,
            storage,
            session,
            repository,
            vault,
            access,
        }
    }

    fn restart(self, space_id: &SpaceId) -> Self {
        let root = self.session.get_master_key().unwrap();
        let Self {
            directory,
            storage,
            session,
            repository,
            vault,
            access,
        } = self;
        // 旧会话、仓储和安装能力全部释放；从真实数据库重新打开。
        vault.close();
        drop(access);
        drop(repository);
        drop(vault);
        drop(session);
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(space_id.clone(), root);
        Self::open(directory, storage, session)
    }
}

#[tokio::test]
async fn saved_mls_update_reinstalls_from_sqlite_and_resumes_the_next_epoch() {
    let space_id = SpaceId::from("sqlite-mls-restart");
    let sponsor = GroupFixture::new(&space_id);
    let bob = GroupFixture::new(&space_id);
    let initial = MlsGroupEngine::create_sponsor(space_id.as_ref().as_bytes(), b"alice").unwrap();
    let material = sponsor
        .session
        .create_legacy_bootstrap_material(&space_id, initial.into_bytes(), 1)
        .unwrap();
    sponsor
        .repository
        .save_space_material(&material)
        .await
        .unwrap();
    sponsor.session.install_space_material(&material).unwrap();
    let alice = DeviceId::new("alice");
    let bob_id = DeviceId::new("bob");
    let pending = bob.access.prepare_group_join(&bob_id).await.unwrap();
    let admission = sponsor
        .access
        .admit_group_member(&space_id, &alice, &bob_id, &[], &pending.key_package)
        .await
        .unwrap();
    bob.access
        .install_group_join(
            &space_id,
            &Passphrase::new("sqlite integration passphrase"),
            pending,
            &admission.welcome,
            &admission.encrypted_key_catalog,
            admission.group_epoch,
        )
        .await
        .unwrap();
    let charlie = DeviceId::new("charlie");
    let pending = sponsor.access.prepare_group_join(&charlie).await.unwrap();
    let baseline = sponsor
        .access
        .admit_group_member(&space_id, &alice, &charlie, &[bob_id], &pending.key_package)
        .await
        .unwrap();
    bob.access
        .apply_group_epoch_update(baseline.existing_member_updates[0].payload())
        .await
        .unwrap();
    let before = bob
        .session
        .current_content_protection_key()
        .unwrap()
        .epoch();
    assert_eq!(before.value(), 3);
    let dave = DeviceId::new("dave");
    let pending = sponsor.access.prepare_group_join(&dave).await.unwrap();
    let fourth = sponsor
        .access
        .admit_group_member(&space_id, &alice, &dave, &[bob_id], &pending.key_package)
        .await
        .unwrap();
    assert_eq!(fourth.group_epoch, 4);
    let fourth_payload = fourth.existing_member_updates[0].payload();
    let bob = bob.crash_at_update_boundary(&space_id, fourth_payload, true);
    let saved = bob
        .repository
        .load_space_material(&space_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.state().epoch().value(), fourth.group_epoch);
    assert!(bob.session.current_content_protection_key().is_err());
    bob.vault.close();
    assert!(bob
        .access
        .apply_group_epoch_update(fourth_payload)
        .await
        .is_err());

    let bob = bob.crash_at_update_boundary(&space_id, fourth_payload, false);
    assert_eq!(
        bob.repository
            .load_space_material(&space_id)
            .await
            .unwrap()
            .unwrap(),
        saved
    );
    assert_eq!(
        bob.access
            .apply_group_epoch_update(fourth_payload)
            .await
            .unwrap()
            .value(),
        fourth.group_epoch
    );
    assert_eq!(
        bob.session
            .current_content_protection_key()
            .unwrap()
            .epoch()
            .value(),
        fourth.group_epoch
    );
    // 第四次已安装、尚未确认时再重启；重复包不能再次推进 MLS。
    let bob = bob.restart(&space_id);
    bob.access
        .apply_group_epoch_update(fourth_payload)
        .await
        .unwrap();
    assert_eq!(
        bob.repository
            .load_space_material(&space_id)
            .await
            .unwrap()
            .unwrap(),
        saved
    );
    let eve = DeviceId::new("eve");
    let pending = sponsor.access.prepare_group_join(&eve).await.unwrap();
    let fifth = sponsor
        .access
        .admit_group_member(&space_id, &alice, &eve, &[bob_id], &pending.key_package)
        .await
        .unwrap();
    assert_eq!(fifth.group_epoch, 5);
    bob.access
        .apply_group_epoch_update(fifth.existing_member_updates[0].payload())
        .await
        .unwrap();
    bob.access
        .apply_group_epoch_update(fourth_payload)
        .await
        .unwrap();
    let final_material = bob
        .repository
        .load_space_material(&space_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        MlsGroupEngine::current_epoch(&MlsClientState::from_bytes(
            final_material.group_state().to_vec()
        ))
        .unwrap(),
        fifth.group_epoch
    );
    assert_eq!(
        bob.session
            .current_content_protection_key()
            .unwrap()
            .epoch()
            .value(),
        fifth.group_epoch
    );
}

#[derive(Serialize, Deserialize)]
struct CrashInput {
    directory: PathBuf,
    space_id: String,
    master: Vec<u8>,
    storage: BTreeMap<String, Vec<u8>>,
    payload: Vec<u8>,
    fail_install: bool,
}

/// 只由上方父测试经匿名管道驱动；到达真实持久边界后等待父进程强杀。
#[tokio::test]
#[ignore = "真实持久边界强杀验收的内部子进程，由父测试驱动"]
async fn durable_group_update_crash_child() {
    if std::env::var_os("UC_GROUP_UPDATE_CRASH_CHILD").is_none() {
        return;
    }
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
    let input: CrashInput = serde_json::from_str(&input).unwrap();
    let storage = Arc::new(MemorySecureStorage(Mutex::new(input.storage)));
    let session = Arc::new(InMemorySession::new());
    let space_id = SpaceId::from(input.space_id);
    session.set_master_key_for_space(
        space_id.clone(),
        MasterKey::from_bytes(&input.master).unwrap(),
    );
    let repository = Arc::new(DieselSpaceSecurityStore::new(
        DieselSqliteExecutor::new(
            init_db_pool(input.directory.join("group.sqlite").to_str().unwrap()).unwrap(),
        ),
        session.as_ref().clone(),
    ));
    let vault = Arc::new(ProfileContentKeyVault::new(
        input.directory.join("vault"),
        storage.clone(),
        [0x61; 16],
    ));
    let access = RuntimeSpaceAccessAdapter::new(
        Arc::new(KeyMaterialStore::new(
            storage,
            Arc::new(JsonKeySlotStore::new(input.directory)),
        )),
        Arc::new(DefaultCurrentProfile::new()),
        session.clone(),
        repository.clone(),
        Arc::new(MemoryLegacyBootstrapRepository::default()),
        vault.clone(),
    );
    if input.fail_install {
        vault.close();
    }
    let result = access.apply_group_epoch_update(&input.payload).await;
    let saved = repository
        .load_space_material(&space_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.state().epoch().value(), 4);
    assert_eq!(
        MlsGroupEngine::current_epoch(&MlsClientState::from_bytes(saved.group_state().to_vec()))
            .unwrap(),
        4
    );
    if input.fail_install {
        assert_ne!(
            group_update_apply_verdict(&result.unwrap_err()),
            GroupUpdateVerdict::Rejected
        );
        assert!(session.current_content_protection_key().is_err());
    } else {
        assert_eq!(result.unwrap().value(), 4);
        assert_eq!(
            session
                .current_content_protection_key()
                .unwrap()
                .epoch()
                .value(),
            4
        );
    }
    println!("GROUP_UPDATE_CRASH_BOUNDARY");
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
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
