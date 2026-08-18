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
    Superseded,
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
    SupersededByNewJoin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupersedeAdmissionAttemptError {
    NotJoiner,
    AlreadyTerminal,
    UnsafeStage,
    RecoveryRequired,
    InvalidCleanupMessage,
}

impl fmt::Display for SupersedeAdmissionAttemptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotJoiner => "only a local join can be superseded",
            Self::AlreadyTerminal => "a terminal admission attempt cannot be superseded",
            Self::UnsafeStage => "the local join has crossed the supersession boundary",
            Self::RecoveryRequired => "the local join requires recovery before replacement",
            Self::InvalidCleanupMessage => "the supersession cleanup message is invalid",
        })
    }
}

impl std::error::Error for SupersedeAdmissionAttemptError {}

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
    #[serde(default)]
    pub sponsor_continuation_address: Option<Vec<u8>>,
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
    pub fn decode_persisted(bytes: &[u8]) -> Result<Self, postcard::Error> {
        match postcard::from_bytes(bytes) {
            Ok(attempt) => Ok(attempt),
            Err(postcard::Error::DeserializeUnexpectedEnd) => {
                let mut previous_version = Vec::with_capacity(bytes.len() + 1);
                previous_version.extend_from_slice(bytes);
                previous_version.push(0);
                postcard::from_bytes(&previous_version)
            }
            Err(error) => Err(error),
        }
    }

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
            sponsor_continuation_address: None,
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
                JoinerAdmissionStageV1::Completed
                | JoinerAdmissionStageV1::Rejected
                | JoinerAdmissionStageV1::Superseded => 6,
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

    pub fn superseded_by_new_join(
        &self,
        cleanup: AdmissionOutboxMessageV1,
    ) -> Result<Self, SupersedeAdmissionAttemptError> {
        let AdmissionAttemptRoleStateV1::Joiner(joiner) = self.role_state else {
            return Err(SupersedeAdmissionAttemptError::NotJoiner);
        };
        if self.is_terminal() {
            return Err(SupersedeAdmissionAttemptError::AlreadyTerminal);
        }
        if !matches!(
            joiner.stage,
            JoinerAdmissionStageV1::Initiated | JoinerAdmissionStageV1::Candidate
        ) {
            return Err(SupersedeAdmissionAttemptError::UnsafeStage);
        }
        let initial_join_request = self.outboxes.iter().rev().find(|message| {
            message.purpose == AdmissionOutboxPurposeV1::JoinRequest
                && message.predecessor_message_id.is_none()
                && !message.recipient.is_empty()
                && !message.payload.is_empty()
                && message.message_id != [0; 32]
        });
        let candidate_material_is_complete = joiner.stage != JoinerAdmissionStageV1::Candidate
            || (self.lineage_id.is_some()
                && self.base_history_position.is_some()
                && self.candidate_event.is_some()
                && self.candidate_event_id.is_some()
                && self.target_members_digest.is_some()
                && self.security_commitment.is_some()
                && self.security_commit.is_some()
                && self.security_welcome.is_some()
                && self.target_protection_group_id.is_some()
                && self.target_key_catalog.is_some()
                && self.target_relationships.is_some()
                && self.existing_member_security_deliveries.is_some()
                && self.staged_security_state.is_some()
                && self.base_membership_history.is_some()
                && self.identity_binding.is_some());
        if self.prepared_proof.is_some()
            || self.write_ahead_recovery.is_some()
            || self.space_transition.is_some()
            || self.space_transition_result.is_some()
            || self.cleanup_pending
            || initial_join_request.is_none()
            || !candidate_material_is_complete
            || self.join_id.is_none()
            || self.local_join_ordinal.is_none()
            || self.joiner_pending_security_state.is_none()
            || self.candidate_key_package.is_none()
            || self.joiner_member_instance.is_none()
            || self
                .resume_public_key
                .as_ref()
                .is_none_or(|key| key.len() != 32)
            || self
                .resume_private_key
                .as_ref()
                .is_none_or(|key| key.len() != 32)
        {
            return Err(SupersedeAdmissionAttemptError::RecoveryRequired);
        }
        let active_predecessor = self
            .outboxes
            .iter()
            .rev()
            .find(|message| !message.superseded)
            .or(initial_join_request)
            .map(|message| message.message_id);
        if cleanup.purpose != AdmissionOutboxPurposeV1::CancelRequested
            || cleanup.recipient.is_empty()
            || cleanup.payload.is_empty()
            || cleanup.message_id == [0; 32]
            || cleanup.predecessor_message_id != active_predecessor
            || cleanup.superseded
        {
            return Err(SupersedeAdmissionAttemptError::InvalidCleanupMessage);
        }

        let mut superseded = self.clone();
        for message in &mut superseded.outboxes {
            message.superseded = true;
        }
        superseded.cancel_request = Some(cleanup.payload.clone());
        superseded.outboxes.push(cleanup);
        superseded.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Superseded,
        });
        superseded.terminal_result = Some(AdmissionTerminalResultV1::SupersededByNewJoin);
        superseded.rejection_reason = None;
        Ok(superseded)
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

    use super::{
        AdmissionAttemptId, AdmissionAttemptV1, AdmissionIdentityBindingV1,
        AdmissionOutboxMessageV1, AdmissionOutboxPurposeV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, SupersedeAdmissionAttemptError,
    };

    fn join_request(attempt_id: AdmissionAttemptId) -> AdmissionOutboxMessageV1 {
        AdmissionOutboxMessageV1 {
            purpose: AdmissionOutboxPurposeV1::JoinRequest,
            recipient: vec![9],
            message_id: *attempt_id.as_bytes(),
            predecessor_message_id: None,
            payload: vec![8],
            superseded: false,
        }
    }

    fn cancel_request(attempt_id: AdmissionAttemptId) -> AdmissionOutboxMessageV1 {
        AdmissionOutboxMessageV1 {
            purpose: AdmissionOutboxPurposeV1::CancelRequested,
            recipient: vec![9],
            message_id: [7; 32],
            predecessor_message_id: Some(*attempt_id.as_bytes()),
            payload: vec![6],
            superseded: false,
        }
    }

    #[test]
    fn initiated_join_can_be_superseded_without_losing_replay_facts() {
        let attempt_id = AdmissionAttemptId::from_bytes([1; 32]);
        let mut attempt =
            AdmissionAttemptV1::new_joiner(attempt_id, [2; 16], JoinerAdmissionStageV1::Initiated);
        attempt.local_join_ordinal = Some(3);
        attempt.joiner_pending_security_state = Some(vec![3]);
        attempt.candidate_key_package = Some(vec![4]);
        attempt.joiner_member_instance =
            Some(crate::membership::MemberInstanceId::from_bytes([5; 32]));
        attempt.resume_public_key = Some(vec![6; 32]);
        attempt.resume_private_key = Some(vec![7; 32]);
        attempt.inbox_dedup.push(super::AdmissionInboxRecordV1 {
            message_id: [8; 32],
            payload_digest: [9; 32],
            acknowledgment_payload: vec![10],
        });
        attempt.outboxes.push(join_request(attempt_id));

        let superseded = attempt
            .superseded_by_new_join(cancel_request(attempt_id))
            .unwrap();

        assert_eq!(
            superseded.stage_rank(),
            Some(6),
            "superseded is a terminal joiner stage"
        );
        assert_eq!(
            superseded.terminal_result,
            Some(AdmissionTerminalResultV1::SupersededByNewJoin)
        );
        assert_eq!(superseded.rejection_reason, None);
        assert_eq!(superseded.inbox_dedup, attempt.inbox_dedup);
        assert!(superseded.outboxes[0].superseded);
        assert_eq!(
            superseded.outboxes[1].purpose,
            AdmissionOutboxPurposeV1::CancelRequested
        );
        assert!(!superseded.outboxes[1].superseded);
        assert_eq!(superseded.cancel_request, Some(vec![6]));
    }

    #[test]
    fn prepared_or_recovery_bound_join_cannot_be_superseded() {
        let attempt_id = AdmissionAttemptId::from_bytes([11; 32]);
        let mut prepared =
            AdmissionAttemptV1::new_joiner(attempt_id, [12; 16], JoinerAdmissionStageV1::Prepared);
        prepared.local_join_ordinal = Some(1);
        prepared.prepared_proof = Some(vec![1]);
        assert_eq!(
            prepared.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeAdmissionAttemptError::UnsafeStage)
        );

        let mut contradictory =
            AdmissionAttemptV1::new_joiner(attempt_id, [12; 16], JoinerAdmissionStageV1::Candidate);
        contradictory.local_join_ordinal = Some(1);
        contradictory.prepared_proof = Some(vec![1]);
        let original = contradictory.clone();
        assert_eq!(
            contradictory.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeAdmissionAttemptError::RecoveryRequired)
        );
        assert_eq!(contradictory, original);
    }

    #[test]
    fn join_without_a_valid_initial_request_cannot_be_superseded() {
        let attempt_id = AdmissionAttemptId::from_bytes([13; 32]);
        let mut attempt =
            AdmissionAttemptV1::new_joiner(attempt_id, [14; 16], JoinerAdmissionStageV1::Initiated);
        attempt.local_join_ordinal = Some(1);
        attempt.joiner_pending_security_state = Some(vec![1]);
        attempt.candidate_key_package = Some(vec![2]);
        attempt.joiner_member_instance = Some(MemberInstanceId::from_bytes([3; 32]));
        attempt.resume_public_key = Some(vec![4; 32]);
        attempt.resume_private_key = Some(vec![5; 32]);

        assert_eq!(
            attempt.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeAdmissionAttemptError::RecoveryRequired)
        );
    }

    #[test]
    fn candidate_without_identity_binding_cannot_be_superseded() {
        let attempt_id = AdmissionAttemptId::from_bytes([15; 32]);
        let mut attempt =
            AdmissionAttemptV1::new_joiner(attempt_id, [16; 16], JoinerAdmissionStageV1::Candidate);
        attempt.local_join_ordinal = Some(1);
        attempt.joiner_pending_security_state = Some(vec![1]);
        attempt.candidate_key_package = Some(vec![2]);
        attempt.joiner_member_instance = Some(MemberInstanceId::from_bytes([3; 32]));
        attempt.resume_public_key = Some(vec![4; 32]);
        attempt.resume_private_key = Some(vec![5; 32]);
        attempt.outboxes.push(join_request(attempt_id));
        attempt.lineage_id = Some("target-space".to_owned());
        attempt.base_history_position = Some(vec![6]);
        attempt.candidate_event = Some(vec![7]);
        attempt.candidate_event_id = Some([8; 32]);
        attempt.target_members_digest = Some([9; 32]);
        attempt.security_commitment = Some(vec![10]);
        attempt.security_commit = Some(vec![11]);
        attempt.security_welcome = Some(vec![12]);
        attempt.target_protection_group_id = Some("target-group".to_owned());
        attempt.target_key_catalog = Some(vec![13]);
        attempt.target_relationships = Some(Vec::new());
        attempt.existing_member_security_deliveries = Some(Vec::new());
        attempt.staged_security_state = Some(vec![14]);
        attempt.base_membership_history = Some(vec![15]);

        assert_eq!(
            attempt.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeAdmissionAttemptError::RecoveryRequired)
        );
    }

    #[test]
    fn unsafe_roles_stages_and_recovery_states_cannot_be_superseded() {
        let attempt_id = AdmissionAttemptId::from_bytes([17; 32]);
        let mut base =
            AdmissionAttemptV1::new_joiner(attempt_id, [18; 16], JoinerAdmissionStageV1::Initiated);
        base.local_join_ordinal = Some(1);
        base.joiner_pending_security_state = Some(vec![1]);
        base.candidate_key_package = Some(vec![2]);
        base.joiner_member_instance = Some(MemberInstanceId::from_bytes([3; 32]));
        base.resume_public_key = Some(vec![4; 32]);
        base.resume_private_key = Some(vec![5; 32]);
        base.outboxes.push(join_request(attempt_id));

        let mut sponsor = base.clone();
        sponsor.join_id = None;
        sponsor.local_join_ordinal = None;
        sponsor.role_state =
            super::AdmissionAttemptRoleStateV1::Sponsor(super::SponsorAdmissionStateV1 {
                stage: super::SponsorAdmissionStageV1::Accepted,
            });
        assert_eq!(
            sponsor.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeAdmissionAttemptError::NotJoiner)
        );

        let mut terminal = base.clone();
        terminal.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        assert_eq!(
            terminal.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeAdmissionAttemptError::AlreadyTerminal)
        );

        for stage in [
            JoinerAdmissionStageV1::Prepared,
            JoinerAdmissionStageV1::Committed,
            JoinerAdmissionStageV1::Applied,
            JoinerAdmissionStageV1::Completed,
            JoinerAdmissionStageV1::Rejected,
            JoinerAdmissionStageV1::Superseded,
        ] {
            let mut unsafe_stage = base.clone();
            unsafe_stage.role_state =
                super::AdmissionAttemptRoleStateV1::Joiner(super::JoinerAdmissionStateV1 { stage });
            assert_eq!(
                unsafe_stage.superseded_by_new_join(cancel_request(attempt_id)),
                Err(SupersedeAdmissionAttemptError::UnsafeStage)
            );
        }

        let mut recovery_bound = Vec::new();
        let mut write_ahead = base.clone();
        write_ahead.write_ahead_recovery = Some(vec![1]);
        recovery_bound.push(write_ahead);
        let mut transition = base.clone();
        transition.space_transition = Some(vec![1]);
        recovery_bound.push(transition);
        let mut transition_result = base.clone();
        transition_result.space_transition_result = Some(vec![1]);
        recovery_bound.push(transition_result);
        let mut cleanup_pending = base.clone();
        cleanup_pending.cleanup_pending = true;
        recovery_bound.push(cleanup_pending);
        let mut missing_member = base.clone();
        missing_member.joiner_member_instance = None;
        recovery_bound.push(missing_member);
        let mut missing_resume_key = base;
        missing_resume_key.resume_private_key = None;
        recovery_bound.push(missing_resume_key);

        for attempt in recovery_bound {
            assert_eq!(
                attempt.superseded_by_new_join(cancel_request(attempt_id)),
                Err(SupersedeAdmissionAttemptError::RecoveryRequired)
            );
        }
    }

    #[test]
    fn admission_attempt_without_continuation_address_still_decodes() {
        let attempt_id = AdmissionAttemptId::from_bytes([19; 32]);
        let mut expected =
            AdmissionAttemptV1::new_joiner(attempt_id, [20; 16], JoinerAdmissionStageV1::Initiated);
        expected.cancel_request = Some(vec![21]);
        expected.cancel_outcome = Some(vec![22]);
        expected.resume_public_key = Some(vec![23; 32]);
        expected.resume_private_key = Some(vec![24; 32]);
        expected.preserve_unreadable_history = true;

        let mut previous_version = postcard::to_stdvec(&expected).unwrap();
        assert_eq!(previous_version.pop(), Some(0));
        let decoded = AdmissionAttemptV1::decode_persisted(&previous_version).unwrap();

        assert_eq!(decoded, expected);
        assert_eq!(decoded.sponsor_continuation_address, None);
    }

    #[test]
    fn appended_join_stage_and_terminal_variants_keep_old_wire_values() {
        #[derive(Debug, serde::Deserialize, PartialEq, Eq)]
        enum PreviousTerminalResultV1 {
            Active,
            Completed,
            Rejected,
        }

        let joiner_stages = [
            JoinerAdmissionStageV1::Initiated,
            JoinerAdmissionStageV1::Candidate,
            JoinerAdmissionStageV1::Prepared,
            JoinerAdmissionStageV1::Committed,
            JoinerAdmissionStageV1::Applied,
            JoinerAdmissionStageV1::Completed,
            JoinerAdmissionStageV1::Rejected,
            JoinerAdmissionStageV1::Superseded,
        ];
        for (index, stage) in joiner_stages.into_iter().enumerate() {
            assert_eq!(postcard::to_stdvec(&stage).unwrap(), vec![index as u8]);
        }

        let terminal_results = [
            AdmissionTerminalResultV1::Active,
            AdmissionTerminalResultV1::Completed,
            AdmissionTerminalResultV1::Rejected,
            AdmissionTerminalResultV1::SupersededByNewJoin,
        ];
        for (index, terminal) in terminal_results.into_iter().enumerate() {
            assert_eq!(postcard::to_stdvec(&terminal).unwrap(), vec![index as u8]);
        }
        for index in 0..3 {
            assert!(postcard::from_bytes::<PreviousTerminalResultV1>(&[index]).is_ok());
        }
        assert!(postcard::from_bytes::<PreviousTerminalResultV1>(&[3]).is_err());
    }

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
