use super::{
    map_entry_receive_cancellation, map_inbound_transfer_cancellation, map_receive_progress,
};
use crate::BindingError;
use uc_engine::{
    EntryReceiveCancellationOutcome, InboundTransferCancellationOutcome, OperationResult,
    ReceiveItemFailureReason, ReceiveItemKind, ReceiveItemSummary, ReceiveProgressSummary,
};

#[test]
fn receive_mapping_keeps_exact_counts_and_engine_vocabulary() {
    let summary = ReceiveProgressSummary {
        snapshot_version: None,
        entry_id: "entry".into(),
        attempt_id: "attempt".into(),
        state: "failed".into(),
        total_bytes: i64::MAX,
        completed_bytes: 7,
        items_total: 2,
        items_completed: 1,
        items: vec![ReceiveItemSummary {
            content: None,
            transfer: None,
            history_file_index: None,
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

#[test]
fn cancellation_mapping_keeps_incomplete_distinct_from_cancelled_and_not_inflight() {
    assert_eq!(
        map_entry_receive_cancellation(OperationResult::EntryReceiveCancellation(
            EntryReceiveCancellationOutcome::CancellationIncomplete
        ))
        .unwrap(),
        "cancellation_incomplete"
    );
    assert_eq!(
        map_entry_receive_cancellation(OperationResult::EntryReceiveCancellation(
            EntryReceiveCancellationOutcome::Cancelled
        ))
        .unwrap(),
        "cancelled"
    );
    assert_eq!(
        map_inbound_transfer_cancellation(OperationResult::InboundTransferCancellation(
            InboundTransferCancellationOutcome::CancellationIncomplete
        ))
        .unwrap(),
        "cancellation_incomplete"
    );
    assert_eq!(
        map_inbound_transfer_cancellation(OperationResult::InboundTransferCancellation(
            InboundTransferCancellationOutcome::NotInflight
        ))
        .unwrap(),
        "not_inflight"
    );
    assert!(matches!(
        map_entry_receive_cancellation(OperationResult::EntryReceiveProgress(None)),
        Err(BindingError::UnexpectedResult)
    ));
}

#[test]
fn retry_mapping_preserves_new_attempt_and_all_nonaccepted_outcomes() {
    use super::{map_entry_receive_retry, ReceiveRetryOutcome};
    use uc_engine::{EntryReceiveRetryOutcome as EngineOutcome, ReceiveRetryUnavailableReason};
    for (engine, binding) in [
        (
            EngineOutcome::Accepted {
                attempt_id: "new-attempt".into(),
            },
            ReceiveRetryOutcome::Accepted {
                attempt_id: "new-attempt".into(),
            },
        ),
        (
            EngineOutcome::AlreadyReceiving,
            ReceiveRetryOutcome::AlreadyReceiving,
        ),
        (EngineOutcome::Superseded, ReceiveRetryOutcome::Superseded),
        (
            EngineOutcome::CancellationIncomplete,
            ReceiveRetryOutcome::CancellationIncomplete,
        ),
        (
            EngineOutcome::Unavailable {
                reason: ReceiveRetryUnavailableReason::SourceUnavailable,
            },
            ReceiveRetryOutcome::Unavailable {
                reason: "source_unavailable".into(),
            },
        ),
        (
            EngineOutcome::Unavailable {
                reason: ReceiveRetryUnavailableReason::AccessDenied,
            },
            ReceiveRetryOutcome::Unavailable {
                reason: "access_denied".into(),
            },
        ),
    ] {
        assert_eq!(
            map_entry_receive_retry(OperationResult::EntryReceiveRetry(engine)).unwrap(),
            binding
        );
    }
    assert!(matches!(
        map_entry_receive_retry(OperationResult::EntryReceiveProgress(None)),
        Err(BindingError::UnexpectedResult)
    ));
}
