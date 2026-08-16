use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uc_core::membership::ActiveSpaceManifestV2;

use super::{AdmissionKeyError, AdmissionKeyManager};

const ACTIVE_MANIFEST_FILE: &str = ".active-space-manifest-v2";
const ACTIVE_MANIFEST_PURPOSE: &[u8] = b"active-space-manifest-v2";

#[derive(Debug, thiserror::Error)]
pub enum ActiveSpaceManifestStoreError {
    #[error("active space manifest storage is unavailable")]
    Storage,
    #[error("active space manifest is corrupt")]
    Corrupt,
}

pub struct ActiveSpaceManifestStore {
    path: PathBuf,
    keys: Arc<AdmissionKeyManager>,
    write_lock: Mutex<()>,
}

impl ActiveSpaceManifestStore {
    pub fn new(base_dir: PathBuf, keys: Arc<AdmissionKeyManager>) -> Self {
        Self {
            path: base_dir.join(ACTIVE_MANIFEST_FILE),
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
