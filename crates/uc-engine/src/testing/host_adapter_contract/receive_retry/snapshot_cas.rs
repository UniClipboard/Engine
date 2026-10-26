//! 查询结束后的快照不授予下载权限，重发仍经过原接收owner最终当前尝试及清理围栏。
use super::super::wait_entry_delivered;
use super::{
    batch_outcomes::{assert_file_failure, reconciled_view},
    member_id, next_engine_event_matching, progress, receive_case, saved_files, ReleaseOnDrop,
    ENGINE_TEST_LOCK,
};
use crate::{
    EngineErrorCategory, EngineEvent, EntryDeliveryStatusSummary, EntryReceiveProgressInput,
    EntryReceiveRetryOutcome, HistoryEntryInput, Operation, OperationResult, OutboundProgressInput,
    OutboundProgressReconciliation, ReceiveRetryUnavailableReason, ReconcileOutboundProgressInput,
    ResendEntryInput, ResendEntryOutcome,
};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_resend_from_a_snapshot_taken_before_retry_keeps_the_current_receive_attempt() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    assert_file_failure(&case, &case.first, 1).await;
    let EngineEvent::InboundNotice(initial_notice) = next_engine_event_matching(&mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == case.source_id)
    ).await else { panic!("initial actual inbound action finished") };
    let OperationResult::OutboundProgressList(views) = case
        .source
        .execute(Operation::ListOutboundProgress)
        .await
        .unwrap()
    else {
        panic!("original send intent")
    };
    let original = views
        .into_iter()
        .find(|view| view.attempt_id.as_deref() == Some(case.first.attempt_id.as_str()))
        .unwrap();
    let target = member_id(&case.receiver).await;
    case.exports
        .hold_registrations
        .store(true, Ordering::SeqCst);
    case.exports
        .pause_snapshot_reply
        .store(true, Ordering::SeqCst);
    let resend = case
        .source
        .execute(Operation::ResendEntry(ResendEntryInput {
            entry_id: original.entry_id.clone(),
            target_devices: vec![target.clone()],
            confirm_new_transfer: true,
        }));
    let replace = async {
        timeout(
            Duration::from_secs(5),
            case.exports.snapshot_held.notified(),
        )
        .await
        .unwrap();
        let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
            case.receiver
                .execute(Operation::RetryEntryReceive(case.retry_input()))
                .await
                .unwrap()
        else {
            panic!("actual local retry wins the original expected-attempt CAS")
        };
        assert_ne!(attempt_id, case.first.attempt_id);
        timeout(Duration::from_secs(5), async {
            case.exports.paused.notified().await;
            case.exports.registration_held.notified().await;
        })
        .await
        .unwrap();
        let active = progress(&case.receiver, &case.first.entry_id, "receiving").await;
        assert_eq!(active.attempt_id, attempt_id);
        let OperationResult::OutboundProgress(Some(before)) = case
            .source
            .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
                entry_id: original.entry_id.clone(),
                target_device_id: target.clone(),
            }))
            .await
            .unwrap()
        else {
            panic!("old projection is still awaiting the new registration")
        };
        assert_eq!(before.attempt_id, original.attempt_id);
        case.exports.snapshot_release.add_permits(1);
        attempt_id
    };
    let (resent, attempt) = tokio::join!(resend, replace);
    let OperationResult::EntryResent(ResendEntryOutcome::Completed(report)) = resent.unwrap()
    else {
        panic!("real resend completes through the original delivery action")
    };
    assert_eq!(report.reconciled.len(), 1);
    let queried = &report.reconciled[0];
    assert_eq!(queried.state, OutboundProgressReconciliation::Settled);
    assert!(queried.new_transfer_requested);
    assert_eq!(
        queried.current.as_ref().unwrap().attempt_id,
        original.attempt_id
    );
    assert_eq!(
        report.pending, 1,
        "原完整接收仍持身份锁，重发只能保持原在途动作"
    );
    assert_eq!(
        report.accepted + report.duplicate + report.offline + report.errored,
        0
    );
    let OperationResult::EntryDelivery(pending) = case
        .source
        .execute(Operation::QueryEntryDelivery(HistoryEntryInput {
            entry_id: original.entry_id.clone(),
        }))
        .await
        .unwrap()
    else {
        panic!("actual deferred delivery record")
    };
    assert!(pending
        .deliveries
        .iter()
        .any(|delivery| delivery.target_device_id == target
            && delivery.status == EntryDeliveryStatusSummary::Pending));
    let still_running = progress(&case.receiver, &case.first.entry_id, "receiving").await;
    assert_eq!(still_running.attempt_id, attempt);
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    assert_eq!(case.exports.protects.lock().unwrap().len(), 4);
    assert_eq!(
        saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path()
        )
        .await,
        case.saved
    );
    case.exports
        .hold_registrations
        .store(false, Ordering::SeqCst);
    case.exports.registration_release.add_permits(1);
    case.exports.release.add_permits(1);
    let completed = progress(&case.receiver, &case.first.entry_id, "completed").await;
    assert_eq!(completed.attempt_id, attempt);
    let EngineEvent::InboundNotice(repeated_notice) = next_engine_event_matching(&mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == case.source_id)
    ).await else { panic!("actual network redelivery finished after the original owner released its lock") };
    assert_eq!(repeated_notice.snapshot_hash, initial_notice.snapshot_hash);
    wait_entry_delivered(&case.source, &original.entry_id, &target).await;

    assert_eq!(
        progress(&case.receiver, &case.first.entry_id, "completed")
            .await
            .attempt_id,
        attempt
    );

    let settled = reconciled_view(&case, &completed, OutboundProgressReconciliation::Settled).await;
    assert_eq!(
        settled.attempt_id.as_deref(),
        Some(completed.attempt_id.as_str())
    );
    let saved = saved_files(
        &case.receiver,
        &case.first.entry_id,
        case.receiver_root.path(),
    )
    .await;
    assert_eq!(saved.len(), 3);
    for (bytes, path) in &case.saved {
        assert_eq!(saved.get(bytes), Some(path));
    }
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    assert_eq!(case.exports.protects.lock().unwrap().len(), 4);
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_snapshot_returning_after_receiver_deletion_cannot_authorize_a_stale_retry() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    assert_file_failure(&case, &case.first, 1).await;
    let OperationResult::OutboundProgressList(views) = case
        .source
        .execute(Operation::ListOutboundProgress)
        .await
        .unwrap()
    else {
        panic!("original send intent")
    };
    let original = views
        .into_iter()
        .find(|view| view.attempt_id.as_deref() == Some(case.first.attempt_id.as_str()))
        .unwrap();
    let stale = case.retry_input();
    let exports = case.exports.exports.lock().unwrap().clone();
    let protects = case.exports.protects.lock().unwrap().clone();
    let target = member_id(&case.receiver).await;
    case.exports
        .pause_snapshot_reply
        .store(true, Ordering::SeqCst);
    let resend = case
        .source
        .execute(Operation::ResendEntry(ResendEntryInput {
            entry_id: original.entry_id.clone(),
            target_devices: vec![target.clone()],
            confirm_new_transfer: false,
        }));
    let delete = async {
        timeout(
            Duration::from_secs(5),
            case.exports.snapshot_held.notified(),
        )
        .await
        .unwrap();
        assert_eq!(
            case.receiver
                .execute(Operation::DeleteHistoryEntry(HistoryEntryInput {
                    entry_id: case.first.entry_id.clone(),
                }))
                .await
                .unwrap(),
            OperationResult::HistoryEntryDeleted
        );
        assert!(case.saved.values().all(|path| !path.exists()));
        case.exports.snapshot_release.add_permits(1);
    };
    let (resent, ()) = tokio::join!(resend, delete);
    let OperationResult::EntryResent(ResendEntryOutcome::Completed(report)) = resent.unwrap()
    else {
        panic!("late actual snapshot completes original reconciliation")
    };
    assert_eq!(
        report.reconciled[0].state,
        OutboundProgressReconciliation::Settled
    );
    assert_eq!(
        report.reconciled[0].current.as_ref().unwrap().attempt_id,
        original.attempt_id
    );
    assert!(!report.reconciled[0].new_transfer_requested);
    assert_eq!(
        report.accepted + report.duplicate + report.offline + report.errored + report.pending,
        0
    );
    assert_eq!(
        case.receiver
            .execute(Operation::RetryEntryReceive(stale))
            .await
            .unwrap(),
        OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Unavailable {
            reason: ReceiveRetryUnavailableReason::SourceUnavailable,
        })
    );
    assert_eq!(
        case.receiver
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: case.first.entry_id.clone(),
            }))
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::NotFound
    );
    assert_eq!(
        case.receiver
            .execute(Operation::QueryEntryReceiveProgress(
                EntryReceiveProgressInput {
                    entry_id: case.first.entry_id.clone(),
                }
            ))
            .await
            .unwrap(),
        OperationResult::EntryReceiveProgress(None)
    );
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: original.entry_id.clone(),
                    target_device_id: target.clone(),
                    expected_attempt_id: original.attempt_id.clone(),
                }
            ))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(
            OutboundProgressReconciliation::NotRetained
        )
    );
    let OperationResult::OutboundProgress(Some(known)) = case
        .source
        .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
            entry_id: original.entry_id,
            target_device_id: target,
        }))
        .await
        .unwrap()
    else {
        panic!("known historical terminal facts")
    };
    assert_eq!(known.items, original.items);
    assert_eq!(*case.exports.exports.lock().unwrap(), exports);
    assert_eq!(*case.exports.protects.lock().unwrap(), protects);
    assert!(case.saved.values().all(|path| !path.exists()));
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_source_deletion_during_resend_query_retires_the_old_action_and_keeps_remote_files(
) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    assert_file_failure(&case, &case.first, 1).await;
    let OperationResult::OutboundProgressList(views) = case
        .source
        .execute(Operation::ListOutboundProgress)
        .await
        .unwrap()
    else {
        panic!("original send intent")
    };
    let original = views
        .into_iter()
        .find(|view| view.attempt_id.as_deref() == Some(case.first.attempt_id.as_str()))
        .unwrap();
    let target = member_id(&case.receiver).await;
    let exports = case.exports.exports.lock().unwrap().clone();
    let protects = case.exports.protects.lock().unwrap().clone();
    case.exports
        .pause_snapshot_reply
        .store(true, Ordering::SeqCst);
    let resend = case
        .source
        .execute(Operation::ResendEntry(ResendEntryInput {
            entry_id: original.entry_id.clone(),
            target_devices: vec![target.clone()],
            confirm_new_transfer: true,
        }));
    let delete = async {
        timeout(
            Duration::from_secs(5),
            case.exports.snapshot_held.notified(),
        )
        .await
        .unwrap();
        assert_eq!(
            case.source
                .execute(Operation::DeleteHistoryEntry(HistoryEntryInput {
                    entry_id: original.entry_id.clone(),
                }))
                .await
                .unwrap(),
            OperationResult::HistoryEntryDeleted
        );
        case.exports.snapshot_release.add_permits(1);
    };
    let (resent, ()) = tokio::join!(resend, delete);
    let OperationResult::EntryResent(ResendEntryOutcome::Completed(report)) = resent.unwrap()
    else {
        panic!("retired original action returns its complete reconciliation outcome")
    };
    assert_eq!(report.reconciled.len(), 1);
    assert_eq!(
        report.reconciled[0].state,
        OutboundProgressReconciliation::Superseded
    );
    assert!(report.reconciled[0].current.is_none());
    assert!(
        !report.reconciled[0].new_transfer_requested,
        "明确重发也不能越过本机删除世代"
    );
    assert_eq!(
        report.accepted + report.duplicate + report.offline + report.errored + report.pending,
        0
    );
    assert_eq!(
        case.source
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: original.entry_id.clone(),
            }))
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::NotFound
    );
    assert_eq!(
        case.source
            .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
                entry_id: original.entry_id.clone(),
                target_device_id: target.clone(),
            }))
            .await
            .unwrap(),
        OperationResult::OutboundProgress(None)
    );
    let OperationResult::OutboundProgressList(remaining) = case
        .source
        .execute(Operation::ListOutboundProgress)
        .await
        .unwrap()
    else {
        panic!("complete source views after deletion")
    };
    assert!(remaining
        .iter()
        .all(|view| view.entry_id != original.entry_id));
    assert_eq!(
        case.source
            .execute(Operation::ResendEntry(ResendEntryInput {
                entry_id: original.entry_id.clone(),
                target_devices: vec![target],
                confirm_new_transfer: true,
            }))
            .await
            .unwrap(),
        OperationResult::EntryResent(ResendEntryOutcome::EntryNotFound {
            entry_id: original.entry_id
        })
    );
    assert_eq!(
        progress(&case.receiver, &case.first.entry_id, "failed")
            .await
            .attempt_id,
        case.first.attempt_id
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
    assert_eq!(*case.exports.exports.lock().unwrap(), exports);
    assert_eq!(*case.exports.protects.lock().unwrap(), protects);
    case.shutdown().await;
}
