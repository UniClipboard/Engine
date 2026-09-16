//! V3 content protection 的紧凑二进制 envelope。
//!
//! ```text
//! magic(4) | version(2, BE) | aead_id(1) | key_id_len(2, BE) |
//! group_epoch(8, BE) | nonce(24) | content_key_id | ciphertext+tag
//! ```
//!
//! Header 只携带历史 key resolution 所需 identity；保护组、Space 和 purpose
//! 由 vault 与构造时固定的 module context 恢复，不进入明文 framing。

use uc_core::membership::{ContentKeyId, GroupEpoch};

use super::ContentProtectionError;

pub(super) const FORMAT_VERSION_V3: u16 = 3;
pub(super) const AEAD_XCHACHA20_POLY1305: &str = "XChaCha20Poly1305";
const MAGIC: [u8; 4] = *b"UCP3";
const AEAD_ID_XCHACHA20_POLY1305: u8 = 1;
const NONCE_BYTES: usize = 24;
const KEY_ID_LENGTH_BYTES: usize = 2;
const FIXED_HEADER_BYTES: usize = MAGIC.len() + 2 + 1 + KEY_ID_LENGTH_BYTES + 8 + NONCE_BYTES;
const MIN_AEAD_CIPHERTEXT_BYTES: usize = 16;

pub(super) struct DecodedCiphertextV3 {
    pub(super) content_key_id: ContentKeyId,
    pub(super) group_epoch: GroupEpoch,
    pub(super) nonce: [u8; NONCE_BYTES],
    pub(super) ciphertext: Vec<u8>,
}

pub(super) fn encode(
    content_key_id: &ContentKeyId,
    group_epoch: GroupEpoch,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
) -> Result<Vec<u8>, ContentProtectionError> {
    let key_id = content_key_id.as_str().as_bytes();
    let key_id_length =
        u16::try_from(key_id.len()).map_err(|source| ContentProtectionError::Cryptography {
            source: anyhow::Error::new(source).context("V3 content key id is too large"),
        })?;
    let nonce: [u8; NONCE_BYTES] =
        nonce
            .try_into()
            .map_err(|_| ContentProtectionError::Cryptography {
                source: anyhow::anyhow!("V3 content nonce has an invalid length"),
            })?;
    if ciphertext.len() < MIN_AEAD_CIPHERTEXT_BYTES {
        return Err(ContentProtectionError::Cryptography {
            source: anyhow::anyhow!("V3 AEAD ciphertext is shorter than its tag"),
        });
    }
    let capacity = FIXED_HEADER_BYTES
        .checked_add(key_id.len())
        .and_then(|length| length.checked_add(ciphertext.len()))
        .ok_or_else(|| ContentProtectionError::Cryptography {
            source: anyhow::anyhow!("V3 ciphertext length overflow"),
        })?;
    let mut encoded = Vec::with_capacity(capacity);
    encoded.extend_from_slice(&MAGIC);
    encoded.extend_from_slice(&FORMAT_VERSION_V3.to_be_bytes());
    encoded.push(AEAD_ID_XCHACHA20_POLY1305);
    encoded.extend_from_slice(&key_id_length.to_be_bytes());
    encoded.extend_from_slice(&group_epoch.value().to_be_bytes());
    encoded.extend_from_slice(&nonce);
    encoded.extend_from_slice(key_id);
    encoded.extend_from_slice(&ciphertext);
    Ok(encoded)
}

pub(super) fn decode(bytes: &[u8]) -> Result<DecodedCiphertextV3, ContentProtectionError> {
    if bytes.len() < FIXED_HEADER_BYTES + 1 + MIN_AEAD_CIPHERTEXT_BYTES {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "V3 ciphertext header is truncated"
        )));
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "V3 ciphertext magic is invalid"
        )));
    }
    let version = u16::from_be_bytes(
        bytes[4..6]
            .try_into()
            .map_err(|source| invalid_ciphertext(anyhow::Error::new(source)))?,
    );
    if version != FORMAT_VERSION_V3 {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "unsupported ciphertext version"
        )));
    }
    if bytes[6] != AEAD_ID_XCHACHA20_POLY1305 {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "unsupported ciphertext algorithm"
        )));
    }
    let key_id_length = usize::from(u16::from_be_bytes(
        bytes[7..9]
            .try_into()
            .map_err(|source| invalid_ciphertext(anyhow::Error::new(source)))?,
    ));
    let ciphertext_start = FIXED_HEADER_BYTES
        .checked_add(key_id_length)
        .ok_or_else(|| invalid_ciphertext(anyhow::anyhow!("V3 header length overflow")))?;
    if key_id_length == 0 || bytes.len() < ciphertext_start + MIN_AEAD_CIPHERTEXT_BYTES {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "invalid ciphertext framing"
        )));
    }
    let group_epoch = GroupEpoch::new(u64::from_be_bytes(
        bytes[9..17]
            .try_into()
            .map_err(|source| invalid_ciphertext(anyhow::Error::new(source)))?,
    ));
    let nonce = bytes[17..FIXED_HEADER_BYTES]
        .try_into()
        .map_err(|source| invalid_ciphertext(anyhow::Error::new(source)))?;
    let key_id = std::str::from_utf8(&bytes[FIXED_HEADER_BYTES..ciphertext_start])
        .map_err(|source| invalid_ciphertext(anyhow::Error::new(source)))?;
    let content_key_id = ContentKeyId::from_string(key_id).map_err(invalid_ciphertext)?;
    if content_key_id == ContentKeyId::legacy_v1() {
        return Err(invalid_ciphertext(anyhow::anyhow!(
            "legacy content key cannot protect V3 ciphertext"
        )));
    }
    Ok(DecodedCiphertextV3 {
        content_key_id,
        group_epoch,
        nonce,
        ciphertext: bytes[ciphertext_start..].to_vec(),
    })
}

fn invalid_ciphertext(source: impl Into<anyhow::Error>) -> ContentProtectionError {
    ContentProtectionError::InvalidCiphertext {
        source: source.into().context("V3 ciphertext validation failed"),
    }
}
