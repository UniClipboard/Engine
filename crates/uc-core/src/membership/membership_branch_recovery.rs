use std::fmt;

use serde::{Deserialize, Serialize};

use super::{
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier, MemberInstanceId,
    MembershipBranchId, MembershipConflictId, MembershipConflictPolicy, VersionedMembershipHistory,
};

pub const MEMBERSHIP_BRANCH_RECOVERY_PACKAGE_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipBranchRecoveryError {
    InvalidPackage,
    Expired,
    WrongRecipient,
    WrongConflict,
    WrongBranch,
    InvalidHistory,
    Unauthorized,
}

impl fmt::Display for MembershipBranchRecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPackage => "membership branch recovery package is invalid",
            Self::Expired => "membership branch recovery package expired",
            Self::WrongRecipient => "membership branch recovery recipient does not match",
            Self::WrongConflict => "membership branch recovery conflict does not match",
            Self::WrongBranch => "membership branch recovery target does not match",
            Self::InvalidHistory => "membership branch recovery history is invalid",
            Self::Unauthorized => "membership branch recovery authorization is invalid",
        })
    }
}

impl std::error::Error for MembershipBranchRecoveryError {}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipBranchRecoveryPackageV1 {
    format_version: u16,
    conflict_id: MembershipConflictId,
    target_branch_id: MembershipBranchId,
    recipient_member: MemberInstanceId,
    authorizing_member: MemberInstanceId,
    expires_at_ms: i64,
    nonce: [u8; 32],
    target_membership_history: Vec<u8>,
    sealed_mls_recovery_material: Vec<u8>,
    encrypted_content_key_catalog: Vec<u8>,
    authorization_signature: Vec<u8>,
}

impl MembershipBranchRecoveryPackageV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new_unsigned(
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        recipient_member: MemberInstanceId,
        authorizing_member: MemberInstanceId,
        expires_at_ms: i64,
        nonce: [u8; 32],
        target_membership_history: Vec<u8>,
        sealed_mls_recovery_material: Vec<u8>,
        encrypted_content_key_catalog: Vec<u8>,
    ) -> Result<Self, MembershipBranchRecoveryError> {
        let package = Self {
            format_version: MEMBERSHIP_BRANCH_RECOVERY_PACKAGE_FORMAT_V1,
            conflict_id,
            target_branch_id,
            recipient_member,
            authorizing_member,
            expires_at_ms,
            nonce,
            target_membership_history,
            sealed_mls_recovery_material,
            encrypted_content_key_catalog,
            authorization_signature: Vec::new(),
        };
        package.validate_shape()?;
        Ok(package)
    }

    pub fn authorization_signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard/membership-branch-recovery/v1\0");
        bytes.extend_from_slice(&self.format_version.to_be_bytes());
        bytes.extend_from_slice(self.conflict_id.as_bytes());
        bytes.extend_from_slice(self.target_branch_id.as_bytes());
        bytes.extend_from_slice(self.recipient_member.as_bytes());
        bytes.extend_from_slice(self.authorizing_member.as_bytes());
        bytes.extend_from_slice(&self.expires_at_ms.to_be_bytes());
        bytes.extend_from_slice(&self.nonce);
        append_field(&mut bytes, &self.target_membership_history);
        append_field(&mut bytes, &self.sealed_mls_recovery_material);
        append_field(&mut bytes, &self.encrypted_content_key_catalog);
        bytes
    }

    pub fn with_authorization_signature(mut self, signature: Vec<u8>) -> Self {
        self.authorization_signature = signature;
        self
    }

    pub const fn nonce(&self) -> &[u8; 32] {
        &self.nonce
    }

    pub const fn conflict_id(&self) -> MembershipConflictId {
        self.conflict_id
    }

    pub const fn target_branch_id(&self) -> MembershipBranchId {
        self.target_branch_id
    }

    pub const fn recipient_member(&self) -> MemberInstanceId {
        self.recipient_member
    }

    pub fn validate(
        &self,
        expected_conflict_id: MembershipConflictId,
        expected_target_branch_id: MembershipBranchId,
        expected_recipient: MemberInstanceId,
        now_ms: i64,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<VersionedMembershipHistory, MembershipBranchRecoveryError> {
        self.validate_shape()?;
        if self.conflict_id != expected_conflict_id {
            return Err(MembershipBranchRecoveryError::WrongConflict);
        }
        if self.target_branch_id != expected_target_branch_id {
            return Err(MembershipBranchRecoveryError::WrongBranch);
        }
        if self.recipient_member != expected_recipient {
            return Err(MembershipBranchRecoveryError::WrongRecipient);
        }
        if now_ms >= self.expires_at_ms {
            return Err(MembershipBranchRecoveryError::Expired);
        }
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &self.target_membership_history,
            verifier,
        )
        .map_err(|_| MembershipBranchRecoveryError::InvalidHistory)?;
        if MembershipConflictPolicy::branch_id(&history)
            .map_err(|_| MembershipBranchRecoveryError::InvalidHistory)?
            != self.target_branch_id
        {
            return Err(MembershipBranchRecoveryError::WrongBranch);
        }
        let active = history.active_members();
        if !active.contains(&self.recipient_member) || !active.contains(&self.authorizing_member) {
            return Err(MembershipBranchRecoveryError::Unauthorized);
        }
        let credential = history
            .credential_for(self.authorizing_member)
            .ok_or(MembershipBranchRecoveryError::Unauthorized)?;
        match verifier.verify(
            credential.signature_algorithm_version,
            &credential.public_key,
            &self.authorization_signing_payload(),
            &self.authorization_signature,
        ) {
            Ok(true) => Ok(history),
            Ok(false)
            | Err(HistoricalMembershipSignatureError::VerificationFailed)
            | Err(HistoricalMembershipSignatureError::UnsupportedAlgorithm) => {
                Err(MembershipBranchRecoveryError::Unauthorized)
            }
        }
    }

    fn validate_shape(&self) -> Result<(), MembershipBranchRecoveryError> {
        if self.format_version != MEMBERSHIP_BRANCH_RECOVERY_PACKAGE_FORMAT_V1
            || self.expires_at_ms <= 0
            || self.nonce == [0; 32]
            || self.target_membership_history.is_empty()
            || self.sealed_mls_recovery_material.is_empty()
            || self.encrypted_content_key_catalog.is_empty()
        {
            return Err(MembershipBranchRecoveryError::InvalidPackage);
        }
        Ok(())
    }
}

impl fmt::Debug for MembershipBranchRecoveryPackageV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MembershipBranchRecoveryPackageV1")
            .field("bindings", &"[REDACTED]")
            .field("history_len", &self.target_membership_history.len())
            .field(
                "has_mls_material",
                &!self.sealed_mls_recovery_material.is_empty(),
            )
            .field(
                "has_key_catalog",
                &!self.encrypted_content_key_catalog.is_empty(),
            )
            .finish()
    }
}

fn append_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}
