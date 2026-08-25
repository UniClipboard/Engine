use uc_core::ids::DeviceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipMaintenanceTrigger {
    Startup,
    Resume,
    Periodic,
    StateChanged,
    PeerOnline(DeviceId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipMaintenanceStepOutcome {
    Completed,
    Deferred,
    StableFailure,
    Corrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MembershipMaintenanceReport {
    pub completed_count: usize,
    pub deferred_count: usize,
    pub stable_failure_count: usize,
    pub corrupt_count: usize,
}
