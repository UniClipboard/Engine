//! `IdentityFingerprintFactoryPort` 背后的 SHA-256 + Base32 派生实现。
//!
//! The value object (`IdentityFingerprint`) and its format errors live in
//! `uc_core::security` — this module only implements the algorithm.
//!
//! # Algorithms
//!
//! - Identity fingerprint: `Base32( SHA-256("uc-identity-fp-v1" || pub_key)[0..10] )`
//!   → 16 chars grouped as `ABCD-EFGH-IJKL-MNOP`

use sha2::{Digest, Sha256};
use thiserror::Error;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::security::IdentityFingerprint;

/// Derivation-time failure for the Ed25519 → fingerprint pipeline.
///
/// Format-level errors (invalid fingerprint string, verify mismatches) live
/// on `uc_core::security::FingerprintError` and are emitted when parsing an
/// already-materialized fingerprint — never here.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FingerprintDerivationError {
    #[error("Invalid public key length: expected {expected}, got {actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
}

/// Derive the canonical identity fingerprint from an Ed25519 public key.
///
/// Uses a fixed domain separator so the same key cannot collide with
/// fingerprints minted for other purposes.
fn derive_identity_fingerprint(
    public_key: &[u8],
) -> Result<IdentityFingerprint, FingerprintDerivationError> {
    if public_key.len() != 32 {
        return Err(FingerprintDerivationError::InvalidKeyLength {
            expected: 32,
            actual: public_key.len(),
        });
    }

    let mut hasher = Sha256::new();
    hasher.update(b"uc-identity-fp-v1");
    hasher.update(public_key);
    let hash = hasher.finalize();

    let truncated = &hash[0..10];
    let encoded = base32::encode(base32::Alphabet::Rfc4648 { padding: false }, truncated);

    Ok(IdentityFingerprint::from_raw_string(encoded)
        .expect("16-char alphanumeric Base32 output is always a valid fingerprint"))
}

/// SHA-256 + Base32 implementation of `IdentityFingerprintFactoryPort`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Sha256IdentityFingerprintFactory;

impl IdentityFingerprintFactoryPort for Sha256IdentityFingerprintFactory {
    fn from_public_key(&self, public_key: &[u8]) -> anyhow::Result<IdentityFingerprint> {
        derive_identity_fingerprint(public_key)
            .map_err(|err| anyhow::anyhow!("identity fingerprint derivation failed: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_fingerprint_is_stable_for_same_pubkey() {
        let pk = [7u8; 32];
        let fp_a = derive_identity_fingerprint(&pk).unwrap();
        let fp_b = derive_identity_fingerprint(&pk).unwrap();
        assert_eq!(fp_a, fp_b);
    }

    #[test]
    fn derive_fingerprint_rejects_wrong_length() {
        let err = derive_identity_fingerprint(&[0u8; 16]).unwrap_err();
        assert!(matches!(
            err,
            FingerprintDerivationError::InvalidKeyLength { .. }
        ));
    }

    #[test]
    fn derive_fingerprint_differs_across_keys() {
        let a = derive_identity_fingerprint(&[1u8; 32]).unwrap();
        let b = derive_identity_fingerprint(&[2u8; 32]).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn factory_port_returns_same_value_as_free_fn() {
        let pk = [3u8; 32];
        let via_port = Sha256IdentityFingerprintFactory
            .from_public_key(&pk)
            .unwrap();
        let via_fn = derive_identity_fingerprint(&pk).unwrap();
        assert_eq!(via_port, via_fn);
    }
}
