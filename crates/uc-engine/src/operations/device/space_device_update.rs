use uc_application::facade::{
    SpaceDeviceUpdatePhase, SpaceDeviceUpdateProblem, SpaceDeviceUpdateRecovery,
    SpaceDeviceUpdateStatus,
};

use crate::{
    MembershipMaintenanceHealthPhaseSummary, MembershipMaintenanceHealthSummary,
    MembershipMaintenanceProblemSummary, MembershipMaintenanceRecoverySummary,
    SpaceDeviceUpdatePhaseSummary, SpaceDeviceUpdateProblemSummary,
    SpaceDeviceUpdateRecoverySummary, SpaceDeviceUpdateStatusSummary,
};

pub(super) fn summaries(
    status: SpaceDeviceUpdateStatus,
) -> (
    SpaceDeviceUpdateStatusSummary,
    MembershipMaintenanceHealthSummary,
) {
    let current = SpaceDeviceUpdateStatusSummary {
        phase: match status.phase {
            SpaceDeviceUpdatePhase::Updating => SpaceDeviceUpdatePhaseSummary::Updating,
            SpaceDeviceUpdatePhase::Completed => SpaceDeviceUpdatePhaseSummary::Completed,
            SpaceDeviceUpdatePhase::RetryableFailure => {
                SpaceDeviceUpdatePhaseSummary::RetryableFailure
            }
            SpaceDeviceUpdatePhase::NeedsAttention => SpaceDeviceUpdatePhaseSummary::NeedsAttention,
        },
        reason: status.reason.map(|reason| match reason {
            SpaceDeviceUpdateProblem::DeviceStateRejected => {
                SpaceDeviceUpdateProblemSummary::DeviceStateRejected
            }
            SpaceDeviceUpdateProblem::DeviceRelationshipConflict => {
                SpaceDeviceUpdateProblemSummary::DeviceRelationshipConflict
            }
            SpaceDeviceUpdateProblem::DeviceSecurityUpdateRejected => {
                SpaceDeviceUpdateProblemSummary::DeviceSecurityUpdateRejected
            }
            SpaceDeviceUpdateProblem::DeviceUpgradeRequired => {
                SpaceDeviceUpdateProblemSummary::DeviceUpgradeRequired
            }
        }),
        recovery: status.recovery.map(|recovery| match recovery {
            SpaceDeviceUpdateRecovery::ReviewDevices => {
                SpaceDeviceUpdateRecoverySummary::ReviewDevices
            }
            SpaceDeviceUpdateRecovery::UpdateApp => SpaceDeviceUpdateRecoverySummary::UpdateApp,
        }),
        next_retry_at_ms: status.next_retry_at_ms,
    };
    let legacy = MembershipMaintenanceHealthSummary {
        phase: match current.phase {
            SpaceDeviceUpdatePhaseSummary::Completed => {
                MembershipMaintenanceHealthPhaseSummary::Healthy
            }
            SpaceDeviceUpdatePhaseSummary::Updating
            | SpaceDeviceUpdatePhaseSummary::RetryableFailure => {
                MembershipMaintenanceHealthPhaseSummary::Retrying
            }
            SpaceDeviceUpdatePhaseSummary::NeedsAttention => {
                MembershipMaintenanceHealthPhaseSummary::NeedsAttention
            }
        },
        reason: matches!(
            current.reason,
            Some(SpaceDeviceUpdateProblemSummary::DeviceStateRejected)
        )
        .then_some(MembershipMaintenanceProblemSummary::MembershipHistoryRejected),
        recovery: matches!(
            current.recovery,
            Some(SpaceDeviceUpdateRecoverySummary::ReviewDevices)
        )
        .then_some(MembershipMaintenanceRecoverySummary::ResolveDeviceTrust),
        next_retry_at_ms: current.next_retry_at_ms,
    };
    (current, legacy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_status_is_canonical_and_legacy_status_is_only_a_projection() {
        let (updating, legacy_updating) = summaries(SpaceDeviceUpdateStatus::updating());
        assert_eq!(updating.phase, SpaceDeviceUpdatePhaseSummary::Updating);
        assert_eq!(
            legacy_updating.phase,
            MembershipMaintenanceHealthPhaseSummary::Retrying
        );

        let (retryable, legacy_retryable) =
            summaries(SpaceDeviceUpdateStatus::retryable_failure(60_000));
        assert_eq!(
            retryable.phase,
            SpaceDeviceUpdatePhaseSummary::RetryableFailure
        );
        assert_eq!(retryable.next_retry_at_ms, Some(60_000));
        assert_eq!(legacy_retryable.next_retry_at_ms, Some(60_000));
        let json = serde_json::to_value(retryable).expect("serialize device update");
        assert_eq!(json["phase"], "retryable_failure");
        assert!(json.get("internal_step").is_none());

        let (attention, legacy_attention) = summaries(SpaceDeviceUpdateStatus::needs_attention(
            SpaceDeviceUpdateProblem::DeviceStateRejected,
            SpaceDeviceUpdateRecovery::ReviewDevices,
        ));
        assert_eq!(
            attention.phase,
            SpaceDeviceUpdatePhaseSummary::NeedsAttention
        );
        assert_eq!(
            legacy_attention.reason,
            Some(MembershipMaintenanceProblemSummary::MembershipHistoryRejected)
        );
        assert_eq!(
            legacy_attention.recovery,
            Some(MembershipMaintenanceRecoverySummary::ResolveDeviceTrust)
        );
    }
}
