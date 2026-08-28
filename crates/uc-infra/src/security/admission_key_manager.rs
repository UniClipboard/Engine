use std::fmt;
use std::sync::Arc;

use rand::RngCore;
use uc_core::ports::SecureStoragePort;

use super::crypto_model::EncryptedBlob;
use super::{v1_aead, MasterKey};

const PROFILE_ADMISSION_KEY_NAME: &str = "profile_admission_master_key:v1";

#[derive(Debug, thiserror::Error)]
pub enum AdmissionKeyError {
    #[error("profile admission key storage is unavailable")]
    SecureStorage,
    #[error("profile admission key is corrupt")]
    Corrupt,
    #[error("attempt data key could not be opened")]
    OpenFailed,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SpaceAdmissionDataKey([u8; 32]);

impl fmt::Debug for SpaceAdmissionDataKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SpaceAdmissionDataKey([REDACTED])")
    }
}

impl SpaceAdmissionDataKey {
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WrappedSpaceAdmissionDataKey {
    pub format_version: u16,
    encrypted_key: EncryptedBlob,
}

#[derive(Clone)]
pub struct AdmissionKeyManager {
    secure_storage: Arc<dyn SecureStoragePort>,
    profile_generation: [u8; 16],
}

impl AdmissionKeyManager {
    pub fn new(secure_storage: Arc<dyn SecureStoragePort>, profile_generation: [u8; 16]) -> Self {
        Self {
            secure_storage,
            profile_generation,
        }
    }

    fn profile_key(&self) -> Result<MasterKey, AdmissionKeyError> {
        if let Some(bytes) = self
            .secure_storage
            .get(PROFILE_ADMISSION_KEY_NAME)
            .map_err(|_| AdmissionKeyError::SecureStorage)?
        {
            return MasterKey::from_bytes(&bytes).map_err(|_| AdmissionKeyError::Corrupt);
        }

        let mut generated = [0u8; 32];
        rand::rng().fill_bytes(&mut generated);
        self.secure_storage
            .set(PROFILE_ADMISSION_KEY_NAME, &generated)
            .map_err(|_| AdmissionKeyError::SecureStorage)?;
        let persisted = self
            .secure_storage
            .get(PROFILE_ADMISSION_KEY_NAME)
            .map_err(|_| AdmissionKeyError::SecureStorage)?
            .ok_or(AdmissionKeyError::SecureStorage)?;
        MasterKey::from_bytes(&persisted).map_err(|_| AdmissionKeyError::Corrupt)
    }

    pub(crate) const fn profile_generation(&self) -> [u8; 16] {
        self.profile_generation
    }

    pub fn profile_key_exists(&self) -> Result<bool, AdmissionKeyError> {
        self.secure_storage
            .get(PROFILE_ADMISSION_KEY_NAME)
            .map(|value| value.is_some())
            .map_err(|_| AdmissionKeyError::SecureStorage)
    }

    pub fn delete_profile_key(&self) -> Result<(), AdmissionKeyError> {
        self.secure_storage
            .delete(PROFILE_ADMISSION_KEY_NAME)
            .map_err(|_| AdmissionKeyError::SecureStorage)?;
        if self.profile_key_exists()? {
            return Err(AdmissionKeyError::SecureStorage);
        }
        Ok(())
    }

    fn profile_payload_aad(&self, purpose: &[u8]) -> Vec<u8> {
        let mut aad = Vec::with_capacity(64 + purpose.len());
        aad.extend_from_slice(b"uniclipboard/admission-profile-payload/v1\0");
        aad.extend_from_slice(&self.profile_generation);
        aad.extend_from_slice(&(purpose.len() as u64).to_be_bytes());
        aad.extend_from_slice(purpose);
        aad
    }

    pub(crate) fn seal_profile_payload(
        &self,
        purpose: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, AdmissionKeyError> {
        let encrypted = v1_aead::encrypt_blob_xchacha(
            &self.profile_key()?,
            plaintext,
            &self.profile_payload_aad(purpose),
        )
        .map_err(|_| AdmissionKeyError::OpenFailed)?;
        serde_json::to_vec(&encrypted).map_err(|_| AdmissionKeyError::Corrupt)
    }

    pub(crate) fn open_profile_payload(
        &self,
        purpose: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, AdmissionKeyError> {
        let encrypted: EncryptedBlob =
            serde_json::from_slice(ciphertext).map_err(|_| AdmissionKeyError::Corrupt)?;
        v1_aead::decrypt_blob_xchacha(
            &self.profile_key()?,
            &encrypted.nonce,
            &encrypted.ciphertext,
            &self.profile_payload_aad(purpose),
        )
        .map_err(|_| AdmissionKeyError::OpenFailed)
    }

    fn attempt_key_aad(&self, attempt_id: [u8; 32]) -> Vec<u8> {
        let mut aad = Vec::with_capacity(88);
        aad.extend_from_slice(b"uniclipboard/admission-attempt-data-key/v1\0");
        aad.extend_from_slice(&self.profile_generation);
        aad.extend_from_slice(&attempt_id);
        aad
    }

    pub fn create_wrapped_attempt_key(
        &self,
        attempt_id: [u8; 32],
    ) -> Result<WrappedSpaceAdmissionDataKey, AdmissionKeyError> {
        let profile_key = self.profile_key()?;
        let mut attempt_key = [0u8; 32];
        rand::rng().fill_bytes(&mut attempt_key);
        let encrypted_key = v1_aead::encrypt_blob_xchacha(
            &profile_key,
            &attempt_key,
            &self.attempt_key_aad(attempt_id),
        )
        .map_err(|_| AdmissionKeyError::OpenFailed)?;
        Ok(WrappedSpaceAdmissionDataKey {
            format_version: 1,
            encrypted_key,
        })
    }

    pub(crate) fn unwrap_attempt_key(
        &self,
        attempt_id: [u8; 32],
        wrapped: &WrappedSpaceAdmissionDataKey,
    ) -> Result<SpaceAdmissionDataKey, AdmissionKeyError> {
        if wrapped.format_version != 1 {
            return Err(AdmissionKeyError::Corrupt);
        }
        let profile_key = self.profile_key()?;
        let plaintext = v1_aead::decrypt_blob_xchacha(
            &profile_key,
            &wrapped.encrypted_key.nonce,
            &wrapped.encrypted_key.ciphertext,
            &self.attempt_key_aad(attempt_id),
        )
        .map_err(|_| AdmissionKeyError::OpenFailed)?;
        let bytes: [u8; 32] = plaintext
            .try_into()
            .map_err(|_| AdmissionKeyError::Corrupt)?;
        Ok(SpaceAdmissionDataKey(bytes))
    }

    fn attempt_payload_aad(&self, attempt_id: [u8; 32]) -> Vec<u8> {
        let mut aad = Vec::with_capacity(88);
        aad.extend_from_slice(b"uniclipboard/admission-attempt-payload/v1\0");
        aad.extend_from_slice(&self.profile_generation);
        aad.extend_from_slice(&attempt_id);
        aad
    }

    pub(crate) fn seal_attempt_payload(
        &self,
        attempt_id: [u8; 32],
        wrapped: &WrappedSpaceAdmissionDataKey,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, AdmissionKeyError> {
        let attempt_key = self.unwrap_attempt_key(attempt_id, wrapped)?;
        let key = MasterKey::from_bytes(attempt_key.as_bytes())
            .map_err(|_| AdmissionKeyError::Corrupt)?;
        let encrypted =
            v1_aead::encrypt_blob_xchacha(&key, plaintext, &self.attempt_payload_aad(attempt_id))
                .map_err(|_| AdmissionKeyError::OpenFailed)?;
        serde_json::to_vec(&encrypted).map_err(|_| AdmissionKeyError::Corrupt)
    }

    pub(crate) fn open_attempt_payload(
        &self,
        attempt_id: [u8; 32],
        wrapped: &WrappedSpaceAdmissionDataKey,
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, AdmissionKeyError> {
        let attempt_key = self.unwrap_attempt_key(attempt_id, wrapped)?;
        let key = MasterKey::from_bytes(attempt_key.as_bytes())
            .map_err(|_| AdmissionKeyError::Corrupt)?;
        let encrypted: EncryptedBlob =
            serde_json::from_slice(ciphertext).map_err(|_| AdmissionKeyError::Corrupt)?;
        v1_aead::decrypt_blob_xchacha(
            &key,
            &encrypted.nonce,
            &encrypted.ciphertext,
            &self.attempt_payload_aad(attempt_id),
        )
        .map_err(|_| AdmissionKeyError::OpenFailed)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::AdmissionKeyManager;

    #[derive(Default)]
    struct MemorySecureStorage {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl SecureStoragePort for MemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_vec());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    #[test]
    fn profile_key_survives_restart_and_attempt_wrapping_is_context_bound() {
        let storage = Arc::new(MemorySecureStorage::default());
        let generation = [1; 16];
        let manager = AdmissionKeyManager::new(storage.clone(), generation);
        let wrapped = manager.create_wrapped_attempt_key([2; 32]).unwrap();

        let reopened = AdmissionKeyManager::new(storage, generation);
        let first = reopened
            .unwrap_attempt_key([2; 32], &wrapped)
            .expect("same attempt can recover its data key");
        let second = reopened
            .unwrap_attempt_key([2; 32], &wrapped)
            .expect("recovery is deterministic");
        assert_eq!(first, second);
        assert!(reopened.unwrap_attempt_key([3; 32], &wrapped).is_err());
    }
}
