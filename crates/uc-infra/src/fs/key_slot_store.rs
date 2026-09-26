use async_trait::async_trait;
use std::path::PathBuf;
use uc_core::crypto::model::EncryptionError;

use crate::security::crypto_model::KeySlotFile;

#[async_trait]
pub trait KeySlotStore: Send + Sync {
    async fn load(&self) -> Result<KeySlotFile, EncryptionError>;
    async fn store(&self, slot: &KeySlotFile) -> Result<(), EncryptionError>;
    async fn delete(&self) -> Result<(), EncryptionError>;
    async fn exists(&self) -> bool;
    async fn quarantine(&self) -> Result<(), EncryptionError>;
}

pub struct JsonKeySlotStore {
    path: PathBuf,
}

impl JsonKeySlotStore {
    pub fn new(path_or_dir: PathBuf) -> Self {
        let path = if path_or_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "keyslot.json")
        {
            path_or_dir
        } else {
            path_or_dir.join("keyslot.json")
        };

        Self { path }
    }

    fn effective_path(&self) -> PathBuf {
        if self.path.is_dir() {
            self.path.join("keyslot.json")
        } else {
            self.path.clone()
        }
    }
}

#[async_trait]
impl KeySlotStore for JsonKeySlotStore {
    async fn load(&self) -> Result<KeySlotFile, EncryptionError> {
        let path = self.effective_path();

        if !path.exists() {
            return Err(EncryptionError::KeyNotFound);
        }

        let content = tokio::fs::read_to_string(&path)
            .await
            .map_err(EncryptionError::io_failure_from)?;

        let slot: KeySlotFile =
            serde_json::from_str(&content).map_err(EncryptionError::key_material_corrupt_from)?;

        Ok(slot)
    }

    async fn store(&self, slot: &KeySlotFile) -> Result<(), EncryptionError> {
        if self.path.is_dir() {
            tokio::fs::remove_dir_all(&self.path)
                .await
                .map_err(EncryptionError::io_failure_from)?;
        }

        let path = self.effective_path();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(EncryptionError::io_failure_from)?;
        }

        let tmp = path.with_extension("json.tmp");

        let json = serde_json::to_string_pretty(slot)
            .map_err(EncryptionError::key_material_corrupt_from)?;

        tokio::fs::write(&tmp, json)
            .await
            .map_err(EncryptionError::io_failure_from)?;

        tokio::fs::rename(&tmp, &path)
            .await
            .map_err(EncryptionError::io_failure_from)?;

        Ok(())
    }

    async fn delete(&self) -> Result<(), EncryptionError> {
        let path = self.effective_path();

        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(EncryptionError::io_failure_from)?;
        }

        if self.path.is_dir() {
            tokio::fs::remove_dir_all(&self.path)
                .await
                .map_err(EncryptionError::io_failure_from)?;
        }

        Ok(())
    }

    async fn exists(&self) -> bool {
        self.effective_path().exists()
    }

    async fn quarantine(&self) -> Result<(), EncryptionError> {
        let path = self.effective_path();

        if path.exists() {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let quarantine_path = path.with_extension(format!("corrupt.{}", timestamp));
            tokio::fs::rename(&path, &quarantine_path)
                .await
                .map_err(|_| EncryptionError::IoFailure)?;
        }

        if self.path.is_dir() {
            let _ = tokio::fs::remove_dir_all(&self.path).await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn quarantine_renames_existing_keyslot() {
        let dir = tempdir().unwrap();
        let store = JsonKeySlotStore::new(dir.path().to_path_buf());
        let file_path = dir.path().join("keyslot.json");
        tokio::fs::write(&file_path, "{}").await.unwrap();
        assert!(file_path.exists());

        store.quarantine().await.unwrap();
        assert!(!file_path.exists());

        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        let mut found = false;
        while let Some(entry) = entries.next_entry().await.unwrap() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("keyslot.corrupt.")
            {
                found = true;
                break;
            }
        }
        assert!(found, "quarantined keyslot backup should exist");
    }

    #[tokio::test]
    async fn quarantine_succeeds_when_no_keyslot_exists() {
        let dir = tempdir().unwrap();
        let store = JsonKeySlotStore::new(dir.path().to_path_buf());
        store.quarantine().await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    use uc_observability_contract::error_source::find_source;

    use super::*;

    #[tokio::test]
    async fn unreadable_keyslot_keeps_io_source() {
        let dir = tempfile::tempdir().unwrap();
        let store = JsonKeySlotStore::new(dir.path().to_path_buf());
        // keyslot.json 位置放一个目录：路径存在，但读取返回真实的 IO 错误。
        std::fs::create_dir_all(dir.path().join("keyslot.json").join("keyslot.json")).unwrap();

        let error = store.load().await.unwrap_err();

        assert!(matches!(error, EncryptionError::IoFailure { .. }));
        assert!(find_source::<std::io::Error>(&error).is_some());
    }

    #[tokio::test]
    async fn malformed_keyslot_keeps_decode_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("keyslot.json"), b"{ not json").unwrap();
        let store = JsonKeySlotStore::new(dir.path().to_path_buf());

        let error = store.load().await.unwrap_err();

        assert!(matches!(error, EncryptionError::KeyMaterialCorrupt { .. }));
        assert!(find_source::<serde_json::Error>(&error).is_some());
    }
}
