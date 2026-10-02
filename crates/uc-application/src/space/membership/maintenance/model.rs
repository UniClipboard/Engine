use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;
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

/// 准入动作等待独占执行时发出的让位请求；普通成员工作许可的持有者据此在本轮中途放弃许可。
///
/// 让位只由准入负责人发出：请求存续期间 [`SpaceWorkPermit::preempted`] 立即完成，请求结束后恢复挂起。
#[derive(Clone, Default)]
pub(crate) struct WorkPreemption {
    state: Arc<PreemptionState>,
}

#[derive(Default)]
struct PreemptionState {
    waiting: AtomicUsize,
    changed: Notify,
}

/// 一次让位请求；丢弃时撤销。
pub(crate) struct PreemptionRequest {
    state: Arc<PreemptionState>,
}

impl WorkPreemption {
    pub(crate) fn request(&self) -> PreemptionRequest {
        self.state.waiting.fetch_add(1, Ordering::SeqCst);
        self.state.changed.notify_waiters();
        PreemptionRequest {
            state: Arc::clone(&self.state),
        }
    }

    async fn requested(&self) {
        loop {
            let changed = self.state.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.state.waiting.load(Ordering::SeqCst) > 0 {
                return;
            }
            changed.await;
        }
    }
}

impl Drop for PreemptionRequest {
    fn drop(&mut self) {
        self.state.waiting.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct SpaceWorkPermit {
    mode: SpaceWorkMode,
    preemption: WorkPreemption,
    _guard: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
}

impl SpaceWorkPermit {
    pub(crate) fn guarded(
        mode: SpaceWorkMode,
        guard: tokio::sync::OwnedRwLockReadGuard<()>,
        preemption: WorkPreemption,
    ) -> Self {
        Self {
            mode,
            preemption,
            _guard: Some(guard),
        }
    }

    #[cfg(test)]
    pub(crate) fn unlocked(mode: SpaceWorkMode) -> Self {
        Self {
            mode,
            preemption: WorkPreemption::default(),
            _guard: None,
        }
    }

    pub const fn mode(&self) -> SpaceWorkMode {
        self.mode
    }

    /// 准入动作正在等待本许可释放时完成；持有者应在下一个安全点结束本轮并交还许可。
    pub(crate) async fn preempted(&self) {
        self.preemption.requested().await;
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

/// 成员维护互斥许可：存续期间不会有维护轮次运行；取得许可前进行中的轮次已完整结束。
pub(crate) struct MembershipMaintenanceExclusion {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

impl MembershipMaintenanceExclusion {
    pub(crate) fn new(guard: tokio::sync::OwnedMutexGuard<()>) -> Self {
        Self { _guard: guard }
    }
}
