mod model;
mod ports;
mod runtime;
mod use_case;

pub use model::{
    MembershipMaintenanceReport, MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger,
};
pub use ports::{
    CleanupLegacyMembershipDataPort, DeliverPendingGroupUpdatesPort,
    DeliverRestrictedMembershipPort, RecoverMembershipEffectsPort, RecoverSpaceAdmissionsPort,
    SynchronizeMembershipMaintenancePort, WakeSpaceMembershipMaintenancePort,
};
pub use runtime::MembershipNetworkActivityPort;
pub(crate) use runtime::{
    PreparedSpaceMembershipMaintenanceRuntime, SpaceMembershipMaintenanceActivity,
    SpaceMembershipMaintenanceRuntime,
};
pub(crate) use use_case::{MaintainSpaceMembershipDeps, MaintainSpaceMembershipUseCase};

#[cfg(test)]
mod tests;
