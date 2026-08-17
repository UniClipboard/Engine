use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::DeviceId;
use crate::security::IdentityFingerprint;

use super::{
    AdmissionChangeFacts, BaseMembershipHistoryPositionV1, MemberInstanceId,
    MembershipCredentialId, MembershipEventId, SponsorAdmissionSecurityDelivery,
};

pub const ADMISSION_ATTEMPT_FORMAT_V1: u16 = 1;
pub const ADMISSION_PROFILE_METADATA_FORMAT_V1: u16 = 1;
pub const TERMINAL_ADMISSION_ATTEMPT_FORMAT_V1: u16 = 1;
pub const ADMISSION_IDENTITY_BINDING_FORMAT_V1: u16 = 1;
pub const ADMISSION_COMPLETION_RECOVERY_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionCompletionRecoveryValidationError {
    Invalid,
    UpgradeRequired,
}

impl fmt::Display for AdmissionCompletionRecoveryValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "admission completion recovery message is invalid",
            Self::UpgradeRequired => "admission completion recovery requires a newer engine",
        })
    }
}

impl std::error::Error for AdmissionCompletionRecoveryValidationError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCompletionRecoveryHelloV1 {
    pub format_version: u16,
    pub attempt_id: AdmissionAttemptId,
    pub lineage_id: String,
    pub event_id: MembershipEventId,
    pub sponsor_member_instance: MemberInstanceId,
    pub joiner_member_instance: MemberInstanceId,
    pub helper_member_instance: MemberInstanceId,
    pub resume_public_key: Vec<u8>,
}

impl AdmissionCompletionRecoveryHelloV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        attempt_id: AdmissionAttemptId,
        lineage_id: String,
        event_id: MembershipEventId,
        sponsor_member_instance: MemberInstanceId,
        joiner_member_instance: MemberInstanceId,
        helper_member_instance: MemberInstanceId,
        resume_public_key: Vec<u8>,
    ) -> Result<Self, AdmissionCompletionRecoveryValidationError> {
        let hello = Self {
            format_version: ADMISSION_COMPLETION_RECOVERY_FORMAT_V1,
            attempt_id,
            lineage_id,
            event_id,
            sponsor_member_instance,
            joiner_member_instance,
            helper_member_instance,
            resume_public_key,
        };
        hello.validate()?;
        Ok(hello)
    }

    pub fn validate(&self) -> Result<(), AdmissionCompletionRecoveryValidationError> {
        if self.format_version != ADMISSION_COMPLETION_RECOVERY_FORMAT_V1 {
            return Err(AdmissionCompletionRecoveryValidationError::UpgradeRequired);
        }
        if self.lineage_id.is_empty()
            || self.resume_public_key.len() != 32
            || self.sponsor_member_instance == self.joiner_member_instance
            || self.sponsor_member_instance == self.helper_member_instance
            || self.joiner_member_instance == self.helper_member_instance
        {
            return Err(AdmissionCompletionRecoveryValidationError::Invalid);
        }
        Ok(())
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/admission-completion-recovery-hello/v1\0");
        hasher.update(self.format_version.to_be_bytes());
        hasher.update(self.attempt_id.as_bytes());
        append_recovery_field(&mut hasher, self.lineage_id.as_bytes());
        hasher.update(self.event_id.as_bytes());
        hasher.update(self.sponsor_member_instance.as_bytes());
        hasher.update(self.joiner_member_instance.as_bytes());
        hasher.update(self.helper_member_instance.as_bytes());
        append_recovery_field(&mut hasher, &self.resume_public_key);
        hasher.finalize().into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCompletionRecoveryTransportBindingV1 {
    pub joiner_transport_identity_digest: [u8; 32],
    pub helper_transport_identity_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCompletionRecoveryChallengeV1 {
    pub format_version: u16,
    pub hello_digest: [u8; 32],
    pub transport_binding: AdmissionCompletionRecoveryTransportBindingV1,
    pub challenge_counter: u64,
    pub nonce: [u8; 32],
    pub joiner_last_message_id: [u8; 32],
    pub helper_last_message_id: [u8; 32],
    pub helper_credential_id: MembershipCredentialId,
    pub helper_history_position: BaseMembershipHistoryPositionV1,
    pub signature: Vec<u8>,
}

impl AdmissionCompletionRecoveryChallengeV1 {
    pub fn new(
        hello: &AdmissionCompletionRecoveryHelloV1,
        transport_binding: AdmissionCompletionRecoveryTransportBindingV1,
        challenge_counter: u64,
        nonce: [u8; 32],
        joiner_last_message_id: [u8; 32],
        helper_last_message_id: [u8; 32],
        helper_credential_id: MembershipCredentialId,
        helper_history_position: BaseMembershipHistoryPositionV1,
    ) -> Result<Self, AdmissionCompletionRecoveryValidationError> {
        hello.validate()?;
        if challenge_counter == 0
            || nonce == [0; 32]
            || joiner_last_message_id == [0; 32]
            || helper_last_message_id == [0; 32]
        {
            return Err(AdmissionCompletionRecoveryValidationError::Invalid);
        }
        Ok(Self {
            format_version: ADMISSION_COMPLETION_RECOVERY_FORMAT_V1,
            hello_digest: hello.digest(),
            transport_binding,
            challenge_counter,
            nonce,
            joiner_last_message_id,
            helper_last_message_id,
            helper_credential_id,
            helper_history_position,
            signature: Vec::new(),
        })
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = b"uniclipboard/admission-completion-recovery-challenge/v1\0".to_vec();
        bytes.extend_from_slice(&self.format_version.to_be_bytes());
        bytes.extend_from_slice(&self.hello_digest);
        bytes.extend_from_slice(&self.transport_binding.joiner_transport_identity_digest);
        bytes.extend_from_slice(&self.transport_binding.helper_transport_identity_digest);
        bytes.extend_from_slice(&self.challenge_counter.to_be_bytes());
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.joiner_last_message_id);
        bytes.extend_from_slice(&self.helper_last_message_id);
        bytes.extend_from_slice(self.helper_credential_id.as_bytes());
        if let Some(event_id) = self.helper_history_position.event_id {
            bytes.push(1);
            bytes.extend_from_slice(event_id.as_bytes());
        } else {
            bytes.push(0);
        }
        bytes.extend_from_slice(&self.helper_history_position.depth.to_be_bytes());
        bytes.extend_from_slice(&self.helper_history_position.history_digest);
        bytes
    }

    pub fn validate(&self) -> Result<(), AdmissionCompletionRecoveryValidationError> {
        if self.format_version != ADMISSION_COMPLETION_RECOVERY_FORMAT_V1 {
            return Err(AdmissionCompletionRecoveryValidationError::UpgradeRequired);
        }
        if self.challenge_counter == 0
            || self.nonce == [0; 32]
            || self.joiner_last_message_id == [0; 32]
            || self.helper_last_message_id == [0; 32]
        {
            return Err(AdmissionCompletionRecoveryValidationError::Invalid);
        }
        Ok(())
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.signing_payload());
        append_recovery_field(&mut hasher, &self.signature);
        hasher.finalize().into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCompletionRecoveryBundleV1 {
    pub format_version: u16,
    pub candidate_event: Vec<u8>,
    pub candidate_key_package: Vec<u8>,
    pub security_commitment: Vec<u8>,
    pub security_commit: Vec<u8>,
    pub security_welcome: Vec<u8>,
    pub target_protection_group_id: String,
    pub target_key_catalog: Vec<u8>,
    pub existing_member_deliveries: Vec<SponsorAdmissionSecurityDelivery>,
    pub activation_receipt: Vec<u8>,
    pub resume_public_key: Vec<u8>,
}

impl AdmissionCompletionRecoveryBundleV1 {
    pub fn validate(&self) -> Result<(), AdmissionCompletionRecoveryValidationError> {
        if self.format_version != ADMISSION_COMPLETION_RECOVERY_FORMAT_V1 {
            return Err(AdmissionCompletionRecoveryValidationError::UpgradeRequired);
        }
        if self.candidate_event.is_empty()
            || self.candidate_key_package.is_empty()
            || self.security_commitment.is_empty()
            || self.security_commit.is_empty()
            || self.security_welcome.is_empty()
            || self.target_protection_group_id.is_empty()
            || self.target_key_catalog.is_empty()
            || self.existing_member_deliveries.len() > 256
            || self.activation_receipt.is_empty()
            || self.resume_public_key.len() != 32
        {
            return Err(AdmissionCompletionRecoveryValidationError::Invalid);
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<[u8; 32], AdmissionCompletionRecoveryValidationError> {
        self.validate()?;
        let encoded = postcard::to_stdvec(self)
            .map_err(|_| AdmissionCompletionRecoveryValidationError::Invalid)?;
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/admission-completion-recovery-bundle/v1\0");
        append_recovery_field(&mut hasher, &encoded);
        Ok(hasher.finalize().into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCompletionRecoveryResponseV1 {
    pub format_version: u16,
    pub hello_digest: [u8; 32],
    pub challenge_digest: [u8; 32],
    pub bundle: AdmissionCompletionRecoveryBundleV1,
    pub resume_signature: Vec<u8>,
}

impl AdmissionCompletionRecoveryResponseV1 {
    pub fn new(
        hello_digest: [u8; 32],
        challenge_digest: [u8; 32],
        bundle: AdmissionCompletionRecoveryBundleV1,
    ) -> Result<Self, AdmissionCompletionRecoveryValidationError> {
        bundle.validate()?;
        Ok(Self {
            format_version: ADMISSION_COMPLETION_RECOVERY_FORMAT_V1,
            hello_digest,
            challenge_digest,
            bundle,
            resume_signature: Vec::new(),
        })
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = b"uniclipboard/admission-completion-recovery-response/v1\0".to_vec();
        bytes.extend_from_slice(&self.format_version.to_be_bytes());
        bytes.extend_from_slice(&self.hello_digest);
        bytes.extend_from_slice(&self.challenge_digest);
        match self.bundle.digest() {
            Ok(digest) => bytes.extend_from_slice(&digest),
            Err(_) => bytes.extend_from_slice(&[0; 32]),
        }
        bytes
    }

    pub fn validate(&self) -> Result<(), AdmissionCompletionRecoveryValidationError> {
        if self.format_version != ADMISSION_COMPLETION_RECOVERY_FORMAT_V1 {
            return Err(AdmissionCompletionRecoveryValidationError::UpgradeRequired);
        }
        self.bundle.validate()
    }

    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(self.signing_payload());
        append_recovery_field(&mut hasher, &self.resume_signature);
        hasher.finalize().into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCompletionRecoveryPeerV1 {
    pub format_version: u16,
    pub hello: AdmissionCompletionRecoveryHelloV1,
    pub challenge: AdmissionCompletionRecoveryChallengeV1,
    pub response_digest: Option<[u8; 32]>,
}

fn append_recovery_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionIdentityBindingV1 {
    pub format_version: u16,
    pub lineage_id: String,
    pub candidate_event_id: MembershipEventId,
    pub sponsor_member_instance: MemberInstanceId,
    pub sponsor_device_id: DeviceId,
    pub sponsor_identity_fingerprint: IdentityFingerprint,
    pub joiner_member_instance: MemberInstanceId,
    pub joiner_device_id: DeviceId,
    pub joiner_identity_fingerprint: IdentityFingerprint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionIdentityBindingError {
    Invalid,
    UpgradeRequired,
}

impl fmt::Display for AdmissionIdentityBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "admission identity binding is invalid",
            Self::UpgradeRequired => "admission identity binding requires a newer engine",
        })
    }
}

impl std::error::Error for AdmissionIdentityBindingError {}

impl fmt::Debug for AdmissionIdentityBindingV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdmissionIdentityBindingV1")
            .field("format_version", &self.format_version)
            .field("identities", &"[REDACTED]")
            .finish()
    }
}

impl AdmissionIdentityBindingV1 {
    pub fn new(
        lineage_id: String,
        candidate_event_id: MembershipEventId,
        sponsor: &AdmissionChangeFacts,
        joiner: &AdmissionChangeFacts,
    ) -> Result<Self, AdmissionIdentityBindingError> {
        let binding = Self {
            format_version: ADMISSION_IDENTITY_BINDING_FORMAT_V1,
            lineage_id,
            candidate_event_id,
            sponsor_member_instance: sponsor.member_instance,
            sponsor_device_id: sponsor.device_id.clone(),
            sponsor_identity_fingerprint: sponsor.identity_fingerprint.clone(),
            joiner_member_instance: joiner.member_instance,
            joiner_device_id: joiner.device_id.clone(),
            joiner_identity_fingerprint: joiner.identity_fingerprint.clone(),
        };
        binding.validate_shape()?;
        Ok(binding)
    }

    pub fn encode(&self) -> Result<Vec<u8>, AdmissionIdentityBindingError> {
        self.validate_shape()?;
        postcard::to_stdvec(self).map_err(|_| AdmissionIdentityBindingError::Invalid)
    }

    pub fn decode(encoded: &[u8]) -> Result<Self, AdmissionIdentityBindingError> {
        let binding: Self =
            postcard::from_bytes(encoded).map_err(|_| AdmissionIdentityBindingError::Invalid)?;
        if binding.format_version != ADMISSION_IDENTITY_BINDING_FORMAT_V1 {
            return Err(AdmissionIdentityBindingError::UpgradeRequired);
        }
        binding.validate_shape()?;
        Ok(binding)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn decode_and_validate(
        encoded: &[u8],
        lineage_id: &str,
        candidate_event_id: MembershipEventId,
        sponsor_member_instance: MemberInstanceId,
        joiner: &AdmissionChangeFacts,
        relationships: &[AdmissionChangeFacts],
    ) -> Result<Self, AdmissionIdentityBindingError> {
        let binding = Self::decode(encoded)?;
        let sponsor_matches = relationships
            .iter()
            .filter(|facts| facts.member_instance == sponsor_member_instance)
            .collect::<Vec<_>>();
        let joiner_matches = relationships
            .iter()
            .filter(|facts| facts.member_instance == joiner.member_instance)
            .collect::<Vec<_>>();
        if binding.lineage_id != lineage_id
            || binding.candidate_event_id != candidate_event_id
            || binding.sponsor_member_instance != sponsor_member_instance
            || binding.joiner_member_instance != joiner.member_instance
            || sponsor_matches.len() != 1
            || joiner_matches.len() != 1
            || binding.sponsor_device_id != sponsor_matches[0].device_id
            || binding.sponsor_identity_fingerprint != sponsor_matches[0].identity_fingerprint
            || binding.joiner_device_id != joiner.device_id
            || binding.joiner_identity_fingerprint != joiner.identity_fingerprint
            || joiner_matches[0] != joiner
        {
            return Err(AdmissionIdentityBindingError::Invalid);
        }
        Ok(binding)
    }

    fn validate_shape(&self) -> Result<(), AdmissionIdentityBindingError> {
        if self.lineage_id.is_empty()
            || self.sponsor_device_id.as_str().is_empty()
            || self.joiner_device_id.as_str().is_empty()
            || self.sponsor_member_instance == self.joiner_member_instance
            || self.sponsor_device_id == self.joiner_device_id
        {
            return Err(AdmissionIdentityBindingError::Invalid);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct AdmissionAttemptId([u8; 32]);

impl AdmissionAttemptId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for AdmissionAttemptId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionAttemptId([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SponsorAdmissionStageV1 {
    Accepted,
    Candidate,
    Prepared,
    Committed,
    Applied,
    Completed,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JoinerAdmissionStageV1 {
    Initiated,
    Candidate,
    Prepared,
    Committed,
    Applied,
    Completed,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionHelperAdmissionStageV1 {
    Applied,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorAdmissionStateV1 {
    pub stage: SponsorAdmissionStageV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinerAdmissionStateV1 {
    pub stage: JoinerAdmissionStageV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionHelperAdmissionStateV1 {
    pub stage: CompletionHelperAdmissionStageV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionAttemptRoleStateV1 {
    Sponsor(SponsorAdmissionStateV1),
    Joiner(JoinerAdmissionStateV1),
    CompletionHelper(CompletionHelperAdmissionStateV1),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionOutboxPurposeV1 {
    JoinRequest,
    Candidate,
    Prepared,
    Commit,
    Applied,
    CancelRequested,
    Rejected,
    Complete,
    InvitationConsume,
    ExistingMemberSecurityUpdate,
    HistoryOrReceiptBatch,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionOutboxMessageV1 {
    pub purpose: AdmissionOutboxPurposeV1,
    pub recipient: Vec<u8>,
    pub message_id: [u8; 32],
    pub predecessor_message_id: Option<[u8; 32]>,
    pub payload: Vec<u8>,
    pub superseded: bool,
}

impl std::fmt::Debug for AdmissionOutboxMessageV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionOutboxMessageV1")
            .field("purpose", &self.purpose)
            .field("message_id", &"[REDACTED]")
            .field("superseded", &self.superseded)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionInboxRecordV1 {
    pub message_id: [u8; 32],
    pub payload_digest: [u8; 32],
    pub acknowledgment_payload: Vec<u8>,
}

impl std::fmt::Debug for AdmissionInboxRecordV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionInboxRecordV1([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionTerminalResultV1 {
    Active,
    Completed,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionRejectionReasonV1 {
    InvitationUnavailable,
    AuthenticationRejected,
    IdentityConflict,
    BaseHistoryChanged,
    JoinerHistoryAhead,
    HistoryConflict,
    PeerUpgradeRequired,
    Cancelled,
    RemovedBeforeActivation,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionAttemptV1 {
    pub format_version: u16,
    pub record_version: u64,
    pub attempt_id: AdmissionAttemptId,
    pub join_id: Option<[u8; 16]>,
    pub local_join_ordinal: Option<u64>,
    pub role_state: AdmissionAttemptRoleStateV1,
    pub lineage_id: Option<String>,
    pub base_history_position: Option<Vec<u8>>,
    pub candidate_event: Option<Vec<u8>>,
    pub candidate_event_id: Option<[u8; 32]>,
    pub candidate_key_package: Option<Vec<u8>>,
    pub target_members_digest: Option<[u8; 32]>,
    pub security_commitment: Option<Vec<u8>>,
    pub security_commit: Option<Vec<u8>>,
    pub security_welcome: Option<Vec<u8>>,
    pub target_protection_group_id: Option<String>,
    pub target_key_catalog: Option<Vec<u8>>,
    pub target_relationships: Option<Vec<super::AdmissionChangeFacts>>,
    pub staged_security_state: Option<Vec<u8>>,
    pub joiner_pending_security_state: Option<Vec<u8>>,
    pub base_membership_history: Option<Vec<u8>>,
    pub verified_membership_history: Option<Vec<u8>>,
    pub invitation_claim: Option<Vec<u8>>,
    pub space_transition: Option<Vec<u8>>,
    pub space_transition_result: Option<Vec<u8>>,
    pub prepared_proof: Option<Vec<u8>>,
    pub activation_receipt: Option<Vec<u8>>,
    pub completion: Option<Vec<u8>>,
    pub completion_recovery_routes: Vec<Vec<u8>>,
    pub completion_recovery_deliveries: Vec<Vec<u8>>,
    pub cancel_request: Option<Vec<u8>>,
    pub cancel_outcome: Option<Vec<u8>>,
    pub resume_public_key: Option<Vec<u8>>,
    pub resume_private_key: Option<Vec<u8>>,
    pub identity_binding: Option<Vec<u8>>,
    pub resume_peers: Vec<Vec<u8>>,
    pub inbox_dedup: Vec<AdmissionInboxRecordV1>,
    pub outboxes: Vec<AdmissionOutboxMessageV1>,
    pub terminal_result: Option<AdmissionTerminalResultV1>,
    pub rejection_reason: Option<AdmissionRejectionReasonV1>,
    pub write_ahead_recovery: Option<Vec<u8>>,
    pub cleanup_pending: bool,
    #[serde(default)]
    pub target_access_state: Option<Vec<u8>>,
    #[serde(default)]
    pub joiner_member_instance: Option<MemberInstanceId>,
    #[serde(default)]
    pub existing_member_security_deliveries: Option<Vec<super::SponsorAdmissionSecurityDelivery>>,
    #[serde(default)]
    pub preserve_unreadable_history: bool,
}

impl std::fmt::Debug for AdmissionAttemptV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionAttemptV1")
            .field("format_version", &self.format_version)
            .field("record_version", &self.record_version)
            .field("attempt_id", &self.attempt_id)
            .field("stage", &self.stage_rank())
            .field("outbox_count", &self.outboxes.len())
            .field("terminal_result", &self.terminal_result)
            .field("cleanup_pending", &self.cleanup_pending)
            .finish()
    }
}

impl AdmissionAttemptV1 {
    pub fn new_completion_helper(attempt_id: AdmissionAttemptId) -> Self {
        let mut attempt = Self::new_joiner(attempt_id, [0; 16], JoinerAdmissionStageV1::Initiated);
        attempt.join_id = None;
        attempt.local_join_ordinal = None;
        attempt.role_state =
            AdmissionAttemptRoleStateV1::CompletionHelper(CompletionHelperAdmissionStateV1 {
                stage: CompletionHelperAdmissionStageV1::Applied,
            });
        attempt
    }

    pub fn new_joiner(
        attempt_id: AdmissionAttemptId,
        join_id: [u8; 16],
        stage: JoinerAdmissionStageV1,
    ) -> Self {
        Self {
            format_version: ADMISSION_ATTEMPT_FORMAT_V1,
            record_version: 0,
            attempt_id,
            join_id: Some(join_id),
            local_join_ordinal: None,
            role_state: AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 { stage }),
            lineage_id: None,
            base_history_position: None,
            candidate_event: None,
            candidate_event_id: None,
            candidate_key_package: None,
            target_members_digest: None,
            security_commitment: None,
            security_commit: None,
            security_welcome: None,
            target_protection_group_id: None,
            target_key_catalog: None,
            target_relationships: None,
            staged_security_state: None,
            joiner_pending_security_state: None,
            base_membership_history: None,
            verified_membership_history: None,
            invitation_claim: None,
            space_transition: None,
            space_transition_result: None,
            prepared_proof: None,
            activation_receipt: None,
            completion: None,
            completion_recovery_routes: Vec::new(),
            completion_recovery_deliveries: Vec::new(),
            cancel_request: None,
            cancel_outcome: None,
            resume_public_key: None,
            resume_private_key: None,
            identity_binding: None,
            resume_peers: Vec::new(),
            inbox_dedup: Vec::new(),
            outboxes: Vec::new(),
            terminal_result: None,
            rejection_reason: None,
            write_ahead_recovery: None,
            cleanup_pending: false,
            target_access_state: None,
            joiner_member_instance: None,
            existing_member_security_deliveries: None,
            preserve_unreadable_history: false,
        }
    }

    pub fn stage_rank(&self) -> Option<u8> {
        Some(match self.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 { stage }) => {
                match stage {
                    SponsorAdmissionStageV1::Accepted => 1,
                    SponsorAdmissionStageV1::Candidate => 2,
                    SponsorAdmissionStageV1::Prepared => 3,
                    SponsorAdmissionStageV1::Committed => 4,
                    SponsorAdmissionStageV1::Applied => 5,
                    SponsorAdmissionStageV1::Completed | SponsorAdmissionStageV1::Rejected => 6,
                }
            }
            AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 { stage }) => match stage {
                JoinerAdmissionStageV1::Initiated => 0,
                JoinerAdmissionStageV1::Candidate => 2,
                JoinerAdmissionStageV1::Prepared => 3,
                JoinerAdmissionStageV1::Committed => 4,
                JoinerAdmissionStageV1::Applied => 5,
                JoinerAdmissionStageV1::Completed | JoinerAdmissionStageV1::Rejected => 6,
            },
            AdmissionAttemptRoleStateV1::CompletionHelper(CompletionHelperAdmissionStateV1 {
                stage,
            }) => match stage {
                CompletionHelperAdmissionStageV1::Applied => 5,
                CompletionHelperAdmissionStageV1::Completed => 6,
            },
        })
    }

    pub fn set_joiner_stage(&mut self, stage: JoinerAdmissionStageV1) -> bool {
        let AdmissionAttemptRoleStateV1::Joiner(state) = &mut self.role_state else {
            return false;
        };
        state.stage = stage;
        true
    }

    pub const fn is_joiner(&self) -> bool {
        matches!(self.role_state, AdmissionAttemptRoleStateV1::Joiner(_))
    }

    pub fn same_role_as(&self, other: &Self) -> bool {
        matches!(
            (self.role_state, other.role_state),
            (
                AdmissionAttemptRoleStateV1::Sponsor(_),
                AdmissionAttemptRoleStateV1::Sponsor(_)
            ) | (
                AdmissionAttemptRoleStateV1::Joiner(_),
                AdmissionAttemptRoleStateV1::Joiner(_)
            ) | (
                AdmissionAttemptRoleStateV1::CompletionHelper(_),
                AdmissionAttemptRoleStateV1::CompletionHelper(_)
            )
        )
    }

    pub fn is_terminal(&self) -> bool {
        self.terminal_result.is_some()
    }

    pub fn has_recovery_work(&self) -> bool {
        !self.is_terminal()
            || self.outboxes.iter().any(|message| !message.superseded)
            || self.write_ahead_recovery.is_some()
            || (self.space_transition.is_some() && self.space_transition_result.is_none())
            || self.cleanup_pending
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionProfileMetadataV1 {
    pub format_version: u16,
    pub profile_generation: [u8; 16],
    pub next_local_join_ordinal: u64,
    pub join_projection_floor_ordinal: u64,
    pub device_trust_revision: u64,
    pub consumed_invitation_attempts: BTreeMap<[u8; 32], AdmissionAttemptId>,
    #[serde(default)]
    pub completion_recovery_challenges: BTreeMap<AdmissionAttemptId, Vec<u8>>,
}

impl std::fmt::Debug for AdmissionProfileMetadataV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionProfileMetadataV1")
            .field("format_version", &self.format_version)
            .field("profile_generation", &"[REDACTED]")
            .field("next_local_join_ordinal", &self.next_local_join_ordinal)
            .field(
                "join_projection_floor_ordinal",
                &self.join_projection_floor_ordinal,
            )
            .field("device_trust_revision", &self.device_trust_revision)
            .field(
                "consumed_invitation_count",
                &self.consumed_invitation_attempts.len(),
            )
            .finish()
    }
}

impl AdmissionProfileMetadataV1 {
    pub fn fresh(profile_generation: [u8; 16]) -> Self {
        Self {
            format_version: ADMISSION_PROFILE_METADATA_FORMAT_V1,
            profile_generation,
            next_local_join_ordinal: 0,
            join_projection_floor_ordinal: 0,
            device_trust_revision: 0,
            consumed_invitation_attempts: BTreeMap::new(),
            completion_recovery_challenges: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentLocalJoinProjectionV1 {
    pub device_trust_revision: u64,
    pub attempt_id: AdmissionAttemptId,
    pub join_id: [u8; 16],
    pub local_join_ordinal: u64,
    pub terminal_result: Option<AdmissionTerminalResultV1>,
    pub rejection_reason: Option<AdmissionRejectionReasonV1>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAdmissionAttemptV1 {
    pub format_version: u16,
    pub attempt_id: AdmissionAttemptId,
    pub join_id: Option<[u8; 16]>,
    pub local_join_ordinal: Option<u64>,
    pub invitation_digest: Option<[u8; 32]>,
    pub identity_binding: Option<Vec<u8>>,
    pub terminal_result: AdmissionTerminalResultV1,
    pub rejection_reason: Option<AdmissionRejectionReasonV1>,
    pub candidate_event_id: Option<[u8; 32]>,
    pub cancel_outcome: Option<Vec<u8>>,
    pub replay_result: Vec<u8>,
    pub space_transition_result: Option<Vec<u8>>,
    pub acknowledgment_rebuild: Vec<AdmissionInboxRecordV1>,
}

impl std::fmt::Debug for TerminalAdmissionAttemptV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalAdmissionAttemptV1")
            .field("attempt_id", &self.attempt_id)
            .field("terminal_result", &self.terminal_result)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::DeviceId;
    use crate::membership::{AdmissionChangeFacts, MemberInstanceId, MembershipEventId};
    use crate::security::IdentityFingerprint;

    use super::AdmissionIdentityBindingV1;

    fn facts(instance: u8, device_id: &str, fingerprint: &str) -> AdmissionChangeFacts {
        AdmissionChangeFacts {
            member_instance: MemberInstanceId::from_bytes([instance; 32]),
            device_id: DeviceId::new(device_id),
            device_name: device_id.to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string(fingerprint).unwrap(),
            transport_public_key: vec![instance],
            transport_address_blob: vec![instance.wrapping_add(1)],
            identity_signature: vec![instance.wrapping_add(2)],
        }
    }

    #[test]
    fn admission_identity_binding_round_trips_and_validates_exact_relationships() {
        let sponsor = facts(1, "sponsor", "QRST-UVWX-YZ23-4567");
        let joiner = facts(2, "joiner", "ABCD-EFGH-IJKL-MNOP");
        let event_id = MembershipEventId::from_bytes([3; 32]);
        let binding =
            AdmissionIdentityBindingV1::new("space-a".to_owned(), event_id, &sponsor, &joiner)
                .unwrap();

        let encoded = binding.encode().unwrap();
        let decoded = AdmissionIdentityBindingV1::decode_and_validate(
            &encoded,
            "space-a",
            event_id,
            sponsor.member_instance,
            &joiner,
            &[sponsor.clone(), joiner.clone()],
        )
        .unwrap();

        assert_eq!(decoded, binding);
        assert!(!format!("{decoded:?}").contains("sponsor"));
        assert!(AdmissionIdentityBindingV1::decode_and_validate(
            &encoded,
            "space-b",
            event_id,
            sponsor.member_instance,
            &joiner,
            &[sponsor, joiner.clone()],
        )
        .is_err());
    }
}
