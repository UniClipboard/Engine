use uc_application::facade::{
    MembershipConflictsView, QueryMembershipConflictsError,
    ResolveMembershipConflictError as ApplicationResolveError,
    ResolveMembershipConflictInput as ApplicationResolveInput,
    ResolveMembershipConflictResult as ApplicationResolveResult,
};
use uc_core::membership::{
    MembershipBranchId, MembershipBranchTransitionPhaseV1, MembershipConflictChoice,
    MembershipConflictId,
};

use crate::error_codes::{
    MEMBERSHIP_CONFLICT_COMMITTED_PENDING_CODE, MEMBERSHIP_CONFLICT_INVALID_CHOICE_CODE,
    MEMBERSHIP_CONFLICT_INVALID_INPUT_CODE, MEMBERSHIP_CONFLICT_LOCKED_CODE,
    MEMBERSHIP_CONFLICT_RECOVERY_REQUIRED_CODE, MEMBERSHIP_CONFLICT_UNAVAILABLE_CODE,
};
use crate::{
    EngineError, EngineErrorCategory, MembershipBranchTransitionPhaseSummary,
    MembershipConflictBranchSummary, MembershipConflictChoiceSummary,
    MembershipConflictResolutionOutcomeSummary, MembershipConflictResolutionSummary,
    MembershipConflictStatusSummary, MembershipConflictSummary, MembershipConflictsSummary,
    OperationResult, ResolveMembershipConflictInput,
};

pub async fn execute_query_membership_conflicts(
    facade: &uc_application::facade::AppFacade,
) -> Result<OperationResult, EngineError> {
    facade
        .query_membership_conflicts()
        .await
        .map(membership_conflicts)
        .map(OperationResult::MembershipConflicts)
        .map_err(map_query_error)
}

pub async fn execute_resolve_membership_conflict(
    facade: &uc_application::facade::AppFacade,
    input: ResolveMembershipConflictInput,
) -> Result<OperationResult, EngineError> {
    let conflict_id = parse_identifier(&input.conflict_id)
        .map(MembershipConflictId::from_bytes)
        .ok_or_else(invalid_input)?;
    let target_branch_id = parse_identifier(&input.target_branch_id)
        .map(MembershipBranchId::from_bytes)
        .ok_or_else(invalid_input)?;
    facade
        .resolve_membership_conflict(ApplicationResolveInput {
            conflict_id,
            target_branch_id,
        })
        .await
        .map(membership_conflict_resolution)
        .map(OperationResult::MembershipConflictResolved)
        .map_err(map_resolve_error)
}

fn membership_conflicts(view: MembershipConflictsView) -> MembershipConflictsSummary {
    MembershipConflictsSummary {
        revision: view.revision,
        conflicts: view
            .conflicts
            .into_iter()
            .map(|conflict| MembershipConflictSummary {
                conflict_id: encode_identifier(conflict.conflict_id.as_bytes()),
                status: match conflict.status {
                    uc_application::facade::MembershipConflictStatus::Unresolved => {
                        MembershipConflictStatusSummary::Unresolved
                    }
                    uc_application::facade::MembershipConflictStatus::Selected => {
                        MembershipConflictStatusSummary::Selected
                    }
                    uc_application::facade::MembershipConflictStatus::Transitioning => {
                        MembershipConflictStatusSummary::Transitioning
                    }
                    uc_application::facade::MembershipConflictStatus::Completed => {
                        MembershipConflictStatusSummary::Completed
                    }
                    uc_application::facade::MembershipConflictStatus::RePairingRequired => {
                        MembershipConflictStatusSummary::RePairingRequired
                    }
                },
                selected_branch_id: conflict
                    .selected_branch_id
                    .map(|id| encode_identifier(id.as_bytes())),
                transition_phase: conflict.transition_phase.map(transition_phase),
                detected_at_revision: conflict.detected_at_revision,
                evidence_peer_count: u32::try_from(conflict.evidence_peer_count)
                    .unwrap_or(u32::MAX),
                branches: conflict
                    .branches
                    .into_iter()
                    .map(|branch| MembershipConflictBranchSummary {
                        branch_id: encode_identifier(branch.branch_id.as_bytes()),
                        is_local: branch.is_local,
                        choice: match branch.choice {
                            MembershipConflictChoice::ActiveMemberRecovery => {
                                MembershipConflictChoiceSummary::ActiveMemberRecovery
                            }
                            MembershipConflictChoice::RePairingRequired => {
                                MembershipConflictChoiceSummary::RePairingRequired
                            }
                        },
                    })
                    .collect(),
                local_resolution_completed: conflict.local_resolution_completed,
            })
            .collect(),
    }
}

fn membership_conflict_resolution(
    result: ApplicationResolveResult,
) -> MembershipConflictResolutionSummary {
    let (outcome, conflict_id, local_resolution_completed) = match result {
        ApplicationResolveResult::Completed { .. } => (
            MembershipConflictResolutionOutcomeSummary::Completed,
            None,
            true,
        ),
        ApplicationResolveResult::AlreadyCompleted { .. } => (
            MembershipConflictResolutionOutcomeSummary::AlreadyCompleted,
            None,
            true,
        ),
        ApplicationResolveResult::Pending { conflict_id } => (
            MembershipConflictResolutionOutcomeSummary::Pending,
            Some(encode_identifier(conflict_id.as_bytes())),
            false,
        ),
        ApplicationResolveResult::RePairingRequired { conflict_id } => (
            MembershipConflictResolutionOutcomeSummary::RePairingRequired,
            Some(encode_identifier(conflict_id.as_bytes())),
            false,
        ),
        ApplicationResolveResult::StateChanged {
            current_conflict_id,
        } => (
            MembershipConflictResolutionOutcomeSummary::StateChanged,
            current_conflict_id.map(|id| encode_identifier(id.as_bytes())),
            false,
        ),
    };
    MembershipConflictResolutionSummary {
        outcome,
        conflict_id,
        local_resolution_completed,
    }
}

fn transition_phase(
    phase: MembershipBranchTransitionPhaseV1,
) -> MembershipBranchTransitionPhaseSummary {
    match phase {
        MembershipBranchTransitionPhaseV1::Prepared => {
            MembershipBranchTransitionPhaseSummary::Prepared
        }
        MembershipBranchTransitionPhaseV1::SourceBackedUp => {
            MembershipBranchTransitionPhaseSummary::SourceBackedUp
        }
        MembershipBranchTransitionPhaseV1::TargetVerified => {
            MembershipBranchTransitionPhaseSummary::TargetVerified
        }
        MembershipBranchTransitionPhaseV1::TargetStaged => {
            MembershipBranchTransitionPhaseSummary::TargetStaged
        }
        MembershipBranchTransitionPhaseV1::Promoted => {
            MembershipBranchTransitionPhaseSummary::Promoted
        }
        MembershipBranchTransitionPhaseV1::RuntimeRestored => {
            MembershipBranchTransitionPhaseSummary::RuntimeRestored
        }
        MembershipBranchTransitionPhaseV1::Completed => {
            MembershipBranchTransitionPhaseSummary::Completed
        }
    }
}

fn parse_identifier(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(decoded)
}

fn encode_identifier(value: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(64);
    for byte in value {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn invalid_input() -> EngineError {
    EngineError::new(
        MEMBERSHIP_CONFLICT_INVALID_INPUT_CODE,
        EngineErrorCategory::InvalidInput,
        false,
    )
}

fn map_query_error(error: QueryMembershipConflictsError) -> EngineError {
    match error {
        QueryMembershipConflictsError::Locked { .. } => EngineError::new(
            MEMBERSHIP_CONFLICT_LOCKED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        QueryMembershipConflictsError::Unavailable { .. } => EngineError::new(
            MEMBERSHIP_CONFLICT_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        QueryMembershipConflictsError::RecoveryRequired { .. } => EngineError::new(
            MEMBERSHIP_CONFLICT_RECOVERY_REQUIRED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
    }
}

fn map_resolve_error(error: ApplicationResolveError) -> EngineError {
    match error {
        ApplicationResolveError::Locked { .. } => EngineError::new(
            MEMBERSHIP_CONFLICT_LOCKED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        ApplicationResolveError::InvalidChoice => EngineError::new(
            MEMBERSHIP_CONFLICT_INVALID_CHOICE_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        ApplicationResolveError::TargetUnavailable { .. } => EngineError::new(
            MEMBERSHIP_CONFLICT_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
        ApplicationResolveError::RecoveryRequired { .. } => EngineError::new(
            MEMBERSHIP_CONFLICT_RECOVERY_REQUIRED_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        ApplicationResolveError::CommittedButPending { .. } => EngineError::new(
            MEMBERSHIP_CONFLICT_COMMITTED_PENDING_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_identifiers_map_to_stable_invalid_input() {
        let error = invalid_input();
        assert_eq!(error.code(), MEMBERSHIP_CONFLICT_INVALID_INPUT_CODE);
        assert_eq!(error.category(), EngineErrorCategory::InvalidInput);
        assert!(!error.is_retryable());
        assert!(parse_identifier("not-an-identifier").is_none());
    }
}
