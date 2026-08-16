use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const ADMISSION_ATTEMPT_FORMAT_V1: u16 = 1;
pub const ADMISSION_PROFILE_METADATA_FORMAT_V1: u16 = 1;
pub const TERMINAL_ADMISSION_ATTEMPT_FORMAT_V1: u16 = 1;

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
    pub staged_security_state: Option<Vec<u8>>,
    pub invitation_claim: Option<Vec<u8>>,
    pub space_transition: Option<Vec<u8>>,
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
    pub write_ahead_recovery: Option<Vec<u8>>,
    pub cleanup_pending: bool,
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
            staged_security_state: None,
            invitation_claim: None,
            space_transition: None,
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
            write_ahead_recovery: None,
            cleanup_pending: false,
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
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalAdmissionAttemptV1 {
    pub format_version: u16,
    pub attempt_id: AdmissionAttemptId,
    pub join_id: Option<[u8; 16]>,
    pub local_join_ordinal: Option<u64>,
    pub invitation_digest: Option<[u8; 32]>,
    pub identity_binding: Vec<u8>,
    pub terminal_result: AdmissionTerminalResultV1,
    pub candidate_event_id: Option<[u8; 32]>,
    pub cancel_outcome: Option<Vec<u8>>,
    pub replay_result: Vec<u8>,
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
