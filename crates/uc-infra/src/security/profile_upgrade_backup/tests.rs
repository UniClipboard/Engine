use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use std::fs::OpenOptions;
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;

use crate::security::{ProfileLifecycleRepository, ProfileStartupStorage};
use diesel::connection::SimpleConnection;
use diesel::{Connection, RunQueryDsl, SqliteConnection};
use tempfile::{tempdir, TempDir};
use uc_application::deps::{
    PrepareProfileStartupUseCase, ProfileGeneration, ProfileLifecycle,
    ProfileLifecycleRepositoryPort, ProfileStartupError, ProfileUpgradeBackupPort,
    ProfileUpgradeVersions,
};
use uc_core::app_dirs::AppPaths;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::inventory::{read_secrets, BACKUP_DIRECTORY};
use super::record::{self, read_file_record};
use super::ProfileUpgradeBackupStore;
use crate::app_version_state::DEFAULT_FILE_NAME;
use crate::fs::VaultLayout;
use crate::security::ProfileBackupArchive;

#[derive(Default)]
struct MemoryStorage(Mutex<BTreeMap<String, Vec<u8>>>);

impl SecureStoragePort for MemoryStorage {
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

struct Fixture {
    temporary: TempDir,
    paths: AppPaths,
    storage: Arc<MemoryStorage>,
    backup: Arc<ProfileUpgradeBackupStore>,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempdir().unwrap();
        let paths = AppPaths::with_base_data_local_dir(temporary.path().join("profile"));
        let storage = Arc::new(MemoryStorage::default());
        let backup = Arc::new(ProfileUpgradeBackupStore::new(
            paths.clone(),
            "default".into(),
            storage.clone(),
            temporary.path().join("upgrade-backups"),
        ));
        Self {
            temporary,
            paths,
            storage,
            backup,
        }
    }

    fn seed(&self) {
        ProfileLifecycleRepository::new(self.storage.clone())
            .compare_and_swap(None, &ProfileLifecycle::new(ProfileGeneration::new()))
            .unwrap();
        fs::create_dir_all(&self.paths.vault_dir).unwrap();
        fs::create_dir_all(&self.paths.file_cache_dir).unwrap();
        fs::write(
            &self.paths.settings_path,
            b"private-settings-before-upgrade",
        )
        .unwrap();
        fs::write(self.paths.file_cache_dir.join("photo.bin"), [3; 4096]).unwrap();
        fs::write(self.paths.vault_dir.join("legacy-keyslot"), b"old-keyslot").unwrap();
        self.storage
            .set("kek:v1:profile:default", b"private-old-kek")
            .unwrap();
        self.storage
            .set("profile_admission_master_key:v1", &[6; 32])
            .unwrap();
        self.storage
            .set("profile_content_vault_key:v1", &[7; 32])
            .unwrap();
    }

    fn target(&self) -> ProfileUpgradeVersions {
        ProfileUpgradeVersions {
            product: "2.0.0".into(),
            engine: "3.0.0".into(),
        }
    }

    async fn prepare(&self) -> Result<ProfileLifecycle, ProfileStartupError> {
        self.workflow(self.target()).execute().await
    }

    fn workflow(&self, target: ProfileUpgradeVersions) -> PrepareProfileStartupUseCase {
        PrepareProfileStartupUseCase::new(
            self.backup.clone(),
            Arc::new(ProfileStartupStorage::new(
                self.paths.clone(),
                self.storage.clone(),
            )),
            Arc::new(ProfileLifecycleRepository::new(self.storage.clone())),
            target,
        )
    }

    fn record(&self) -> record::SecurityBackupRecord {
        record::read_record(&self.backup.directory(), self.storage.as_ref())
            .unwrap()
            .unwrap()
    }
}

#[tokio::test]
async fn fresh_install_prepares_lifecycle_without_backup() {
    let fixture = Fixture::new();
    fixture.prepare().await.unwrap();
    assert!(!fixture.paths.app_data_root_dir.exists());
    assert_eq!(fixture.storage.0.lock().unwrap().len(), 1);
    assert!(fixture
        .storage
        .get("profile_lifecycle_marker:v1")
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn startup_backup_preserves_old_sqlite_wal_files_and_security_materials() {
    let fixture = Fixture::new();
    fixture.seed();
    let mut database =
        SqliteConnection::establish(fixture.paths.db_path.to_str().unwrap()).unwrap();
    database.batch_execute("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA user_version=71; CREATE TABLE old_history (content TEXT); INSERT INTO old_history VALUES ('before-upgrade');").unwrap();
    let before = fs::read(&fixture.paths.db_path).unwrap();
    let wal = fs::read(fixture.paths.db_path.with_extension("db-wal")).unwrap();
    fs::create_dir_all(&fixture.paths.logs_dir).unwrap();
    fs::write(
        fixture.paths.logs_dir.join("live.log"),
        b"logs-not-user-data",
    )
    .unwrap();

    fs::write(
        fixture.paths.app_data_root_dir.join("daemon-startup.conn"),
        b"runtime-only-token",
    )
    .unwrap();
    let webview_cookie = fixture
        .paths
        .app_data_root_dir
        .join("EBWebView/Default/Network/Cookies");
    fs::create_dir_all(webview_cookie.parent().unwrap()).unwrap();
    fs::write(&webview_cookie, b"regenerable-webview-state").unwrap();
    #[cfg(windows)]
    let _webview_cookie_lock = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&webview_cookie)
        .unwrap();
    fixture.prepare().await.unwrap();

    assert_eq!(fs::read(&fixture.paths.db_path).unwrap(), before);
    assert_eq!(
        fs::read(fixture.paths.db_path.with_extension("db-wal")).unwrap(),
        wal
    );
    let record = fixture.record();
    assert_eq!(record.files.receipt.source.product_version, None);
    assert_eq!(record.files.receipt.source.artifact_digest, None);
    assert!(
        record.secrets
            == read_secrets(&fixture.paths, "default", fixture.storage.as_ref()).unwrap()
    );
    let destination = fixture.temporary.path().join("restored");
    ProfileBackupArchive::new(fixture.backup.directory())
        .restore_to_new_directory(&record.files.receipt, &destination)
        .unwrap();
    assert_eq!(
        fs::read(destination.join("settings.json")).unwrap(),
        b"private-settings-before-upgrade"
    );
    assert_eq!(
        fs::read(destination.join("file-cache/photo.bin")).unwrap(),
        [3; 4096]
    );
    assert!(!destination.join(BACKUP_DIRECTORY).exists());
    assert!(!destination.join("logs").exists());
    assert!(!destination.join("daemon-startup.conn").exists());
    assert!(!destination.join("EBWebView").exists());
    let mut old =
        SqliteConnection::establish(destination.join("uniclipboard.db").to_str().unwrap()).unwrap();
    #[derive(diesel::QueryableByName)]
    struct Row {
        #[diesel(sql_type = diesel::sql_types::Text)]
        content: String,
    }
    let row: Row = diesel::sql_query("SELECT content FROM old_history")
        .get_result(&mut old)
        .unwrap();
    assert_eq!(row.content, "before-upgrade");
    for entry in fs::read_dir(fixture.backup.directory()).unwrap() {
        let bytes = fs::read(entry.unwrap().path()).unwrap();
        for probe in [b"private-old-kek".as_slice()] {
            assert!(!bytes.windows(probe.len()).any(|part| part == probe));
        }
    }
}

#[tokio::test]
async fn retry_keeps_pre_upgrade_copy_even_after_source_was_changed() {
    let fixture = Fixture::new();
    fixture.seed();
    fixture.prepare().await.unwrap();
    let first = fixture.record().files.receipt;
    fs::write(&fixture.paths.settings_path, b"already-upgraded").unwrap();
    fixture
        .storage
        .set("kek:v1:profile:default", b"new-kek")
        .unwrap();
    fixture.prepare().await.unwrap();
    assert_eq!(fixture.record().files.receipt, first);
    assert_eq!(
        fs::read(&fixture.paths.settings_path).unwrap(),
        b"already-upgraded"
    );
    assert!(
        fixture.record().secrets
            != read_secrets(&fixture.paths, "default", fixture.storage.as_ref()).unwrap()
    );
}

#[tokio::test]
async fn backup_preserves_pending_payloads_in_separate_and_nested_cache_roots() {
    for nested in [false, true] {
        let mut fixture = Fixture::new();
        if nested {
            fixture.paths.cache_dir = fixture.paths.app_data_root_dir.join("cache");
            fixture.paths.spool_dir = fixture.paths.cache_dir.join("spool");
            fixture.backup = Arc::new(ProfileUpgradeBackupStore::new(
                fixture.paths.clone(),
                "default".into(),
                fixture.storage.clone(),
                fixture.temporary.path().join("upgrade-backups"),
            ));
        }
        fixture.seed();
        fs::create_dir_all(&fixture.paths.spool_dir).unwrap();
        fs::write(
            fixture.paths.spool_dir.join("pending-content"),
            b"only-pending-copy",
        )
        .unwrap();
        fs::write(fixture.paths.cache_dir.join("rebuildable-index"), b"index").unwrap();
        fixture.prepare().await.unwrap();
        let record = fixture.record();
        let spool = record.files.spool_receipt.as_ref().unwrap();
        let archive = ProfileBackupArchive::new(fixture.backup.directory());
        let destination = fixture.temporary.path().join("restored-spool");
        archive
            .restore_to_new_directory(spool, &destination)
            .unwrap();
        assert_eq!(
            fs::read(destination.join("pending-content")).unwrap(),
            b"only-pending-copy"
        );
        assert!(!destination.join("rebuildable-index").exists());
        fs::write(
            fixture.paths.spool_dir.join("pending-content"),
            b"new-pending-copy",
        )
        .unwrap();
        fixture.prepare().await.unwrap();
        assert_eq!(fixture.record().files.spool_receipt.as_ref(), Some(spool));
        fs::write(
            fixture.backup.directory().join(format!(
                "{}.archive",
                uuid::Uuid::from_bytes(spool.archive_id)
            )),
            b"corrupt",
        )
        .unwrap();
        assert!(fixture.prepare().await.is_err());
    }
}

#[tokio::test]
async fn pending_payloads_without_a_database_still_require_a_backup() {
    let fixture = Fixture::new();
    fs::create_dir_all(&fixture.paths.spool_dir).unwrap();
    fs::write(
        fixture.paths.spool_dir.join("pending-content"),
        b"only-pending-copy",
    )
    .unwrap();
    fixture.prepare().await.unwrap();
    assert!(fixture.record().files.spool_receipt.is_some());
}

#[tokio::test]
async fn missing_key_or_corrupt_archive_stops_retry_without_replacing_backup() {
    let fixture = Fixture::new();
    fixture.seed();
    fixture.prepare().await.unwrap();
    let first = fixture.record().files.receipt;
    let archive = fixture.backup.directory().join(format!(
        "{}.archive",
        uuid::Uuid::from_bytes(first.archive_id)
    ));
    fs::write(&archive, b"corrupt").unwrap();
    assert!(fixture.prepare().await.is_err());
    assert_eq!(fixture.record().files.receipt, first);
    fixture.storage.delete(record::RECORD_KEY).unwrap();
    assert!(fixture.prepare().await.is_err());
    assert!(fixture.storage.get(record::RECORD_KEY).unwrap().is_none());
    assert_eq!(fs::read(archive).unwrap(), b"corrupt");
}

#[tokio::test]
async fn later_version_creates_another_backup_without_removing_the_first() {
    let fixture = Fixture::new();
    fixture.seed();
    fixture.prepare().await.unwrap();
    let first = fixture.record().files.receipt;
    let target = ProfileUpgradeVersions {
        product: "2.1.0".into(),
        engine: "3.0.0".into(),
    };
    fixture.workflow(target).execute().await.unwrap();
    assert_ne!(fixture.record().files.receipt.archive_id, first.archive_id);
    ProfileBackupArchive::new(fixture.backup.directory())
        .verify(&first)
        .unwrap();
}

#[tokio::test]
async fn matching_product_and_engine_versions_skip_capture() {
    let fixture = Fixture::new();
    fixture.seed();
    // 同版本启动只读取来源版本，不需要打开已损坏的历史备份记录。
    fs::create_dir_all(fixture.backup.directory()).unwrap();
    fs::write(
        fixture.backup.directory().join("current"),
        b"unreadable retained backup",
    )
    .unwrap();
    for (path, version) in [
        (
            fixture.paths.app_data_root_dir.join(DEFAULT_FILE_NAME),
            "2.0.0",
        ),
        (
            VaultLayout::new(&fixture.paths.app_data_root_dir).engine_upgrade_cursor_path(),
            "3.0.0",
        ),
    ] {
        fs::write(
            path,
            serde_json::to_vec(
                &serde_json::json!({"schema_version": 1, "last_seen_version": version}),
            )
            .unwrap(),
        )
        .unwrap();
    }
    fixture.prepare().await.unwrap();
    assert_eq!(
        fs::read(fixture.backup.directory().join("current")).unwrap(),
        b"unreadable retained backup"
    );
    assert!(fixture.storage.get(record::RECORD_KEY).unwrap().is_none());
}

#[tokio::test]
async fn competing_backup_attempt_fails_without_publishing_or_adopting() {
    let fixture = Fixture::new();
    fixture.seed();
    let lease = fixture.backup.lease().unwrap();
    assert!(fixture.prepare().await.is_err());
    assert!(!fixture.backup.directory().join("current").exists());
    assert_eq!(
        fs::read(&fixture.paths.settings_path).unwrap(),
        b"private-settings-before-upgrade"
    );
    drop(lease);
    fixture.prepare().await.unwrap();
    fixture
        .backup
        .verify_prepared(&fixture.target())
        .await
        .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn unknown_symlink_is_rejected_without_changing_source() {
    let fixture = Fixture::new();
    fixture.seed();
    std::os::unix::fs::symlink(
        &fixture.paths.settings_path,
        fixture.paths.app_data_root_dir.join("unexpected-link"),
    )
    .unwrap();
    assert!(fixture.prepare().await.is_err());
    assert!(!fixture.backup.directory().join("current").exists());
    assert_eq!(
        fs::read(&fixture.paths.settings_path).unwrap(),
        b"private-settings-before-upgrade"
    );
}

#[tokio::test]
async fn inaccessible_keychain_still_leaves_files_restorable_after_userdata_deletion() {
    struct DeniedStorage;
    impl SecureStoragePort for DeniedStorage {
        fn get(&self, _: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Err(SecureStorageError::PermissionDenied(
                "injected keychain denial".into(),
            ))
        }
        fn set(&self, _: &str, _: &[u8]) -> Result<(), SecureStorageError> {
            panic!("no key write expected");
        }
        fn delete(&self, _: &str) -> Result<(), SecureStorageError> {
            panic!("no key deletion expected");
        }
    }
    let fixture = Fixture::new();
    fixture.seed();
    let denied: Arc<dyn SecureStoragePort> = Arc::new(DeniedStorage);
    let backup = Arc::new(ProfileUpgradeBackupStore::new(
        fixture.paths.clone(),
        "default".into(),
        denied.clone(),
        fixture.temporary.path().join("upgrade-backups"),
    ));
    let workflow = PrepareProfileStartupUseCase::new(
        backup.clone(),
        Arc::new(ProfileStartupStorage::new(
            fixture.paths.clone(),
            denied.clone(),
        )),
        Arc::new(ProfileLifecycleRepository::new(denied)),
        fixture.target(),
    );
    assert!(matches!(
        workflow.execute().await,
        Err(ProfileStartupError::Lifecycle(_))
    ));
    let files = read_file_record(&backup.directory()).unwrap().unwrap();
    assert!(!fs::read(backup.directory().join("current"))
        .unwrap()
        .windows(7)
        .any(|part| part == b"secrets"));
    assert!(!backup
        .directory()
        .starts_with(&fixture.paths.app_data_root_dir));
    assert!(!backup.directory().join("security-current").exists());
    fs::remove_dir_all(&fixture.paths.app_data_root_dir).unwrap();
    let destination = fixture.temporary.path().join("restored-without-keychain");
    ProfileBackupArchive::new(backup.directory())
        .restore_to_new_directory(&files.receipt, &destination)
        .unwrap();
    assert_eq!(
        fs::read(destination.join("settings.json")).unwrap(),
        b"private-settings-before-upgrade"
    );
}

#[tokio::test]
async fn file_backup_survives_security_failure_and_retry_does_not_replace_it() {
    let fixture = Fixture::new();
    fixture.seed();
    fixture
        .backup
        .capture_verified(&fixture.target())
        .await
        .unwrap();
    let first = read_file_record(&fixture.backup.directory())
        .unwrap()
        .unwrap()
        .receipt;
    fs::write(&fixture.paths.settings_path, b"changed-after-files").unwrap();
    assert!(fixture.prepare().await.is_err());
    assert_eq!(
        read_file_record(&fixture.backup.directory())
            .unwrap()
            .unwrap()
            .receipt,
        first
    );
    assert!(!fixture.backup.directory().join("security-current").exists());
}

#[tokio::test]
async fn nested_backup_root_is_rejected_before_publishing_files() {
    let fixture = Fixture::new();
    fixture.seed();
    let backup = ProfileUpgradeBackupStore::new(
        fixture.paths.clone(),
        "default".into(),
        fixture.storage.clone(),
        fixture.paths.app_data_root_dir.join("nested-backups"),
    );
    assert!(backup.capture_verified(&fixture.target()).await.is_err());
    assert!(!fixture
        .paths
        .app_data_root_dir
        .join("nested-backups")
        .exists());
}

#[tokio::test]
async fn missing_security_record_key_does_not_prevent_file_verification_or_restore() {
    let fixture = Fixture::new();
    fixture.seed();
    fixture.prepare().await.unwrap();
    let receipt = fixture.record().files.receipt;
    fixture.storage.delete(record::RECORD_KEY).unwrap();
    fixture
        .backup
        .verify_prepared(&fixture.target())
        .await
        .unwrap();
    assert!(fixture.prepare().await.is_err());
    assert!(fixture.storage.get(record::RECORD_KEY).unwrap().is_none());
    let destination = fixture.temporary.path().join("no-security-record-key");
    ProfileBackupArchive::new(fixture.backup.directory())
        .restore_to_new_directory(&receipt, &destination)
        .unwrap();
    assert_eq!(
        fs::read(destination.join("settings.json")).unwrap(),
        b"private-settings-before-upgrade"
    );
}

#[tokio::test]
async fn independent_userdata_roots_never_reuse_each_others_backup() {
    let first = Fixture::new();
    let second = Fixture::new();
    first.seed();
    second.seed();
    fs::write(&second.paths.settings_path, b"second-installation").unwrap();
    first.prepare().await.unwrap();
    let original = first.record().files.receipt;
    let second_backup = Arc::new(ProfileUpgradeBackupStore::new(
        second.paths.clone(),
        "default".into(),
        second.storage.clone(),
        first.temporary.path().join("upgrade-backups"),
    ));
    assert_ne!(first.backup.directory(), second_backup.directory());
    let workflow = PrepareProfileStartupUseCase::new(
        second_backup.clone(),
        Arc::new(ProfileStartupStorage::new(
            second.paths.clone(),
            second.storage.clone(),
        )),
        Arc::new(ProfileLifecycleRepository::new(second.storage.clone())),
        second.target(),
    );
    workflow.execute().await.unwrap();
    assert_eq!(first.record().files.receipt, original);
    let files = read_file_record(&second_backup.directory())
        .unwrap()
        .unwrap();
    let destination = second.temporary.path().join("second-restored");
    ProfileBackupArchive::new(second_backup.directory())
        .restore_to_new_directory(&files.receipt, &destination)
        .unwrap();
    assert_eq!(
        fs::read(destination.join("settings.json")).unwrap(),
        b"second-installation"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn shared_backup_directory_is_rejected_without_changing_its_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    fixture.seed();
    fs::create_dir_all(fixture.backup.directory()).unwrap();
    fs::set_permissions(
        fixture.backup.directory(),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(fixture.prepare().await.is_err());
    assert!(!fixture.backup.directory().join("current").exists());
    assert_eq!(
        fs::metadata(fixture.backup.directory())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}

#[tokio::test]
async fn completed_backups_keep_only_the_five_most_recent() {
    let fixture = Fixture::new();
    fixture.seed();
    let mut created = Vec::new();
    for version in 1..=6 {
        let target = ProfileUpgradeVersions {
            product: format!("2.0.{version}"),
            engine: format!("3.0.{version}"),
        };
        fixture.workflow(target).execute().await.unwrap();
        created.push(fixture.record().files.receipt);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let backups = fixture.backup.list_backups().await.unwrap();
    assert_eq!(backups.len(), 5);
    assert!(!fixture.backup.archive_path(&created[0]).exists());
    for receipt in created.iter().skip(1) {
        assert!(fixture.backup.archive_path(receipt).exists());
    }
}

#[tokio::test]
async fn listed_backup_can_be_deleted_without_touching_the_profile() {
    let fixture = Fixture::new();
    fixture.seed();
    fixture.prepare().await.unwrap();
    let backup = fixture.backup.list_backups().await.unwrap().remove(0);

    fixture.backup.delete_backup(&backup.id).await.unwrap();

    assert!(fixture.backup.list_backups().await.unwrap().is_empty());
    assert!(fixture.paths.settings_path.exists());
    assert!(!fixture.backup.directory().join("current").exists());
    assert!(!fixture.backup.directory().join("security-current").exists());
    assert!(fixture.storage.get(record::RECORD_KEY).unwrap().is_none());
}
