use async_trait::async_trait;

use super::{MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger};

#[async_trait]
pub trait RecoverSpaceAdmissionsPort: Send + Sync {
    async fn recover_space_admissions(&self) -> MembershipMaintenanceStepOutcome;
}

#[async_trait]
pub trait RecoverMembershipEffectsPort: Send + Sync {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome;
}

#[async_trait]
pub trait DeliverRestrictedMembershipPort: Send + Sync {
    async fn deliver_restricted_membership(&self) -> MembershipMaintenanceStepOutcome;
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
pub trait CleanupLegacyMembershipDataPort: Send + Sync {
    async fn cleanup_legacy_membership_data(&self) -> MembershipMaintenanceStepOutcome;
}
