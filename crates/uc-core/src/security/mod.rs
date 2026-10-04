//! Core security domain types shared across pairing / membership / trust.
//!
//! Only algorithm-agnostic value objects live here. Concrete cryptographic
//! derivations (SHA-256, Base32 encoding of public keys, KDFs) belong in
//! the Infra layer (`uc-infra-crypto`).

pub mod identity_fingerprint;

pub use identity_fingerprint::{FingerprintError, IdentityFingerprint};
