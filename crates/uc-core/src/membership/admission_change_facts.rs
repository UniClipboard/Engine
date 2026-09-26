//! 成员加入时写入本机名单与传输记录所需的已签名事实。

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;
use crate::security::IdentityFingerprint;

use super::member_instance::MemberInstanceId;

/// Facts required to save a member's local roster and transport record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionChangeFacts {
    pub member_instance: MemberInstanceId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub identity_fingerprint: IdentityFingerprint,
    pub transport_public_key: Vec<u8>,
    pub transport_address_blob: Vec<u8>,
    pub identity_signature: Vec<u8>,
}

impl AdmissionChangeFacts {
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard-workspace-admission/v1\\0");
        bytes.extend_from_slice(self.member_instance.as_bytes());
        bytes.extend_from_slice(self.device_id.as_str().as_bytes());
        bytes.extend_from_slice(self.device_name.as_bytes());
        bytes.extend_from_slice(self.identity_fingerprint.as_display().as_bytes());
        bytes.extend_from_slice(&self.transport_public_key);
        bytes.extend_from_slice(&self.transport_address_blob);
        bytes
    }
}
