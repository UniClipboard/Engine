use uc_core::ids::DeviceId;
use uc_core::membership::SpaceJoinRecord;

use crate::space::membership::{PeerReconciliationRecord, PendingMembershipEffect};

#[derive(Clone, PartialEq, Eq)]
pub struct AuthenticatedSpaceAdmissionMessage {
    pub source_device_id: DeviceId,
    pub attempt_id: uc_core::membership::SpaceJoinRecordId,
    pub message_id: [u8; 32],
    pub payload: Vec<u8>,
    pub invitation_code: Option<uc_core::pairing::invitation::InvitationCode>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SpaceAdmissionPreparationContext {
    pub revision: u64,
    pub invitation_generation: Option<u64>,
    pub membership_history_v2: Vec<u8>,
    pub current_record: Option<SpaceJoinRecord>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PreparedSpaceAdmissionCommit {
    pub expected_record_version: Option<u64>,
    pub invitation_generation: Option<u64>,
    pub record: SpaceJoinRecord,
    pub membership_history_v2: Vec<u8>,
    pub relationship: PeerReconciliationRecord,
    pub effect: PendingMembershipEffect,
    pub reply: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq)]
pub enum PreparedSpaceAdmissionMessage {
    NoChange { reply: Vec<u8> },
    Commit(PreparedSpaceAdmissionCommit),
}

impl std::fmt::Debug for AuthenticatedSpaceAdmissionMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticatedSpaceAdmissionMessage")
            .field("source_device_id", &"[REDACTED]")
            .field("attempt_id", &"[REDACTED]")
            .field("message_id", &"[REDACTED]")
            .field("payload_len", &self.payload.len())
            .field("has_invitation_code", &self.invitation_code.is_some())
            .finish()
    }
}

impl std::fmt::Debug for SpaceAdmissionPreparationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpaceAdmissionPreparationContext")
            .field("revision", &self.revision)
            .field("invitation_generation", &self.invitation_generation)
            .field("membership_history_len", &self.membership_history_v2.len())
            .field("has_current_record", &self.current_record.is_some())
            .finish()
    }
}

impl std::fmt::Debug for PreparedSpaceAdmissionCommit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreparedSpaceAdmissionCommit")
            .field("expected_record_version", &self.expected_record_version)
            .field("invitation_generation", &self.invitation_generation)
            .field("membership_history_len", &self.membership_history_v2.len())
            .field("reply_len", &self.reply.len())
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for PreparedSpaceAdmissionMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoChange { reply } => formatter
                .debug_struct("PreparedSpaceAdmissionMessage::NoChange")
                .field("reply_len", &reply.len())
                .finish(),
            Self::Commit(commit) => commit.fmt(formatter),
        }
    }
}
