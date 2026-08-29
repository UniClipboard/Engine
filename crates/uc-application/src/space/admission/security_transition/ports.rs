use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    AdmissionContentKeyCatalogV1, AdmissionSecurityCommitmentV1, BaseMembershipHistoryPosition,
    MembershipCredentialId,
};

/// 为现有成员准备、等待随准入激活的安全更新。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedMemberSecurityDelivery {
    pub recipient: DeviceId,
    pub credential_id: MembershipCredentialId,
    pub payload: Vec<u8>,
}

impl std::fmt::Debug for PreparedMemberSecurityDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedMemberSecurityDelivery")
            .field("recipient", &self.recipient)
            .field("payload_len", &self.payload.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum AdmissionSecurityTransitionError {
    #[error("admission security state is invalid")]
    InvalidState,
    #[error("admission security commitment does not match")]
    CommitmentMismatch,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AdmissionSecurityTransitionInput {
    pub attempt_id: [u8; 32],
    pub base_history_position: BaseMembershipHistoryPosition,
    pub candidate_core_digest: [u8; 32],
    pub key_catalog_digest: [u8; 32],
    pub admission_bundle_digest: [u8; 32],
}

impl std::fmt::Debug for AdmissionSecurityTransitionInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionSecurityTransitionInput([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsorPreparedSecurityTransition {
    pub staged_state: Vec<u8>,
    pub commit: Vec<u8>,
    pub welcome: Vec<u8>,
    pub public_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for SponsorPreparedSecurityTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorPreparedSecurityTransition")
            .field("staged_state", &"[REDACTED]")
            .field("commit_len", &self.commit.len())
            .field("welcome_len", &self.welcome.len())
            .field("public_commitment", &self.public_commitment)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct JoinerStagedSecurityTransition {
    pub staged_state: Vec<u8>,
    pub public_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for JoinerStagedSecurityTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JoinerStagedSecurityTransition")
            .field("staged_state", &"[REDACTED]")
            .field("public_commitment", &self.public_commitment)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorAdmissionSecurityRecipient {
    pub device_id: DeviceId,
    pub credential_id: MembershipCredentialId,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsorAdmissionSecurityRequest {
    pub space_id: SpaceId,
    pub attempt_id: [u8; 32],
    pub base_history_position: BaseMembershipHistoryPosition,
    pub candidate_core_digest: [u8; 32],
    pub candidate_identity: Vec<u8>,
    pub candidate_key_package: Vec<u8>,
    pub existing_recipients: Vec<SponsorAdmissionSecurityRecipient>,
}

impl std::fmt::Debug for SponsorAdmissionSecurityRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorAdmissionSecurityRequest")
            .field("space_id", &self.space_id)
            .field("recipient_count", &self.existing_recipients.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsorPreparedAdmissionSecurity {
    pub staged_state: Vec<u8>,
    pub commit: Vec<u8>,
    pub welcome: Vec<u8>,
    pub public_commitment: AdmissionSecurityCommitmentV1,
    pub target_protection_group_id: String,
    pub target_key_catalog: AdmissionContentKeyCatalogV1,
    pub existing_member_deliveries: Vec<PreparedMemberSecurityDelivery>,
}

impl std::fmt::Debug for SponsorPreparedAdmissionSecurity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SponsorPreparedAdmissionSecurity")
            .field("staged_state", &"[REDACTED]")
            .field("commit_len", &self.commit.len())
            .field("welcome_len", &self.welcome.len())
            .field("public_commitment", &self.public_commitment)
            .field("target_key_catalog", &self.target_key_catalog)
            .field("delivery_count", &self.existing_member_deliveries.len())
            .finish()
    }
}

#[async_trait]
pub trait PrepareSponsorAdmissionSecurityPort: Send + Sync {
    async fn prepare_sponsor_admission_security(
        &self,
        request: SponsorAdmissionSecurityRequest,
    ) -> Result<SponsorPreparedAdmissionSecurity, AdmissionSecurityTransitionError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct ActivateSponsorAdmissionSecurityRequest {
    pub space_id: SpaceId,
    pub staged_state: Vec<u8>,
    pub commit: Vec<u8>,
    pub expected_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for ActivateSponsorAdmissionSecurityRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivateSponsorAdmissionSecurityRequest")
            .field("space_id", &self.space_id)
            .field("staged_state", &"[REDACTED]")
            .field("commit_len", &self.commit.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait ActivateSponsorAdmissionSecurityPort: Send + Sync {
    async fn activate_sponsor_admission_security(
        &self,
        request: ActivateSponsorAdmissionSecurityRequest,
    ) -> Result<(), AdmissionSecurityTransitionError>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct ActivateCompletionHelperAdmissionSecurityRequest {
    pub space_id: SpaceId,
    pub attempt_id: [u8; 32],
    pub helper_device_id: DeviceId,
    pub helper_credential_id: MembershipCredentialId,
    pub candidate_core_digest: [u8; 32],
    pub security_commit: Vec<u8>,
    pub security_welcome: Vec<u8>,
    pub target_key_catalog: Vec<u8>,
    pub existing_member_deliveries: Vec<PreparedMemberSecurityDelivery>,
    pub expected_commitment: AdmissionSecurityCommitmentV1,
}

impl std::fmt::Debug for ActivateCompletionHelperAdmissionSecurityRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivateCompletionHelperAdmissionSecurityRequest")
            .field("space_id", &self.space_id)
            .field("delivery_count", &self.existing_member_deliveries.len())
            .finish_non_exhaustive()
    }
}

#[async_trait]
pub trait ActivateCompletionHelperAdmissionSecurityPort: Send + Sync {
    async fn activate_completion_helper_admission_security(
        &self,
        request: ActivateCompletionHelperAdmissionSecurityRequest,
    ) -> Result<(), AdmissionSecurityTransitionError>;
}

pub trait AdmissionSecurityTransitionPort: Send + Sync {
    fn prepare_sponsor(
        &self,
        sponsor_state: &[u8],
        candidate_identity: &[u8],
        key_package: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<SponsorPreparedSecurityTransition, AdmissionSecurityTransitionError>;

    fn stage_joiner(
        &self,
        pending_state: &[u8],
        key_package: &[u8],
        expected_space_id: &[u8],
        welcome: &[u8],
        commit: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<JoinerStagedSecurityTransition, AdmissionSecurityTransitionError>;

    fn derive_public_commitment(
        &self,
        staged_state: &[u8],
        commit: &[u8],
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<AdmissionSecurityCommitmentV1, AdmissionSecurityTransitionError>;

    fn activate(
        &self,
        staged_state: Vec<u8>,
        commit: &[u8],
        expected: &AdmissionSecurityCommitmentV1,
        input: &AdmissionSecurityTransitionInput,
    ) -> Result<Vec<u8>, AdmissionSecurityTransitionError>;

    fn discard(&self, staged_state: Vec<u8>);
}
