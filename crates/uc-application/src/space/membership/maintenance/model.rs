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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipMaintenanceStepOutcome {
    Completed,
    Deferred,
    StableFailure,
    Corrupt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpaceWorkMode {
    Pairing,
    #[default]
    Active,
    NeedsAttention,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuerySpaceWorkModeError {
    #[error("space work state is temporarily unavailable")]
    Unavailable,
    #[error("space work state requires recovery")]
    NeedsAttention,
}

pub struct SpaceWorkPermit {
    mode: SpaceWorkMode,
    _guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl SpaceWorkPermit {
    pub(crate) fn guarded(mode: SpaceWorkMode, guard: tokio::sync::OwnedMutexGuard<()>) -> Self {
        Self {
            mode,
            _guard: Some(guard),
        }
    }

    #[cfg(test)]
    pub(crate) fn unlocked(mode: SpaceWorkMode) -> Self {
        Self { mode, _guard: None }
    }

    pub const fn mode(&self) -> SpaceWorkMode {
        self.mode
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionMaintenanceOutcome {
    mode: SpaceWorkMode,
    step: MembershipMaintenanceStepOutcome,
}

impl AdmissionMaintenanceOutcome {
    pub const fn new(mode: SpaceWorkMode, step: MembershipMaintenanceStepOutcome) -> Self {
        Self { mode, step }
    }

    pub const fn step(self) -> MembershipMaintenanceStepOutcome {
        self.step
    }

    pub const fn allows_ordinary_membership(self) -> bool {
        matches!(self.mode, SpaceWorkMode::Active)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MembershipMaintenanceReport {
    pub completed_count: usize,
    pub deferred_count: usize,
    pub stable_failure_count: usize,
    pub corrupt_count: usize,
}
