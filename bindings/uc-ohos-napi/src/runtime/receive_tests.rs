use super::receive_progress;
use uc_engine::{
    ReceiveItemFailureReason, ReceiveItemKind, ReceiveItemSummary, ReceiveProgressSummary,
};

#[test]
fn receive_mapping_preserves_integers_beyond_javascript_precision() {
    let summary = ReceiveProgressSummary {
        snapshot_version: None,
        entry_id: "entry".into(),
        attempt_id: "attempt".into(),
        state: "failed".into(),
        total_bytes: i64::MAX,
        completed_bytes: 7,
        items_total: 1,
        items_completed: 0,
        items: vec![ReceiveItemSummary {
            content: None,
            transfer: None,
            history_file_index: None,
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

#[test]
fn retry_mapping_keeps_attempt_only_for_accepted_and_fixed_unavailable_reasons() {
    use super::receive_retry;
    use uc_engine::{EntryReceiveRetryOutcome, ReceiveRetryUnavailableReason};
    for (engine, status, attempt, reason) in [
        (
            EntryReceiveRetryOutcome::Accepted {
                attempt_id: "new-attempt".into(),
            },
            "accepted",
            Some("new-attempt"),
            None,
        ),
        (
            EntryReceiveRetryOutcome::AlreadyReceiving,
            "already_receiving",
            None,
            None,
        ),
        (
            EntryReceiveRetryOutcome::Superseded,
            "superseded",
            None,
            None,
        ),
        (
            EntryReceiveRetryOutcome::CancellationIncomplete,
            "cancellation_incomplete",
            None,
            None,
        ),
        (
            EntryReceiveRetryOutcome::Unavailable {
                reason: ReceiveRetryUnavailableReason::SourceUnavailable,
            },
            "unavailable",
            None,
            Some("source_unavailable"),
        ),
        (
            EntryReceiveRetryOutcome::Unavailable {
                reason: ReceiveRetryUnavailableReason::AccessDenied,
            },
            "unavailable",
            None,
            Some("access_denied"),
        ),
    ] {
        let result = receive_retry(engine);
        assert_eq!(result.status, status);
        assert_eq!(result.attempt_id.as_deref(), attempt);
        assert_eq!(result.reason.as_deref(), reason);
    }
}
