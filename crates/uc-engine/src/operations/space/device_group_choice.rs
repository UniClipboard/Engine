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
use uc_observability_contract::{uc_info, uc_warn};

pub async fn execute_query_device_group_choices(
    facade: &uc_application::facade::AppFacade,
) -> Result<OperationResult, EngineError> {
    let view = facade.query_device_group_choices().await.map_err(|error| {
        record_query_failure(&error);
        query_error(&error)
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
    let (issue, choice) = parse_request(&input)?;
    let result = facade
        .choose_device_group(ChooseDeviceGroup {
            issue,
            choice,
            expected_revision: input.expected_revision,
            confirm_local_removal: input.confirm_local_removal,
        })
        .await
        .map_err(|error| {
            record_choose_failure(&error);
            unavailable()
        })?;
    settle(result)
}

/// 宿主传来的议题与选项标识必须可解析；无法解析是宿主的输入错误，留下固定原因而不是静默拒绝。
fn parse_request(
    input: &ChooseDeviceGroupInput,
) -> Result<(DeviceGroupIssue, DeviceGroupChoice), EngineError> {
    let issue = if let Some(value) = input.issue_id.strip_prefix("p:") {
        DeviceGroupIssue::PendingChange(
            uc_core::membership::MembershipEventId::from_hex(value)
                .ok_or_else(|| invalid_request("invalid_issue"))?,
        )
    } else if let Some(value) = input.issue_id.strip_prefix("c:") {
        DeviceGroupIssue::BranchConflict(MembershipConflictId::from_bytes(
            decode(value).ok_or_else(|| invalid_request("invalid_issue"))?,
        ))
    } else {
        return Err(invalid_request("invalid_issue"));
    };
    let choice = match input.choice_id.as_str() {
        "apply" => DeviceGroupChoice::ApplyPendingChange,
        "keep" => DeviceGroupChoice::KeepCurrentGroup,
        value if value.starts_with("b:") => {
            DeviceGroupChoice::Branch(MembershipBranchId::from_bytes(
                decode(&value[2..]).ok_or_else(|| invalid_request("invalid_choice"))?,
            ))
        }
        _ => return Err(invalid_request("invalid_choice")),
    };
    Ok((issue, choice))
}

/// 把应用层结果映射为公开摘要，并为这次选择的结局留下一条 INFO 记录。
fn settle(result: ChooseDeviceGroupResult) -> Result<OperationResult, EngineError> {
    let Some((outcome, current_revision)) = outcome(result) else {
        return Err(invalid_request("choice_rejected"));
    };
    uc_info!(
        outcome = outcome_label(outcome),
        "device group choice settled"
    );
    Ok(OperationResult::DeviceGroupChosen(
        DeviceGroupChoiceResultSummary {
            outcome,
            current_revision,
        },
    ))
}

fn outcome_label(outcome: DeviceGroupChoiceOutcomeSummary) -> &'static str {
    match outcome {
        DeviceGroupChoiceOutcomeSummary::Completed => "completed",
        DeviceGroupChoiceOutcomeSummary::Pending => "pending",
        DeviceGroupChoiceOutcomeSummary::RePairingRequired => "re_pairing_required",
        DeviceGroupChoiceOutcomeSummary::AlreadyCompleted => "already_completed",
        DeviceGroupChoiceOutcomeSummary::StateChanged => "state_changed",
        DeviceGroupChoiceOutcomeSummary::LocalDeviceConfirmationRequired => {
            "local_device_confirmation_required"
        }
    }
}

fn record_query_failure(error: &uc_application::facade::QueryDeviceGroupChoicesError) {
    uc_warn!(
        error_kind = query_error_kind(error),
        error_stage = "load_space_device_state",
        "device group choice query failed"
    );
}

fn record_choose_failure(error: &uc_application::facade::ChooseDeviceGroupError) {
    uc_warn!(
        error_kind = choose_error_kind(error),
        "device group choice failed"
    );
}

fn invalid_request(reason: &'static str) -> EngineError {
    uc_warn!(
        reason = reason,
        "device group choice request was not accepted"
    );
    invalid_input()
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

fn query_error(error: &uc_application::facade::QueryDeviceGroupChoicesError) -> EngineError {
    use uc_application::facade::{QueryDeviceGroupChoicesError, QueryDeviceTrustError};

    match error {
        QueryDeviceGroupChoicesError::DeviceTrust {
            source: QueryDeviceTrustError::Locked,
        }
        | QueryDeviceGroupChoicesError::MembershipConflict {
            source: uc_application::facade::QueryMembershipConflictsError::Locked { .. },
        } => EngineError::new(1212, EngineErrorCategory::InvalidState, false),
        QueryDeviceGroupChoicesError::DeviceTrust {
            source: QueryDeviceTrustError::RecoveryRequired { .. },
        }
        | QueryDeviceGroupChoicesError::MembershipConflict {
            source: uc_application::facade::QueryMembershipConflictsError::RecoveryRequired { .. },
        } => EngineError::new(1213, EngineErrorCategory::InvalidState, false),
        QueryDeviceGroupChoicesError::DeviceTrust {
            source: QueryDeviceTrustError::Unavailable | QueryDeviceTrustError::Dependency { .. },
        }
        | QueryDeviceGroupChoicesError::MembershipConflict {
            source: uc_application::facade::QueryMembershipConflictsError::Unavailable { .. },
        } => unavailable(),
    }
}

fn query_error_kind(error: &uc_application::facade::QueryDeviceGroupChoicesError) -> &'static str {
    use uc_application::facade::QueryDeviceGroupChoicesError;

    match error {
        QueryDeviceGroupChoicesError::DeviceTrust { source } => match source {
            uc_application::facade::QueryDeviceTrustError::Locked => "device_trust_locked",
            uc_application::facade::QueryDeviceTrustError::RecoveryRequired { .. } => {
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
    }
}

fn choose_error_kind(error: &uc_application::facade::ChooseDeviceGroupError) -> &'static str {
    use uc_application::facade::{
        ChooseDeviceGroupError, DecideDeviceTrustChangeError, ResolveMembershipConflictError,
    };

    match error {
        ChooseDeviceGroupError::PendingChange { source } => match source {
            DecideDeviceTrustChangeError::Locked => "pending_change_locked",
            DecideDeviceTrustChangeError::RecoveryRequired { .. } => {
                "pending_change_recovery_required"
            }
            DecideDeviceTrustChangeError::Unavailable { .. } => "pending_change_unavailable",
            DecideDeviceTrustChangeError::StateChanged => "pending_change_state_changed",
            DecideDeviceTrustChangeError::CommittedButPending { .. } => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use uc_application::facade::{QueryDeviceGroupChoicesError, QueryDeviceTrustError};

    #[test]
    fn public_query_errors_distinguish_retry_from_user_recovery() {
        let unavailable = QueryDeviceGroupChoicesError::DeviceTrust {
            source: QueryDeviceTrustError::Unavailable,
        };
        let recovery = QueryDeviceGroupChoicesError::DeviceTrust {
            source: QueryDeviceTrustError::recovery_required(),
        };
        let locked = QueryDeviceGroupChoicesError::DeviceTrust {
            source: QueryDeviceTrustError::Locked,
        };

        let unavailable = query_error(&unavailable);
        assert_eq!(unavailable.code(), 1211);
        assert!(unavailable.is_retryable());
        let recovery = query_error(&recovery);
        assert_eq!(recovery.code(), 1213);
        assert!(!recovery.is_retryable());
        let locked = query_error(&locked);
        assert_eq!(locked.code(), 1212);
        assert!(!locked.is_retryable());
    }

    fn captured() -> (
        uc_testkit::log_capture::CapturedLogs,
        tracing::subscriber::DefaultGuard,
    ) {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let guard = logs.install();
        (logs, guard)
    }

    fn lines(logs: &uc_testkit::log_capture::CapturedLogs, needle: &str) -> Vec<String> {
        logs.output()
            .lines()
            .filter(|line| line.contains(needle))
            .map(str::to_owned)
            .collect()
    }

    fn request(issue: &str, choice: &str) -> ChooseDeviceGroupInput {
        ChooseDeviceGroupInput {
            issue_id: issue.to_owned(),
            choice_id: choice.to_owned(),
            expected_revision: 1,
            confirm_local_removal: false,
        }
    }

    #[test]
    fn unparseable_requests_leave_one_warning_with_a_fixed_reason() {
        for (issue, choice, reason) in [
            ("x:1", "keep", "invalid_issue"),
            ("p:not-hex", "keep", "invalid_issue"),
            (&format!("c:{}", "0".repeat(10)), "keep", "invalid_issue"),
            (&format!("p:{}", "0".repeat(64)), "bogus", "invalid_choice"),
            (&format!("p:{}", "0".repeat(64)), "b:zz", "invalid_choice"),
        ] {
            let (logs, _guard) = captured();
            let error = parse_request(&request(issue, choice)).expect_err("rejected");
            assert_eq!(error.code(), 1210);
            let records = lines(&logs, "device group choice request was not accepted");
            assert_eq!(records.len(), 1, "{issue} {choice}: {}", logs.output());
            assert!(records[0].contains(" WARN "), "{}", records[0]);
            assert!(
                records[0].contains(&format!("reason=\"{reason}\"")),
                "{}",
                records[0]
            );
            assert!(!logs.output().contains(issue) || issue.is_empty());
        }
        let (logs, _guard) = captured();
        assert!(parse_request(&request(&format!("p:{}", "0".repeat(64)), "keep")).is_ok());
        assert!(lines(&logs, "was not accepted").is_empty());
    }

    #[test]
    fn every_settled_outcome_leaves_one_info_record() {
        use uc_application::facade::ResolveMembershipConflictResult as R;
        let cases = [
            (
                ChooseDeviceGroupResult::StateChanged {
                    current_revision: 4,
                },
                "state_changed",
            ),
            (
                ChooseDeviceGroupResult::BranchConflict(R::RePairingRequired {
                    conflict_id: MembershipConflictId::from_bytes([1; 32]),
                }),
                "re_pairing_required",
            ),
        ];
        for (result, expected) in cases {
            let (logs, _guard) = captured();
            assert!(settle(result).is_ok());
            let records = lines(&logs, "device group choice settled");
            assert_eq!(records.len(), 1, "{}", logs.output());
            assert!(records[0].contains(" INFO "), "{}", records[0]);
            assert!(
                records[0].contains(&format!("outcome=\"{expected}\"")),
                "{}",
                records[0]
            );
        }
        let (logs, _guard) = captured();
        assert!(settle(ChooseDeviceGroupResult::InvalidChoice).is_err());
        assert_eq!(
            lines(&logs, "choice_rejected").len(),
            1,
            "{}",
            logs.output()
        );
    }

    #[test]
    fn failures_that_reach_the_host_are_warnings_not_debug() {
        use uc_application::facade::{ChooseDeviceGroupError, DecideDeviceTrustChangeError};
        let (logs, _guard) = captured();
        record_query_failure(&QueryDeviceGroupChoicesError::DeviceTrust {
            source: QueryDeviceTrustError::Unavailable,
        });
        record_choose_failure(&ChooseDeviceGroupError::PendingChange {
            source: DecideDeviceTrustChangeError::StateChanged,
        });
        let query = lines(&logs, "device group choice query failed");
        let choose = lines(&logs, "device group choice failed");
        assert_eq!((query.len(), choose.len()), (1, 1), "{}", logs.output());
        assert!(query[0].contains(" WARN ") && query[0].contains("device_trust_unavailable"));
        assert!(choose[0].contains(" WARN ") && choose[0].contains("pending_change_state_changed"));
    }
}
