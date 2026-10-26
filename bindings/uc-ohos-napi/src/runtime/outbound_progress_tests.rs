//! JS边界保持大整数、未知总量与真实阶段，不经浮点或宿主推断。
use super::outbound_progress;
use uc_engine::{
    OutboundProgressBatchState, OutboundProgressFreshness, OutboundProgressItemSummary,
    OutboundProgressPhase, OutboundProgressState, OutboundProgressSummary, ReceiveItemKind,
};

#[test]
fn outbound_view_preserves_large_integer_positions_and_unknown_totals_for_javascript() {
    let view = OutboundProgressSummary {
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
    };
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
