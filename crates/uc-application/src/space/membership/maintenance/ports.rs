use async_trait::async_trait;

use super::{
    AdmissionMaintenanceOutcome, MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger,
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
pub trait RecoverMembershipEffectsPort: Send + Sync {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome;
}

#[async_trait]
pub trait RecoverMembershipConflictsPort: Send + Sync {
    async fn recover_membership_conflicts(&self) -> MembershipMaintenanceStepOutcome;
}

#[async_trait]
pub trait DeliverRestrictedMembershipPort: Send + Sync {
    async fn deliver_restricted_membership(&self) -> MembershipMaintenanceStepOutcome;
}

#[async_trait]
pub trait DeliverPendingGroupUpdatesPort: Send + Sync {
    async fn deliver_pending_group_updates(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome;
}

#[async_trait]
pub trait SynchronizeMembershipMaintenancePort: Send + Sync {
    async fn periodic_synchronization_required(
        &self,
    ) -> Result<bool, MembershipMaintenanceStepOutcome>;

    async fn synchronize_membership(
        &self,
        trigger: &MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceStepOutcome;
}

#[async_trait]
pub trait ReconcileMembershipProjectionPort: Send + Sync {
    async fn reconcile_membership_projection(&self) -> MembershipMaintenanceStepOutcome;
}
