use super::{execute_command, operation_response, probe_error, ProbeCommand, ProbeState};
use serde_json::json;
use uc_engine::{
    OperationResult, OutboundProgressBatchState, OutboundProgressFreshness,
    OutboundProgressItemSummary, OutboundProgressReconciliation, OutboundProgressState,
    OutboundProgressSummary, ReceiveItemKind,
};

#[tokio::test]
async fn complete_progress_commands_reach_the_existing_engine_boundary() {
    for value in [
        json!({"command":"query_outbound_progress","entry_id":"entry","target_device_id":"peer"}),
        json!({"command":"list_outbound_progress"}),
        json!({"command":"reconcile_outbound_progress","entry_id":"entry","target_device_id":"peer","expected_attempt_id":"attempt"}),
    ] {
        let command: ProbeCommand = serde_json::from_value(value).unwrap();
        let response = execute_command(&mut ProbeState::default(), command).await;
        assert_eq!(response, probe_error("not_started"));
    }
}

#[test]
fn absent_progress_is_a_successful_missing_view_instead_of_an_unsupported_result() {
    assert_eq!(
        operation_response(OperationResult::OutboundProgress(None)),
        json!({"ok":true,"kind":"outbound_progress","present":false})
    );
}

#[test]
fn progress_summary_preserves_unknown_counts_without_exporting_task_identities() {
    let progress = OutboundProgressSummary {
        entry_id: "sensitive-entry-marker".into(),
        target_device_id: "sensitive-peer-marker".into(),
        attempt_id: Some("sensitive-attempt-marker".into()),
        revision: u64::MAX,
        freshness: OutboundProgressFreshness::Unconfirmed,
        freshness_available: true,
        state: OutboundProgressBatchState::PartiallyUnconfirmed,
        items_total: 2,
        items_waiting: 0,
        items_running: 0,
        items_completed: 1,
        items_failed: 0,
        items_cancelled: 0,
        items_unconfirmed: 1,
        items: vec![
            OutboundProgressItemSummary {
                kind: ReceiveItemKind::File,
                index: 0,
                state: OutboundProgressState::Completed,
                last_known_phase: None,
                round: Some(0),
                completed_bytes: Some(0),
                total_bytes: Some(0),
                failure_reason: None,
                cancellation_reason: None,
            },
            OutboundProgressItemSummary {
                kind: ReceiveItemKind::File,
                index: 1,
                state: OutboundProgressState::Unconfirmed,
                last_known_phase: None,
                round: None,
                completed_bytes: None,
                total_bytes: None,
                failure_reason: None,
                cancellation_reason: None,
            },
        ],
    };
    let response = operation_response(OperationResult::OutboundProgress(Some(progress.clone())));
    assert_eq!(response["ok"], true);
    assert_eq!(response["state"], "partially_unconfirmed");
    assert_eq!(response["freshness"], "unconfirmed");
    assert_eq!(response["items_completed"], 1);
    assert_eq!(response["items_failed"], 0);
    assert_eq!(response["items_unconfirmed"], 1);
    assert_eq!(response["revision"], u64::MAX.to_string());
    assert_eq!(response["has_attempt"], true);
    assert!(!response.to_string().contains("sensitive-"));

    let list = operation_response(OperationResult::OutboundProgressList(vec![progress]));
    assert_eq!(list["ok"], true);
    assert_eq!(list["count"], 1);
    assert_eq!(list["states"], json!(["partially_unconfirmed"]));
    assert!(!list.to_string().contains("sensitive-"));
}

#[test]
fn reconciliation_keeps_each_stable_result_distinct_from_file_completion() {
    for state in [
        OutboundProgressReconciliation::Running,
        OutboundProgressReconciliation::Settled,
        OutboundProgressReconciliation::Superseded,
        OutboundProgressReconciliation::NotRetained,
        OutboundProgressReconciliation::Unsupported,
        OutboundProgressReconciliation::Unreachable,
        OutboundProgressReconciliation::Busy,
    ] {
        let response = operation_response(OperationResult::OutboundProgressReconciliation(state));
        assert_eq!(
            response,
            json!({"ok":true,"kind":"outbound_progress_reconciliation","state":state})
        );
    }
}
