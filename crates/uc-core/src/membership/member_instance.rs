use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable identifier for one device admission in a membership history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MemberInstanceId([u8; 32]);

impl MemberInstanceId {
    pub fn derive(device_id: &str, signature_key: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard-member-instance/v1\\0");
        hasher.update(device_id.as_bytes());
        hasher.update((signature_key.len() as u64).to_be_bytes());
        hasher.update(signature_key);
        Self(hasher.finalize().into())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for MemberInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}
