use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use diesel::{Connection as _, RunQueryDsl};
use uc_core::ids::ProfileId;
use uc_core::membership::ActiveSpaceGenerationManifestV2;
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_infra::db::pool::{init_db_pool, DbPool};
use uc_infra::security::{
    ActiveSpaceGenerationManifestStore, AdmissionKeyManager, ProfileContentKeyVault,
    ProfileRuntimeLayout, ProfileStorageUpgrade, ProfileStorageUpgradeOutcome,
};
use uc_infra::space::InMemorySession;

#[derive(Default)]
struct TestStorage(Mutex<BTreeMap<String, Vec<u8>>>);

impl SecureStoragePort for TestStorage {
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
    root: PathBuf,
    pool: DbPool,
    storage: Arc<TestStorage>,
    keys: Arc<AdmissionKeyManager>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
}

impl Fixture {
    fn open(root: &Path) -> Self {
        let storage = Arc::new(TestStorage::default());
        // 固定合成凭据让独立进程读取同一测试 journal，不使用系统凭据或用户资料。
        storage
            .set("profile_admission_master_key:v1", &[0xA1; 32])
            .unwrap();
        let keys = Arc::new(AdmissionKeyManager::new(storage.clone(), [0xA2; 16]));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            root.join("vault"),
            keys.clone(),
        ));
        Self {
            root: root.to_path_buf(),
            pool: init_db_pool(root.join("source.sqlite").to_str().unwrap()).unwrap(),
            storage,
            keys,
            manifests,
        }
    }

    fn upgrade(&self) -> ProfileStorageUpgrade {
        ProfileStorageUpgrade::new_stepwise_for_testing(
            self.root.clone(),
            self.pool.clone(),
            self.root.join("blobs"),
            ProfileId::from("default"),
            Arc::new(InMemorySession::new()),
            Arc::new(ProfileContentKeyVault::new(
                self.root.join("content-vault"),
                self.storage.clone(),
                [0xA2; 16],
            )),
            self.keys.clone(),
            self.manifests.clone(),
        )
    }

    fn candidate(&self, directory: &str, name: &str) -> PathBuf {
        std::fs::read_dir(self.root.join(directory))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join(name)
    }
}

#[derive(diesel::QueryableByName)]
struct Count {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

fn history_count(path: &Path) -> i64 {
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(path.to_str().unwrap()).unwrap();
    diesel::sql_query("SELECT COUNT(*) AS count FROM clipboard_event")
        .get_result::<Count>(&mut connection)
        .unwrap()
        .count
}

#[tokio::test]
async fn unfinished_separation_recovers_after_process_exit() {
    for boundary in [
        "profile_partial",
        "both_partial",
        "candidate_missing",
        "restart_plan_saved",
        "separation_saved",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let fixture = Fixture::open(directory.path());
        fixture
            .manifests
            .promote(
                &ActiveSpaceGenerationManifestV2::new(
                    "crash-recovery-space".to_owned(),
                    [0xB1; 16],
                    [0xB2; 16],
                    [0xB3; 16],
                )
                .unwrap(),
            )
            .await
            .unwrap();
        diesel::sql_query("INSERT INTO clipboard_event (event_id, captured_at_ms, source_device, snapshot_hash) VALUES ('retained-event', 1, 'synthetic-device', 'synthetic-hash')")
            .execute(&mut fixture.pool.get().unwrap()).unwrap();
        let revision = fixture.pool.persistent_revision().unwrap();
        let upgrade = fixture.upgrade();
        for _ in 0..2 {
            assert_eq!(
                upgrade.ensure_v3().await.unwrap(),
                ProfileStorageUpgradeOutcome::Pending
            );
        }
        drop(upgrade);
        drop(fixture);

        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "crash_child", "--nocapture"])
            .env("UC_UPGRADE_CRASH_ROOT", directory.path())
            .env("UC_UPGRADE_CRASH_BOUNDARY", boundary)
            .output()
            .unwrap();
        assert_eq!(
            child.status.code(),
            Some(73),
            "{boundary}: {}",
            String::from_utf8_lossy(&child.stderr)
        );

        let reopened = Fixture::open(directory.path());
        assert_eq!(reopened.pool.persistent_revision().unwrap(), revision);
        let upgrade = reopened.upgrade();
        let mut outcome = ProfileStorageUpgradeOutcome::Pending;
        for _ in 0..12 {
            outcome = upgrade
                .ensure_v3()
                .await
                .unwrap_or_else(|error| panic!("{boundary}: {error:?}"));
            if outcome != ProfileStorageUpgradeOutcome::Pending {
                break;
            }
        }
        assert_eq!(
            outcome,
            ProfileStorageUpgradeOutcome::Upgraded,
            "{boundary}"
        );
        let manifest = reopened.manifests.load_v3_sync().unwrap().unwrap();
        let layout = ProfileRuntimeLayout::v3(directory.path(), &manifest);
        assert_eq!(history_count(layout.profile_database()), 1, "{boundary}");
        assert_eq!(
            history_count(&directory.path().join("source.sqlite")),
            1,
            "{boundary}"
        );
        drop(upgrade);
        let resumed = reopened.upgrade();
        for _ in 0..4 {
            if resumed.ensure_v3().await.unwrap() == ProfileStorageUpgradeOutcome::UpToDate {
                break;
            }
        }
        assert_eq!(
            resumed.ensure_v3().await.unwrap(),
            ProfileStorageUpgradeOutcome::UpToDate
        );
        assert_eq!(history_count(layout.profile_database()), 1, "{boundary}");
    }
}

#[tokio::test]
#[ignore = "child process entry for unfinished_separation_recovers_after_process_exit"]
async fn crash_child() {
    let root = PathBuf::from(std::env::var_os("UC_UPGRADE_CRASH_ROOT").unwrap());
    let boundary = std::env::var("UC_UPGRADE_CRASH_BOUNDARY").unwrap();
    let fixture = Fixture::open(&root);
    let profile = fixture.candidate("profile-data-generations", "profile.sqlite");
    if boundary == "candidate_missing" {
        std::fs::remove_file(profile).unwrap();
    } else if boundary == "separation_saved" {
        assert_eq!(
            fixture.upgrade().ensure_v3().await.unwrap(),
            ProfileStorageUpgradeOutcome::Pending
        );
    } else {
        let mut connection =
            diesel::sqlite::SqliteConnection::establish(profile.to_str().unwrap()).unwrap();
        diesel::sql_query("DELETE FROM clipboard_event")
            .execute(&mut connection)
            .unwrap();
        if boundary == "both_partial" {
            let control = fixture.candidate("space-control-generations", "control.sqlite");
            let mut control_connection =
                diesel::sqlite::SqliteConnection::establish(control.to_str().unwrap()).unwrap();
            diesel::sql_query("PRAGMA user_version = 17")
                .execute(&mut control_connection)
                .unwrap();
        }
        if boundary == "restart_plan_saved" {
            assert_eq!(
                fixture.upgrade().ensure_v3().await.unwrap(),
                ProfileStorageUpgradeOutcome::Pending
            );
        }
    }
    // 故意跳过 Rust 析构，覆盖独立进程终止时的文件和 SQLite 句柄释放。
    std::process::exit(73);
}
