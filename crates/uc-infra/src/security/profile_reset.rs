use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use diesel::connection::SimpleConnection;
use diesel::prelude::*;
use uc_application::deps::{
    ClearProfileStatePort, ProfileFactoryResetCapabilityError, ProfileGeneration,
    WipeProfileKeysPort,
};
use uc_core::app_dirs::AppPaths;
use uc_core::ports::SecureStoragePort;

use crate::db::pool::DbPool;

use super::admission_key_manager::AdmissionKeyManager;
use super::key_migration_adapter::DefaultKeyMigrationAdapter;
use super::profile_content_key_vault::PROFILE_CONTENT_VAULT_KEY_NAME;

pub struct ProfileKeyWiper {
    admission_keys: AdmissionKeyManager,
    secure_storage: Arc<dyn SecureStoragePort>,
    legacy_migration_base_dir: PathBuf,
    profile_id: String,
    keyslot_path: PathBuf,
    network_identity_dir: PathBuf,
}

impl ProfileKeyWiper {
    pub fn new(
        admission_keys: AdmissionKeyManager,
        secure_storage: Arc<dyn SecureStoragePort>,
        legacy_migration_base_dir: PathBuf,
        profile_id: String,
        keyslot_path: PathBuf,
        network_identity_dir: PathBuf,
    ) -> Self {
        Self {
            admission_keys,
            secure_storage,
            legacy_migration_base_dir,
            profile_id,
            keyslot_path,
            network_identity_dir,
        }
    }

    async fn wipe_migration_key(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
        let Some(run_id) =
            crate::migration_state::legacy_migration_run_id(&self.legacy_migration_base_dir)
                .await
                .map_err(|_| capability_error())?
        else {
            return Ok(());
        };
        let name = DefaultKeyMigrationAdapter::keyring_name(&run_id);
        self.secure_storage
            .delete(&name)
            .map_err(|_| capability_error())?;
        if self
            .secure_storage
            .get(&name)
            .map_err(|_| capability_error())?
            .is_some()
        {
            return Err(capability_error());
        }
        Ok(())
    }

    fn wipe_active_space_keys(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
        remove_path_if_present(&self.keyslot_path)?;
        if self.keyslot_path.exists() {
            return Err(capability_error());
        }
        for secret in crate::config_migration::secret_keys::migratable_secret_keys(&self.profile_id)
        {
            self.secure_storage
                .delete(&secret.key)
                .map_err(|_| capability_error())?;
            if self
                .secure_storage
                .get(&secret.key)
                .map_err(|_| capability_error())?
                .is_some()
            {
                return Err(capability_error());
            }
        }
        Ok(())
    }

    fn wipe_profile_content_vault_key(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
        self.secure_storage
            .delete(PROFILE_CONTENT_VAULT_KEY_NAME)
            .map_err(|_| capability_error())?;
        if self
            .secure_storage
            .get(PROFILE_CONTENT_VAULT_KEY_NAME)
            .map_err(|_| capability_error())?
            .is_some()
        {
            return Err(capability_error());
        }
        Ok(())
    }
}

#[async_trait]
impl WipeProfileKeysPort for ProfileKeyWiper {
    async fn wipe_and_verify_profile_keys(
        &self,
        profile_generation: ProfileGeneration,
    ) -> Result<(), ProfileFactoryResetCapabilityError> {
        if self.admission_keys.profile_generation() != profile_generation.into_bytes() {
            return Err(capability_error());
        }
        self.wipe_active_space_keys()?;
        self.wipe_migration_key().await?;
        self.wipe_profile_content_vault_key()?;
        remove_path_if_present(&self.network_identity_dir)?;
        if self.network_identity_dir.exists() {
            return Err(capability_error());
        }
        self.admission_keys
            .delete_profile_key()
            .map_err(|_| capability_error())?;
        if self
            .admission_keys
            .profile_key_exists()
            .map_err(|_| capability_error())?
        {
            return Err(capability_error());
        }
        Ok(())
    }
}

pub struct ProfileStateCleaner {
    database: DbPool,
    paths: AppPaths,
    current_database_path: PathBuf,
}

impl ProfileStateCleaner {
    pub fn new(database: DbPool, paths: AppPaths, current_database_path: PathBuf) -> Self {
        Self {
            database,
            paths,
            current_database_path,
        }
    }

    fn clear_database(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
        #[derive(QueryableByName)]
        struct TableName {
            #[diesel(sql_type = diesel::sql_types::Text)]
            name: String,
        }

        let mut connection = self.database.get().map_err(|_| capability_error())?;
        let tables = diesel::sql_query(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .load::<TableName>(&mut connection)
        .map_err(|_| capability_error())?;
        connection
            .batch_execute("PRAGMA foreign_keys = OFF")
            .map_err(|_| capability_error())?;
        let result = connection.transaction::<_, diesel::result::Error, _>(|connection| {
            for table in &tables {
                if !safe_sqlite_identifier(&table.name) {
                    return Err(diesel::result::Error::RollbackTransaction);
                }
                connection.batch_execute(&format!("DROP TABLE IF EXISTS \"{}\"", table.name))?;
            }
            Ok(())
        });
        let restore_foreign_keys = connection.batch_execute("PRAGMA foreign_keys = ON");
        result.map_err(|_| capability_error())?;
        restore_foreign_keys.map_err(|_| capability_error())?;
        connection
            .batch_execute("PRAGMA wal_checkpoint(TRUNCATE); VACUUM")
            .map_err(|_| capability_error())?;

        let remaining = diesel::sql_query(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT LIKE 'sqlite_%'",
        )
        .load::<TableName>(&mut connection)
        .map_err(|_| capability_error())?;
        if !remaining.is_empty() {
            return Err(capability_error());
        }
        Ok(())
    }

    fn clear_files(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
        for path in self.profile_state_paths() {
            remove_path_if_present(&path)?;
        }
        Ok(())
    }

    fn remove_database_files(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
        let mut databases = vec![self.current_database_path.clone()];
        if self.paths.db_path != self.current_database_path {
            databases.push(self.paths.db_path.clone());
        }
        for database in databases {
            remove_path_if_present(&database)?;
            remove_path_if_present(&PathBuf::from(format!("{}-wal", database.display())))?;
            remove_path_if_present(&PathBuf::from(format!("{}-shm", database.display())))?;
        }
        Ok(())
    }

    fn verify_files_absent(&self) -> Result<(), ProfileFactoryResetCapabilityError> {
        if self.profile_state_paths().iter().any(|path| path.exists()) {
            return Err(capability_error());
        }
        Ok(())
    }

    fn profile_state_paths(&self) -> Vec<PathBuf> {
        vec![
            self.paths.vault_dir.clone(),
            self.paths.settings_path.clone(),
            self.paths.file_cache_dir.clone(),
            self.paths.cache_dir.clone(),
            self.paths.app_data_root_dir.join("space-generations"),
            self.paths.app_data_root_dir.join("iroh-blobs"),
            self.paths.app_data_root_dir.join("iroh-identity"),
            self.paths.app_data_root_dir.join("import-staging"),
            self.paths.app_data_root_dir.join("pending-import.json"),
            self.paths.app_data_root_dir.join("upgrade-cursor.json"),
            self.paths.app_data_root_dir.join("first-sync-state.json"),
            self.paths.daemon_token_path(),
            self.paths.daemon_pid_path(),
            self.paths.last_notified_update_path(),
            self.paths.skipped_version_path(),
            self.paths.update_prompt_throttle_path(),
        ]
    }
}

#[async_trait]
impl ClearProfileStatePort for ProfileStateCleaner {
    async fn clear_and_verify_profile_state(
        &self,
    ) -> Result<(), ProfileFactoryResetCapabilityError> {
        self.clear_database()?;
        self.database
            .detach_to_ephemeral_database()
            .map_err(|_| capability_error())?;
        self.remove_database_files()?;
        self.clear_files()?;
        self.verify_files_absent()
    }
}

fn safe_sqlite_identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn remove_path_if_present(path: &Path) -> Result<(), ProfileFactoryResetCapabilityError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(capability_error()),
    };
    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path).map_err(|_| capability_error())
    } else {
        std::fs::remove_file(path).map_err(|_| capability_error())
    }
}

fn capability_error() -> ProfileFactoryResetCapabilityError {
    ProfileFactoryResetCapabilityError
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use diesel::{Connection, RunQueryDsl, SqliteConnection};
    use uc_application::deps::{ClearProfileStatePort, WipeProfileKeysPort};
    use uc_core::app_dirs::AppPaths;
    use uc_core::ports::security::secure_storage::{SecureStorageError, SecureStoragePort};
    use uc_core::ports::security::MigrationRunId;

    use super::{ProfileGeneration, ProfileKeyWiper, ProfileStateCleaner};

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

    #[tokio::test]
    async fn profile_key_wipe_removes_and_verifies_every_known_key_before_state_clear() {
        let directory = tempfile::tempdir().unwrap();
        let identity_dir = directory.path().join("network-identity");
        std::fs::create_dir_all(&identity_dir).unwrap();
        std::fs::write(identity_dir.join("identity-key"), b"identity").unwrap();
        let keyslot_path = directory.path().join("vault/keyslot.json");
        std::fs::create_dir_all(keyslot_path.parent().unwrap()).unwrap();
        std::fs::write(&keyslot_path, b"keyslot").unwrap();
        let storage = Arc::new(MemorySecureStorage::default());
        storage.set("kek:v1:profile:default", b"space").unwrap();
        storage
            .set("profile_admission_master_key:v1", &[0x55; 32])
            .unwrap();
        storage
            .set("profile_content_vault_key:v1", &[0x56; 32])
            .unwrap();
        let run_id = MigrationRunId::new("reset-migration");
        let migration_key_name = format!("migration_key:v1:{}", run_id.as_str());
        storage.set(&migration_key_name, &[0x66; 32]).unwrap();
        let legacy_migration_base_dir = directory.path().join("vault");
        std::fs::write(
            legacy_migration_base_dir.join(".migration_state"),
            format!(
                "{{\"kind\":\"prepared\",\"run_id\":\"{}\",\"preserved_unreadable_records\":0}}",
                run_id.as_str()
            ),
        )
        .unwrap();
        let admission_keys = super::super::AdmissionKeyManager::new(storage.clone(), [0x77; 16]);
        let wiper = ProfileKeyWiper::new(
            admission_keys,
            storage.clone(),
            legacy_migration_base_dir,
            "default".to_owned(),
            keyslot_path.clone(),
            identity_dir.clone(),
        );

        wiper
            .wipe_and_verify_profile_keys(ProfileGeneration::from_bytes([0x77; 16]))
            .await
            .unwrap();

        assert!(storage.get("kek:v1:profile:default").unwrap().is_none());
        assert!(!keyslot_path.exists());
        assert!(storage
            .get("profile_admission_master_key:v1")
            .unwrap()
            .is_none());
        assert!(storage
            .get("profile_content_vault_key:v1")
            .unwrap()
            .is_none());
        assert!(storage.get(&migration_key_name).unwrap().is_none());
        assert!(!identity_dir.exists());
    }

    #[tokio::test]
    async fn profile_state_clear_removes_business_storage_but_keeps_a_rebuildable_database() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths::with_base_data_local_dir(directory.path().join("data"));
        std::fs::create_dir_all(&paths.vault_dir).unwrap();
        std::fs::create_dir_all(&paths.cache_dir).unwrap();
        std::fs::create_dir_all(&paths.file_cache_dir).unwrap();
        std::fs::write(paths.vault_dir.join("private-state"), b"private").unwrap();
        std::fs::write(&paths.settings_path, b"settings").unwrap();
        std::fs::write(paths.cache_dir.join("search-state"), b"search").unwrap();
        std::fs::write(paths.file_cache_dir.join("received-file"), b"file").unwrap();
        std::fs::create_dir_all(paths.app_data_root_dir.join("import-staging")).unwrap();
        std::fs::write(
            paths.app_data_root_dir.join("import-staging/settings.json"),
            b"staged-settings",
        )
        .unwrap();
        std::fs::write(
            paths.app_data_root_dir.join("pending-import.json"),
            b"pending",
        )
        .unwrap();
        std::fs::create_dir_all(paths.app_data_root_dir.join("space-generations/g1")).unwrap();
        std::fs::write(
            paths
                .app_data_root_dir
                .join("space-generations/g1/target.sqlite"),
            b"generation",
        )
        .unwrap();

        std::fs::create_dir_all(paths.db_path.parent().unwrap()).unwrap();
        let mut connection = SqliteConnection::establish(paths.db_path.to_str().unwrap()).unwrap();
        diesel::sql_query(
            "CREATE TABLE private_profile_fact (id INTEGER PRIMARY KEY, payload BLOB NOT NULL)",
        )
        .execute(&mut connection)
        .unwrap();
        diesel::sql_query("INSERT INTO private_profile_fact (payload) VALUES (X'736563726574')")
            .execute(&mut connection)
            .unwrap();
        drop(connection);
        let pool = crate::db::pool::init_db_pool(paths.db_path.to_str().unwrap()).unwrap();
        let cleaner = ProfileStateCleaner::new(pool, paths.clone(), paths.db_path.clone());

        cleaner.clear_and_verify_profile_state().await.unwrap();

        assert!(!paths.db_path.exists());
        assert!(!paths.vault_dir.exists());
        assert!(!paths.settings_path.exists());
        assert!(!paths.cache_dir.exists());
        assert!(!paths.file_cache_dir.exists());
        assert!(!paths.app_data_root_dir.join("space-generations").exists());
        assert!(!paths.app_data_root_dir.join("import-staging").exists());
        assert!(!paths.app_data_root_dir.join("pending-import.json").exists());
        let reopened_pool = crate::db::pool::init_db_pool(paths.db_path.to_str().unwrap()).unwrap();
        let mut reopened = reopened_pool.get().unwrap();
        #[derive(diesel::QueryableByName)]
        struct CountRow {
            #[diesel(sql_type = diesel::sql_types::BigInt)]
            count: i64,
        }
        let remaining = diesel::sql_query(
            "SELECT COUNT(*) AS count FROM sqlite_master WHERE type = 'table' \
             AND name = 'private_profile_fact'",
        )
        .get_result::<CountRow>(&mut reopened)
        .unwrap();
        assert_eq!(remaining.count, 0);
    }
}
