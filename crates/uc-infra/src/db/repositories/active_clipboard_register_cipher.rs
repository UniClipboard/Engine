use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uc_core::clipboard::MobileConsumableRef;
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::ids::{EntryId, ProfileId};

use crate::security::v1_aead::{decrypt_xchacha_raw, encrypt_xchacha_raw};
use crate::security::ContentProtection;
use crate::space::InMemorySession;

const MAGIC: [u8; 4] = *b"UCAR";
const FORMAT_VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = MAGIC.len() + 1 + NONCE_LEN;

/// Domain-separation labels for the consumable-reference envelope. Both derive
/// from the same versioned prefix so the HKDF info and the AEAD AAD can only
/// evolve together; the distinct suffixes keep the two roles from ever being
/// interchangeable.
macro_rules! consumable_label {
    ($role:literal) => {
        concat!("uniclipboard-active-register-consumable/v1#", $role).as_bytes()
    };
}

/// HKDF `info` input for deriving the envelope key (used by the repository).
pub(crate) const CONSUMABLE_HKDF_INFO: &[u8] = consumable_label!("hkdf");
const AAD: &[u8] = consumable_label!("aad");

#[derive(Debug, Serialize, Deserialize)]
struct ConsumableRefPayload {
    snapshot_hash: String,
    entry_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ActiveRegisterCipherError {
    #[error("active-register consumable envelope too short")]
    Truncated,
    #[error("active-register consumable envelope magic mismatch")]
    BadMagic,
    #[error("unsupported active-register consumable envelope version: {0}")]
    UnsupportedVersion(u8),
    #[error("active-register consumable serialization failed")]
    Serialize,
    #[error("active-register consumable encryption failed")]
    Encrypt,
    #[error("active-register consumable verification failed")]
    Decrypt,
    #[error("active-register consumable payload is invalid")]
    Deserialize,
    #[error("V3 active-register protection failed")]
    V3 {
        #[source]
        source: anyhow::Error,
    },
}

pub(crate) struct V3ActiveClipboardRegisterCipher {
    protection: Arc<ContentProtection>,
}

impl V3ActiveClipboardRegisterCipher {
    pub(crate) fn new(protection: Arc<ContentProtection>) -> Self {
        Self { protection }
    }

    pub(crate) async fn seal(
        &self,
        reference: &MobileConsumableRef,
    ) -> Result<Vec<u8>, ActiveRegisterCipherError> {
        let plaintext = postcard::to_stdvec(&ConsumableRefPayload {
            snapshot_hash: reference.snapshot_hash.clone(),
            entry_id: reference.entry_id.as_ref().to_string(),
        })
        .map_err(|_| ActiveRegisterCipherError::Serialize)?;
        self.protection
            .seal_for_active(&Plaintext::new(plaintext), &Aad::new(AAD.to_vec()))
            .await
            .map(|ciphertext| ciphertext.into_bytes())
            .map_err(|source| ActiveRegisterCipherError::V3 {
                source: anyhow::Error::new(source).context("seal V3 active register"),
            })
    }

    pub(crate) async fn open(
        &self,
        ciphertext: &[u8],
    ) -> Result<MobileConsumableRef, ActiveRegisterCipherError> {
        let plaintext = self
            .protection
            .open(
                &Ciphertext::new(ciphertext.to_vec()),
                &Aad::new(AAD.to_vec()),
            )
            .await
            .map_err(|source| ActiveRegisterCipherError::V3 {
                source: anyhow::Error::new(source).context("open V3 active register"),
            })?;
        let payload: ConsumableRefPayload = postcard::from_bytes(plaintext.as_bytes())
            .map_err(|_| ActiveRegisterCipherError::Deserialize)?;
        Ok(MobileConsumableRef {
            snapshot_hash: payload.snapshot_hash,
            entry_id: EntryId::from(payload.entry_id.as_str()),
        })
    }
}

pub struct ActiveClipboardRegisterCipher {
    key: [u8; 32],
}

impl ActiveClipboardRegisterCipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    pub(crate) fn legacy_for_upgrade(
        session: &InMemorySession,
        profile_id: &ProfileId,
    ) -> anyhow::Result<Self> {
        session
            .derive_stable_subkey(profile_id.as_ref().as_bytes(), CONSUMABLE_HKDF_INFO)
            .map(Self::new)
            .map_err(|source| {
                anyhow::Error::new(source).context("derive legacy active-register key")
            })
    }

    pub fn seal(
        &self,
        reference: &MobileConsumableRef,
    ) -> Result<Vec<u8>, ActiveRegisterCipherError> {
        let plaintext = postcard::to_stdvec(&ConsumableRefPayload {
            snapshot_hash: reference.snapshot_hash.clone(),
            entry_id: reference.entry_id.as_ref().to_string(),
        })
        .map_err(|_| ActiveRegisterCipherError::Serialize)?;
        let (nonce, ciphertext) = encrypt_xchacha_raw(&self.key, &plaintext, AAD)
            .map_err(|_| ActiveRegisterCipherError::Encrypt)?;
        let mut envelope = Vec::with_capacity(HEADER_LEN + ciphertext.len());
        envelope.extend_from_slice(&MAGIC);
        envelope.push(FORMAT_VERSION);
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&ciphertext);
        Ok(envelope)
    }

    pub fn open(&self, envelope: &[u8]) -> Result<MobileConsumableRef, ActiveRegisterCipherError> {
        if envelope.len() < HEADER_LEN {
            return Err(ActiveRegisterCipherError::Truncated);
        }
        if envelope[..MAGIC.len()] != MAGIC {
            return Err(ActiveRegisterCipherError::BadMagic);
        }
        if envelope[MAGIC.len()] != FORMAT_VERSION {
            return Err(ActiveRegisterCipherError::UnsupportedVersion(
                envelope[MAGIC.len()],
            ));
        }
        let nonce_start = MAGIC.len() + 1;
        let plaintext = decrypt_xchacha_raw(
            &self.key,
            &envelope[nonce_start..HEADER_LEN],
            &envelope[HEADER_LEN..],
            AAD,
        )
        .map_err(|_| ActiveRegisterCipherError::Decrypt)?;
        let payload: ConsumableRefPayload =
            postcard::from_bytes(&plaintext).map_err(|_| ActiveRegisterCipherError::Deserialize)?;
        Ok(MobileConsumableRef::new(
            payload.snapshot_hash,
            EntryId::from(payload.entry_id),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_hides_both_reference_fields() {
        let cipher = ActiveClipboardRegisterCipher::new([0xA5; 32]);
        let reference = MobileConsumableRef::new(
            "blake3v1:secret-content-hash",
            EntryId::from("secret-entry-id"),
        );
        let envelope = cipher.seal(&reference).unwrap();

        assert_eq!(&envelope[..4], b"UCAR");
        assert!(!envelope
            .windows(reference.snapshot_hash.len())
            .any(|window| { window == reference.snapshot_hash.as_bytes() }));
        assert!(!envelope
            .windows(reference.entry_id.as_ref().len())
            .any(|window| { window == reference.entry_id.as_ref().as_bytes() }));
        assert_eq!(cipher.open(&envelope).unwrap(), reference);
    }

    #[test]
    fn wrong_key_cannot_open_envelope() {
        let reference = MobileConsumableRef::new("hash", EntryId::from("entry"));
        let envelope = ActiveClipboardRegisterCipher::new([1; 32])
            .seal(&reference)
            .unwrap();
        assert!(matches!(
            ActiveClipboardRegisterCipher::new([2; 32]).open(&envelope),
            Err(ActiveRegisterCipherError::Decrypt)
        ));
    }
}
