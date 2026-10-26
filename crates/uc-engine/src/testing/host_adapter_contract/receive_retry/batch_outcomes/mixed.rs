//! 同一批次的真实成功、运行、取消和未确认必须分别计数。
use super::super::{
    progress, receive_case, saved_files, ReceiveCase, ReleaseOnDrop, ENGINE_TEST_LOCK,
};
use super::{assert_file_failure, reconciled_view};
use crate::{
    CancelEntryReceiveInput, EngineErrorCategory, EntryReceiveRetryOutcome, Operation,
    OperationResult, OutboundProgressBatchState, OutboundProgressCancellationReason,
    OutboundProgressFreshness, OutboundProgressInput, OutboundProgressReconciliation,
    OutboundProgressState, OutboundProgressSummary, ReconcileOutboundProgressInput, SecretString,
    UnlockSpaceInput,
};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::{sleep, timeout};

async fn active_retry(case: &ReceiveCase) -> OutboundProgressSummary {
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("original failed item retry")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let receiving = progress(&case.receiver, &case.first.entry_id, "receiving").await;
    assert_eq!(receiving.attempt_id, attempt_id);
    let view = reconciled_view(case, &receiving, OutboundProgressReconciliation::Running).await;
    assert_eq!(view.state, OutboundProgressBatchState::InProgress);
    assert_eq!(
        (view.items_total, view.items_completed, view.items_running),
        (3, 2, 1)
    );
    assert_eq!(
        (
            view.items_failed,
            view.items_cancelled,
            view.items_unconfirmed,
            view.items_waiting
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        view.items.iter().map(|item| item.state).collect::<Vec<_>>(),
        [
            OutboundProgressState::Completed,
            OutboundProgressState::Processing,
            OutboundProgressState::Completed
        ]
    );
    view
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_batch_counts_keep_successes_separate_from_running_and_cancelled_retry() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let active = active_retry(&case).await;
    let attempt_id = active.attempt_id.unwrap();
    let cancellation =
        case.receiver
            .execute(Operation::CancelEntryReceive(CancelEntryReceiveInput {
                entry_id: case.first.entry_id.clone(),
                attempt_id: attempt_id.clone(),
            }));
    let release = async {
        let cancelling = progress(&case.receiver, &case.first.entry_id, "cancelling").await;
        assert_eq!(cancelling.attempt_id, attempt_id);
        case.exports.release.add_permits(1);
    };
    let (result, ()) = tokio::join!(cancellation, release);
    result.unwrap();
    let received = progress(&case.receiver, &case.first.entry_id, "cancelled").await;
    let settled = reconciled_view(&case, &received, OutboundProgressReconciliation::Settled).await;
    assert_eq!(
        settled.state,
        OutboundProgressBatchState::PartiallyCancelled
    );
    assert_eq!(settled.freshness, OutboundProgressFreshness::Settled);
    assert_eq!(
        (
            settled.items_total,
            settled.items_completed,
            settled.items_cancelled
        ),
        (3, 2, 1)
    );
    assert_eq!(
        (
            settled.items_failed,
            settled.items_running,
            settled.items_unconfirmed,
            settled.items_waiting
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(settled.items[0], active.items[0]);
    assert_eq!(settled.items[2], active.items[2]);
    assert_eq!(settled.items[1].state, OutboundProgressState::Cancelled);
    assert_eq!(
        settled.items[1].cancellation_reason,
        Some(OutboundProgressCancellationReason::RemotePeer)
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
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_batch_unknown_does_not_become_failure_or_erase_known_successes() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    assert_file_failure(&case, &case.first, 1).await;
    case.exports.hold_terminals.store(true, Ordering::SeqCst);
    let active = active_retry(&case).await;
    timeout(
        Duration::from_secs(20),
        case.exports.terminal_held.notified(),
    )
    .await
    .unwrap();
    let lock = case.receiver.execute(Operation::LockEncryption);
    let release =
        async {
            timeout(Duration::from_secs(20), async {
            loop {
                if matches!(case.receiver.execute(Operation::QueryReceiveReadiness).await.unwrap(),
                    OperationResult::ReceiveReadiness(readiness) if !readiness.ready) { break; }
                sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
            let denied = case
                .source
                .execute(Operation::ReconcileOutboundProgress(
                    ReconcileOutboundProgressInput {
                        entry_id: active.entry_id.clone(),
                        target_device_id: active.target_device_id.clone(),
                        expected_attempt_id: active.attempt_id.clone(),
                    },
                ))
                .await
                .unwrap_err();
            assert_eq!(denied.category(), EngineErrorCategory::Unauthorized);
            assert!(case.exports.paths.lock().unwrap()[3].is_file());
            case.exports.release.add_permits(1);
        };
    let (locked, ()) = tokio::join!(lock, release);
    locked.unwrap();
    case.receiver
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new("real retry fixture"),
        }))
        .await
        .unwrap();
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: active.entry_id.clone(),
                    target_device_id: active.target_device_id.clone(),
                    expected_attempt_id: active.attempt_id.clone(),
                }
            ))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(
            OutboundProgressReconciliation::NotRetained
        )
    );
    let OperationResult::OutboundProgress(Some(unknown)) = case
        .source
        .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
            entry_id: active.entry_id,
            target_device_id: active.target_device_id,
        }))
        .await
        .unwrap()
    else {
        panic!("original retained source view")
    };
    assert_eq!(
        unknown.state,
        OutboundProgressBatchState::PartiallyUnconfirmed
    );
    assert_eq!(unknown.freshness, OutboundProgressFreshness::Unconfirmed);
    assert_eq!(
        (
            unknown.items_total,
            unknown.items_completed,
            unknown.items_unconfirmed
        ),
        (3, 2, 1)
    );
    assert_eq!(
        (
            unknown.items_failed,
            unknown.items_cancelled,
            unknown.items_running,
            unknown.items_waiting
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(unknown.items[0], active.items[0]);
    assert_eq!(unknown.items[2], active.items[2]);
    assert_eq!(unknown.items[1].state, OutboundProgressState::Unconfirmed);
    assert_eq!(unknown.items[1].failure_reason, None);
    for (bytes, path) in &case.saved {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    case.shutdown().await;
}
