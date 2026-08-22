use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;
use uc_application::deps::{RePairingStateError, RePairingStateStorePort};

use crate::security::{AdmissionKeyError, AdmissionKeyManager};

const PURPOSE: &[u8] = b"re-pairing-state-v1";
const FORMAT_VERSION: u16 = 1;

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedRePairingStateV1 {
    format_version: u16,
    required: bool,
}

pub struct EncryptedRePairingStateStore {
    path: PathBuf,
    keys: Arc<AdmissionKeyManager>,
    write_lock: Mutex<()>,
}

impl EncryptedRePairingStateStore {
    pub fn new(path: PathBuf, keys: Arc<AdmissionKeyManager>) -> Self {
        Self {
            path,
            keys,
            write_lock: Mutex::new(()),
        }
    }
}

#[async_trait]
impl RePairingStateStorePort for EncryptedRePairingStateStore {
    async fn is_required(&self) -> Result<bool, RePairingStateError> {
        let ciphertext = match fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(RePairingStateError::Unavailable),
        };
        let plaintext = self
            .keys
            .open_profile_payload(PURPOSE, &ciphertext)
            .map_err(map_key_error)?;
        let state: PersistedRePairingStateV1 =
            postcard::from_bytes(&plaintext).map_err(|_| RePairingStateError::Inconsistent)?;
        if state.format_version != FORMAT_VERSION {
            return Err(RePairingStateError::Inconsistent);
        }
        Ok(state.required)
    }

    async fn set_required(&self, required: bool) -> Result<(), RePairingStateError> {
        let _guard = self.write_lock.lock().await;
        let state = PersistedRePairingStateV1 {
            format_version: FORMAT_VERSION,
            required,
        };
        let plaintext =
            postcard::to_stdvec(&state).map_err(|_| RePairingStateError::Inconsistent)?;
        let ciphertext = self
            .keys
            .seal_profile_payload(PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        let parent = self.path.parent().ok_or(RePairingStateError::Unavailable)?;
        fs::create_dir_all(parent)
            .await
            .map_err(|_| RePairingStateError::Unavailable)?;
        let mut file = fs::File::create(&self.path)
            .await
            .map_err(|_| RePairingStateError::Unavailable)?;
        file.write_all(&ciphertext)
            .await
            .map_err(|_| RePairingStateError::Unavailable)?;
        file.sync_all()
            .await
            .map_err(|_| RePairingStateError::Unavailable)
    }
}

fn map_key_error(error: AdmissionKeyError) -> RePairingStateError {
    match error {
        AdmissionKeyError::SecureStorage => RePairingStateError::Unavailable,
        AdmissionKeyError::Corrupt | AdmissionKeyError::OpenFailed => {
            RePairingStateError::Inconsistent
        }
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

    fn store(path: PathBuf) -> EncryptedRePairingStateStore {
        EncryptedRePairingStateStore::new(
            path,
            Arc::new(AdmissionKeyManager::new(
                Arc::new(MemorySecureStorage::default()),
                [0x51; 16],
            )),
        )
    }

    #[tokio::test]
    async fn missing_state_defaults_to_not_required() {
        let directory = tempfile::tempdir().unwrap();
        let store = store(directory.path().join("re-pairing"));

        assert!(!store.is_required().await.unwrap());
    }

    #[tokio::test]
    async fn required_and_resolved_states_survive_reads_without_plaintext_persistence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("re-pairing");
        let store = store(path.clone());

        store.set_required(true).await.unwrap();
        assert!(store.is_required().await.unwrap());
        assert_ne!(fs::read(&path).await.unwrap(), vec![1]);

        store.set_required(false).await.unwrap();
        assert!(!store.is_required().await.unwrap());
    }
}
