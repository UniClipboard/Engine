use std::future::Future;
use std::sync::Arc;
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

use super::{
    AcquireSpaceWorkPermitPort, DeliverPendingGroupUpdatesPort, DeliverRestrictedMembershipPort,
    MembershipMaintenanceReport, MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger,
    QuerySpaceWorkModeError, ReconcileMembershipProjectionPort, RecoverMembershipConflictsPort,
    RecoverMembershipEffectsPort, RecoverSpaceAdmissionsPort, SpaceWorkMode,
    SynchronizeMembershipMaintenancePort,
};

pub(crate) struct MaintainSpaceMembershipDeps {
    pub admissions: Arc<dyn RecoverSpaceAdmissionsPort>,
    pub effects: Arc<dyn RecoverMembershipEffectsPort>,
    pub conflicts: Arc<dyn RecoverMembershipConflictsPort>,
    pub group_update_delivery: Arc<dyn DeliverPendingGroupUpdatesPort>,
    pub restricted_delivery: Arc<dyn DeliverRestrictedMembershipPort>,
    pub synchronization: Arc<dyn SynchronizeMembershipMaintenancePort>,
    pub cleanup: Arc<dyn ReconcileMembershipProjectionPort>,
}

pub(crate) struct MaintainSpaceMembershipUseCase {
    deps: MaintainSpaceMembershipDeps,
    execution_lock: tokio::sync::Mutex<()>,
    work_permit: Option<Arc<dyn AcquireSpaceWorkPermitPort>>,
}

impl MaintainSpaceMembershipUseCase {
    #[cfg(test)]
    pub(crate) fn new(deps: MaintainSpaceMembershipDeps) -> Self {
        Self {
            deps,
            execution_lock: tokio::sync::Mutex::new(()),
            work_permit: None,
        }
    }

    pub(crate) fn new_coordinated(
        deps: MaintainSpaceMembershipDeps,
        work_permit: Arc<dyn AcquireSpaceWorkPermitPort>,
    ) -> Self {
        Self {
            deps,
            execution_lock: tokio::sync::Mutex::new(()),
            work_permit: Some(work_permit),
        }
    }

    pub(crate) async fn execute(
        &self,
        trigger: MembershipMaintenanceTrigger,
    ) -> MembershipMaintenanceReport {
        let waiting = LocalWorkObservation::begin(LocalWorkStep::MaintenanceLock);
        let _guard = self.execution_lock.lock().await;
        waiting.finish(LocalWorkOutcome::Ok);
        let mut report = MembershipMaintenanceReport::default();
        let full_round = !matches!(trigger, MembershipMaintenanceTrigger::Periodic);
        let periodic = matches!(trigger, MembershipMaintenanceTrigger::Periodic);

        let observation = LocalWorkObservation::begin(LocalWorkStep::MaintenanceAdmissions);
        let admissions = self
            .deps
            .admissions
            .recover_space_admissions(&trigger)
            .await;
        observation.finish(diagnostic_outcome(admissions.step()));
        if !record_outcome(&mut report, admissions.step())
            || !admissions.allows_ordinary_membership()
        {
            return report;
        }
        let _work_permit = if let Some(work_permit) = self.work_permit.as_ref() {
            match work_permit.acquire_space_work_permit().await {
                Ok(permit) if permit.mode() == SpaceWorkMode::Active => Some(permit),
                Ok(_) => return report,
                Err(QuerySpaceWorkModeError::Unavailable) => {
                    report.deferred_count += 1;
                    return report;
                }
                Err(QuerySpaceWorkModeError::NeedsAttention) => {
                    report.corrupt_count += 1;
                    return report;
                }
            }
        } else {
            None
        };
        if !record(
            &mut report,
            LocalWorkStep::MaintenanceRestricted,
            self.deps
                .restricted_delivery
                .deliver_restricted_membership(),
        )
        .await
        {
            return report;
        }
        if !record(
            &mut report,
            LocalWorkStep::MaintenanceEffects,
            self.deps.effects.recover_membership_effects(),
        )
        .await
        {
            return report;
        }
        if !record(
            &mut report,
            LocalWorkStep::MaintenanceConflicts,
            self.deps.conflicts.recover_membership_conflicts(),
        )
        .await
        {
            return report;
        }
        if !record(
            &mut report,
            LocalWorkStep::MaintenanceGroupUpdates,
            self.deps
                .group_update_delivery
                .deliver_pending_group_updates(&trigger),
        )
        .await
        {
            return report;
        }
        let should_synchronize = if periodic {
            let observation =
                LocalWorkObservation::begin(LocalWorkStep::MaintenanceSynchronizationCheck);
            let required = self
                .deps
                .synchronization
                .periodic_synchronization_required()
                .await;
            observation.finish(match &required {
                Ok(_) => LocalWorkOutcome::Ok,
                Err(outcome) => diagnostic_outcome(*outcome),
            });
            match required {
                Ok(required) => required,
                Err(outcome) => {
                    record_outcome(&mut report, outcome);
                    return report;
                }
            }
        } else {
            full_round
        };
        if should_synchronize {
            if !record(
                &mut report,
                LocalWorkStep::MaintenanceSynchronization,
                self.deps.synchronization.synchronize_membership(&trigger),
            )
            .await
            {
                return report;
            }
        }
        record(
            &mut report,
            LocalWorkStep::MaintenanceCleanup,
            self.deps.cleanup.reconcile_membership_projection(),
        )
        .await;
        report
    }
}

async fn record(
    report: &mut MembershipMaintenanceReport,
    step: LocalWorkStep,
    work: impl Future<Output = MembershipMaintenanceStepOutcome>,
) -> bool {
    let observation = LocalWorkObservation::begin(step);
    let outcome = work.await;
    observation.finish(diagnostic_outcome(outcome));
    record_outcome(report, outcome)
}

fn diagnostic_outcome(outcome: MembershipMaintenanceStepOutcome) -> LocalWorkOutcome {
    match outcome {
        MembershipMaintenanceStepOutcome::Completed => LocalWorkOutcome::Ok,
        MembershipMaintenanceStepOutcome::Deferred => LocalWorkOutcome::Deferred,
        MembershipMaintenanceStepOutcome::StableFailure => LocalWorkOutcome::Error,
        MembershipMaintenanceStepOutcome::Corrupt => LocalWorkOutcome::Corrupt,
    }
}

fn record_outcome(
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
