use super::map_receive_progress;
use uc_engine::{
    OperationResult, ReceiveItemFailureReason, ReceiveItemKind, ReceiveItemSummary,
    ReceiveProgressSummary,
};

#[test]
fn receive_mapping_keeps_exact_counts_and_engine_vocabulary() {
    let summary = ReceiveProgressSummary {
        entry_id: "entry".into(),
        attempt_id: "attempt".into(),
        state: "failed".into(),
        total_bytes: i64::MAX,
        completed_bytes: 7,
        items_total: 2,
        items_completed: 1,
        items: vec![ReceiveItemSummary {
            item_id: "stable-item".into(),
            kind: ReceiveItemKind::Representation,
            state: "failed".into(),
            failure_reason: Some(ReceiveItemFailureReason::StorageUnavailable),
        }],
    };
    let result = map_receive_progress(OperationResult::EntryReceiveProgress(Some(summary)))
        .unwrap()
        .unwrap();
    assert_eq!(result.total_bytes, i64::MAX);
    assert_eq!(result.items[0].kind, "representation");
    assert_eq!(
        result.items[0].failure_reason.as_deref(),
        Some("storage_unavailable")
    );
    assert_eq!(result.items[0].item_id, "stable-item");
    assert!(
        map_receive_progress(OperationResult::EntryReceiveProgress(None))
            .unwrap()
            .is_none()
    );
}
