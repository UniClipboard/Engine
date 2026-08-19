//! File-based setup status repository
//!
//! This module provides a file-based implementation of the SetupStatusPort,
//! persisting setup status to a local JSON file in the application data directory.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use uc_core::ports::SetupStatusPort;
use uc_core::setup::SetupStatus;

use crate::security::ActiveSpaceManifestStore;

pub const DEFAULT_SETUP_STATUS_FILE: &str = ".setup_status";

pub struct FileSetupStatusRepository {
    status_file_path: PathBuf,
}

pub struct ManifestProjectingSetupStatusRepository {
    legacy: Arc<dyn SetupStatusPort>,
    manifest: Arc<ActiveSpaceManifestStore>,
}

impl ManifestProjectingSetupStatusRepository {
    pub fn new(legacy: Arc<dyn SetupStatusPort>, manifest: Arc<ActiveSpaceManifestStore>) -> Self {
        Self { legacy, manifest }
    }
}

#[async_trait]
impl SetupStatusPort for ManifestProjectingSetupStatusRepository {
    async fn get_status(&self) -> anyhow::Result<SetupStatus> {
        if let Some(manifest) = self.manifest.load().await? {
            return Ok(SetupStatus {
                has_completed: true,
                space_id: Some(uc_core::ids::SpaceId::from_str(&manifest.space_id)),
                re_pairing_required: false,
            });
        }
        self.legacy.get_status().await
    }

    async fn set_status(&self, status: &SetupStatus) -> anyhow::Result<()> {
        self.legacy.set_status(status).await
    }
}

impl FileSetupStatusRepository {
    /// Create repository with custom file path
    pub fn new(status_file_path: PathBuf) -> Self {
        Self { status_file_path }
    }

    /// Create repository with base dir and filename
    pub fn with_base_dir(base_dir: PathBuf, filename: impl Into<String>) -> Self {
        Self {
            status_file_path: base_dir.join(filename.into()),
        }
    }

    /// Create repository with defaults
    pub fn with_defaults(base_dir: PathBuf) -> Self {
        Self {
            status_file_path: base_dir.join(DEFAULT_SETUP_STATUS_FILE),
        }
    }

    async fn ensure_parent_dir(&self) -> anyhow::Result<()> {
        if let Some(parent) = self.status_file_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        Ok(())
    }
}

#[async_trait]
impl SetupStatusPort for FileSetupStatusRepository {
    async fn get_status(&self) -> anyhow::Result<SetupStatus> {
        if !self.status_file_path.exists() {
            return Ok(SetupStatus::default());
        }

        self.ensure_parent_dir().await?;
        let content = fs::read_to_string(&self.status_file_path).await?;

        if content.trim().is_empty() {
            return Ok(SetupStatus::default());
        }

        let status: SetupStatus = serde_json::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse setup status: {e}"))?;

        Ok(status)
    }

    async fn set_status(&self, status: &SetupStatus) -> anyhow::Result<()> {
        self.ensure_parent_dir().await?;

        let json = serde_json::to_string_pretty(status)
            .map_err(|e| anyhow::anyhow!("Failed to serialize setup status: {e}"))?;

        let mut file = fs::File::create(&self.status_file_path)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create status file: {e}"))?;

        file.write_all(json.as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write status file: {e}"))?;

        file.sync_all()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to sync status file: {e}"))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use uc_core::membership::ActiveSpaceManifestV2;
    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use crate::security::AdmissionKeyManager;

    use super::*;

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
    async fn active_manifest_overrides_legacy_setup_projection() {
        let directory = tempfile::tempdir().unwrap();
        let legacy: Arc<dyn SetupStatusPort> = Arc::new(FileSetupStatusRepository::new(
            directory.path().join("setup.json"),
        ));
        legacy
            .set_status(&SetupStatus {
                has_completed: true,
                space_id: Some(uc_core::ids::SpaceId::from_str("legacy-space")),
                re_pairing_required: false,
            })
            .await
            .unwrap();
        let manifest = Arc::new(ActiveSpaceManifestStore::new(
            directory.path().to_path_buf(),
            Arc::new(AdmissionKeyManager::new(
                Arc::new(MemorySecureStorage::default()),
                [0x31; 16],
            )),
        ));
        let projecting = ManifestProjectingSetupStatusRepository::new(
            Arc::clone(&legacy),
            Arc::clone(&manifest),
        );
        assert_eq!(
            projecting.get_status().await.unwrap().space_id,
            Some(uc_core::ids::SpaceId::from_str("legacy-space"))
        );

        manifest
            .promote(
                &ActiveSpaceManifestV2::new(
                    "target-space".to_owned(),
                    [0x32; 16],
                    [0x33; 16],
                    [0x34; 16],
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let projected = projecting.get_status().await.unwrap();
        assert!(projected.has_completed);
        assert_eq!(
            projected.space_id,
            Some(uc_core::ids::SpaceId::from_str("target-space"))
        );
    }
}
