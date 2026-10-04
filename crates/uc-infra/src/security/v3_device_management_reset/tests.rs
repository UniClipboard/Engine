use crate::security::space_transition_activation::WithoutProfileVault;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use diesel::connection::SimpleConnection as _;
use diesel::{Connection as _, SqliteConnection};
use tempfile::{tempdir, TempDir};
use uc_application::deps::{AdmissionSpaceTransitionError, DeviceManagementResetDataPort};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ActiveRuntimeLayout, ActiveSpaceGenerationManifestV2, RevocationRepositoryPort,
};
use uc_core::ports::security::current_profile::CurrentProfilePort;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::V3DeviceManagementReset;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::{init_db_pool, DbPool};
use crate::db::repositories::DieselSpaceSecurityStore;
use crate::security::active_space_generation_manifest_store::V3ManifestPromotionOutcome;
use crate::security::{
    ActiveRuntimeManifest, ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    AdmissionKeyManager, DefaultCurrentProfile, MasterKey, ProfileContentKeyVault,
    ProfileRuntimeLayout, SpaceControlGeneration, SpaceTransitionActivation,
};
use crate::space::{InMemorySession, KeyMaterialStore, RuntimeSpaceAccessAdapter};
use uc_infra_security::key_slot_store::JsonKeySlotStore;

#[derive(Default)]
struct MemorySecureStorage(Mutex<HashMap<String, Vec<u8>>>);

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

#[tokio::test]
async fn v3_device_reset_replaces_only_the_control_generation() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("profile");
    let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
    let current_profile: Arc<dyn CurrentProfilePort> = Arc::new(DefaultCurrentProfile::new());
    let admission_keys = Arc::new(AdmissionKeyManager::new(
        Arc::clone(&secure_storage),
        [0x81; 16],
    ));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        root.join("vault"),
        Arc::clone(&admission_keys),
    ));
    let source_space = SpaceId::from_str("reset-source-space");
    let source = ActiveRuntimeManifestV3::new(
        ActiveRuntimeLayout::new(source_space.clone(), [0x82; 16], [0x83; 16]).unwrap(),
        [0x84; 16],
    )
    .unwrap();
    let legacy = ActiveSpaceGenerationManifestV2::new(
        source_space.as_ref().to_owned(),
        [0x84; 16],
        [0x85; 16],
        [0x86; 16],
    )
    .unwrap();
    manifests.promote(&legacy).await.unwrap();
    assert_eq!(
        manifests
            .promote_v3_from_v2(&legacy, &source)
            .await
            .unwrap(),
        V3ManifestPromotionOutcome::Promoted
    );

    let source_layout = ProfileRuntimeLayout::v3(&root, &source);
    std::fs::create_dir_all(source_layout.profile_database().parent().unwrap()).unwrap();
    std::fs::write(source_layout.profile_database(), b"reset-retained-profile").unwrap();
    std::fs::create_dir_all(source_layout.blob_root()).unwrap();
    std::fs::write(
        source_layout.blob_root().join("history.ucbl"),
        b"reset-retained-blob",
    )
    .unwrap();
    std::fs::create_dir_all(source_layout.control_database().parent().unwrap()).unwrap();
    let control_pool = init_db_pool(source_layout.control_database().to_str().unwrap()).unwrap();
    let session = Arc::new(InMemorySession::new());
    let master_key = MasterKey::from_bytes(&[0x87; 32]).unwrap();
    session.set_master_key_for_space(source_space.clone(), master_key.clone());
    let repository = Arc::new(DieselSpaceSecurityStore::new(
        Arc::new(DieselSqliteExecutor::new(control_pool.clone())),
        session.as_ref().clone(),
    ));
    let source_material = session
        .create_legacy_bootstrap_material(&source_space, b"source-mls-state".to_vec(), 1)
        .unwrap();
    repository
        .save_space_material(&source_material)
        .await
        .unwrap();
    let vault = Arc::new(ProfileContentKeyVault::new(
        root.join("vault"),
        Arc::clone(&secure_storage),
        [0x81; 16],
    ));
    let access = Arc::new(RuntimeSpaceAccessAdapter::new(
        Arc::new(KeyMaterialStore::new(
            Arc::clone(&secure_storage),
            Arc::new(JsonKeySlotStore::new(root.join("keys"))),
        )),
        Arc::clone(&current_profile),
        Arc::clone(&session),
        repository.clone(),
        repository.clone(),
        vault,
    ));
    let generations = Arc::new(SpaceControlGeneration::new(
        root.clone(),
        access.clone(),
        current_profile,
        Arc::clone(&admission_keys),
    ));
    let activation = Arc::new(SpaceTransitionActivation::new(
        root.clone(),
        control_pool.clone(),
        Arc::clone(&manifests),
        Arc::clone(&generations),
        access,
        Arc::new(WithoutProfileVault),
    ));
    let reset = V3DeviceManagementReset::new(
        root.clone(),
        control_pool,
        Arc::clone(&manifests),
        generations,
        activation,
    );
    let target_space = SpaceId::from_str("reset-target-space");

    reset
        .prepare_device_management_reset(&target_space)
        .await
        .unwrap();
    reset
        .stage_device_management_reset_mutations(&target_space)
        .await
        .unwrap();

    // Application 的既有 rebuild 流程在 stage 后保留 MasterKey、切换 Space，
    // 并通过正式 control repository 写入新的本机 MLS/security material。
    session.set_master_key_for_space(target_space.clone(), master_key.clone());
    let target_material = session
        .create_legacy_bootstrap_material(&target_space, b"target-mls-state".to_vec(), 2)
        .unwrap();
    repository
        .save_space_material(&target_material)
        .await
        .unwrap();

    reset
        .promote_device_management_reset(&target_space)
        .await
        .unwrap();
    reset
        .finalize_device_management_reset(&target_space)
        .await
        .unwrap();

    let Some(ActiveRuntimeManifest::V3(active)) = manifests.load_runtime().await.unwrap() else {
        panic!("reset target manifest is not active");
    };
    assert_eq!(active.layout().space_id(), &target_space);
    assert_eq!(active.layout().profile_data_generation(), &[0x82; 16]);
    assert_eq!(active.keyslot_generation(), &[0x84; 16]);
    assert_ne!(active.layout().space_control_generation(), &[0x83; 16]);
    assert_eq!(session.current_space_id().unwrap(), target_space);
    assert_eq!(session.get_master_key().unwrap(), master_key);
    assert_eq!(
        std::fs::read(source_layout.profile_database()).unwrap(),
        b"reset-retained-profile"
    );
    assert_eq!(
        std::fs::read(source_layout.blob_root().join("history.ucbl")).unwrap(),
        b"reset-retained-blob"
    );
    assert!(!source_layout.control_database().exists());
    let target_layout = ProfileRuntimeLayout::v3(&root, &active);
    assert!(target_layout.control_database().is_file());
    assert_no_forbidden_paths(
        &root,
        &[
            "source-backup.sqlite",
            "source-final.sqlite",
            "target.sqlite",
        ],
    );
}

fn assert_no_forbidden_paths(root: &std::path::Path, forbidden: &[&str]) {
    for entry in std::fs::read_dir(root).unwrap().filter_map(Result::ok) {
        assert!(!forbidden.contains(&entry.file_name().to_string_lossy().as_ref()));
        if entry.file_type().unwrap().is_dir() {
            assert_no_forbidden_paths(&entry.path(), forbidden);
        }
    }
}

const PROFILE_GENERATION: [u8; 16] = [0x81; 16];
const MASTER_KEY: [u8; 32] = [0x87; 32];

/// 一份已激活 V3 资料：来源 control generation 中有来源 Space 的安全材料。
struct ResetProfile {
    _directory: TempDir,
    root: PathBuf,
    secure_storage: Arc<dyn SecureStoragePort>,
    source_space: SpaceId,
    source: ActiveRuntimeManifestV3,
    target_space: SpaceId,
}

/// 一个 Engine 进程对该资料持有的完整 Reset 依赖；丢弃即模拟进程退出。
struct ResetProcess {
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    session: Arc<InMemorySession>,
    control_pool: DbPool,
    repository: Arc<DieselSpaceSecurityStore<Arc<DieselSqliteExecutor>>>,
    generations: Arc<SpaceControlGeneration>,
    activation: Arc<SpaceTransitionActivation>,
    reset: V3DeviceManagementReset,
}

impl ResetProfile {
    async fn create() -> Self {
        let directory = tempdir().unwrap();
        let root = directory.path().join("profile");
        let secure_storage: Arc<dyn SecureStoragePort> = Arc::new(MemorySecureStorage::default());
        let admission_keys = Arc::new(AdmissionKeyManager::new(
            Arc::clone(&secure_storage),
            PROFILE_GENERATION,
        ));
        let manifests = ActiveSpaceGenerationManifestStore::new(root.join("vault"), admission_keys);
        let source_space = SpaceId::from_str("reset-source-space");
        let source = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(source_space.clone(), [0x82; 16], [0x83; 16]).unwrap(),
            [0x84; 16],
        )
        .unwrap();
        let legacy = ActiveSpaceGenerationManifestV2::new(
            source_space.as_ref().to_owned(),
            [0x84; 16],
            [0x85; 16],
            [0x86; 16],
        )
        .unwrap();
        manifests.promote(&legacy).await.unwrap();
        manifests
            .promote_v3_from_v2(&legacy, &source)
            .await
            .unwrap();
        let layout = ProfileRuntimeLayout::v3(&root, &source);
        fs::create_dir_all(layout.profile_database().parent().unwrap()).unwrap();
        fs::write(layout.profile_database(), b"reset-retained-profile").unwrap();
        fs::create_dir_all(layout.control_database().parent().unwrap()).unwrap();
        let pool = init_db_pool(layout.control_database().to_str().unwrap()).unwrap();
        let session = InMemorySession::new();
        session.set_master_key_for_space(
            source_space.clone(),
            MasterKey::from_bytes(&MASTER_KEY).unwrap(),
        );
        let material = session
            .create_legacy_bootstrap_material(&source_space, b"source-mls-state".to_vec(), 1)
            .unwrap();
        DieselSpaceSecurityStore::new(Arc::new(DieselSqliteExecutor::new(pool)), session)
            .save_space_material(&material)
            .await
            .unwrap();
        Self {
            _directory: directory,
            root,
            secure_storage,
            source_space,
            source,
            target_space: SpaceId::from_str("reset-target-space"),
        }
    }

    /// 以资料当前的活动 manifest 启动一个新进程。
    async fn open(&self) -> ResetProcess {
        let current_profile: Arc<dyn CurrentProfilePort> = Arc::new(DefaultCurrentProfile::new());
        let admission_keys = Arc::new(AdmissionKeyManager::new(
            Arc::clone(&self.secure_storage),
            PROFILE_GENERATION,
        ));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            self.root.join("vault"),
            Arc::clone(&admission_keys),
        ));
        let Some(ActiveRuntimeManifest::V3(active)) = manifests.load_runtime().await.unwrap()
        else {
            panic!("profile has no active V3 manifest");
        };
        let layout = ProfileRuntimeLayout::v3(&self.root, &active);
        let control_pool = init_db_pool(layout.control_database().to_str().unwrap()).unwrap();
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            active.layout().space_id().clone(),
            MasterKey::from_bytes(&MASTER_KEY).unwrap(),
        );
        let repository = Arc::new(DieselSpaceSecurityStore::new(
            Arc::new(DieselSqliteExecutor::new(control_pool.clone())),
            session.as_ref().clone(),
        ));
        let vault = Arc::new(ProfileContentKeyVault::new(
            self.root.join("vault"),
            Arc::clone(&self.secure_storage),
            PROFILE_GENERATION,
        ));
        let access = Arc::new(RuntimeSpaceAccessAdapter::new(
            Arc::new(KeyMaterialStore::new(
                Arc::clone(&self.secure_storage),
                Arc::new(JsonKeySlotStore::new(self.root.join("keys"))),
            )),
            Arc::clone(&current_profile),
            Arc::clone(&session),
            repository.clone(),
            repository.clone(),
            vault,
        ));
        let generations = Arc::new(SpaceControlGeneration::new(
            self.root.clone(),
            access.clone(),
            current_profile,
            admission_keys,
        ));
        let activation = Arc::new(SpaceTransitionActivation::new(
            self.root.clone(),
            control_pool.clone(),
            Arc::clone(&manifests),
            Arc::clone(&generations),
            access,
            Arc::new(WithoutProfileVault),
        ));
        let reset = V3DeviceManagementReset::new(
            self.root.clone(),
            control_pool.clone(),
            Arc::clone(&manifests),
            Arc::clone(&generations),
            Arc::clone(&activation),
        );
        ResetProcess {
            manifests,
            session,
            control_pool,
            repository,
            generations,
            activation,
            reset,
        }
    }

    /// 控制代目录（忽略租约与临时目录）。
    fn control_generations(&self) -> Vec<PathBuf> {
        let layout = ProfileRuntimeLayout::v3(&self.root, &self.source);
        let parent = layout
            .control_database()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        let mut directories = fs::read_dir(parent)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().unwrap().is_dir())
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        directories.sort();
        directories
    }
}

impl ResetProcess {
    /// Application 重建在 stage 之后经正式 control repository 写入的目标 Space 材料。
    async fn write_target_material(&self, profile: &ResetProfile, marker: &[u8]) {
        self.session.set_master_key_for_space(
            profile.target_space.clone(),
            MasterKey::from_bytes(&MASTER_KEY).unwrap(),
        );
        let material = self
            .session
            .create_legacy_bootstrap_material(&profile.target_space, marker.to_vec(), 2)
            .unwrap();
        self.repository
            .save_space_material(&material)
            .await
            .unwrap();
    }

    /// 经运行期连接池读取指定 Space 的材料；stage 之后该连接池指向目标控制代。
    async fn has_material(&self, space: &SpaceId) -> bool {
        self.repository
            .load_space_material(space)
            .await
            .unwrap()
            .is_some()
    }

    async fn staged_target(&self) -> ActiveRuntimeManifestV3 {
        let journal = self
            .manifests
            .load_device_reset_journal_v3()
            .await
            .unwrap()
            .expect("device reset journal is missing");
        V3DeviceManagementReset::target_manifest(&journal).unwrap()
    }

    async fn prepare_and_stage(&self, profile: &ResetProfile) {
        self.reset
            .prepare_device_management_reset(&profile.target_space)
            .await
            .unwrap();
        self.reset
            .stage_device_management_reset_mutations(&profile.target_space)
            .await
            .unwrap();
    }

    async fn promote_and_finalize(&self, profile: &ResetProfile) {
        self.reset
            .promote_device_management_reset(&profile.target_space)
            .await
            .unwrap();
        self.reset
            .finalize_device_management_reset(&profile.target_space)
            .await
            .unwrap();
    }
}

async fn active_space(profile: &ResetProfile) -> SpaceId {
    let process = profile.open().await;
    let Some(ActiveRuntimeManifest::V3(active)) = process.manifests.load_runtime().await.unwrap()
    else {
        panic!("profile has no active V3 manifest");
    };
    active.layout().space_id().clone()
}

/// 首次运行在 stage 后已改写目标（写入目标材料）却未提升；重启后目标必须由未改变的来源重新
/// 快照，而不是在已改写的目标上重做重建。
#[tokio::test]
async fn restart_rebuilds_a_staged_target_from_the_unchanged_source() {
    let profile = ResetProfile::create().await;
    {
        let first = profile.open().await;
        first.prepare_and_stage(&profile).await;
        first
            .write_target_material(&profile, b"first-attempt")
            .await;
    }

    let second = profile.open().await;
    second.prepare_and_stage(&profile).await;

    assert!(
        !second.has_material(&profile.target_space).await,
        "the staged target kept the first attempt's rebuild output"
    );
    assert!(
        second.has_material(&profile.source_space).await,
        "the rebuilt target is not a snapshot of the source"
    );
    second
        .write_target_material(&profile, b"second-attempt")
        .await;
    second.promote_and_finalize(&profile).await;
    drop(second);
    assert_eq!(active_space(&profile).await, profile.target_space);
    assert_eq!(profile.control_generations().len(), 1);
}

/// 同一进程内提交失败后重试：运行期连接池仍指向目标，重试必须先回到来源再重新快照。
#[tokio::test]
async fn retry_in_the_same_process_rebuilds_the_staged_target_from_the_source() {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _guard = logs.install();
    let profile = ResetProfile::create().await;
    let process = profile.open().await;
    process.prepare_and_stage(&profile).await;
    process
        .write_target_material(&profile, b"first-attempt")
        .await;

    process.prepare_and_stage(&profile).await;

    assert!(!process.has_material(&profile.target_space).await);
    assert!(process.has_material(&profile.source_space).await);
    process
        .write_target_material(&profile, b"second-attempt")
        .await;
    process.promote_and_finalize(&profile).await;
    drop(process);
    assert_eq!(active_space(&profile).await, profile.target_space);
    assert_eq!(profile.control_generations().len(), 1);
    assert_eq!(logs.count("rewinds a staged target"), 1);
    assert_eq!(logs.count("promotes the staged target"), 1);
    assert!(logs.output().contains("previous_phase=\"Staged\""));
}

/// 删除已改写目标后、改回日志前崩溃：目标目录已不存在，重启不得打开一个只有表结构的空库。
#[tokio::test]
async fn a_missing_staged_target_is_rebuilt_from_the_source_instead_of_opened_empty() {
    let profile = ResetProfile::create().await;
    let target_directory = {
        let first = profile.open().await;
        first.prepare_and_stage(&profile).await;
        first
            .write_target_material(&profile, b"first-attempt")
            .await;
        let target = first.staged_target().await;
        ProfileRuntimeLayout::v3(&profile.root, &target)
            .control_database()
            .parent()
            .unwrap()
            .to_path_buf()
    };
    fs::remove_dir_all(&target_directory).unwrap();

    let second = profile.open().await;
    second.prepare_and_stage(&profile).await;

    assert!(second.has_material(&profile.source_space).await);
    assert!(!second.has_material(&profile.target_space).await);
}

/// manifest 已提升到目标、日志尚未推进时崩溃：目标已是当前数据，重启只能向前收尾，绝不能丢弃。
#[tokio::test]
async fn an_activated_target_is_finished_instead_of_discarded() {
    let logs = uc_testkit::log_capture::CapturedLogs::default();
    let _guard = logs.install();
    let profile = ResetProfile::create().await;
    {
        let first = profile.open().await;
        first.prepare_and_stage(&profile).await;
        first.write_target_material(&profile, b"activated").await;
        let target = first.staged_target().await;
        let prepared = first
            .generations
            .finalize_device_reset_target(&profile.source, &target, &first.control_pool)
            .await
            .unwrap();
        first
            .activation
            .activate_device_reset(&profile.source, &prepared)
            .await
            .unwrap();
    }

    let second = profile.open().await;
    second.prepare_and_stage(&profile).await;
    assert!(second.has_material(&profile.target_space).await);
    second.promote_and_finalize(&profile).await;

    assert!(second.has_material(&profile.target_space).await);
    drop(second);
    assert_eq!(active_space(&profile).await, profile.target_space);
    assert_eq!(profile.control_generations().len(), 1);
    assert_eq!(logs.count("finds the target already active"), 1);
    assert!(logs.output().contains("reason=\"already_target\""));
}

/// 提交时另一连接短暂持有目标库写锁（例如成员维护的写事务）：提交应等待其结束后成功。
#[tokio::test]
async fn commit_waits_for_a_short_concurrent_writer() {
    let profile = ResetProfile::create().await;
    let process = profile.open().await;
    process.prepare_and_stage(&profile).await;
    process.write_target_material(&profile, b"rebuilt").await;
    let target = process.staged_target().await;
    let database = ProfileRuntimeLayout::v3(&profile.root, &target)
        .control_database()
        .to_str()
        .unwrap()
        .to_owned();
    let (locked, wait_locked) = mpsc::channel();
    let writer = thread::spawn(move || {
        let mut connection = SqliteConnection::establish(&database).unwrap();
        connection.batch_execute("BEGIN IMMEDIATE;").unwrap();
        locked.send(()).unwrap();
        thread::sleep(Duration::from_millis(300));
        connection.batch_execute("COMMIT;").unwrap();
    });
    wait_locked.recv().unwrap();

    process.promote_and_finalize(&profile).await;

    writer.join().unwrap();
    drop(process);
    assert_eq!(active_space(&profile).await, profile.target_space);
}

/// 写锁一直不释放：提交必须报告为暂时不可用（可重试），释放后在同一状态上重试即可完成。
#[tokio::test]
async fn a_held_writer_lock_is_reported_as_retryable_and_the_commit_can_be_retried() {
    let profile = ResetProfile::create().await;
    let process = profile.open().await;
    process.prepare_and_stage(&profile).await;
    process.write_target_material(&profile, b"rebuilt").await;
    let target = process.staged_target().await;
    let database = ProfileRuntimeLayout::v3(&profile.root, &target)
        .control_database()
        .to_str()
        .unwrap()
        .to_owned();
    let mut writer = SqliteConnection::establish(&database).unwrap();
    writer.batch_execute("BEGIN IMMEDIATE;").unwrap();

    let error = process
        .reset
        .promote_device_management_reset(&profile.target_space)
        .await
        .unwrap_err();

    assert!(
        matches!(error, AdmissionSpaceTransitionError::Unavailable { .. }),
        "a busy target must be retryable, got {error:?}"
    );
    writer.batch_execute("ROLLBACK;").unwrap();
    process.promote_and_finalize(&profile).await;
    drop(process);
    assert_eq!(active_space(&profile).await, profile.target_space);
}
