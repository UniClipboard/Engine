use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uc_core::membership::ActiveSpaceManifestV2;

use super::{AdmissionKeyError, AdmissionKeyManager};

const ACTIVE_MANIFEST_FILE: &str = ".active-space-manifest-v2";
const ACTIVE_MANIFEST_PURPOSE: &[u8] = b"active-space-manifest-v2";
const DEVICE_RESET_JOURNAL_FILE: &str = ".device-management-reset-v1";
const DEVICE_RESET_JOURNAL_PURPOSE: &[u8] = b"device-management-reset-v1";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DeviceManagementResetJournalV1 {
    pub(crate) format_version: u16,
    pub(crate) target_space_id: String,
    pub(crate) target_generation: [u8; 16],
    pub(crate) source_space_id: Option<String>,
    pub(crate) source_generation: Option<[u8; 16]>,
}

impl DeviceManagementResetJournalV1 {
    pub(crate) fn validate(&self) -> bool {
        self.format_version == 1
            && !self.target_space_id.is_empty()
            && self.source_space_id.is_some() == self.source_generation.is_some()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ActiveSpaceManifestStoreError {
    #[error("active space manifest storage is unavailable")]
    Storage,
    #[error("active space manifest is corrupt")]
    Corrupt,
}

pub struct ActiveSpaceManifestStore {
    path: PathBuf,
    reset_journal_path: PathBuf,
    keys: Arc<AdmissionKeyManager>,
    write_lock: Mutex<()>,
}

impl ActiveSpaceManifestStore {
    pub fn new(base_dir: PathBuf, keys: Arc<AdmissionKeyManager>) -> Self {
        Self {
            path: base_dir.join(ACTIVE_MANIFEST_FILE),
            reset_journal_path: base_dir.join(DEVICE_RESET_JOURNAL_FILE),
            keys,
            write_lock: Mutex::new(()),
        }
    }

    pub async fn load(
        &self,
    ) -> Result<Option<ActiveSpaceManifestV2>, ActiveSpaceManifestStoreError> {
        let ciphertext = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceManifestStoreError::Storage),
        };
        let plaintext = self
            .keys
            .open_profile_payload(ACTIVE_MANIFEST_PURPOSE, &ciphertext)
            .map_err(map_key_error)?;
        let manifest: ActiveSpaceManifestV2 =
            postcard::from_bytes(&plaintext).map_err(|_| ActiveSpaceManifestStoreError::Corrupt)?;
        manifest
            .validate()
            .then_some(Some(manifest))
            .ok_or(ActiveSpaceManifestStoreError::Corrupt)
    }

    pub fn load_sync(
        &self,
    ) -> Result<Option<ActiveSpaceManifestV2>, ActiveSpaceManifestStoreError> {
        let ciphertext = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceManifestStoreError::Storage),
        };
        self.decode(&ciphertext).map(Some)
    }

    fn decode(
        &self,
        ciphertext: &[u8],
    ) -> Result<ActiveSpaceManifestV2, ActiveSpaceManifestStoreError> {
        let plaintext = self
            .keys
            .open_profile_payload(ACTIVE_MANIFEST_PURPOSE, ciphertext)
            .map_err(map_key_error)?;
        let manifest: ActiveSpaceManifestV2 =
            postcard::from_bytes(&plaintext).map_err(|_| ActiveSpaceManifestStoreError::Corrupt)?;
        manifest
            .validate()
            .then_some(manifest)
            .ok_or(ActiveSpaceManifestStoreError::Corrupt)
    }

    pub async fn promote(
        &self,
        manifest: &ActiveSpaceManifestV2,
    ) -> Result<(), ActiveSpaceManifestStoreError> {
        if !manifest.validate() {
            return Err(ActiveSpaceManifestStoreError::Corrupt);
        }
        let _guard = self.write_lock.lock().await;
        let parent = self
            .path
            .parent()
            .ok_or(ActiveSpaceManifestStoreError::Storage)?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        let plaintext =
            postcard::to_stdvec(manifest).map_err(|_| ActiveSpaceManifestStoreError::Corrupt)?;
        let ciphertext = self
            .keys
            .seal_profile_payload(ACTIVE_MANIFEST_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        let temporary = self.path.with_extension("tmp");
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        file.write_all(&ciphertext)
            .await
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        file.sync_all()
            .await
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        drop(file);
        replace_file_atomically(&temporary, &self.path)
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        sync_parent_directory(parent).map_err(|_| ActiveSpaceManifestStoreError::Storage)
    }

    pub(crate) async fn load_device_reset_journal(
        &self,
    ) -> Result<Option<DeviceManagementResetJournalV1>, ActiveSpaceManifestStoreError> {
        let ciphertext = match tokio::fs::read(&self.reset_journal_path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(ActiveSpaceManifestStoreError::Storage),
        };
        let plaintext = self
            .keys
            .open_profile_payload(DEVICE_RESET_JOURNAL_PURPOSE, &ciphertext)
            .map_err(map_key_error)?;
        let journal: DeviceManagementResetJournalV1 =
            postcard::from_bytes(&plaintext).map_err(|_| ActiveSpaceManifestStoreError::Corrupt)?;
        journal
            .validate()
            .then_some(Some(journal))
            .ok_or(ActiveSpaceManifestStoreError::Corrupt)
    }

    pub(crate) async fn save_device_reset_journal(
        &self,
        journal: &DeviceManagementResetJournalV1,
    ) -> Result<(), ActiveSpaceManifestStoreError> {
        if !journal.validate() {
            return Err(ActiveSpaceManifestStoreError::Corrupt);
        }
        let _guard = self.write_lock.lock().await;
        let parent = self
            .reset_journal_path
            .parent()
            .ok_or(ActiveSpaceManifestStoreError::Storage)?;
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        let plaintext =
            postcard::to_stdvec(journal).map_err(|_| ActiveSpaceManifestStoreError::Corrupt)?;
        let ciphertext = self
            .keys
            .seal_profile_payload(DEVICE_RESET_JOURNAL_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        let temporary = self.reset_journal_path.with_extension("tmp");
        let mut file = tokio::fs::File::create(&temporary)
            .await
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        file.write_all(&ciphertext)
            .await
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        file.sync_all()
            .await
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        drop(file);
        replace_file_atomically(&temporary, &self.reset_journal_path)
            .map_err(|_| ActiveSpaceManifestStoreError::Storage)?;
        sync_parent_directory(parent).map_err(|_| ActiveSpaceManifestStoreError::Storage)
    }

    pub(crate) async fn clear_device_reset_journal(
        &self,
    ) -> Result<(), ActiveSpaceManifestStoreError> {
        let _guard = self.write_lock.lock().await;
        match tokio::fs::remove_file(&self.reset_journal_path).await {
            Ok(()) => {
                let parent = self
                    .reset_journal_path
                    .parent()
                    .ok_or(ActiveSpaceManifestStoreError::Storage)?;
                sync_parent_directory(parent).map_err(|_| ActiveSpaceManifestStoreError::Storage)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ActiveSpaceManifestStoreError::Storage),
        }
    }
}

#[cfg(not(windows))]
fn replace_file_atomically(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(not(windows))]
fn sync_parent_directory(parent: &std::path::Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(windows)]
fn sync_parent_directory(_parent: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn replace_file_atomically(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let wide = |path: &std::path::Path| {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        value.push(0);
        value
    };
    let source = wide(source);
    let destination = wide(destination);
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn map_key_error(error: AdmissionKeyError) -> ActiveSpaceManifestStoreError {
    match error {
        AdmissionKeyError::Corrupt | AdmissionKeyError::OpenFailed => {
            ActiveSpaceManifestStoreError::Corrupt
        }
        AdmissionKeyError::SecureStorage => ActiveSpaceManifestStoreError::Storage,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::*;

    #[tokio::test]
    async fn device_reset_journal_round_trips_encrypted_and_clears_idempotently() {
        let directory = tempfile::tempdir().unwrap();
        let store = ActiveSpaceManifestStore::new(
            directory.path().to_path_buf(),
            Arc::new(AdmissionKeyManager::new(
                Arc::new(MemorySecureStorage::default()),
                [0x61; 16],
            )),
        );
        let journal = DeviceManagementResetJournalV1 {
            format_version: 1,
            target_space_id: "private-reset-target".to_owned(),
            target_generation: [0x62; 16],
            source_space_id: Some("private-source-space".to_owned()),
            source_generation: Some([0x63; 16]),
        };

        store.save_device_reset_journal(&journal).await.unwrap();

        assert_eq!(
            store.load_device_reset_journal().await.unwrap(),
            Some(journal)
        );
        let bytes = tokio::fs::read(directory.path().join(DEVICE_RESET_JOURNAL_FILE))
            .await
            .unwrap();
        assert!(!bytes
            .windows(b"private-reset-target".len())
            .any(|window| window == b"private-reset-target"));
        assert!(!bytes
            .windows(b"private-source-space".len())
            .any(|window| window == b"private-source-space"));
        store.clear_device_reset_journal().await.unwrap();
        store.clear_device_reset_journal().await.unwrap();
        assert_eq!(store.load_device_reset_journal().await.unwrap(), None);
    }

    #[derive(Default)]
    struct MemorySecureStorage(StdMutex<HashMap<String, Vec<u8>>>);

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
    async fn promotes_one_encrypted_self_verifying_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemorySecureStorage::default()),
            [0x21; 16],
        ));
        let store = ActiveSpaceManifestStore::new(directory.path().to_path_buf(), keys);
        let first =
            ActiveSpaceManifestV2::new("space-a".to_owned(), [0x22; 16], [0x23; 16], [0x24; 16])
                .unwrap();
        store.promote(&first).await.unwrap();
        assert_eq!(store.load().await.unwrap(), Some(first));
        let bytes = tokio::fs::read(directory.path().join(ACTIVE_MANIFEST_FILE))
            .await
            .unwrap();
        assert!(!bytes
            .windows(b"space-a".len())
            .any(|window| window == b"space-a"));

        let second =
            ActiveSpaceManifestV2::new("space-b".to_owned(), [0x25; 16], [0x26; 16], [0x27; 16])
                .unwrap();
        store.promote(&second).await.unwrap();
        assert_eq!(store.load().await.unwrap(), Some(second));
        assert_eq!(store.load_sync().unwrap(), store.load().await.unwrap());
    }
}
