use hmac::{Hmac, Mac};
use serde::{de::DeserializeOwned, Serialize};
use sha2::Sha256;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::KeyEpochError;

use crate::security::crypto_model::EncryptedBlob;
use crate::security::{v1_aead, MasterKey};

use super::backend;

pub(super) fn space_lookup_token(
    master_key: &MasterKey,
    space_id: &SpaceId,
) -> Result<String, KeyEpochError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(master_key.as_bytes()).map_err(backend)?;
    mac.update(b"uc-space-lookup-v1|");
    mac.update(&(space_id.as_ref().len() as u64).to_be_bytes());
    mac.update(space_id.as_ref().as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

pub(super) fn device_lookup_token(
    master_key: &MasterKey,
    device_id: &DeviceId,
) -> Result<String, KeyEpochError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(master_key.as_bytes()).map_err(backend)?;
    mac.update(b"uc-legacy-upgrade-peer-lookup-v1|");
    mac.update(&(device_id.as_str().len() as u64).to_be_bytes());
    mac.update(device_id.as_str().as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

pub(super) fn seal<T: Serialize>(
    master_key: &MasterKey,
    value: &T,
    aad: &[u8],
) -> Result<Vec<u8>, KeyEpochError> {
    let plaintext = serde_json::to_vec(value).map_err(backend)?;
    let encrypted = v1_aead::encrypt_blob_xchacha(master_key, &plaintext, aad).map_err(backend)?;
    serde_json::to_vec(&encrypted).map_err(backend)
}

pub(super) fn open<T: DeserializeOwned>(
    master_key: &MasterKey,
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<T, KeyEpochError> {
    let encrypted: EncryptedBlob =
        serde_json::from_slice(ciphertext).map_err(|_| KeyEpochError::DecryptionFailed)?;
    let plaintext =
        v1_aead::decrypt_blob_xchacha(master_key, &encrypted.nonce, &encrypted.ciphertext, aad)
            .map_err(|_| KeyEpochError::DecryptionFailed)?;
    serde_json::from_slice(&plaintext).map_err(|_| KeyEpochError::PersistedStateIntegrityFailed)
}
