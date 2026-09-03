//! SearchKey — 32-byte HMAC key derived from MasterKey via SearchKeyDerivationPort.
//!
//! Opaque newtype: no Serialize/Deserialize, redacted Debug.
//! Pattern mirrors `crypto::model::MasterKey`.

use std::fmt;

use zeroize::Zeroize;

/// 搜索 posting 所属保护组的 32-byte 不透明引用。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct SearchProtectionRef([u8; 32]);

impl SearchProtectionRef {
    pub const LEN: usize = 32;

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::search::error::SearchError> {
        let value = bytes.try_into().map_err(|_| {
            crate::search::error::SearchError::Internal(
                "invalid search protection reference length".to_owned(),
            )
        })?;
        Ok(Self(value))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SearchProtectionRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SearchProtectionRef([REDACTED])")
    }
}

/// Opaque 32-byte search key derived from the master key.
///
/// - Do NOT implement Serialize/Deserialize — keys must never appear in JSON.
/// - The HMAC computation (`term_tag = HMAC(search_key, token)`) is a Phase 90
///   infra concern; this type is a pure data contract.
/// - Only `as_bytes()` exposes the raw bytes, for use by infra HMAC adapters.
#[derive(Clone, PartialEq, Eq)]
pub struct SearchKey(pub [u8; 32]);

impl SearchKey {
    /// Length of a SearchKey in bytes.
    pub const LEN: usize = 32;

    /// Access the raw key bytes — for use by uc-infra HMAC adapters only.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Construct a SearchKey from a byte slice, validating length.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::search::error::SearchError> {
        if bytes.len() != Self::LEN {
            return Err(crate::search::error::SearchError::Internal(format!(
                "invalid SearchKey length: expected {}, got {}",
                Self::LEN,
                bytes.len()
            )));
        }
        let mut buf = [0u8; Self::LEN];
        buf.copy_from_slice(bytes);
        Ok(SearchKey(buf))
    }
}

impl fmt::Debug for SearchKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SearchKey([REDACTED])")
    }
}

impl Drop for SearchKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// 一次 posting 构建不可拆分的 key 与保护组引用。
#[derive(Debug)]
pub struct SearchKeyContext {
    key: SearchKey,
    protection_ref: Option<SearchProtectionRef>,
}

impl SearchKeyContext {
    pub fn legacy(key: SearchKey) -> Self {
        Self {
            key,
            protection_ref: None,
        }
    }

    pub fn protected(key: SearchKey, protection_ref: SearchProtectionRef) -> Self {
        Self {
            key,
            protection_ref: Some(protection_ref),
        }
    }

    pub fn key(&self) -> &SearchKey {
        &self.key
    }

    pub fn protection_ref(&self) -> Option<&SearchProtectionRef> {
        self.protection_ref.as_ref()
    }
}

/// Opaque 32-byte render-payload key derived from the master key.
///
/// Distinct from [`SearchKey`] to keep key usage separated: `SearchKey` is an
/// HMAC-PRF key for inverted-index term tags, while `RenderKey` is an AEAD key
/// for encrypting the per-entry render payload. Deriving a dedicated subkey (a
/// different HKDF `info` label) prevents a single key from serving two
/// cryptographic purposes.
///
/// - Do NOT implement Serialize/Deserialize — keys must never appear in JSON.
/// - Only `as_bytes()` exposes the raw bytes, for use by infra AEAD adapters.
#[derive(Clone, PartialEq, Eq)]
pub struct RenderKey(pub [u8; 32]);

impl RenderKey {
    /// Length of a RenderKey in bytes.
    pub const LEN: usize = 32;

    /// Access the raw key bytes — for use by uc-infra AEAD adapters only.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Construct a RenderKey from a byte slice, validating length.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::search::error::SearchError> {
        if bytes.len() != Self::LEN {
            return Err(crate::search::error::SearchError::Internal(format!(
                "invalid RenderKey length: expected {}, got {}",
                Self::LEN,
                bytes.len()
            )));
        }
        let mut buf = [0u8; Self::LEN];
        buf.copy_from_slice(bytes);
        Ok(RenderKey(buf))
    }
}

impl fmt::Debug for RenderKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RenderKey([REDACTED])")
    }
}

impl Drop for RenderKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
