mod model;
mod ports;
mod runtime;
mod use_case;

pub(crate) use model::MembershipMaintenanceExclusion;
pub use model::{
    AdmissionMaintenanceOutcome, KnownPeerContact, MembershipMaintenanceReport,
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, QuerySpaceWorkModeError,
    SpaceWorkMode, SpaceWorkPermit,
};
pub use ports::{
    AcquireSpaceWorkPermitPort, DeliverPendingGroupUpdatesPort, RecoverMembershipConflictsPort,
    RecoverMembershipEffectsPort, RecoverSpaceAdmissionsPort, WakeSpaceMembershipMaintenancePort,
};
pub(crate) use ports::{ExcludeMembershipMaintenancePort, RunMembershipWorkPort};
pub use runtime::MembershipNetworkActivityPort;
pub(crate) use runtime::{
    PreparedSpaceMembershipMaintenanceRuntime, SpaceMembershipMaintenanceActivity,
    SpaceMembershipMaintenanceRuntime,
};
pub(crate) use use_case::{MaintainSpaceMembershipDeps, MaintainSpaceMembershipUseCase};

#[cfg(test)]
mod diagnostic_tests;
#[cfg(test)]
mod tests;
