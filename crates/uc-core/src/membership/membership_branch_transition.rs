use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{MembershipBranchId, MembershipConflictId};

pub const MEMBERSHIP_BRANCH_TRANSITION_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipBranchTransitionPhaseV1 {
    Prepared,
    SourceBackedUp,
    TargetVerified,
    TargetStaged,
    Promoted,
    RuntimeRestored,
    Completed,
}

impl MembershipBranchTransitionPhaseV1 {
    pub const fn rank(self) -> u8 {
        match self {
            Self::Prepared => 0,
            Self::SourceBackedUp => 1,
            Self::TargetVerified => 2,
            Self::TargetStaged => 3,
            Self::Promoted => 4,
            Self::RuntimeRestored => 5,
            Self::Completed => 6,
        }
    }

    const fn successor(self) -> Option<Self> {
        match self {
            Self::Prepared => Some(Self::SourceBackedUp),
            Self::SourceBackedUp => Some(Self::TargetVerified),
            Self::TargetVerified => Some(Self::TargetStaged),
            Self::TargetStaged => Some(Self::Promoted),
            Self::Promoted => Some(Self::RuntimeRestored),
            Self::RuntimeRestored => Some(Self::Completed),
            Self::Completed => None,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipBranchTransitionV1 {
    format_version: u16,
    transition_id: [u8; 32],
    conflict_id: MembershipConflictId,
    target_branch_id: MembershipBranchId,
    source_generation: [u8; 16],
    target_generation: [u8; 16],
    phase: MembershipBranchTransitionPhaseV1,
}

impl MembershipBranchTransitionV1 {
    pub fn derive_id(
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/membership-branch-transition/v1\0");
        hasher.update(conflict_id.as_bytes());
        hasher.update(target_branch_id.as_bytes());
        hasher.finalize().into()
    }

    pub fn new(
        transition_id: [u8; 32],
        conflict_id: MembershipConflictId,
        target_branch_id: MembershipBranchId,
        source_generation: [u8; 16],
        target_generation: [u8; 16],
    ) -> Option<Self> {
        let transition = Self {
            format_version: MEMBERSHIP_BRANCH_TRANSITION_FORMAT_V1,
            transition_id,
            conflict_id,
            target_branch_id,
            source_generation,
            target_generation,
            phase: MembershipBranchTransitionPhaseV1::Prepared,
        };
        transition.validate().then_some(transition)
    }

    pub fn validate(&self) -> bool {
        self.format_version == MEMBERSHIP_BRANCH_TRANSITION_FORMAT_V1
            && self.transition_id != [0; 32]
            && self.source_generation != self.target_generation
    }

    pub const fn phase(&self) -> MembershipBranchTransitionPhaseV1 {
        self.phase
    }

    pub fn advance(&self, phase: MembershipBranchTransitionPhaseV1) -> Option<Self> {
        if !self.validate() || self.phase.successor() != Some(phase) {
            return None;
        }
        let mut next = self.clone();
        next.phase = phase;
        next.validate().then_some(next)
    }

    pub const fn transition_id(&self) -> &[u8; 32] {
        &self.transition_id
    }

    pub const fn conflict_id(&self) -> MembershipConflictId {
        self.conflict_id
    }

    pub const fn target_branch_id(&self) -> MembershipBranchId {
        self.target_branch_id
    }

    pub const fn source_generation(&self) -> &[u8; 16] {
        &self.source_generation
    }

    pub const fn target_generation(&self) -> &[u8; 16] {
        &self.target_generation
    }
}

impl std::fmt::Debug for MembershipBranchTransitionV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipBranchTransitionV1")
            .field("identifiers", &"[REDACTED]")
            .field("phase", &self.phase)
            .finish()
    }
}
