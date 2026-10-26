//! 重开或真实进程重启只恢复可证明的历史归属，不把消息头投递解释为远端文件成果。
use super::{
    batch_outcomes::{assert_file_failure, reconciled_view},
    member_id, persistent_engine_host, receive_case, saved_files, ReleaseOnDrop, ENGINE_TEST_LOCK,
};
use crate::{
    Engine, EngineConfig, Operation, OperationResult, OutboundProgressBatchState,
    OutboundProgressFreshness, OutboundProgressInput, OutboundProgressReconciliation,
    OutboundProgressState, RecoverSessionInput,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_reopened_sender_reads_existing_delivery_as_unconfirmed_without_restoring_attempt(
) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    assert_file_failure(&case, &case.first, 1).await;
    let before = reconciled_view(&case, &case.first, OutboundProgressReconciliation::Settled).await;
    let exports = case.exports.exports.lock().unwrap().clone();
    let protects = case.exports.protects.lock().unwrap().clone();
    case.source.shutdown_until_complete().await.unwrap();
    let (source, events) = Engine::start(
        EngineConfig::new("1.2.3")
            .with_rendezvous_base_url(case._server.uri())
            .with_test_relay_fallback(false),
        persistent_engine_host(case._source_root.path(), case.source_storage.clone()),
    )
    .await
    .unwrap();
    case.source = source;
    case.source_events = events;
    case.source
        .execute(Operation::RecoverSession(RecoverSessionInput {
            allow_secure_storage_unlock: true,
        }))
        .await
        .unwrap();
    assert_eq!(member_id(&case.source).await, case.source_id);
    let OperationResult::OutboundProgress(Some(view)) = case
        .source
        .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
            entry_id: before.entry_id.clone(),
            target_device_id: before.target_device_id.clone(),
        }))
        .await
        .unwrap()
    else {
        panic!("已有持久发送归属必须提供保守未确认视图")
    };
    assert_eq!(view.attempt_id, None);
    assert_eq!(view.freshness, OutboundProgressFreshness::Unconfirmed);
    assert_eq!(view.state, OutboundProgressBatchState::Unconfirmed);
    assert_eq!((view.items_total, view.items_unconfirmed), (3, 3));
    assert_eq!(
        (
            view.items_completed,
            view.items_failed,
            view.items_cancelled,
            view.items_running,
            view.items_waiting
        ),
        (0, 0, 0, 0, 0)
    );
    assert!(view
        .items
        .iter()
        .all(|item| item.state == OutboundProgressState::Unconfirmed
            && item.completed_bytes.is_none()
            && item.total_bytes.is_none()
            && item.round.is_none()));
    let OperationResult::OutboundProgressList(listed) = case
        .source
        .execute(Operation::ListOutboundProgress)
        .await
        .unwrap()
    else {
        panic!("complete retained ownership list")
    };
    assert!(listed.iter().any(|item| item == &view));
    assert_eq!(*case.exports.exports.lock().unwrap(), exports);
    assert_eq!(*case.exports.protects.lock().unwrap(), protects);
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

mod child;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_sender_new_process_does_not_restore_adopted_terminal_results() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    assert_file_failure(&case, &case.first, 1).await;
    let before = reconciled_view(&case, &case.first, OutboundProgressReconciliation::Settled).await;
    let exports = case.exports.exports.lock().unwrap().len();
    case.source.shutdown_until_complete().await.unwrap();
    let (mut process, mut checkpoint) = child::start(
        case._source_root.path().to_owned(),
        &case.source_storage,
        case._server.uri(),
        0,
        child::Mode::Sender {
            entry: before.entry_id,
            peer: before.target_device_id,
        },
    )
    .await;
    assert_eq!(child::line(&mut checkpoint).await, "verified");
    process.kill();
    drop(checkpoint);
    drop(process);
    assert_eq!(case.exports.exports.lock().unwrap().len(), exports);
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_receiver_killed_with_unacked_results_keeps_files_without_restoring_notifications(
) {
    use super::{HostFileHandle, ReconcileOutboundProgressInput, SendFilesInput};
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let peer = member_id(&case.receiver).await;
    let receiver_port = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    case.receiver.shutdown_until_complete().await.unwrap();
    wait_peer(
        &case.source,
        &peer,
        crate::DeviceReachabilitySummary::Offline,
    )
    .await;
    let (mut first, mut checkpoint) = child::start(
        case.receiver_root.path().to_owned(),
        &case.receiver_storage,
        case._server.uri(),
        receiver_port,
        child::Mode::Receive,
    )
    .await;
    wait_peer(
        &case.source,
        &peer,
        crate::DeviceReachabilitySummary::Online,
    )
    .await;
    let OperationResult::EntrySent(sent) = case
        .source
        .execute(Operation::SendFiles(SendFilesInput {
            files: (3..6)
                .map(|index| HostFileHandle::new(index.to_string()))
                .collect(),
            target_devices: vec![peer.clone()],
        }))
        .await
        .unwrap()
    else {
        panic!("real fresh file transfer")
    };
    assert_eq!(sent.total_accepted + sent.total_pending, 1);
    let (entry, attempt): (String, String) =
        serde_json::from_str(&child::line(&mut checkpoint).await).unwrap();
    first.kill();
    drop(checkpoint);
    drop(first);
    wait_peer(
        &case.source,
        &peer,
        crate::DeviceReachabilitySummary::Offline,
    )
    .await;
    let (mut reopened, mut checkpoint) = child::start(
        case.receiver_root.path().to_owned(),
        &case.receiver_storage,
        case._server.uri(),
        receiver_port,
        child::Mode::Saved { entry },
    )
    .await;
    assert_eq!(child::line(&mut checkpoint).await, "verified");
    wait_peer(
        &case.source,
        &peer,
        crate::DeviceReachabilitySummary::Online,
    )
    .await;
    let reconciliation = case
        .source
        .execute(Operation::ReconcileOutboundProgress(
            ReconcileOutboundProgressInput {
                entry_id: sent.entry_id.clone(),
                target_device_id: peer.clone(),
                expected_attempt_id: Some(attempt),
            },
        ))
        .await
        .unwrap();
    assert_eq!(
        reconciliation,
        OperationResult::OutboundProgressReconciliation(
            OutboundProgressReconciliation::NotRetained
        )
    );
    let OperationResult::OutboundProgress(Some(view)) = case
        .source
        .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
            entry_id: sent.entry_id,
            target_device_id: peer,
        }))
        .await
        .unwrap()
    else {
        panic!("source original task")
    };
    assert_eq!(
        (
            view.items_total,
            view.items_completed,
            view.items_unconfirmed
        ),
        (3, 0, 3)
    );
    assert_eq!(view.items_running, 0);
    reopened.kill();
    drop(checkpoint);
    drop(reopened);
    case.shutdown().await;
}

async fn wait_peer(engine: &Engine, peer: &str, expected: crate::DeviceReachabilitySummary) {
    super::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let OperationResult::DeviceGroupChoices(choices) = engine
                .execute(Operation::QueryDeviceGroupChoices)
                .await
                .unwrap()
            else {
                panic!("current peer readiness")
            };
            if choices
                .device_trust
                .devices
                .iter()
                .any(|device| device.device_id == peer && device.reachability == expected)
            {
                break;
            }
            super::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
