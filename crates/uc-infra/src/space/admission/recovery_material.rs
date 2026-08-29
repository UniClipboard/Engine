use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

const SEALED_RECOVERY_FORMAT_V1: u16 = 1;
const RECOVERY_KEY_INFO: &[u8] = b"uniclipboard/space-admission-recovery-key/v1\0";
const RECOVERY_AAD_DOMAIN: &[u8] = b"uniclipboard/space-admission-recovery/v1\0";

#[derive(Debug, thiserror::Error)]
pub(super) enum RecoveryMaterialError {
    #[error("admission recovery public key is invalid")]
    InvalidPublicKey,
    #[error("admission recovery key derivation failed")]
    KeyDerivation,
    #[error("admission recovery material encryption failed")]
    Encryption,
    #[error("admission recovery material encoding failed")]
    Encoding(#[source] postcard::Error),
}

#[derive(Serialize, Deserialize)]
struct SealedRecoveryMaterialV1 {
    format_version: u16,
    ephemeral_public_key: [u8; 32],
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}

pub(super) fn seal_recovery_material(
    admission_id: &[u8; 32],
    recipient_public_key: &[u8; 32],
    plaintext: &[u8],
) -> Result<Vec<u8>, RecoveryMaterialError> {
    if recipient_public_key == &[0; 32] {
        return Err(RecoveryMaterialError::InvalidPublicKey);
    }
    let recipient = PublicKey::from(*recipient_public_key);
    let mut ephemeral_bytes = Zeroizing::new([0u8; 32]);
    rand::rng().fill_bytes(ephemeral_bytes.as_mut());
    let ephemeral_secret = StaticSecret::from(*ephemeral_bytes);
    let ephemeral_public = PublicKey::from(&ephemeral_secret).to_bytes();
    let shared = Zeroizing::new(ephemeral_secret.diffie_hellman(&recipient).to_bytes());
    if shared.as_ref() == &[0; 32] {
        return Err(RecoveryMaterialError::InvalidPublicKey);
    }
    let key = derive_key(
        admission_id,
        &ephemeral_public,
        recipient_public_key,
        &shared,
    )?;
    let aad = aad(admission_id, &ephemeral_public, recipient_public_key);
    let mut nonce = [0u8; 24];
    rand::rng().fill_bytes(&mut nonce);
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|_| RecoveryMaterialError::KeyDerivation)?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| RecoveryMaterialError::Encryption)?;
    postcard::to_stdvec(&SealedRecoveryMaterialV1 {
        format_version: SEALED_RECOVERY_FORMAT_V1,
        ephemeral_public_key: ephemeral_public,
        nonce,
        ciphertext,
    })
    .map_err(RecoveryMaterialError::Encoding)
}

fn derive_key(
    admission_id: &[u8; 32],
    ephemeral_public: &[u8; 32],
    recipient_public: &[u8; 32],
    shared: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>, RecoveryMaterialError> {
    let mut salt = Vec::with_capacity(96);
    salt.extend_from_slice(admission_id);
    salt.extend_from_slice(ephemeral_public);
    salt.extend_from_slice(recipient_public);
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), shared)
        .expand(RECOVERY_KEY_INFO, key.as_mut())
        .map_err(|_| RecoveryMaterialError::KeyDerivation)?;
    Ok(key)
}

fn aad(
    admission_id: &[u8; 32],
    ephemeral_public: &[u8; 32],
    recipient_public: &[u8; 32],
) -> Vec<u8> {
    let mut aad = Vec::with_capacity(RECOVERY_AAD_DOMAIN.len() + 96);
    aad.extend_from_slice(RECOVERY_AAD_DOMAIN);
    aad.extend_from_slice(admission_id);
    aad.extend_from_slice(ephemeral_public);
    aad.extend_from_slice(recipient_public);
    aad
}

#[cfg(test)]
fn open_recovery_material(
    admission_id: &[u8; 32],
    recipient_secret: &[u8; 32],
    encoded: &[u8],
) -> Result<Vec<u8>, RecoveryMaterialError> {
    let sealed: SealedRecoveryMaterialV1 =
        postcard::from_bytes(encoded).map_err(RecoveryMaterialError::Encoding)?;
    if sealed.format_version != SEALED_RECOVERY_FORMAT_V1 {
        return Err(RecoveryMaterialError::Encoding(
            postcard::Error::DeserializeBadEnum,
        ));
    }
    let secret = StaticSecret::from(*recipient_secret);
    let recipient_public = PublicKey::from(&secret).to_bytes();
    let ephemeral = PublicKey::from(sealed.ephemeral_public_key);
    let shared = Zeroizing::new(secret.diffie_hellman(&ephemeral).to_bytes());
    let key = derive_key(
        admission_id,
        &sealed.ephemeral_public_key,
        &recipient_public,
        &shared,
    )?;
    let aad = aad(
        admission_id,
        &sealed.ephemeral_public_key,
        &recipient_public,
    );
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|_| RecoveryMaterialError::KeyDerivation)?;
    cipher
        .decrypt(
            XNonce::from_slice(&sealed.nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| RecoveryMaterialError::Encryption)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_material_is_bound_to_admission_and_recipient() {
        let admission_id = [0x31; 32];
        let recipient_secret = [0x32; 32];
        let recipient_public = PublicKey::from(&StaticSecret::from(recipient_secret)).to_bytes();
        let plaintext = b"saved staged security material";

        let sealed = seal_recovery_material(&admission_id, &recipient_public, plaintext).unwrap();

        assert_eq!(
            open_recovery_material(&admission_id, &recipient_secret, &sealed).unwrap(),
            plaintext
        );
        assert!(open_recovery_material(&[0x33; 32], &recipient_secret, &sealed).is_err());
        assert!(open_recovery_material(&admission_id, &[0x34; 32], &sealed).is_err());
        assert!(!sealed
            .windows(plaintext.len())
            .any(|window| window == plaintext));
    }
}
