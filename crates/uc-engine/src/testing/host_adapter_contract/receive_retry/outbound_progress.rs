//! 稳定发送侧操作读取真实接收、SQLite结算和文件成果；核对不能启动新下载。
use super::*;
use crate::{
    OutboundProgressBatchState, OutboundProgressFreshness, OutboundProgressInput,
    OutboundProgressReconciliation, OutboundProgressState, ReconcileOutboundProgressInput,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_outbound_progress_reports_actual_partial_result_and_reconciles_without_download(
) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    next_engine_event_matching(&mut case.source_events, |event| {
        matches!(event, EngineEvent::OutboundProgressChanged)
    })
    .await;
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
            if let Some(view) = views.into_iter().find(|view| {
                view.attempt_id.as_deref() == Some(case.first.attempt_id.as_str())
                    && view.state == OutboundProgressBatchState::PartiallyFailed
            }) {
                return view;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("actual receiver settlement reaches original sender");
    assert_ne!(
        view.entry_id, case.first.entry_id,
        "source and local received entries have independent identities"
    );
    assert_eq!(view.freshness, OutboundProgressFreshness::Settled);
    assert_eq!(
        (
            view.items_total,
            view.items_completed,
            view.items_failed,
            view.items_unconfirmed
        ),
        (3, 2, 1, 0)
    );
    assert_eq!(
        view.items.iter().map(|item| item.state).collect::<Vec<_>>(),
        [
            OutboundProgressState::Completed,
            OutboundProgressState::Failed,
            OutboundProgressState::Completed
        ]
    );
    assert_eq!(
        view.items[1].failure_reason,
        Some(ReceiveItemFailureReason::StorageUnavailable)
    );
    let OperationResult::OutboundProgress(Some(queried)) = case
        .source
        .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
            entry_id: view.entry_id.clone(),
            target_device_id: view.target_device_id.clone(),
        }))
        .await
        .unwrap()
    else {
        panic!("same retained complete view")
    };
    assert_eq!(queried.items, view.items);
    let before = case.exports.exports.lock().unwrap().len();
    let result = case
        .source
        .execute(Operation::ReconcileOutboundProgress(
            ReconcileOutboundProgressInput {
                entry_id: view.entry_id.clone(),
                target_device_id: view.target_device_id.clone(),
                expected_attempt_id: view.attempt_id.clone(),
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        result,
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Settled)
    );
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        before,
        "query must not download again"
    );
    let result = case
        .source
        .execute(Operation::ReconcileOutboundProgress(
            ReconcileOutboundProgressInput {
                entry_id: view.entry_id,
                target_device_id: view.target_device_id,
                expected_attempt_id: Some("00000000-0000-0000-0000-000000000001".into()),
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        result,
        OperationResult::OutboundProgressReconciliation(OutboundProgressReconciliation::Superseded)
    );
    assert_eq!(case.exports.exports.lock().unwrap().len(), before);
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
