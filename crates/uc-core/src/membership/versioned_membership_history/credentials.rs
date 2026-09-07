//! 成员凭据及历史签名验证能力。

use super::{
    MemberInstanceId, MembershipHistoryV2Error, ED25519_SIGNATURE_ALGORITHM_V1,
    MEMBERSHIP_CREDENTIAL_FORMAT_V1,
};
use crate::ids::DeviceId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipCredentialId([u8; 32]);

impl MembershipCredentialId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipCredential {
    pub credential_format_version: u16,
    pub signature_algorithm_version: u16,
    pub public_key: Vec<u8>,
    pub credential_id: MembershipCredentialId,
}

impl MembershipCredential {
    pub fn new(signature_algorithm_version: u16, public_key: Vec<u8>) -> Self {
        let credential_format_version = MEMBERSHIP_CREDENTIAL_FORMAT_V1;
        let credential_id = credential_id(
            credential_format_version,
            signature_algorithm_version,
            &public_key,
        );
        Self {
            credential_format_version,
            signature_algorithm_version,
            public_key,
            credential_id,
        }
    }

    pub fn member_instance_id(&self, device_id: &DeviceId) -> MemberInstanceId {
        MemberInstanceId::derive(device_id.as_str(), &self.public_key)
    }

    pub fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.credential_format_version != MEMBERSHIP_CREDENTIAL_FORMAT_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.public_key.is_empty()
            || self.credential_id
                != credential_id(
                    self.credential_format_version,
                    self.signature_algorithm_version,
                    &self.public_key,
                )
        {
            return Err(MembershipHistoryV2Error::InvalidCredential);
        }
        Ok(())
    }
}

pub(super) fn credential_id(
    credential_format_version: u16,
    signature_algorithm_version: u16,
    public_key: &[u8],
) -> MembershipCredentialId {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-credential/v1\0");
    hasher.update(credential_format_version.to_be_bytes());
    hasher.update(signature_algorithm_version.to_be_bytes());
    hasher.update((public_key.len() as u64).to_be_bytes());
    hasher.update(public_key);
    MembershipCredentialId(hasher.finalize().into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoricalMembershipSignatureError {
    UnsupportedAlgorithm,
    VerificationFailed,
}

impl fmt::Display for HistoricalMembershipSignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedAlgorithm => "membership signature algorithm is not supported",
            Self::VerificationFailed => "membership signature verification failed",
        })
    }
}

impl std::error::Error for HistoricalMembershipSignatureError {}

pub trait HistoricalMembershipSignatureVerifier: Send + Sync {
    fn verify(
        &self,
        signature_algorithm_version: u16,
        public_key: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError>;
}
