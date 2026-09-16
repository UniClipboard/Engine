use std::fmt;
use std::sync::Arc;

use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use uc_core::ports::SecureStoragePort;

use super::crypto_model::EncryptedBlob;
use super::{v1_aead, MasterKey};

pub(super) const PROFILE_ADMISSION_KEY_NAME: &str = "profile_admission_master_key:v1";

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

// 单次读取固定使用同一把密钥；缓存只保存绑定摘要，不延长密钥的生命周期。
pub(crate) struct ProfilePayloadReader {
    key: MasterKey,
    aad: Vec<u8>,
}

impl ProfilePayloadReader {
    pub(crate) fn cache_binding(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"uniclipboard/admission-read-cache/v1\0");
        digest.update(self.key.as_bytes());
        digest.update(&self.aad);
        digest.finalize().into()
    }

    pub(crate) fn open(&self, ciphertext: &[u8]) -> Result<Vec<u8>, AdmissionKeyError> {
        let encrypted = decode_json_blob(ciphertext)?;
        self.open_blob(&encrypted)
    }

    pub(crate) fn open_compact(&self, ciphertext: &[u8]) -> Result<Vec<u8>, AdmissionKeyError> {
        let encrypted = decode_compact_blob(ciphertext)?;
        self.open_blob(&encrypted)
    }

    fn open_blob(&self, encrypted: &EncryptedBlob) -> Result<Vec<u8>, AdmissionKeyError> {
        v1_aead::decrypt_blob_xchacha(
            &self.key,
            &encrypted.nonce,
            &encrypted.ciphertext,
            &self.aad,
        )
        .map_err(|_| AdmissionKeyError::OpenFailed)
    }
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

    pub(crate) fn seal_profile_payload_compact(
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
        postcard::to_stdvec(&encrypted).map_err(|_| AdmissionKeyError::Corrupt)
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

    pub(crate) fn profile_payload_reader(
        &self,
        purpose: &[u8],
    ) -> Result<ProfilePayloadReader, AdmissionKeyError> {
        Ok(ProfilePayloadReader {
            key: self.profile_key()?,
            aad: self.profile_payload_aad(purpose),
        })
    }

    pub(crate) fn repository_token(
        &self,
        purpose: &[u8],
        value: &[u8],
    ) -> Result<[u8; 32], AdmissionKeyError> {
        let key = self.profile_key()?;
        let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes())
            .map_err(|_| AdmissionKeyError::Corrupt)?;
        mac.update(b"uniclipboard/admission-repository-token/v1\0");
        mac.update(&self.profile_generation);
        mac.update(&(purpose.len() as u64).to_be_bytes());
        mac.update(purpose);
        mac.update(&(value.len() as u64).to_be_bytes());
        mac.update(value);
        Ok(mac.finalize().into_bytes().into())
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
        postcard::to_stdvec(&encrypted).map_err(|_| AdmissionKeyError::Corrupt)
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
        let encrypted = decode_compatible_blob(ciphertext)?;
        v1_aead::decrypt_blob_xchacha(
            &key,
            &encrypted.nonce,
            &encrypted.ciphertext,
            &self.attempt_payload_aad(attempt_id),
        )
        .map_err(|_| AdmissionKeyError::OpenFailed)
    }
}

fn decode_json_blob(bytes: &[u8]) -> Result<EncryptedBlob, AdmissionKeyError> {
    serde_json::from_slice(bytes).map_err(|_| AdmissionKeyError::Corrupt)
}

fn decode_compact_blob(bytes: &[u8]) -> Result<EncryptedBlob, AdmissionKeyError> {
    postcard::from_bytes(bytes).map_err(|_| AdmissionKeyError::Corrupt)
}

fn decode_compatible_blob(bytes: &[u8]) -> Result<EncryptedBlob, AdmissionKeyError> {
    decode_compact_blob(bytes).or_else(|_| decode_json_blob(bytes))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use uc_core::ports::{SecureStorageError, SecureStoragePort};

    use super::{v1_aead, AdmissionKeyManager, MasterKey};

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

    #[test]
    fn attempt_payloads_use_compact_storage_and_open_legacy_json() {
        let manager = AdmissionKeyManager::new(Arc::new(MemorySecureStorage::default()), [1; 16]);
        let attempt_id = [2; 32];
        let wrapped = manager.create_wrapped_attempt_key(attempt_id).unwrap();
        let plaintext = vec![0x51; 1024 * 1024];

        let compact = manager
            .seal_attempt_payload(attempt_id, &wrapped, &plaintext)
            .unwrap();
        assert!(compact.len() < plaintext.len() + 256);
        assert_eq!(
            manager
                .open_attempt_payload(attempt_id, &wrapped, &compact)
                .unwrap(),
            plaintext
        );

        let attempt_key = manager.unwrap_attempt_key(attempt_id, &wrapped).unwrap();
        let key = MasterKey::from_bytes(attempt_key.as_bytes()).unwrap();
        let encrypted = v1_aead::encrypt_blob_xchacha(
            &key,
            &plaintext,
            &manager.attempt_payload_aad(attempt_id),
        )
        .unwrap();
        let legacy_json = serde_json::to_vec(&encrypted).unwrap();
        assert_eq!(
            manager
                .open_attempt_payload(attempt_id, &wrapped, &legacy_json)
                .unwrap(),
            plaintext
        );
    }

    #[test]
    fn repository_tokens_are_stable_and_context_bound() {
        let storage = Arc::new(MemorySecureStorage::default());
        let manager = AdmissionKeyManager::new(storage.clone(), [1; 16]);
        let first = manager.repository_token(b"lookup", b"value").unwrap();
        let second = manager.repository_token(b"lookup", b"value").unwrap();
        assert_eq!(first, second);
        assert_ne!(
            first,
            manager.repository_token(b"content", b"value").unwrap()
        );
        assert_ne!(
            first,
            manager.repository_token(b"lookup", b"other").unwrap()
        );

        let other_generation = AdmissionKeyManager::new(storage, [2; 16]);
        assert_ne!(
            first,
            other_generation
                .repository_token(b"lookup", b"value")
                .unwrap()
        );
    }
}
