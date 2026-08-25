use uc_core::membership::MembershipEventId;

use crate::space::query_device_trust::DeviceTrustStatus;

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
