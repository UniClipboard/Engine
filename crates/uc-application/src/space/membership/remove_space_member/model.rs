use uc_core::membership::{AdmissionMemberBindingV2, MembershipEventId, SpaceAdmissionId};

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
