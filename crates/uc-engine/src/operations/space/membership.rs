//! 成员就绪状态查询；内部诊断仅在 `dev-tools` 下编译。

#[cfg(feature = "dev-tools")]
use uc_application::facade::QueryMembershipDiagnosticsError;
use uc_application::facade::{AppFacade, MembershipReadiness, QueryMembershipReadinessError};
#[cfg(feature = "dev-tools")]
use uc_core::membership::MembershipBranchTransitionPhaseV1;

use crate::error_codes::QUERY_MEMBERSHIP_READINESS_UNAVAILABLE_CODE;
#[cfg(feature = "dev-tools")]
use crate::MembershipDiagnosticsSummary;
use crate::{
    EngineError, EngineErrorCategory, MembershipReadinessStateSummary, MembershipReadinessSummary,
    OperationResult,
};

pub async fn execute_query_membership_readiness(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let state = facade
        .query_membership_readiness()
        .await
        .map_err(map_readiness_error)?;
    let state = match state {
        MembershipReadiness::Ready => MembershipReadinessStateSummary::Ready,
        MembershipReadiness::Locked => MembershipReadinessStateSummary::Locked,
        MembershipReadiness::Recovering => MembershipReadinessStateSummary::Recovering,
    };
    Ok(OperationResult::MembershipReadiness(
        MembershipReadinessSummary { state },
    ))
}

fn map_readiness_error(error: QueryMembershipReadinessError) -> EngineError {
    match error {
        QueryMembershipReadinessError::Unavailable { .. } => EngineError::new(
            QUERY_MEMBERSHIP_READINESS_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
    }
}

#[cfg(feature = "dev-tools")]
pub async fn execute_query_membership_diagnostics(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let view = facade
        .query_membership_diagnostics()
        .await
        .map_err(map_diagnostics_error)?;
    Ok(OperationResult::MembershipDiagnostics(
        MembershipDiagnosticsSummary {
            revision: view.revision,
            branch_id: encode(view.branch_id.as_bytes()),
            head_event_id: view.head_event_id.to_hex(),
            group_epoch: view.group_epoch,
            effective_member_count: count(view.effective_member_count),
            pending_conflict_count: count(view.pending_conflict_count),
            pending_confirmation_count: count(view.pending_confirmation_count),
            pending_effect_count: count(view.pending_effect_count),
            transition_phases: view
                .transition_phases
                .into_iter()
                .map(phase)
                .map(str::to_owned)
                .collect(),
        },
    ))
}

#[cfg(feature = "dev-tools")]
fn phase(value: MembershipBranchTransitionPhaseV1) -> &'static str {
    match value {
        MembershipBranchTransitionPhaseV1::Prepared => "prepared",
        MembershipBranchTransitionPhaseV1::SourceBackedUp => "source_backed_up",
        MembershipBranchTransitionPhaseV1::TargetVerified => "target_verified",
        MembershipBranchTransitionPhaseV1::TargetStaged => "target_staged",
        MembershipBranchTransitionPhaseV1::Promoted => "promoted",
        MembershipBranchTransitionPhaseV1::RuntimeRestored => "runtime_restored",
        MembershipBranchTransitionPhaseV1::Completed => "completed",
    }
}

#[cfg(feature = "dev-tools")]
fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(feature = "dev-tools")]
fn encode(value: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in value {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

#[cfg(feature = "dev-tools")]
fn map_diagnostics_error(error: QueryMembershipDiagnosticsError) -> EngineError {
    match error {
        QueryMembershipDiagnosticsError::Ledger { .. }
        | QueryMembershipDiagnosticsError::Security { .. } => {
            EngineError::new(1212, EngineErrorCategory::Unavailable, true)
        }
        QueryMembershipDiagnosticsError::InvalidState { .. } => {
            EngineError::new(1213, EngineErrorCategory::InvalidState, false)
        }
    }
}
