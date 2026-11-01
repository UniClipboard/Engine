//! 真实本地工作尚未结算时，完整源端核对只能采用Processing，不能提前确认本项终态。
mod representation;
use super::{
    progress, receive_case, result_chain, saved_files, ReceiveCase, ReleaseOnDrop, CONTENTS,
    ENGINE_TEST_LOCK,
};
use crate::{
    EntryReceiveRetryOutcome, Operation, OperationResult, OutboundProgressInput,
    OutboundProgressReconciliation, OutboundProgressState, OutboundProgressSummary,
    ReceiveProgressSummary, ReconcileOutboundProgressInput,
};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use uc_core::file_transfer::ProgressIdentity;
use uuid::Uuid;

pub(super) async fn assert_processing(
    case: &ReceiveCase,
    received: &ReceiveProgressSummary,
    index: usize,
) -> OutboundProgressSummary {
    let view = timeout(Duration::from_secs(20), async {
        loop {
            let OperationResult::OutboundProgressList(views) = case
                .source
                .execute(Operation::ListOutboundProgress)
                .await
                .unwrap()
            else {
                panic!("complete outbound list")
            };
            if let Some(view) = views
                .into_iter()
                .find(|view| view.attempt_id.as_deref() == Some(received.attempt_id.as_str()))
            {
                return view;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("actual receive attempt is registered with its message source");
    assert_ne!(view.entry_id, received.entry_id);
    assert_eq!(
        case.source
            .execute(Operation::ReconcileOutboundProgress(
                ReconcileOutboundProgressInput {
                    entry_id: view.entry_id.clone(),
                    target_device_id: view.target_device_id.clone(),
                    expected_attempt_id: Some(received.attempt_id.clone()),
                }
            ))
            .await
            .unwrap(),
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Running),
    );
    let OperationResult::OutboundProgress(Some(current)) = case
        .source
        .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
            entry_id: view.entry_id,
            target_device_id: view.target_device_id,
        }))
        .await
        .unwrap()
    else {
        panic!("same complete source view after reconciliation")
    };
    assert_eq!(
        current.attempt_id.as_deref(),
        Some(received.attempt_id.as_str())
    );
    assert_eq!(
        current.items[index].state,
        OutboundProgressState::Processing
    );
    let source = ProgressIdentity::new(*Uuid::parse_str(&current.entry_id).unwrap().as_bytes());
    let attempt = ProgressIdentity::new(*Uuid::parse_str(&received.attempt_id).unwrap().as_bytes());
    let acknowledgements = case.exports.matching_acks(source, attempt, 0).await;
    assert!(
        acknowledgements
            .iter()
            .all(|notice| usize::from(notice.result.item.index) != index),
        "unsettled local work cannot have a terminal ACK"
    );
    current
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_written_output_stays_processing_until_original_export_returns() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(true).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let path = case.exports.paths.lock().unwrap()[0]
        .canonicalize()
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), CONTENTS[0]);
    let before = assert_processing(&case, &case.first, 0).await;
    assert_eq!(before.items_completed, 0);
    assert_eq!(before.items_failed, 0);
    case.exports.release.add_permits(1);
    let settled = progress(&case.receiver, &case.first.entry_id, "failed").await;
    let saved = saved_files(
        &case.receiver,
        &case.first.entry_id,
        case.receiver_root.path(),
    )
    .await;
    assert_eq!(saved.len(), 2);
    assert_eq!(saved.get(CONTENTS[0]), Some(&path));
    result_chain::assert_chain(
        &case.source,
        &before.entry_id,
        &settled,
        &case.exports,
        true,
    )
    .await;
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_offline_retry_commits_output_while_progress_notification_remains_blocked() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    case.source.shutdown_until_complete().await.unwrap();
    case.exports
        .block_progress_network
        .store(true, Ordering::SeqCst);
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("original cached failed item can be retried offline")
    };
    timeout(Duration::from_secs(20), async {
        case.exports.paused.notified().await;
        case.exports.progress_network_blocked.notified().await;
    })
    .await
    .expect("actual export and permanently blocked notification both started");
    let path = case.exports.paths.lock().unwrap()[3]
        .canonicalize()
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), CONTENTS[1]);
    case.exports.release.add_permits(1);
    let completed = progress(&case.receiver, &case.first.entry_id, "completed").await;
    assert_eq!(completed.attempt_id, attempt_id);
    assert_eq!(completed.items_completed, 3);
    assert!(completed.items.iter().all(|item| item.state == "completed"));
    let saved = saved_files(
        &case.receiver,
        &case.first.entry_id,
        case.receiver_root.path(),
    )
    .await;
    assert_eq!(saved.len(), 3);
    assert_eq!(saved.get(CONTENTS[1]), Some(&path));
    for (bytes, original) in &case.saved {
        assert_eq!(saved.get(bytes.as_slice()), Some(original));
    }
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    assert!(case.exports.block_progress_network.load(Ordering::SeqCst));
    timeout(Duration::from_secs(20), case.shutdown())
        .await
        .expect("local completion and shutdown cannot await notification release");
}
