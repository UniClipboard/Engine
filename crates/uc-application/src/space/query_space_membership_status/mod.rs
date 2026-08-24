mod active_space_status;
mod deps;
mod error;
mod model;
mod use_case;

pub use error::QuerySpaceMembershipStatusError;
pub use model::{
    ActionUnavailableReason, DeviceCompatibility, DeviceMembership, GroupRelationship,
    PendingSpaceMembershipChange, RecoveryAvailability, SpaceMemberRelationship,
    SpaceMembershipAction, SpaceMembershipChangeChoice, SpaceMembershipChangeImpact,
    SpaceMembershipStatus, SyncRelationship,
};
pub(crate) use use_case::QuerySpaceMembershipStatusUseCase;

pub(crate) use active_space_status::{build_active_space_status, ActiveSpaceStatusFacts};
pub(crate) use deps::{ActiveSpaceMembershipStatusDeps, QuerySpaceMembershipStatusDeps};

pub(crate) struct ActiveSpaceStatusResult {
    pub(crate) space_lineage: String,
    pub(crate) status: SpaceMembershipStatus,
}
