use uc_application::facade::{
    ChooseDeviceGroup, ChooseDeviceGroupResult, DeviceGroupChoice, DeviceGroupIssue,
};
use uc_core::membership::{MembershipBranchId, MembershipConflictId};

mod presentation;

use crate::operations::device::member::device_trust_snapshot;
use crate::{
    ChooseDeviceGroupInput, DeviceGroupChoiceIssueSummary, DeviceGroupChoiceOutcomeSummary,
    DeviceGroupChoiceResultSummary, DeviceGroupChoicesSummary, EngineError, EngineErrorCategory,
    OperationResult,
};

pub async fn execute_query_device_group_choices(
    facade: &uc_application::facade::AppFacade,
) -> Result<OperationResult, EngineError> {
    let view = facade.query_device_group_choices().await.map_err(|error| {
        tracing::debug!(
            error_kind = query_error_kind(&error),
            "device group choice query failed"
        );
        unavailable()
    })?;
    let local = view
        .device_trust
        .local_device_id
        .as_ref()
        .map(|id| id.as_str())
        .unwrap_or_default();
    let mut issues = Vec::new();
    if let Some(change) = &view.device_trust.current_change {
        issues.push(DeviceGroupChoiceIssueSummary {
            issue_id: format!("p:{}", change.change_id.to_hex()),
            choices: presentation::pending(change, local),
            reason: presentation::reason(change.explanation.clone()),
        });
    }
    for conflict in view.conflicts.conflicts {
        if conflict.local_resolution_completed {
            continue;
        }
        issues.push(DeviceGroupChoiceIssueSummary {
            issue_id: format!("c:{}", encode(conflict.conflict_id.as_bytes())),
            choices: conflict
                .branches
                .into_iter()
                .map(|branch| presentation::branch(branch, local))
                .collect(),
            reason: presentation::reason(conflict.explanation),
        });
    }
    let device_trust = device_trust_snapshot(view.device_trust);
    Ok(OperationResult::DeviceGroupChoices(
        DeviceGroupChoicesSummary {
            revision: view.revision,
            device_trust,
            issues,
        },
    ))
}

pub async fn execute_choose_device_group(
    facade: &uc_application::facade::AppFacade,
    input: ChooseDeviceGroupInput,
) -> Result<OperationResult, EngineError> {
    let issue = if let Some(value) = input.issue_id.strip_prefix("p:") {
        DeviceGroupIssue::PendingChange(
            uc_core::membership::MembershipEventId::from_hex(value).ok_or_else(invalid_input)?,
        )
    } else if let Some(value) = input.issue_id.strip_prefix("c:") {
        DeviceGroupIssue::BranchConflict(MembershipConflictId::from_bytes(
            decode(value).ok_or_else(invalid_input)?,
        ))
    } else {
        return Err(invalid_input());
    };
    let choice = match input.choice_id.as_str() {
        "apply" => DeviceGroupChoice::ApplyPendingChange,
        "keep" => DeviceGroupChoice::KeepCurrentGroup,
        value if value.starts_with("b:") => DeviceGroupChoice::Branch(
            MembershipBranchId::from_bytes(decode(&value[2..]).ok_or_else(invalid_input)?),
        ),
        _ => return Err(invalid_input()),
    };
    let result = facade
        .choose_device_group(ChooseDeviceGroup {
            issue,
            choice,
            expected_revision: input.expected_revision,
            confirm_local_removal: input.confirm_local_removal,
        })
        .await
        .map_err(|error| {
            tracing::debug!(
                error_kind = choose_error_kind(&error),
                "device group choice failed"
            );
            unavailable()
        })?;
    let (outcome, current_revision) = outcome(result).ok_or_else(invalid_input)?;
    Ok(OperationResult::DeviceGroupChosen(
        DeviceGroupChoiceResultSummary {
            outcome,
            current_revision,
        },
    ))
}

fn outcome(
    result: ChooseDeviceGroupResult,
) -> Option<(DeviceGroupChoiceOutcomeSummary, Option<u64>)> {
    use uc_application::facade::{
        DecideDeviceTrustChangeResult as D, ResolveMembershipConflictResult as R,
    };
    match result {
        ChooseDeviceGroupResult::StateChanged { current_revision } => Some((
            DeviceGroupChoiceOutcomeSummary::StateChanged,
            Some(current_revision),
        )),
        ChooseDeviceGroupResult::InvalidChoice => None,
        ChooseDeviceGroupResult::PendingChange(
            D::Applied { .. } | D::KeptCurrentDeviceGroup { .. },
        ) => Some((DeviceGroupChoiceOutcomeSummary::Completed, None)),
        ChooseDeviceGroupResult::PendingChange(D::AlreadyCompleted { .. }) => {
            Some((DeviceGroupChoiceOutcomeSummary::AlreadyCompleted, None))
        }
        ChooseDeviceGroupResult::PendingChange(D::StateChanged { status, .. }) => Some((
            DeviceGroupChoiceOutcomeSummary::StateChanged,
            Some(status.revision),
        )),
        ChooseDeviceGroupResult::PendingChange(D::LocalConfirmationRequired { .. }) => Some((
            DeviceGroupChoiceOutcomeSummary::LocalDeviceConfirmationRequired,
            None,
        )),
        ChooseDeviceGroupResult::BranchConflict(R::Completed { .. }) => {
            Some((DeviceGroupChoiceOutcomeSummary::Completed, None))
        }
        ChooseDeviceGroupResult::BranchConflict(R::Pending { .. }) => {
            Some((DeviceGroupChoiceOutcomeSummary::Pending, None))
        }
        ChooseDeviceGroupResult::BranchConflict(R::RePairingRequired { .. }) => {
            Some((DeviceGroupChoiceOutcomeSummary::RePairingRequired, None))
        }
        ChooseDeviceGroupResult::BranchConflict(R::AlreadyCompleted { .. }) => {
            Some((DeviceGroupChoiceOutcomeSummary::AlreadyCompleted, None))
        }
        ChooseDeviceGroupResult::BranchConflict(R::StateChanged { .. }) => {
            Some((DeviceGroupChoiceOutcomeSummary::StateChanged, None))
        }
    }
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

fn decode(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut decoded = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        decoded[index] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(decoded)
}

fn nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn invalid_input() -> EngineError {
    EngineError::new(1210, EngineErrorCategory::InvalidInput, false)
}

fn unavailable() -> EngineError {
    EngineError::new(1211, EngineErrorCategory::Unavailable, true)
}

fn query_error_kind(error: &uc_application::facade::QueryDeviceGroupChoicesError) -> &'static str {
    use uc_application::facade::QueryDeviceGroupChoicesError;

    match error {
        QueryDeviceGroupChoicesError::DeviceTrust { source } => match source {
            uc_application::facade::QueryDeviceTrustError::Locked => "device_trust_locked",
            uc_application::facade::QueryDeviceTrustError::RecoveryRequired => {
                "device_trust_recovery_required"
            }
            uc_application::facade::QueryDeviceTrustError::Unavailable => {
                "device_trust_unavailable"
            }
            uc_application::facade::QueryDeviceTrustError::Dependency { .. } => {
                "device_trust_dependency"
            }
        },
        QueryDeviceGroupChoicesError::MembershipConflict { .. } => "membership_conflict",
        QueryDeviceGroupChoicesError::StateChanged => "state_changed",
    }
}

fn choose_error_kind(error: &uc_application::facade::ChooseDeviceGroupError) -> &'static str {
    use uc_application::facade::{
        ChooseDeviceGroupError, DecideDeviceTrustChangeError, ResolveMembershipConflictError,
    };

    match error {
        ChooseDeviceGroupError::PendingChange { source } => match source {
            DecideDeviceTrustChangeError::Locked => "pending_change_locked",
            DecideDeviceTrustChangeError::RecoveryRequired => "pending_change_recovery_required",
            DecideDeviceTrustChangeError::Unavailable => "pending_change_unavailable",
            DecideDeviceTrustChangeError::StateChanged => "pending_change_state_changed",
            DecideDeviceTrustChangeError::CommittedButPending => {
                "pending_change_committed_but_pending"
            }
        },
        ChooseDeviceGroupError::BranchConflict { source } => match source {
            ResolveMembershipConflictError::Locked { .. } => "branch_conflict_locked",
            ResolveMembershipConflictError::InvalidChoice => "branch_conflict_invalid_choice",
            ResolveMembershipConflictError::TargetUnavailable { .. } => {
                "branch_conflict_target_unavailable"
            }
            ResolveMembershipConflictError::RecoveryRequired { .. } => {
                "branch_conflict_recovery_required"
            }
            ResolveMembershipConflictError::CommittedButPending { .. } => {
                "branch_conflict_committed_but_pending"
            }
        },
        ChooseDeviceGroupError::Query { source } => query_error_kind(source),
    }
}
