use uc_core::ids::DeviceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPeerContact {
    pub device_id: DeviceId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipMaintenanceTrigger {
    Startup,
    Resume,
    Periodic,
    StateChanged,
    PeerContact(DeviceId),
    PeerOnline(DeviceId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipMaintenanceStepOutcome {
    Completed,
    Deferred,
    StableFailure,
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionMaintenanceOutcome {
    /// 准入工作结束，本轮可以继续普通成员维护
    Continue(MembershipMaintenanceStepOutcome),
    /// 准入流程仍需独占本轮，普通成员维护留到后续执行
    Yield(MembershipMaintenanceStepOutcome),
}

impl AdmissionMaintenanceOutcome {
    pub const fn step(self) -> MembershipMaintenanceStepOutcome {
        match self {
            Self::Continue(step) | Self::Yield(step) => step,
        }
    }

    pub const fn should_continue(self) -> bool {
        matches!(self, Self::Continue(_))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MembershipMaintenanceReport {
    pub completed_count: usize,
    pub deferred_count: usize,
    pub stable_failure_count: usize,
    pub corrupt_count: usize,
}
