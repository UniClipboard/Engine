use uc_core::ids::DeviceId;
use uc_core::membership::{SpaceJoinRecord, SpaceJoinRecordId};

use crate::space::membership::{PeerReconciliationRecord, PendingMembershipEffect};

#[derive(Clone, PartialEq, Eq)]
pub struct AuthenticatedSpaceAdmissionMessage {
    pub source_device_id: DeviceId,
    pub record_id: uc_core::membership::SpaceJoinRecordId,
    pub message_id: [u8; 32],
    pub payload: Vec<u8>,
    pub invitation_code: Option<uc_core::pairing::invitation::InvitationCode>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SpaceAdmissionPreparationContext {
    pub invitation_generation: Option<u64>,
    pub membership_history_v2: Vec<u8>,
    pub current_record: Option<SpaceJoinRecord>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct PreparedSpaceAdmissionCommit {
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

pub struct LoadedMemberAdmissionActivation {
    current_record: Option<SpaceJoinRecord>,
    signed_membership_history: Vec<u8>,
    required_invitation_generation: u64,
    commit_token: MemberAdmissionCommitToken,
}

pub struct MemberAdmissionCommitToken {
    expected_revision: u64,
    expected_membership_history: Vec<u8>,
    record_id: SpaceJoinRecordId,
    expected_record_version: Option<u64>,
}

pub struct PreparedMemberAdmissionActivation {
    record: SpaceJoinRecord,
    signed_membership_history: Vec<u8>,
    relationship: PeerReconciliationRecord,
    effect: PendingMembershipEffect,
}

impl LoadedMemberAdmissionActivation {
    pub(in crate::space) fn new(
        current_record: Option<SpaceJoinRecord>,
        signed_membership_history: Vec<u8>,
        required_invitation_generation: u64,
        commit_token: MemberAdmissionCommitToken,
    ) -> Self {
        Self {
            current_record,
            signed_membership_history,
            required_invitation_generation,
            commit_token,
        }
    }

    pub(crate) fn has_current_record(&self) -> bool {
        self.current_record.is_some()
    }

    pub(crate) fn required_invitation_generation(&self) -> u64 {
        self.required_invitation_generation
    }

    pub(crate) fn preparation_context(
        &self,
        invitation_generation: Option<u64>,
    ) -> SpaceAdmissionPreparationContext {
        SpaceAdmissionPreparationContext {
            invitation_generation,
            membership_history_v2: self.signed_membership_history.clone(),
            current_record: self.current_record.clone(),
        }
    }

    pub(crate) fn into_commit_token(self) -> MemberAdmissionCommitToken {
        self.commit_token
    }
}

impl MemberAdmissionCommitToken {
    pub(in crate::space) fn new(
        expected_revision: u64,
        expected_membership_history: Vec<u8>,
        record_id: SpaceJoinRecordId,
        expected_record_version: Option<u64>,
    ) -> Self {
        Self {
            expected_revision,
            expected_membership_history,
            record_id,
            expected_record_version,
        }
    }

    pub(in crate::space) fn into_parts(self) -> (u64, Vec<u8>, SpaceJoinRecordId, Option<u64>) {
        (
            self.expected_revision,
            self.expected_membership_history,
            self.record_id,
            self.expected_record_version,
        )
    }
}

impl PreparedMemberAdmissionActivation {
    pub(crate) fn new(
        record: SpaceJoinRecord,
        signed_membership_history: Vec<u8>,
        relationship: PeerReconciliationRecord,
        effect: PendingMembershipEffect,
    ) -> Self {
        Self {
            record,
            signed_membership_history,
            relationship,
            effect,
        }
    }

    pub(in crate::space) fn into_parts(
        self,
    ) -> (
        SpaceJoinRecord,
        Vec<u8>,
        PeerReconciliationRecord,
        PendingMembershipEffect,
    ) {
        (
            self.record,
            self.signed_membership_history,
            self.relationship,
            self.effect,
        )
    }
}

impl std::fmt::Debug for AuthenticatedSpaceAdmissionMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthenticatedSpaceAdmissionMessage")
            .field("source_device_id", &"[REDACTED]")
            .field("record_id", &"[REDACTED]")
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
