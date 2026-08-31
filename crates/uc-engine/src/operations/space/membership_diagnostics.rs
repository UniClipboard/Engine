use uc_application::facade::QueryMembershipDiagnosticsError;
use uc_core::membership::MembershipBranchTransitionPhaseV1;

use crate::{EngineError, EngineErrorCategory, MembershipDiagnosticsSummary, OperationResult};

pub async fn execute_query_membership_diagnostics(
    facade: &uc_application::facade::AppFacade,
) -> Result<OperationResult, EngineError> {
    let view = facade
        .query_membership_diagnostics()
        .await
        .map_err(map_error)?;
    Ok(OperationResult::MembershipDiagnostics(
        MembershipDiagnosticsSummary {
            revision: view.revision,
            branch_id: encode(view.branch_id.as_bytes()),
            head_event_id: view.head_event_id.to_hex(),
            group_epoch: view.group_epoch,
            effective_member_count: count(view.effective_member_count),
            pending_conflict_count: count(view.pending_conflict_count),
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

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn encode(value: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in value {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn map_error(error: QueryMembershipDiagnosticsError) -> EngineError {
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
