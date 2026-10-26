//! 原生下载与本地结算在反向终态超出实际64项保留容量时继续，由原owner完成取消和清理。
use super::{
    batch_outcomes::assert_file_failure, file_bytes, member_id, next_engine_event_matching,
    progress, receive_case, saved_files, ReleaseOnDrop, ENGINE_TEST_LOCK,
};
use crate::{
    CancelEntryReceiveInput, EngineEvent, EntryReceiveCancellationOutcome, HostFileHandle,
    Operation, OperationResult, OutboundProgressBatchState, OutboundProgressFreshness,
    OutboundProgressInput, OutboundProgressReconciliation, OutboundProgressState,
    ReconcileOutboundProgressInput, SendFilesInput,
};
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::timeout;
use uc_core::file_transfer::{ProgressIdentity, TERMINAL_PEER_CAPACITY};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_terminal_capacity_overflow_does_not_block_cold_downloads_cancel_or_cleanup() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    assert_file_failure(&case, &case.first, 1).await;
    let original_misses = case.exports.cache_misses.lock().unwrap().len();
    let original_exports = case.exports.exports.lock().unwrap().len();
    let original_protects = case.exports.protects.lock().unwrap().len();
    let count = TERMINAL_PEER_CAPACITY + 3;
    let completed_count = TERMINAL_PEER_CAPACITY + 1;
    // 超容量的65项先真正提交；第66项已下载并写出，但仍由原输出步骤持完成权。
    *case.exports.paused_exports.lock().unwrap() =
        BTreeSet::from([original_exports + completed_count]);
    case.exports.hold_terminals.store(true, Ordering::SeqCst);
    let target = member_id(&case.receiver).await;
    // 大批次会在发送等待回执时产生超过有界提示容量的事件，接收提示须与发送并行消费。
    let sending = case.source.execute(Operation::SendFiles(SendFilesInput {
        files: (3..3 + count)
            .map(|index| HostFileHandle::new(index.to_string()))
            .collect(),
        target_devices: vec![target.clone()],
    }));
    let pending = next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::IncomingPending(pending) if pending.entry_id != case.first.entry_id),
    );
    let (sent, pending) = tokio::join!(sending, pending);
    let OperationResult::EntrySent(report) = sent.unwrap() else {
        panic!("fresh batch is accepted by original send action")
    };
    assert_eq!(report.total_accepted + report.total_pending, 1);
    let EngineEvent::IncomingPending(pending) = pending else {
        panic!("new receive intent")
    };
    timeout(Duration::from_secs(30), async {
        case.exports.paused.notified().await;
        case.exports.terminal_held.notified().await;
    })
    .await
    .expect("actual downloads and output continue while terminal delivery is held");
    let receiving = progress(&case.receiver, &pending.entry_id, "receiving").await;
    assert_eq!(receiving.items.len(), count);
    assert_eq!(receiving.items_completed as usize, completed_count);
    assert!(receiving.items[..completed_count]
        .iter()
        .all(|item| item.state == "completed"));
    let saved = saved_files(&case.receiver, &pending.entry_id, case.receiver_root.path()).await;
    assert_eq!(saved.len(), completed_count);
    for index in 3..3 + completed_count {
        let bytes = file_bytes(&HostFileHandle::new(index.to_string())).unwrap();
        let path = saved
            .get(bytes.as_ref())
            .expect("each committed independent file is readable");
        assert_eq!(std::fs::read(path).unwrap(), bytes.as_ref());
    }
    let held_output =
        case.exports.paths.lock().unwrap()[original_exports + completed_count].clone();
    assert!(held_output.is_file());
    let source = ProgressIdentity::new(*Uuid::parse_str(&report.entry_id).unwrap().as_bytes());
    let attempt =
        ProgressIdentity::new(*Uuid::parse_str(&receiving.attempt_id).unwrap().as_bytes());
    assert!(case
        .exports
        .matching_acks(source, attempt, 0)
        .await
        .is_empty());
    let cancel = case
        .receiver
        .execute(Operation::CancelEntryReceive(CancelEntryReceiveInput {
            entry_id: pending.entry_id.clone(),
            attempt_id: receiving.attempt_id.clone(),
        }));
    let settle = async {
        let cancelling = progress(&case.receiver, &pending.entry_id, "cancelling").await;
        assert_eq!(cancelling.attempt_id, receiving.attempt_id);
        assert!(held_output.is_file(), "真实输出未交还完成权前不能提前清理");
        case.exports.release.add_permits(1);
    };
    let (cancelled, ()) = tokio::join!(timeout(Duration::from_secs(6), cancel), settle);
    assert_eq!(
        cancelled.expect("取消不等待终态通知容量").unwrap(),
        OperationResult::EntryReceiveCancellation(EntryReceiveCancellationOutcome::Cancelled)
    );
    let received = progress(&case.receiver, &pending.entry_id, "cancelled").await;
    assert_eq!(received.attempt_id, receiving.attempt_id);
    assert_eq!(received.items_completed as usize, completed_count);
    assert!(received.items[completed_count..]
        .iter()
        .all(|item| item.state == "cancelled"));
    assert!(!held_output.exists());
    assert_eq!(
        saved_files(&case.receiver, &pending.entry_id, case.receiver_root.path()).await,
        saved
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
    let exports = case.exports.exports.lock().unwrap().clone();
    let protects = case.exports.protects.lock().unwrap().clone();
    let misses = case.exports.cache_misses.lock().unwrap().clone();
    assert_eq!(exports.len() - original_exports, completed_count + 1);
    assert_eq!(protects.len() - original_protects, completed_count + 1);
    assert_eq!(misses.len() - original_misses, completed_count + 1);
    assert_eq!(&misses[original_misses..], &protects[original_protects..]);
    assert_eq!(
        misses[original_misses..]
            .iter()
            .collect::<HashSet<_>>()
            .len(),
        completed_count + 1
    );
    // 解除原wire屏障；只允许实际保留的64项获得原生精确ACK，淘汰项不能被观察循环重新领取。
    case.exports.hold_terminals.store(false, Ordering::SeqCst);
    case.exports.terminal_release.add_permits(1);
    let acks = case
        .exports
        .matching_acks(source, attempt, TERMINAL_PEER_CAPACITY)
        .await;
    assert_eq!(acks.len(), TERMINAL_PEER_CAPACITY);
    assert_eq!(
        acks.iter()
            .map(|notice| usize::from(notice.result.item.index))
            .collect::<BTreeSet<_>>(),
        (count - TERMINAL_PEER_CAPACITY..count).collect()
    );
    let input = OutboundProgressInput {
        entry_id: report.entry_id.clone(),
        target_device_id: target.clone(),
    };
    let OperationResult::OutboundProgress(Some(before)) = case
        .source
        .execute(Operation::QueryOutboundProgress(input.clone()))
        .await
        .unwrap()
    else {
        panic!("original complete projection")
    };
    assert_eq!(before.items.len(), count);
    assert!(before.items[..count - TERMINAL_PEER_CAPACITY]
        .iter()
        .all(
            |item| item.state != OutboundProgressState::Completed && item.failure_reason.is_none()
        ));
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: report.entry_id,
                    target_device_id: target,
                    expected_attempt_id: Some(received.attempt_id.clone()),
                }
            ))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Settled)
    );
    let OperationResult::OutboundProgress(Some(view)) = case
        .source
        .execute(Operation::QueryOutboundProgress(input))
        .await
        .unwrap()
    else {
        panic!("adopted original receiver facts")
    };
    assert_eq!(view.state, OutboundProgressBatchState::PartiallyCancelled);
    assert_eq!(view.freshness, OutboundProgressFreshness::Settled);
    assert_eq!(
        view.attempt_id.as_deref(),
        Some(received.attempt_id.as_str())
    );
    assert_eq!(
        (
            view.items_total as usize,
            view.items_completed as usize,
            view.items_cancelled
        ),
        (count, completed_count, 2)
    );
    assert_eq!(
        (
            view.items_failed,
            view.items_running,
            view.items_unconfirmed,
            view.items_waiting
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(*case.exports.exports.lock().unwrap(), exports);
    assert_eq!(*case.exports.protects.lock().unwrap(), protects);
    assert_eq!(*case.exports.cache_misses.lock().unwrap(), misses);
    case.shutdown().await;
}
