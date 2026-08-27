use std::sync::Arc;

use super::{
    CleanupLegacyMembershipDataPort, DeliverRestrictedMembershipPort, MembershipMaintenanceReport,
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverMembershipEffectsPort,
    RecoverSpaceAdmissionsPort, SynchronizeMembershipMaintenancePort,
};

pub(crate) struct MaintainSpaceMembershipDeps {
    pub admissions: Arc<dyn RecoverSpaceAdmissionsPort>,
    pub effects: Arc<dyn RecoverMembershipEffectsPort>,
    pub restricted_delivery: Arc<dyn DeliverRestrictedMembershipPort>,
    pub synchronization: Arc<dyn SynchronizeMembershipMaintenancePort>,
    pub cleanup: Arc<dyn CleanupLegacyMembershipDataPort>,
}

pub(crate) struct MaintainSpaceMembershipUseCase {
    deps: MaintainSpaceMembershipDeps,
    execution_lock: tokio::sync::Mutex<()>,
}

impl MaintainSpaceMembershipUseCase {
    pub(crate) fn new(deps: MaintainSpaceMembershipDeps) -> Self {
        Self {
            deps,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        trigger: MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceReport {
        let _guard = self.execution_lock.lock().await;
        let mut report = MembershipMaintenanceReport::default();
        let full_round = matches!(
            trigger,
            MembershipMaintenanceTrigger::Startup
                | MembershipMaintenanceTrigger::Resume
                | MembershipMaintenanceTrigger::StateChanged
        );
        let peer_online = matches!(trigger, MembershipMaintenanceTrigger::PeerOnline(_));
        let periodic = matches!(trigger, MembershipMaintenanceTrigger::Periodic);

        if !peer_online
            && !record(
                &mut report,
                self.deps
                    .admissions
                    .recover_space_admissions(&trigger)
                    .await,
            )
        {
            return report;
        }
        if !peer_online
            && !record(
                &mut report,
                self.deps.effects.recover_membership_effects().await,
            )
        {
            return report;
        }
        if !record(
            &mut report,
            self.deps
                .restricted_delivery
                .deliver_restricted_membership()
                .await,
        ) {
            return report;
        }
        let should_synchronize = if periodic {
            match self
                .deps
                .synchronization
                .periodic_synchronization_required()
                .await
            {
                Ok(required) => required,
                Err(outcome) => {
                    record(&mut report, outcome);
                    return report;
                }
            }
        } else {
            full_round || peer_online
        };
        if should_synchronize {
            if !record(
                &mut report,
                self.deps
                    .synchronization
                    .synchronize_membership(&trigger)
                    .await,
            ) {
                return report;
            }
        }
        if full_round {
            record(
                &mut report,
                self.deps.cleanup.cleanup_legacy_membership_data().await,
            );
        }
        report
    }
}

fn record(
    report: &mut MembershipMaintenanceReport,
    outcome: MembershipMaintenanceStepOutcome,
) -> bool {
    match outcome {
        MembershipMaintenanceStepOutcome::Completed => report.completed_count += 1,
        MembershipMaintenanceStepOutcome::Deferred => report.deferred_count += 1,
        MembershipMaintenanceStepOutcome::StableFailure => report.stable_failure_count += 1,
        MembershipMaintenanceStepOutcome::Corrupt => {
            report.corrupt_count += 1;
            return false;
        }
    }
    true
}
