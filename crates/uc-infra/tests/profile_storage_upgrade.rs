use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use diesel::RunQueryDsl;
use uc_application::deps::{
    LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedgerError,
};
use uc_core::crypto::domain::Passphrase;
use uc_core::ids::{ProfileId, SpaceId};
use uc_core::membership::{ActiveSpaceGenerationManifestV2, InvitationId, SpaceAdmissionId};
use uc_core::ports::space::SpaceAccessStore;
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_infra::db::executor::DieselSqliteExecutor;
use uc_infra::db::pool::init_db_pool;
use uc_infra::db::repositories::DieselSpaceSecurityStore;
use uc_infra::fs::key_slot_store::JsonKeySlotStore;
use uc_infra::network::iroh::SpaceAdmissionChannelCredentialPort;
use uc_infra::security::{
    ActiveSpaceGenerationManifestStore, AdmissionKeyManager, DefaultCurrentProfile,
    ProfileContentKeyVault, ProfileRuntimeLayout, ProfileStorageUpgrade,
    ProfileStorageUpgradeError, ProfileStorageUpgradeOutcome,
};
use uc_infra::space::{
    InMemorySession, KeyMaterialStore, RuntimeSpaceAccessAdapter, SqliteSpaceAdmissionCredentials,
    SqliteSpaceAdmissionState,
};

#[derive(Default)]
struct MemorySecureStorage(Mutex<BTreeMap<String, Vec<u8>>>);

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

struct EmptyLedger;

#[async_trait::async_trait]
impl LoadMembershipLedgerPort for EmptyLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Ok(LoadedMembershipLedger::no_current_space())
    }
}

fn regular_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                directories.push(entry.path());
            } else if entry.file_type().unwrap().is_file() {
                files.push((entry.path(), std::fs::read(entry.path()).unwrap()));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn named_file(root: &Path, name: &str) -> PathBuf {
    regular_files(root)
        .into_iter()
        .find_map(|(path, _)| {
            (path.file_name().and_then(|value| value.to_str()) == Some(name)).then_some(path)
        })
        .unwrap()
}

fn new_upgrade(
    root: &Path,
    secure_storage: Arc<dyn SecureStoragePort>,
    keys: Arc<AdmissionKeyManager>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
) -> ProfileStorageUpgrade {
    std::fs::create_dir_all(root).unwrap();
    let database = root.join("source.sqlite");
    let source_pool = init_db_pool(database.to_str().unwrap()).unwrap();
    new_upgrade_from_pool(root, source_pool, secure_storage, keys, manifests)
}

fn new_upgrade_from_pool(
    root: &Path,
    source_pool: uc_infra::db::pool::DbPool,
    secure_storage: Arc<dyn SecureStoragePort>,
    keys: Arc<AdmissionKeyManager>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
) -> ProfileStorageUpgrade {
    let vault = Arc::new(ProfileContentKeyVault::new(
        root.to_path_buf(),
        secure_storage,
        [0xF1; 16],
    ));
    ProfileStorageUpgrade::new_stepwise_for_testing(
        root.to_path_buf(),
        source_pool,
        root.join("blobs"),
        ProfileId::from("default"),
        Arc::new(InMemorySession::new()),
        vault,
        keys,
        manifests,
    )
}

fn new_production_upgrade_from_pool(
    root: &Path,
    source_pool: uc_infra::db::pool::DbPool,
    source_blob_root: PathBuf,
    secure_storage: Arc<dyn SecureStoragePort>,
    keys: Arc<AdmissionKeyManager>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
) -> ProfileStorageUpgrade {
    let vault = Arc::new(ProfileContentKeyVault::new(
        root.to_path_buf(),
        secure_storage,
        [0xF1; 16],
    ));
    ProfileStorageUpgrade::new(
        root.to_path_buf(),
        source_pool,
        source_blob_root,
        ProfileId::from("default"),
        Arc::new(InMemorySession::new()),
        vault,
        keys,
        manifests,
    )
}

#[tokio::test]
async fn production_upgrade_completes_all_pre_promotion_phases_in_one_call() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("profile");
    let source_database = root.join("source.sqlite");
    std::fs::create_dir_all(&root).unwrap();
    let source_pool = init_db_pool(source_database.to_str().unwrap()).unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x0A; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        root.join("vault"),
        Arc::clone(&keys),
    ));
    manifests
        .promote(
            &ActiveSpaceGenerationManifestV2::new(
                "source-space".to_owned(),
                [0x0B; 16],
                [0x0C; 16],
                [0x0D; 16],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let upgrade = new_production_upgrade_from_pool(
        &root,
        source_pool,
        root.join("blobs"),
        secure_storage,
        keys,
        Arc::clone(&manifests),
    );

    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Upgraded
    );
    assert!(manifests.load_v3_sync().unwrap().is_some());
}

#[tokio::test]
async fn runtime_upgrade_resumes_v2_only_after_the_lease_and_promotes_v3() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("profile");
    let vault_path = root.join("vault");
    let profile_id = ProfileId::from("default");
    let source = ActiveSpaceGenerationManifestV2::new(
        "source-space".to_owned(),
        [0x8A; 16],
        [0x8B; 16],
        [0x8C; 16],
    )
    .unwrap();
    let source_root = root
        .join("space-generations")
        .join("b7d79b0ab606d06893ded80236bc914d");
    let source_database = source_root.join("target.sqlite");
    std::fs::create_dir_all(&source_root).unwrap();
    let source_pool = init_db_pool(source_database.to_str().unwrap()).unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x8D; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault_path.clone(),
        Arc::clone(&keys),
    ));
    manifests.promote(&source).await.unwrap();
    let content_vault = Arc::new(ProfileContentKeyVault::new(
        root.join("profile-content-vault"),
        secure_storage.clone(),
        [0x8E; 16],
    ));
    let source_session = Arc::new(InMemorySession::new());
    let source_space_id = SpaceId::from_string(source.space_id.clone());
    let executor = Arc::new(DieselSqliteExecutor::new(source_pool));
    let security_repository = Arc::new(DieselSpaceSecurityStore::new(
        executor,
        source_session.as_ref().clone(),
    ));
    let access = RuntimeSpaceAccessAdapter::new(
        Arc::new(KeyMaterialStore::new(
            secure_storage.clone(),
            Arc::new(JsonKeySlotStore::new(vault_path.clone())),
        )),
        Arc::new(DefaultCurrentProfile::for_profile(profile_id.clone())),
        source_session,
        security_repository.clone(),
        security_repository,
        Arc::clone(&content_vault),
    );
    access
        .initialize(&source_space_id, &Passphrase::new("upgrade-passphrase"))
        .await
        .unwrap();
    drop(access);

    let upgrade = ProfileStorageUpgrade::for_runtime(
        root.clone(),
        root.join("uniclipboard.db"),
        root.join("blobs"),
        profile_id,
        secure_storage,
        vault_path,
        content_vault,
        keys,
        Arc::clone(&manifests),
    );

    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Upgraded
    );
    assert!(manifests.load_v3_sync().unwrap().is_some());
}

#[tokio::test]
async fn promoted_upgrade_cleanup_removes_only_the_old_source_and_staging() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("profile");
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x1A; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        root.join("vault"),
        Arc::clone(&keys),
    ));
    let source = ActiveSpaceGenerationManifestV2::new(
        "source-space".to_owned(),
        [0x1B; 16],
        [0x1C; 16],
        [0x1D; 16],
    )
    .unwrap();
    manifests.promote(&source).await.unwrap();
    let source_root = root
        .join("space-generations")
        .join("0af414868ec02f28ed3973b2aaa9c041");
    let source_database = source_root.join("target.sqlite");
    let source_blobs = source_root.join("blobs");
    std::fs::create_dir_all(&source_blobs).unwrap();
    std::fs::write(source_blobs.join("legacy-blob"), b"legacy").unwrap();
    let source_pool = init_db_pool(source_database.to_str().unwrap()).unwrap();
    let upgrade = new_production_upgrade_from_pool(
        &root,
        source_pool,
        source_blobs,
        secure_storage.clone(),
        Arc::clone(&keys),
        Arc::clone(&manifests),
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Upgraded
    );
    drop(upgrade);
    assert!(source_root.is_dir());
    let active = manifests.load_v3_sync().unwrap().unwrap();
    let active_layout = ProfileRuntimeLayout::v3(&root, &active);
    let active_pool = init_db_pool(active_layout.profile_database().to_str().unwrap()).unwrap();
    diesel::sql_query(
        "CREATE INDEX post_activation_search_probe \
         ON search_document (profile_id, entry_id)",
    )
    .execute(&mut active_pool.get().unwrap())
    .unwrap();
    drop(active_pool);

    let cleanup = ProfileStorageUpgrade::new_cleanup_only(
        root.clone(),
        root.join("uniclipboard.db"),
        root.join("vault").join("blobs"),
        keys,
        manifests,
    );
    assert_eq!(
        cleanup.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::UpToDate
    );

    assert!(!source_root.exists());
    assert!(!root
        .join("profile-storage-upgrade")
        .join(".journal-v1")
        .exists());
    assert!(active_layout.profile_database().is_file());
    assert!(active_layout.control_database().is_file());
    assert!(active_layout.blob_root().is_dir());
}

#[tokio::test]
async fn fresh_profile_returns_the_prepared_v3_generation_pair() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("profile");
    std::fs::create_dir_all(&root).unwrap();
    let source_pool = init_db_pool(root.join("uniclipboard.db").to_str().unwrap()).unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x2A; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        root.join("vault"),
        Arc::clone(&keys),
    ));
    let upgrade = new_production_upgrade_from_pool(
        &root,
        source_pool,
        root.join("vault").join("blobs"),
        secure_storage,
        keys,
        manifests,
    );

    let ProfileStorageUpgradeOutcome::FreshReady {
        profile_data_generation,
        space_control_generation,
    } = upgrade.ensure_v3().await.unwrap()
    else {
        panic!("fresh profile did not expose its prepared V3 generations");
    };
    assert_ne!(profile_data_generation, [0; 16]);
    assert_ne!(space_control_generation, [0; 16]);
    assert_ne!(profile_data_generation, space_control_generation);
}

#[tokio::test]
async fn v2_upgrade_coordination_is_durable_idempotent_and_encrypted() {
    let directory = tempfile::tempdir().unwrap();
    let vault = directory.path().join("vault");
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x11; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault.clone(),
        Arc::clone(&keys),
    ));
    let source = ActiveSpaceGenerationManifestV2::new(
        "source-space".to_owned(),
        [0x21; 16],
        [0x22; 16],
        [0x23; 16],
    )
    .unwrap();
    manifests.promote(&source).await.unwrap();

    // 每个 Pending 都模拟在持久边界后进程终止；下一次启动只依赖已认证
    // journal 和 source/target 介质继续推进，不能依赖内存中的 phase。
    for _ in 0..6 {
        let upgrade = new_upgrade(
            &vault,
            secure_storage.clone(),
            Arc::clone(&keys),
            Arc::clone(&manifests),
        );
        assert_eq!(
            upgrade.ensure_v3().await.unwrap(),
            ProfileStorageUpgradeOutcome::Pending
        );
    }
    let upgrade = new_upgrade(
        &vault,
        secure_storage.clone(),
        Arc::clone(&keys),
        Arc::clone(&manifests),
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Upgraded
    );
    drop(upgrade);
    assert!(manifests.load_v3_sync().unwrap().is_some());
    let first_files = regular_files(&vault);
    assert!(first_files.len() >= 3);
    for (_, bytes) in &first_files {
        for secret in [
            b"source-space".as_slice(),
            &[0x11; 16],
            &[0x21; 16],
            &[0x22; 16],
            &[0x23; 16],
        ] {
            assert!(!bytes.windows(secret.len()).any(|window| window == secret));
        }
    }

    let reopened = new_upgrade(
        &vault,
        secure_storage.clone(),
        Arc::clone(&keys),
        Arc::clone(&manifests),
    );
    assert_eq!(
        reopened.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    drop(reopened);
    let cleanup = new_upgrade(&vault, secure_storage, keys, Arc::clone(&manifests));
    assert_eq!(
        cleanup.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::UpToDate
    );
    assert!(!vault
        .join("profile-storage-upgrade")
        .join(".journal-v1")
        .exists());
    assert!(manifests.load_v3_sync().unwrap().is_some());
}

#[tokio::test]
async fn v2_admission_registration_is_readable_from_the_promoted_control_store() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("profile");
    let vault = root.join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let source_database = root.join("source.sqlite");
    let source_pool = init_db_pool(source_database.to_str().unwrap()).unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x25; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault,
        Arc::clone(&keys),
    ));
    let source = ActiveSpaceGenerationManifestV2::new(
        "source-space".to_owned(),
        [0x26; 16],
        [0x27; 16],
        [0x28; 16],
    )
    .unwrap();
    manifests.promote(&source).await.unwrap();

    let source_executor = Arc::new(DieselSqliteExecutor::new(source_pool.clone()));
    let source_admissions = Arc::new(SqliteSpaceAdmissionState::new(
        Arc::clone(&source_executor),
        Arc::clone(&keys),
        Arc::clone(&manifests),
        Arc::new(EmptyLedger),
    ));
    let source_credentials = SqliteSpaceAdmissionCredentials::new(
        source_executor,
        Arc::clone(&keys),
        Arc::clone(&manifests),
        Arc::new(EmptyLedger),
        source_admissions,
    );
    source_credentials
        .ensure_registration(&uc_core::crypto::domain::Passphrase::new(
            "upgrade passphrase",
        ))
        .await
        .unwrap();

    let upgrade = new_upgrade_from_pool(
        &root,
        source_pool,
        secure_storage,
        Arc::clone(&keys),
        Arc::clone(&manifests),
    );
    loop {
        match upgrade.ensure_v3().await.unwrap() {
            ProfileStorageUpgradeOutcome::Pending => {}
            ProfileStorageUpgradeOutcome::Upgraded => break,
            outcome => panic!("unexpected upgrade outcome: {outcome:?}"),
        }
    }
    let active = manifests.load_v3_sync().unwrap().unwrap();
    let layout = ProfileRuntimeLayout::v3(&root, &active);
    let control_pool = init_db_pool(layout.control_database().to_str().unwrap()).unwrap();
    let control_executor = Arc::new(DieselSqliteExecutor::new(control_pool));
    let control_admissions = Arc::new(SqliteSpaceAdmissionState::new(
        Arc::clone(&control_executor),
        Arc::clone(&keys),
        Arc::clone(&manifests),
        Arc::new(EmptyLedger),
    ));
    let control_credentials = SqliteSpaceAdmissionCredentials::new(
        control_executor,
        keys,
        manifests,
        Arc::new(EmptyLedger),
        control_admissions,
    );

    control_credentials
        .resolve_initial(
            InvitationId::from_bytes([0x29; 32]).unwrap(),
            SpaceAdmissionId::from_bytes([0x2a; 32]).unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn empty_profile_uses_the_same_durable_recovery_path() {
    let directory = tempfile::tempdir().unwrap();
    let vault = directory.path().join("vault");
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x31; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault.clone(),
        Arc::clone(&keys),
    ));

    let upgrade = new_upgrade(
        &vault,
        secure_storage.clone(),
        Arc::clone(&keys),
        Arc::clone(&manifests),
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    let first_files = regular_files(&vault);

    let reopened = new_upgrade(&vault, secure_storage, keys, manifests);
    assert!(matches!(
        reopened.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::FreshReady { .. }
    ));
    assert_eq!(regular_files(&vault), first_files);
}

#[tokio::test]
async fn held_profile_lease_returns_busy_without_creating_a_journal() {
    let directory = tempfile::tempdir().unwrap();
    let vault = directory.path().join("vault");
    let upgrade_directory = vault.join("profile-storage-upgrade");
    std::fs::create_dir_all(&upgrade_directory).unwrap();
    let lease = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(upgrade_directory.join(".lease"))
        .unwrap();
    lease.try_lock().unwrap();

    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x41; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault.clone(),
        Arc::clone(&keys),
    ));
    let upgrade = new_upgrade(&vault, secure_storage, keys, manifests);

    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Busy
    );
    assert!(!upgrade_directory.join(".journal-v1").exists());
}

#[tokio::test]
async fn tampered_journal_fails_closed_with_a_source_error() {
    let directory = tempfile::tempdir().unwrap();
    let vault = directory.path().join("vault");
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x51; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault.clone(),
        Arc::clone(&keys),
    ));
    let upgrade = new_upgrade(
        &vault,
        secure_storage.clone(),
        Arc::clone(&keys),
        Arc::clone(&manifests),
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    std::fs::write(named_file(&vault, ".journal-v1"), b"tampered-journal").unwrap();

    let reopened = new_upgrade(&vault, secure_storage, keys, manifests);
    let error = reopened.ensure_v3().await.unwrap_err();
    assert!(matches!(error, ProfileStorageUpgradeError::Security { .. }));
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn changed_v2_source_is_rejected_without_replacing_the_journal() {
    let directory = tempfile::tempdir().unwrap();
    let vault = directory.path().join("vault");
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x61; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault.clone(),
        Arc::clone(&keys),
    ));
    let source = ActiveSpaceGenerationManifestV2::new(
        "source-space".to_owned(),
        [0x62; 16],
        [0x63; 16],
        [0x64; 16],
    )
    .unwrap();
    manifests.promote(&source).await.unwrap();
    let upgrade = new_upgrade(
        &vault,
        secure_storage.clone(),
        Arc::clone(&keys),
        Arc::clone(&manifests),
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    let journal_path = named_file(&vault, ".journal-v1");
    let first_journal = std::fs::read(&journal_path).unwrap();

    let changed = ActiveSpaceGenerationManifestV2::new(
        "changed-space".to_owned(),
        [0x72; 16],
        [0x73; 16],
        [0x74; 16],
    )
    .unwrap();
    manifests.promote(&changed).await.unwrap();

    let reopened = new_upgrade(&vault, secure_storage, keys, manifests);
    assert!(matches!(
        reopened.ensure_v3().await.unwrap_err(),
        ProfileStorageUpgradeError::SourceChanged
    ));
    assert_eq!(std::fs::read(journal_path).unwrap(), first_journal);
}

#[tokio::test]
async fn source_snapshot_stages_one_durable_profile_and_control_target() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("profile");
    let vault = root.join("vault");
    let source_database = root.join("source.sqlite");
    std::fs::create_dir_all(&root).unwrap();
    let source_pool = init_db_pool(source_database.to_str().unwrap()).unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_event \
         (event_id, captured_at_ms, source_device, snapshot_hash) \
         VALUES ('event-a', 1, 'device-a', 'snapshot-a')",
    )
    .execute(&mut source_pool.get().unwrap())
    .unwrap();
    let source_before = std::fs::read(&source_database).unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x81; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault,
        Arc::clone(&keys),
    ));
    let source = ActiveSpaceGenerationManifestV2::new(
        "source-space".to_owned(),
        [0x82; 16],
        [0x83; 16],
        [0x84; 16],
    )
    .unwrap();
    manifests.promote(&source).await.unwrap();
    let upgrade = new_upgrade_from_pool(
        &root,
        source_pool,
        secure_storage,
        Arc::clone(&keys),
        Arc::clone(&manifests),
    );

    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );

    let profile_target = named_file(&root, "profile.sqlite");
    let control_target = named_file(&root, "control.sqlite");
    assert!(std::fs::read(profile_target)
        .unwrap()
        .starts_with(b"SQLite format 3\0"));
    assert!(std::fs::read(control_target)
        .unwrap()
        .starts_with(b"SQLite format 3\0"));
    assert_eq!(std::fs::read(&source_database).unwrap(), source_before);
    assert_eq!(manifests.load().await.unwrap(), Some(source));

    let changed_source = init_db_pool(source_database.to_str().unwrap()).unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_event \
         (event_id, captured_at_ms, source_device, snapshot_hash) \
         VALUES ('event-b', 2, 'device-b', 'snapshot-b')",
    )
    .execute(&mut changed_source.get().unwrap())
    .unwrap();
    assert!(matches!(
        upgrade.ensure_v3().await.unwrap_err(),
        ProfileStorageUpgradeError::SourceChanged
    ));
}

#[derive(diesel::QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

fn table_count(database: &Path, table: &str) -> i64 {
    use diesel::Connection as _;
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    diesel::sql_query(format!("SELECT COUNT(*) AS count FROM \"{table}\""))
        .get_result::<CountRow>(&mut connection)
        .unwrap()
        .count
}

#[tokio::test]
async fn target_stores_keep_only_their_declared_rows() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("profile");
    let vault = root.join("vault");
    std::fs::create_dir_all(&root).unwrap();
    let source_database = root.join("source.sqlite");
    let source_pool = init_db_pool(source_database.to_str().unwrap()).unwrap();
    diesel::sql_query(
        "INSERT INTO clipboard_event \
         (event_id, captured_at_ms, source_device, snapshot_hash) \
         VALUES ('event-a', 1, 'device-a', 'snapshot-a')",
    )
    .execute(&mut source_pool.get().unwrap())
    .unwrap();
    diesel::sql_query(
        "INSERT INTO membership_ledger_state (singleton_id, encrypted_payload) \
         VALUES (1, X'010203')",
    )
    .execute(&mut source_pool.get().unwrap())
    .unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x91; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        vault,
        Arc::clone(&keys),
    ));
    let source = ActiveSpaceGenerationManifestV2::new(
        "source-space".to_owned(),
        [0x92; 16],
        [0x93; 16],
        [0x94; 16],
    )
    .unwrap();
    manifests.promote(&source).await.unwrap();
    let upgrade = new_upgrade_from_pool(&root, source_pool, secure_storage, keys, manifests);

    for _ in 0..3 {
        assert_eq!(
            upgrade.ensure_v3().await.unwrap(),
            ProfileStorageUpgradeOutcome::Pending
        );
    }

    let profile_target = named_file(&root, "profile.sqlite");
    let control_target = named_file(&root, "control.sqlite");
    assert_eq!(table_count(&profile_target, "clipboard_event"), 1);
    assert_eq!(table_count(&profile_target, "membership_ledger_state"), 0);
    assert_eq!(table_count(&control_target, "clipboard_event"), 0);
    assert_eq!(table_count(&control_target, "membership_ledger_state"), 1);
}

#[tokio::test]
async fn an_unowned_source_table_blocks_store_separation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("profile");
    std::fs::create_dir_all(&root).unwrap();
    let source_database = root.join("source.sqlite");
    let source_pool = init_db_pool(source_database.to_str().unwrap()).unwrap();
    diesel::sql_query("CREATE TABLE unowned_future_table (id INTEGER PRIMARY KEY)")
        .execute(&mut source_pool.get().unwrap())
        .unwrap();
    let secure_storage = Arc::new(MemorySecureStorage::default());
    let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0xA1; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        root.join("vault"),
        Arc::clone(&keys),
    ));
    let upgrade = new_upgrade_from_pool(&root, source_pool, secure_storage, keys, manifests);

    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    assert_eq!(
        upgrade.ensure_v3().await.unwrap(),
        ProfileStorageUpgradeOutcome::Pending
    );
    let error = upgrade.ensure_v3().await.unwrap_err();
    assert!(matches!(error, ProfileStorageUpgradeError::Corrupt { .. }));
    assert!(std::error::Error::source(&error).is_some());
}
