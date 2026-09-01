use serde::{Deserialize, Serialize};
use uc_core::membership::{ContentKeyId, GroupEpoch};

use super::ContentProtectionError;

pub(super) const FORMAT_VERSION_V3: u16 = 3;
pub(super) const AEAD_XCHACHA20_POLY1305: &str = "XChaCha20Poly1305";
const NONCE_BYTES: usize = 24;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedCiphertextV3 {
    version: u16,
    aead: String,
    content_key_id: String,
    group_epoch: u64,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

pub(super) struct DecodedCiphertextV3 {
    pub(super) content_key_id: ContentKeyId,
    pub(super) group_epoch: GroupEpoch,
    pub(super) nonce: Vec<u8>,
    pub(super) ciphertext: Vec<u8>,
}

pub(super) fn encode(
    content_key_id: &ContentKeyId,
    group_epoch: GroupEpoch,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
) -> Result<Vec<u8>, ContentProtectionError> {
    serde_json::to_vec(&PersistedCiphertextV3 {
        version: FORMAT_VERSION_V3,
        aead: AEAD_XCHACHA20_POLY1305.to_owned(),
        content_key_id: content_key_id.as_str().to_owned(),
        group_epoch: group_epoch.value(),
        nonce,
        ciphertext,
    })
    .map_err(|source| ContentProtectionError::Cryptography {
        source: anyhow::Error::new(source).context("failed to encode V3 ciphertext"),
    })
}

pub(super) fn decode(bytes: &[u8]) -> Result<DecodedCiphertextV3, ContentProtectionError> {
    let persisted: PersistedCiphertextV3 =
        serde_json::from_slice(bytes).map_err(|source| invalid_ciphertext(source))?;
    if persisted.version != FORMAT_VERSION_V3 {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "unsupported ciphertext version"
        )));
    }
    if persisted.aead != AEAD_XCHACHA20_POLY1305 {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "unsupported ciphertext algorithm"
        )));
    }
    if persisted.nonce.len() != NONCE_BYTES || persisted.ciphertext.is_empty() {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "invalid ciphertext framing"
        )));
    }
    let content_key_id =
        ContentKeyId::from_string(persisted.content_key_id).map_err(invalid_ciphertext)?;
    if content_key_id == ContentKeyId::legacy_v1() {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "legacy content key cannot protect V3 ciphertext"
        )));
    }
    Ok(DecodedCiphertextV3 {
        content_key_id,
        group_epoch: GroupEpoch::new(persisted.group_epoch),
        nonce: persisted.nonce,
        ciphertext: persisted.ciphertext,
    })
}

fn invalid_ciphertext(source: impl Into<anyhow::Error>) -> ContentProtectionError {
    ContentProtectionError::InvalidCiphertext {
        source: source.into().context("V3 ciphertext validation failed"),
    }
}
