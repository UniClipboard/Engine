//! Identity fingerprint factory port.
//!
//! Derives a stable `IdentityFingerprint` from a raw identity public key.
//! Used during pairing for out-of-band verification.
//!
//! 具体派生（SHA-256 + Base32 分组）位于 `uc-infra-crypto`。

use anyhow::Result;

use crate::security::IdentityFingerprint;

pub trait IdentityFingerprintFactoryPort: Send + Sync {
    fn from_public_key(&self, public_key: &[u8]) -> Result<IdentityFingerprint>;
}
