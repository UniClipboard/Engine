mod model;
mod ports;
mod runtime;
mod use_case;

pub use model::{
    AdmissionMaintenanceOutcome, KnownPeerContact, MembershipMaintenanceReport,
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, QuerySpaceWorkModeError,
    SpaceWorkMode, SpaceWorkPermit,
};
pub(crate) use ports::RunMembershipWorkPort;
pub use ports::{
    AcquireSpaceWorkPermitPort, DeliverPendingGroupUpdatesPort, RecoverMembershipConflictsPort,
    RecoverMembershipEffectsPort, RecoverSpaceAdmissionsPort, WakeSpaceMembershipMaintenancePort,
};
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
