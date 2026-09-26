use async_trait::async_trait;
use uc_core::ids::DeviceId;

use super::{
    AdmissionMaintenanceOutcome, MembershipMaintenanceReport, MembershipMaintenanceStepOutcome,
    MembershipMaintenanceTrigger, QuerySpaceWorkModeError, SpaceWorkPermit,
};

pub trait WakeSpaceMembershipMaintenancePort: Send + Sync {
    fn wake(&self);

    fn schedule_at(&self, expires_at_ms: i64, now_ms: i64);
}

impl WakeSpaceMembershipMaintenancePort for super::SpaceMembershipMaintenanceActivity {
    fn wake(&self) {
        let _ = self.request_state_changed();
    }

    fn schedule_at(&self, expires_at_ms: i64, now_ms: i64) {
        let remaining_ms = expires_at_ms.saturating_sub(now_ms).max(0) as u64;
        let _ = self.request_deadline(std::time::Duration::from_millis(remaining_ms));
    }
}

#[async_trait]
pub trait RecoverSpaceAdmissionsPort: Send + Sync {
    async fn recover_space_admissions(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> AdmissionMaintenanceOutcome;
}

#[async_trait]
pub trait AcquireSpaceWorkPermitPort: Send + Sync {
    async fn acquire_space_work_permit(&self) -> Result<SpaceWorkPermit, QuerySpaceWorkModeError>;
}

#[async_trait]
pub trait RecoverMembershipEffectsPort: Send + Sync {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome;
}

#[async_trait]
pub trait RecoverMembershipConflictsPort: Send + Sync {
    async fn recover_membership_conflicts(&self) -> MembershipMaintenanceStepOutcome;
}

/// 投递已到期的组密钥更新。`reachable_peers` 是刚与本机成功交换成员历史的对端，发给它们的更新不必等
/// 投递退避到期。
#[async_trait]
pub trait DeliverPendingGroupUpdatesPort: Send + Sync {
    async fn deliver_pending_group_updates(
        &self,
        trigger: &MembershipMaintenanceTrigger,
        reachable_peers: &[DeviceId],
    ) -> MembershipMaintenanceStepOutcome;
}

/// 执行一轮已到期的普通成员待办；由成员待办执行器实现，维护运行期只负责何时调用。
#[async_trait]
pub(crate) trait RunMembershipWorkPort: Send + Sync {
    async fn run_membership_work(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceReport;
}
