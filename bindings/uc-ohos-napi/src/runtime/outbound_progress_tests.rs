//! JS边界保持大整数、未知总量与真实阶段，不经浮点或宿主推断。
use super::outbound_progress;
use uc_engine::{
    OutboundProgressBatchState, OutboundProgressFreshness, OutboundProgressItemSummary,
    OutboundProgressPhase, OutboundProgressState, OutboundProgressSummary, ReceiveItemKind,
};

#[test]
fn outbound_view_preserves_large_integer_positions_and_unknown_totals_for_javascript() {
    let view = example_view();
    let mapped = outbound_progress(view);
    assert_eq!(mapped.revision, "18446744073709551615");
    assert_eq!(
        mapped.items[0].completed_bytes.as_deref(),
        Some("18446744073709551614")
    );
    assert_eq!(mapped.items[0].total_bytes, None);
    assert_eq!(mapped.items[0].state, "retrying");
    assert_eq!(
        mapped.items[0].last_known_phase.as_deref(),
        Some("retrying")
    );
    assert_eq!(mapped.items[0].round, Some(3));
    assert_eq!(mapped.items[0].index, 7);
}

fn example_view() -> OutboundProgressSummary {
    OutboundProgressSummary {
        entry_id: "source".into(),
        target_device_id: "peer".into(),
        attempt_id: Some("attempt".into()),
        revision: u64::MAX,
        freshness: OutboundProgressFreshness::Live,
        freshness_available: true,
        state: OutboundProgressBatchState::InProgress,
        items_total: 1,
        items_waiting: 0,
        items_running: 1,
        items_completed: 0,
        items_failed: 0,
        items_cancelled: 0,
        items_unconfirmed: 0,
        items: vec![OutboundProgressItemSummary {
            kind: ReceiveItemKind::Representation,
            index: 7,
            state: OutboundProgressState::Retrying,
            last_known_phase: Some(OutboundProgressPhase::Retrying),
            round: Some(3),
            completed_bytes: Some(u64::MAX - 1),
            total_bytes: None,
            failure_reason: None,
            cancellation_reason: None,
        }],
    }
}

#[test]
fn resend_maps_per_target_decisions_without_turning_running_into_new_acceptance() {
    let mapped = super::resend_outcome(uc_engine::ResendEntryOutcome::Completed(
        uc_engine::ResendReportSummary {
            accepted: 0,
            duplicate: 0,
            offline: 0,
            errored: 0,
            pending: 0,
            reconciled: vec![
                uc_engine::ResendTargetProgressSummary {
                    target_device_id: "running-peer".into(),
                    state: uc_engine::OutboundProgressReconciliation::Running,
                    current: Some(example_view()),
                    new_transfer_requested: false,
                    duplicate_files_possible: false,
                },
                uc_engine::ResendTargetProgressSummary {
                    target_device_id: "legacy-peer".into(),
                    state: uc_engine::OutboundProgressReconciliation::Unsupported,
                    current: None,
                    new_transfer_requested: false,
                    duplicate_files_possible: true,
                },
            ],
        },
    ))
    .unwrap();
    assert_eq!(mapped.accepted, Some(0));
    assert_eq!(mapped.reconciled[0].state, "running");
    assert!(!mapped.reconciled[0].new_transfer_requested);
    let current = mapped.reconciled[0].current.as_ref().unwrap();
    assert_eq!(current.revision, u64::MAX.to_string());
    assert_eq!(
        current.items[0].completed_bytes,
        Some((u64::MAX - 1).to_string())
    );
    assert_eq!(current.items[0].total_bytes, None);
    assert_eq!(mapped.reconciled[1].state, "unsupported");
    assert!(mapped.reconciled[1].duplicate_files_possible);
}
