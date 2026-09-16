use uc_core::membership::{
    AdmissionMemberBindingV2, MemberInstanceId, MembershipEventId, SpaceAdmissionId,
};

use crate::space::membership::DeviceTrustStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipCommitReceipt {
    pub revision: u64,
    pub history_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveSpaceMemberResult {
    pub change_id: MembershipEventId,
    pub commit: MembershipCommitReceipt,
    pub status: DeviceTrustStatus,
}

#[derive(Debug, Clone)]
pub struct AdmissionRevocationTarget {
    admission_id: SpaceAdmissionId,
    member_binding: AdmissionMemberBindingV2,
}

#[derive(Debug, Clone, Copy)]
pub struct AdmissionAbandonmentRevocationTarget {
    admission_id: SpaceAdmissionId,
    attempt_digest: [u8; 32],
    member_instance_id: MemberInstanceId,
    add_event_id: MembershipEventId,
}

impl AdmissionAbandonmentRevocationTarget {
    pub const fn new(
        admission_id: SpaceAdmissionId,
        attempt_digest: [u8; 32],
        member_instance_id: MemberInstanceId,
        add_event_id: MembershipEventId,
    ) -> Self {
        Self {
            admission_id,
            attempt_digest,
            member_instance_id,
            add_event_id,
        }
    }

    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.admission_id
    }

    pub const fn attempt_digest(&self) -> [u8; 32] {
        self.attempt_digest
    }

    pub const fn member_instance_id(&self) -> MemberInstanceId {
        self.member_instance_id
    }

    pub const fn add_event_id(&self) -> MembershipEventId {
        self.add_event_id
    }
}

impl AdmissionRevocationTarget {
    pub const fn new(
        admission_id: SpaceAdmissionId,
        member_binding: AdmissionMemberBindingV2,
    ) -> Self {
        Self {
            admission_id,
            member_binding,
        }
    }

    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.admission_id
    }

    pub const fn member_binding(&self) -> &AdmissionMemberBindingV2 {
        &self.member_binding
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRevocationResult {
    Removed { change_id: MembershipEventId },
    AlreadyAbsent { change_id: MembershipEventId },
    LocalEffectsPending { change_id: MembershipEventId },
}
