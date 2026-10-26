//! 原单项重试在源端切换尝试，既有成功成果和未选项保持。
use super::super::{progress, receive_case, saved_files, ReleaseOnDrop, ENGINE_TEST_LOCK};
use super::{assert_file_failure, reconciled_view};
use crate::{
    CancelEntryReceiveInput, EntryReceiveCancellationOutcome, EntryReceiveRetryOutcome, Operation,
    OperationResult, OutboundProgressBatchState, OutboundProgressReconciliation,
    OutboundProgressState, ReconcileOutboundProgressInput,
};
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_manual_retry_replaces_source_attempt_and_preserves_unselected_successes() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    assert_file_failure(&case, &case.first, 1).await;
    let original =
        reconciled_view(&case, &case.first, OutboundProgressReconciliation::Settled).await;
    let initial_exports = case.exports.exports.lock().unwrap().clone();
    let initial_protects = case.exports.protects.lock().unwrap().clone();
    assert_eq!(initial_exports.len(), 3);
    assert_eq!(initial_protects.len(), 3);
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("original selected failure retry")
    };
    assert_ne!(attempt_id, case.first.attempt_id);
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let receiving = progress(&case.receiver, &case.first.entry_id, "receiving").await;
    assert_eq!(receiving.attempt_id, attempt_id);
    let active = reconciled_view(&case, &receiving, OutboundProgressReconciliation::Running).await;
    assert_eq!(active.state, OutboundProgressBatchState::InProgress);
    assert_eq!(
        (
            active.items_completed,
            active.items_running,
            active.items_failed
        ),
        (2, 1, 0)
    );
    assert_eq!(active.items[1].state, OutboundProgressState::Processing);
    for index in [0, 2] {
        assert_eq!(active.items[index], original.items[index]);
    }
    assert_eq!(
        case.receiver
            .execute(Operation::CancelEntryReceive(CancelEntryReceiveInput {
                entry_id: case.first.entry_id.clone(),
                attempt_id: case.first.attempt_id.clone(),
            }))
            .await
            .unwrap(),
        OperationResult::EntryReceiveCancellation(EntryReceiveCancellationOutcome::Superseded)
    );
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: original.entry_id.clone(),
                    target_device_id: original.target_device_id.clone(),
                    expected_attempt_id: original.attempt_id.clone(),
                }
            ))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Superseded)
    );
    assert_eq!(
        saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path()
        )
        .await,
        case.saved
    );
    case.exports.release.add_permits(1);
    let completed = progress(&case.receiver, &case.first.entry_id, "completed").await;
    assert_eq!(completed.attempt_id, attempt_id);
    let settled = reconciled_view(&case, &completed, OutboundProgressReconciliation::Settled).await;
    assert_eq!(settled.state, OutboundProgressBatchState::Completed);
    assert_eq!(
        (
            settled.items_completed,
            settled.items_failed,
            settled.items_running
        ),
        (3, 0, 0)
    );
    for index in [0, 2] {
        assert_eq!(settled.items[index], original.items[index]);
    }
    let saved = saved_files(
        &case.receiver,
        &case.first.entry_id,
        case.receiver_root.path(),
    )
    .await;
    assert_eq!(saved.len(), 3);
    for (bytes, path) in &case.saved {
        assert_eq!(saved.get(bytes), Some(path));
        assert_eq!(std::fs::read(path).unwrap(), *bytes);
    }
    for (calls, original_calls) in [
        (&case.exports.protects, &initial_protects),
        (&case.exports.exports, &initial_exports),
    ] {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 4);
        assert_eq!(&calls[..3], original_calls.as_slice());
        assert_eq!(calls[3], initial_exports[1]);
    }
    case.shutdown().await;
}
