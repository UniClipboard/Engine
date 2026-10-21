use super::receive_progress;
use uc_engine::{
    ReceiveItemFailureReason, ReceiveItemKind, ReceiveItemSummary, ReceiveProgressSummary,
};

#[test]
fn receive_mapping_preserves_integers_beyond_javascript_precision() {
    let summary = ReceiveProgressSummary {
        entry_id: "entry".into(),
        attempt_id: "attempt".into(),
        state: "failed".into(),
        total_bytes: i64::MAX,
        completed_bytes: 7,
        items_total: 1,
        items_completed: 0,
        items: vec![ReceiveItemSummary {
            item_id: "directory_unit".into(),
            kind: ReceiveItemKind::DirectoryUnit,
            state: "failed".into(),
            failure_reason: Some(ReceiveItemFailureReason::Interrupted),
        }],
    };
    let result = receive_progress(summary);
    assert_eq!(result.total_bytes, i64::MAX.to_string());
    assert_eq!(result.completed_bytes, "7");
    assert_eq!(result.items[0].kind, "directory_unit");
    assert_eq!(
        result.items[0].failure_reason.as_deref(),
        Some("interrupted")
    );
}
