use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;
use crate::security::IdentityFingerprint;

use super::{
    AdmissionChangeFacts, AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2,
    MemberInstanceId, MembershipCredentialId, MembershipEventId,
};

#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub const SPACE_JOIN_RECORD_FORMAT_V1: u16 = 1;
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub const ADMISSION_PROFILE_METADATA_FORMAT_V1: u16 = 1;
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub const ADMISSION_IDENTITY_BINDING_FORMAT_V1: u16 = 1;

/// Security payload persisted with an Space join record for later delivery.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct SponsorAdmissionSecurityDelivery {
    pub recipient: DeviceId,
    pub credential_id: MembershipCredentialId,
    pub payload: Vec<u8>,
}

impl fmt::Debug for SponsorAdmissionSecurityDelivery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SponsorAdmissionSecurityDelivery")
            .field("recipient", &self.recipient)
            .field("payload_len", &self.payload.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
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
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
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
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct SpaceJoinRecordId([u8; 32]);

impl SpaceJoinRecordId {
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SpaceJoinRecordId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SpaceJoinRecordId([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum SponsorAdmissionStage {
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
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum JoinerAdmissionStage {
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
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum CompletionHelperAdmissionStage {
    Applied,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct SponsorAdmissionState {
    pub stage: SponsorAdmissionStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct JoinerAdmissionState {
    pub stage: JoinerAdmissionStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct CompletionHelperAdmissionState {
    pub stage: CompletionHelperAdmissionStage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum SpaceJoinRoleState {
    Sponsor(SponsorAdmissionState),
    Joiner(JoinerAdmissionState),
    CompletionHelper(CompletionHelperAdmissionState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum AdmissionOutboxPurpose {
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
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct AdmissionOutboxMessage {
    pub purpose: AdmissionOutboxPurpose,
    pub recipient: Vec<u8>,
    pub message_id: [u8; 32],
    pub predecessor_message_id: Option<[u8; 32]>,
    pub payload: Vec<u8>,
    pub superseded: bool,
}

impl std::fmt::Debug for AdmissionOutboxMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionOutboxMessage")
            .field("purpose", &self.purpose)
            .field("message_id", &"[REDACTED]")
            .field("superseded", &self.superseded)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct AdmissionInboxRecord {
    pub message_id: [u8; 32],
    pub payload_digest: [u8; 32],
    pub acknowledgment_payload: Vec<u8>,
}

impl std::fmt::Debug for AdmissionInboxRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionInboxRecord([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum AdmissionTerminalResult {
    Active,
    Completed,
    Rejected,
    SupersededByNewJoin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum SupersedeSpaceJoinError {
    NotJoiner,
    AlreadyTerminal,
    UnsafeStage,
    RecoveryRequired,
    InvalidCleanupMessage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum CancelSpaceJoinRecordError {
    NotJoiner,
    AlreadyTerminal,
    UnsafeStage,
    InvalidCancellationMessage,
}

impl fmt::Display for CancelSpaceJoinRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotJoiner => "only a local join can be cancelled",
            Self::AlreadyTerminal => "a terminal Space join record cannot be cancelled",
            Self::UnsafeStage => "the local join has crossed the cancellation boundary",
            Self::InvalidCancellationMessage => "the Space join cancellation message is invalid",
        })
    }
}

impl std::error::Error for CancelSpaceJoinRecordError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum SpaceJoinTransitionError {
    NotJoiner,
    AlreadyTerminal,
    AdmissionIncomplete,
    StoredTransitionInvalid,
    TransitionMismatch,
    InvalidAdvance,
    InvalidResult,
    MissingVerifiedHistory,
}

impl fmt::Display for SpaceJoinTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotJoiner => "only a local join can advance a Space transition",
            Self::AlreadyTerminal => "a terminal Space join cannot advance its transition",
            Self::AdmissionIncomplete => "Space admission is incomplete",
            Self::StoredTransitionInvalid => "the saved Space transition is invalid",
            Self::TransitionMismatch => "the Space transition does not match the saved join",
            Self::InvalidAdvance => "the Space transition did not advance by one phase",
            Self::InvalidResult => "the Space transition result is invalid",
            Self::MissingVerifiedHistory => "verified membership history is missing",
        })
    }
}

impl std::error::Error for SpaceJoinTransitionError {}

impl fmt::Display for SupersedeSpaceJoinError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotJoiner => "only a local join can be superseded",
            Self::AlreadyTerminal => "a terminal Space join record cannot be superseded",
            Self::UnsafeStage => "the local join has crossed the supersession boundary",
            Self::RecoveryRequired => "the local join requires recovery before replacement",
            Self::InvalidCleanupMessage => "the supersession cleanup message is invalid",
        })
    }
}

impl std::error::Error for SupersedeSpaceJoinError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub enum AdmissionRejectionReason {
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
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct SpaceJoinRecord {
    pub format_version: u16,
    pub record_version: u64,
    pub record_id: SpaceJoinRecordId,
    pub join_id: Option<[u8; 16]>,
    pub local_join_ordinal: Option<u64>,
    pub role_state: SpaceJoinRoleState,
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
    pub inbox_dedup: Vec<AdmissionInboxRecord>,
    pub outboxes: Vec<AdmissionOutboxMessage>,
    pub terminal_result: Option<AdmissionTerminalResult>,
    pub rejection_reason: Option<AdmissionRejectionReason>,
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

impl std::fmt::Debug for SpaceJoinRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpaceJoinRecord")
            .field("format_version", &self.format_version)
            .field("record_version", &self.record_version)
            .field("attempt_id", &self.record_id)
            .field("stage", &self.stage_rank())
            .field("outbox_count", &self.outboxes.len())
            .field("terminal_result", &self.terminal_result)
            .field("cleanup_pending", &self.cleanup_pending)
            .finish()
    }
}

impl SpaceJoinRecord {
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

    pub fn new_completion_helper(attempt_id: SpaceJoinRecordId) -> Self {
        let mut attempt = Self::new_joiner(attempt_id, [0; 16], JoinerAdmissionStage::Initiated);
        attempt.join_id = None;
        attempt.local_join_ordinal = None;
        attempt.role_state = SpaceJoinRoleState::CompletionHelper(CompletionHelperAdmissionState {
            stage: CompletionHelperAdmissionStage::Applied,
        });
        attempt
    }

    pub fn new_joiner(
        record_id: SpaceJoinRecordId,
        join_id: [u8; 16],
        stage: JoinerAdmissionStage,
    ) -> Self {
        Self {
            format_version: SPACE_JOIN_RECORD_FORMAT_V1,
            record_version: 0,
            record_id,
            join_id: Some(join_id),
            local_join_ordinal: None,
            role_state: SpaceJoinRoleState::Joiner(JoinerAdmissionState { stage }),
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
            SpaceJoinRoleState::Sponsor(SponsorAdmissionState { stage }) => match stage {
                SponsorAdmissionStage::Accepted => 1,
                SponsorAdmissionStage::Candidate => 2,
                SponsorAdmissionStage::Prepared => 3,
                SponsorAdmissionStage::Committed => 4,
                SponsorAdmissionStage::Applied => 5,
                SponsorAdmissionStage::Completed | SponsorAdmissionStage::Rejected => 6,
            },
            SpaceJoinRoleState::Joiner(JoinerAdmissionState { stage }) => match stage {
                JoinerAdmissionStage::Initiated => 0,
                JoinerAdmissionStage::Candidate => 2,
                JoinerAdmissionStage::Prepared => 3,
                JoinerAdmissionStage::Committed => 4,
                JoinerAdmissionStage::Applied => 5,
                JoinerAdmissionStage::Completed
                | JoinerAdmissionStage::Rejected
                | JoinerAdmissionStage::Superseded => 6,
            },
            SpaceJoinRoleState::CompletionHelper(CompletionHelperAdmissionState { stage }) => {
                match stage {
                    CompletionHelperAdmissionStage::Applied => 5,
                    CompletionHelperAdmissionStage::Completed => 6,
                }
            }
        })
    }

    pub fn set_joiner_stage(&mut self, stage: JoinerAdmissionStage) -> bool {
        let SpaceJoinRoleState::Joiner(state) = &mut self.role_state else {
            return false;
        };
        state.stage = stage;
        true
    }

    pub const fn is_joiner(&self) -> bool {
        matches!(self.role_state, SpaceJoinRoleState::Joiner(_))
    }

    pub fn same_role_as(&self, other: &Self) -> bool {
        matches!(
            (self.role_state, other.role_state),
            (
                SpaceJoinRoleState::Sponsor(_),
                SpaceJoinRoleState::Sponsor(_)
            ) | (SpaceJoinRoleState::Joiner(_), SpaceJoinRoleState::Joiner(_))
                | (
                    SpaceJoinRoleState::CompletionHelper(_),
                    SpaceJoinRoleState::CompletionHelper(_)
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
        cleanup: AdmissionOutboxMessage,
    ) -> Result<Self, SupersedeSpaceJoinError> {
        let SpaceJoinRoleState::Joiner(joiner) = self.role_state else {
            return Err(SupersedeSpaceJoinError::NotJoiner);
        };
        if self.is_terminal() {
            return Err(SupersedeSpaceJoinError::AlreadyTerminal);
        }
        if !matches!(
            joiner.stage,
            JoinerAdmissionStage::Initiated | JoinerAdmissionStage::Candidate
        ) {
            return Err(SupersedeSpaceJoinError::UnsafeStage);
        }
        let initial_join_request = self.outboxes.iter().rev().find(|message| {
            message.purpose == AdmissionOutboxPurpose::JoinRequest
                && message.predecessor_message_id.is_none()
                && !message.recipient.is_empty()
                && !message.payload.is_empty()
                && message.message_id != [0; 32]
        });
        let candidate_material_is_complete = joiner.stage != JoinerAdmissionStage::Candidate
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
            return Err(SupersedeSpaceJoinError::RecoveryRequired);
        }
        let active_predecessor = self
            .outboxes
            .iter()
            .rev()
            .find(|message| !message.superseded)
            .or(initial_join_request)
            .map(|message| message.message_id);
        if cleanup.purpose != AdmissionOutboxPurpose::CancelRequested
            || cleanup.recipient.is_empty()
            || cleanup.payload.is_empty()
            || cleanup.message_id == [0; 32]
            || cleanup.predecessor_message_id != active_predecessor
            || cleanup.superseded
        {
            return Err(SupersedeSpaceJoinError::InvalidCleanupMessage);
        }

        let mut superseded = self.clone();
        for message in &mut superseded.outboxes {
            message.superseded = true;
        }
        superseded.cancel_request = Some(cleanup.payload.clone());
        superseded.outboxes.push(cleanup);
        superseded.role_state = SpaceJoinRoleState::Joiner(JoinerAdmissionState {
            stage: JoinerAdmissionStage::Superseded,
        });
        superseded.terminal_result = Some(AdmissionTerminalResult::SupersededByNewJoin);
        superseded.rejection_reason = None;
        Ok(superseded)
    }

    pub fn cancelled(
        &self,
        cancellation: AdmissionOutboxMessage,
    ) -> Result<Self, CancelSpaceJoinRecordError> {
        let SpaceJoinRoleState::Joiner(joiner) = self.role_state else {
            return Err(CancelSpaceJoinRecordError::NotJoiner);
        };
        if self.is_terminal() {
            return Err(CancelSpaceJoinRecordError::AlreadyTerminal);
        }
        if matches!(
            joiner.stage,
            JoinerAdmissionStage::Committed
                | JoinerAdmissionStage::Applied
                | JoinerAdmissionStage::Completed
                | JoinerAdmissionStage::Rejected
                | JoinerAdmissionStage::Superseded
        ) {
            return Err(CancelSpaceJoinRecordError::UnsafeStage);
        }
        let join_request = self
            .outboxes
            .iter()
            .find(|message| message.purpose == AdmissionOutboxPurpose::JoinRequest)
            .ok_or(CancelSpaceJoinRecordError::InvalidCancellationMessage)?;
        let active_predecessor = self
            .outboxes
            .iter()
            .rev()
            .find(|message| !message.superseded)
            .map(|message| message.message_id);
        if cancellation.purpose != AdmissionOutboxPurpose::CancelRequested
            || cancellation.recipient != join_request.recipient
            || cancellation.recipient.is_empty()
            || cancellation.payload.is_empty()
            || cancellation.message_id == [0; 32]
            || cancellation.predecessor_message_id != active_predecessor
            || cancellation.superseded
        {
            return Err(CancelSpaceJoinRecordError::InvalidCancellationMessage);
        }

        let mut cancelled = self.clone();
        for message in &mut cancelled.outboxes {
            message.superseded = true;
        }
        cancelled.cancel_request = Some(cancellation.payload.clone());
        cancelled.outboxes.push(cancellation);
        cancelled.role_state = SpaceJoinRoleState::Joiner(JoinerAdmissionState {
            stage: JoinerAdmissionStage::Rejected,
        });
        cancelled.terminal_result = Some(AdmissionTerminalResult::Rejected);
        cancelled.rejection_reason = Some(AdmissionRejectionReason::Cancelled);
        Ok(cancelled)
    }

    pub fn advanced_space_transition(
        &self,
        current: &AdmissionSpaceTransitionV2,
        next: &AdmissionSpaceTransitionV2,
    ) -> Result<Self, SpaceJoinTransitionError> {
        self.validate_current_space_transition(current)?;
        if !current.can_advance_to(next) {
            return Err(SpaceJoinTransitionError::InvalidAdvance);
        }
        let encoded = next
            .encode()
            .ok_or(SpaceJoinTransitionError::InvalidAdvance)?;
        let mut advanced = self.clone();
        advanced.space_transition = Some(encoded);
        Ok(advanced)
    }

    pub fn completed_space_transition(
        &self,
        current: &AdmissionSpaceTransitionV2,
        result: &AdmissionSpaceTransitionResultV2,
    ) -> Result<(Self, Vec<u8>), SpaceJoinTransitionError> {
        self.validate_current_space_transition(current)?;
        if !result.matches_cleanup_pending(current) {
            return Err(SpaceJoinTransitionError::InvalidResult);
        }
        let history = self
            .verified_membership_history
            .clone()
            .ok_or(SpaceJoinTransitionError::MissingVerifiedHistory)?;
        let encoded = result
            .encode()
            .ok_or(SpaceJoinTransitionError::InvalidResult)?;
        let mut completed = self.clone();
        completed.space_transition_result = Some(encoded);
        completed.role_state = SpaceJoinRoleState::Joiner(JoinerAdmissionState {
            stage: JoinerAdmissionStage::Completed,
        });
        completed.terminal_result = Some(AdmissionTerminalResult::Active);
        Ok((completed, history))
    }

    fn validate_current_space_transition(
        &self,
        current: &AdmissionSpaceTransitionV2,
    ) -> Result<(), SpaceJoinTransitionError> {
        if !self.is_joiner() {
            return Err(SpaceJoinTransitionError::NotJoiner);
        }
        if self.is_terminal() {
            return Err(SpaceJoinTransitionError::AlreadyTerminal);
        }
        if self.completion.is_none() {
            return Err(SpaceJoinTransitionError::AdmissionIncomplete);
        }
        let stored = self
            .space_transition
            .as_deref()
            .and_then(AdmissionSpaceTransitionV2::decode)
            .ok_or(SpaceJoinTransitionError::StoredTransitionInvalid)?;
        if stored != *current || current.attempt_id().as_bytes() != self.record_id.as_bytes() {
            return Err(SpaceJoinTransitionError::TransitionMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[deprecated(
    since = "1.1.0-rc.5",
    note = "legacy Space admission model; migrate to SpaceAdmissionAggregate before Spec 028 cleanup"
)]
pub struct AdmissionProfileMetadata {
    pub format_version: u16,
    pub profile_generation: [u8; 16],
    pub next_local_join_ordinal: u64,
    pub join_projection_floor_ordinal: u64,
    pub device_trust_revision: u64,
    pub consumed_invitation_attempts: BTreeMap<[u8; 32], SpaceJoinRecordId>,
    #[serde(default)]
    pub completion_recovery_challenges: BTreeMap<SpaceJoinRecordId, Vec<u8>>,
}

impl std::fmt::Debug for AdmissionProfileMetadata {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionProfileMetadata")
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

impl AdmissionProfileMetadata {
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

#[cfg(test)]
mod tests {
    use crate::ids::DeviceId;
    use crate::membership::{
        AdmissionChangeFacts, AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2,
        FreshSpaceTransitionPhaseV1, FreshSpaceTransitionV1, MemberInstanceId, MembershipEventId,
        FRESH_SPACE_TRANSITION_FORMAT_V1,
    };
    use crate::security::IdentityFingerprint;

    use super::{
        AdmissionIdentityBindingV1, AdmissionOutboxMessage, AdmissionOutboxPurpose,
        AdmissionRejectionReason, AdmissionTerminalResult, JoinerAdmissionStage, SpaceJoinRecord,
        SpaceJoinRecordId, SupersedeSpaceJoinError,
    };

    fn join_request(attempt_id: SpaceJoinRecordId) -> AdmissionOutboxMessage {
        AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::JoinRequest,
            recipient: vec![9],
            message_id: *attempt_id.as_bytes(),
            predecessor_message_id: None,
            payload: vec![8],
            superseded: false,
        }
    }

    fn cancel_request(attempt_id: SpaceJoinRecordId) -> AdmissionOutboxMessage {
        AdmissionOutboxMessage {
            purpose: AdmissionOutboxPurpose::CancelRequested,
            recipient: vec![9],
            message_id: [7; 32],
            predecessor_message_id: Some(*attempt_id.as_bytes()),
            payload: vec![6],
            superseded: false,
        }
    }

    #[test]
    fn initiated_join_can_be_superseded_without_losing_replay_facts() {
        let attempt_id = SpaceJoinRecordId::from_bytes([1; 32]);
        let mut attempt =
            SpaceJoinRecord::new_joiner(attempt_id, [2; 16], JoinerAdmissionStage::Initiated);
        attempt.local_join_ordinal = Some(3);
        attempt.joiner_pending_security_state = Some(vec![3]);
        attempt.candidate_key_package = Some(vec![4]);
        attempt.joiner_member_instance =
            Some(crate::membership::MemberInstanceId::from_bytes([5; 32]));
        attempt.resume_public_key = Some(vec![6; 32]);
        attempt.resume_private_key = Some(vec![7; 32]);
        attempt.inbox_dedup.push(super::AdmissionInboxRecord {
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
            Some(AdmissionTerminalResult::SupersededByNewJoin)
        );
        assert_eq!(superseded.rejection_reason, None);
        assert_eq!(superseded.inbox_dedup, attempt.inbox_dedup);
        assert!(superseded.outboxes[0].superseded);
        assert_eq!(
            superseded.outboxes[1].purpose,
            AdmissionOutboxPurpose::CancelRequested
        );
        assert!(!superseded.outboxes[1].superseded);
        assert_eq!(superseded.cancel_request, Some(vec![6]));
    }

    #[test]
    fn initiated_join_can_be_cancelled_as_one_state_change() {
        let record_id = SpaceJoinRecordId::from_bytes([10; 32]);
        let mut record =
            SpaceJoinRecord::new_joiner(record_id, [11; 16], JoinerAdmissionStage::Initiated);
        record.outboxes.push(join_request(record_id));

        let cancelled = record.cancelled(cancel_request(record_id)).unwrap();

        assert_eq!(cancelled.stage_rank(), Some(6));
        assert_eq!(
            cancelled.terminal_result,
            Some(AdmissionTerminalResult::Rejected)
        );
        assert_eq!(
            cancelled.rejection_reason,
            Some(AdmissionRejectionReason::Cancelled)
        );
        assert!(cancelled.outboxes[0].superseded);
        assert_eq!(
            cancelled.outboxes[1].purpose,
            AdmissionOutboxPurpose::CancelRequested
        );
        assert_eq!(cancelled.cancel_request, Some(vec![6]));
        assert_eq!(cancelled.record_version, record.record_version);
    }

    fn fresh_transition(
        record_id: SpaceJoinRecordId,
        phase: FreshSpaceTransitionPhaseV1,
    ) -> AdmissionSpaceTransitionV2 {
        AdmissionSpaceTransitionV2::Fresh(FreshSpaceTransitionV1 {
            transition_format_version: FRESH_SPACE_TRANSITION_FORMAT_V1,
            attempt_id: crate::membership::SpaceAdmissionId::from_bytes(*record_id.as_bytes())
                .expect("non-zero legacy record id fixture"),
            target_space_id: "target-space".to_owned(),
            target_generation: [12; 16],
            target_keyslot_ref: vec![13],
            target_workspace_ref: vec![14],
            phase,
        })
    }

    #[test]
    fn join_record_advances_one_valid_space_transition_phase() {
        let record_id = SpaceJoinRecordId::from_bytes([15; 32]);
        let mut record =
            SpaceJoinRecord::new_joiner(record_id, [16; 16], JoinerAdmissionStage::Applied);
        record.completion = Some(vec![17]);
        let current = fresh_transition(record_id, FreshSpaceTransitionPhaseV1::TargetStaged);
        record.space_transition = current.encode();
        let next = fresh_transition(record_id, FreshSpaceTransitionPhaseV1::ActivationStarted);

        let advanced = record.advanced_space_transition(&current, &next).unwrap();

        assert_eq!(advanced.space_transition, next.encode());
        assert_eq!(advanced.record_version, record.record_version);
    }

    #[test]
    fn cleanup_result_completes_join_and_returns_verified_history() {
        let record_id = SpaceJoinRecordId::from_bytes([18; 32]);
        let mut record =
            SpaceJoinRecord::new_joiner(record_id, [19; 16], JoinerAdmissionStage::Applied);
        record.completion = Some(vec![20]);
        record.verified_membership_history = Some(vec![21]);
        let transition = fresh_transition(record_id, FreshSpaceTransitionPhaseV1::CleanupPending);
        record.space_transition = transition.encode();
        let result = AdmissionSpaceTransitionResultV2::Fresh {
            target_space_id: "target-space".to_owned(),
        };

        let (completed, history) = record
            .completed_space_transition(&transition, &result)
            .unwrap();

        assert_eq!(history, vec![21]);
        assert_eq!(completed.space_transition_result, result.encode());
        assert_eq!(completed.stage_rank(), Some(6));
        assert_eq!(
            completed.terminal_result,
            Some(AdmissionTerminalResult::Active)
        );
        assert_eq!(completed.record_version, record.record_version);
    }

    #[test]
    fn prepared_or_recovery_bound_join_cannot_be_superseded() {
        let attempt_id = SpaceJoinRecordId::from_bytes([11; 32]);
        let mut prepared =
            SpaceJoinRecord::new_joiner(attempt_id, [12; 16], JoinerAdmissionStage::Prepared);
        prepared.local_join_ordinal = Some(1);
        prepared.prepared_proof = Some(vec![1]);
        assert_eq!(
            prepared.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeSpaceJoinError::UnsafeStage)
        );

        let mut contradictory =
            SpaceJoinRecord::new_joiner(attempt_id, [12; 16], JoinerAdmissionStage::Candidate);
        contradictory.local_join_ordinal = Some(1);
        contradictory.prepared_proof = Some(vec![1]);
        let original = contradictory.clone();
        assert_eq!(
            contradictory.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeSpaceJoinError::RecoveryRequired)
        );
        assert_eq!(contradictory, original);
    }

    #[test]
    fn join_without_a_valid_initial_request_cannot_be_superseded() {
        let attempt_id = SpaceJoinRecordId::from_bytes([13; 32]);
        let mut attempt =
            SpaceJoinRecord::new_joiner(attempt_id, [14; 16], JoinerAdmissionStage::Initiated);
        attempt.local_join_ordinal = Some(1);
        attempt.joiner_pending_security_state = Some(vec![1]);
        attempt.candidate_key_package = Some(vec![2]);
        attempt.joiner_member_instance = Some(MemberInstanceId::from_bytes([3; 32]));
        attempt.resume_public_key = Some(vec![4; 32]);
        attempt.resume_private_key = Some(vec![5; 32]);

        assert_eq!(
            attempt.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeSpaceJoinError::RecoveryRequired)
        );
    }

    #[test]
    fn candidate_without_identity_binding_cannot_be_superseded() {
        let attempt_id = SpaceJoinRecordId::from_bytes([15; 32]);
        let mut attempt =
            SpaceJoinRecord::new_joiner(attempt_id, [16; 16], JoinerAdmissionStage::Candidate);
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
            Err(SupersedeSpaceJoinError::RecoveryRequired)
        );
    }

    #[test]
    fn unsafe_roles_stages_and_recovery_states_cannot_be_superseded() {
        let attempt_id = SpaceJoinRecordId::from_bytes([17; 32]);
        let mut base =
            SpaceJoinRecord::new_joiner(attempt_id, [18; 16], JoinerAdmissionStage::Initiated);
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
        sponsor.role_state = super::SpaceJoinRoleState::Sponsor(super::SponsorAdmissionState {
            stage: super::SponsorAdmissionStage::Accepted,
        });
        assert_eq!(
            sponsor.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeSpaceJoinError::NotJoiner)
        );

        let mut terminal = base.clone();
        terminal.terminal_result = Some(AdmissionTerminalResult::Rejected);
        assert_eq!(
            terminal.superseded_by_new_join(cancel_request(attempt_id)),
            Err(SupersedeSpaceJoinError::AlreadyTerminal)
        );

        for stage in [
            JoinerAdmissionStage::Prepared,
            JoinerAdmissionStage::Committed,
            JoinerAdmissionStage::Applied,
            JoinerAdmissionStage::Completed,
            JoinerAdmissionStage::Rejected,
            JoinerAdmissionStage::Superseded,
        ] {
            let mut unsafe_stage = base.clone();
            unsafe_stage.role_state =
                super::SpaceJoinRoleState::Joiner(super::JoinerAdmissionState { stage });
            assert_eq!(
                unsafe_stage.superseded_by_new_join(cancel_request(attempt_id)),
                Err(SupersedeSpaceJoinError::UnsafeStage)
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
                Err(SupersedeSpaceJoinError::RecoveryRequired)
            );
        }
    }

    #[test]
    fn space_join_record_without_continuation_address_still_decodes() {
        let attempt_id = SpaceJoinRecordId::from_bytes([19; 32]);
        let mut expected =
            SpaceJoinRecord::new_joiner(attempt_id, [20; 16], JoinerAdmissionStage::Initiated);
        expected.cancel_request = Some(vec![21]);
        expected.cancel_outcome = Some(vec![22]);
        expected.resume_public_key = Some(vec![23; 32]);
        expected.resume_private_key = Some(vec![24; 32]);
        expected.preserve_unreadable_history = true;

        let mut previous_version = postcard::to_stdvec(&expected).unwrap();
        assert_eq!(previous_version.pop(), Some(0));
        let decoded = SpaceJoinRecord::decode_persisted(&previous_version).unwrap();

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
            JoinerAdmissionStage::Initiated,
            JoinerAdmissionStage::Candidate,
            JoinerAdmissionStage::Prepared,
            JoinerAdmissionStage::Committed,
            JoinerAdmissionStage::Applied,
            JoinerAdmissionStage::Completed,
            JoinerAdmissionStage::Rejected,
            JoinerAdmissionStage::Superseded,
        ];
        for (index, stage) in joiner_stages.into_iter().enumerate() {
            assert_eq!(postcard::to_stdvec(&stage).unwrap(), vec![index as u8]);
        }

        let terminal_results = [
            AdmissionTerminalResult::Active,
            AdmissionTerminalResult::Completed,
            AdmissionTerminalResult::Rejected,
            AdmissionTerminalResult::SupersededByNewJoin,
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
