use std::sync::Arc;

use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

use super::{
    AcquireSpaceWorkPermitPort, MembershipMaintenanceReport, MembershipMaintenanceStepOutcome,
    MembershipMaintenanceTrigger, QuerySpaceWorkModeError, RecoverSpaceAdmissionsPort,
    RunMembershipWorkPort, SpaceWorkMode,
};
use crate::space::membership::worker::record_outcome;

pub(crate) struct MaintainSpaceMembershipDeps {
    pub admissions: Arc<dyn RecoverSpaceAdmissionsPort>,
    pub work: Arc<dyn RunMembershipWorkPort>,
}

/// 一轮成员维护：先恢复准入并取得普通成员工作许可，再交给执行器完成全部已到期待办。
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

        let observation = LocalWorkObservation::begin(LocalWorkStep::MaintenanceAdmissions);
        let admissions = self
            .deps
            .admissions
            .recover_space_admissions(&trigger)
            .await;
        observation.finish(diagnostic_outcome(admissions.step()));
        record_outcome(&mut report, admissions.step());
        if !admissions.allows_ordinary_membership() {
            return report;
        }
        // 配对期间普通成员工作暂停；许可在整轮执行期间保持，避免与配对交错。
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
        let worked = self.deps.work.run_membership_work(&trigger).await;
        report.completed_count += worked.completed_count;
        report.deferred_count += worked.deferred_count;
        report.stable_failure_count += worked.stable_failure_count;
        report.corrupt_count += worked.corrupt_count;
        report
    }
}

fn diagnostic_outcome(outcome: MembershipMaintenanceStepOutcome) -> LocalWorkOutcome {
    match outcome {
        MembershipMaintenanceStepOutcome::Completed => LocalWorkOutcome::Ok,
        MembershipMaintenanceStepOutcome::Deferred => LocalWorkOutcome::Deferred,
        MembershipMaintenanceStepOutcome::StableFailure => LocalWorkOutcome::Error,
        MembershipMaintenanceStepOutcome::Corrupt => LocalWorkOutcome::Corrupt,
    }
}
