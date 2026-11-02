//! 普通同内容再次投递复用既有成功文件，不把整批当成全新下载。
use super::*;
use crate::{
    OutboundProgressBatchState, OutboundProgressFreshness, OutboundProgressInput,
    OutboundProgressState, OutboundProgressSummary, RecoverSessionInput,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_partial_redelivery_reuses_saved_outputs_and_only_fetches_missing_file() {
    verify_redelivery(vec![0, 1, 2], false, Reopen::None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_reordered_partial_redelivery_keeps_original_success_positions() {
    verify_redelivery(vec![1, 2, 0], false, Reopen::None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_completed_redelivery_keeps_held_files_without_new_attempt() {
    verify_redelivery(vec![0, 1, 2], true, Reopen::None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_reopened_completed_redelivery_reports_sender_completion_without_download() {
    verify_redelivery(vec![0, 1, 2], true, Reopen::Both).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_reopened_sender_completed_redelivery_gets_fresh_completion_without_download() {
    verify_redelivery(vec![0, 1, 2], true, Reopen::Sender).await;
}

#[derive(Clone, Copy, PartialEq)]
enum Reopen {
    None,
    Both,
    Sender,
}

async fn verify_redelivery(order: Vec<usize>, repeat_complete: bool, reopen: Reopen) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let entry = case.first.entry_id.clone();
    let original = case.saved.clone();
    let initial_exports = case.exports.exports.lock().unwrap().clone();
    assert_eq!(initial_exports.len(), 3);
    let receiver = member_id(&case.receiver).await;
    let OperationResult::EntrySent(report) = case
        .source
        .execute(Operation::SendFiles(SendFilesInput {
            files: order
                .into_iter()
                .map(|index| HostFileHandle::new(index.to_string()))
                .collect(),
            target_devices: vec![receiver],
        }))
        .await
        .unwrap()
    else {
        panic!("file send expected")
    };
    assert_eq!(report.total_accepted + report.total_pending, 1);
    next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::IncomingPending(pending) if pending.entry_id == entry),
    )
    .await;
    // 原fixture只在第四次真实导出暂停；它必须属于原失败项。
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let redelivered = case.exports.exports.lock().unwrap()[3];
    case.exports.release.add_permits(1);
    assert_eq!(
        redelivered, initial_exports[1],
        "普通重投不能重新获取已有成功项"
    );
    let complete = progress(&case.receiver, &entry, "completed").await;
    assert_ne!(complete.attempt_id, case.first.attempt_id);
    assert_eq!(complete.items_completed, 3);
    let files = saved_files(&case.receiver, &entry, case.receiver_root.path()).await;
    assert_eq!(files.len(), 3);
    for index in [0, 2] {
        assert_eq!(
            files.get(CONTENTS[index]),
            original.get(CONTENTS[index]),
            "普通重投不能替换原成功路径"
        );
        assert_eq!(
            std::fs::read(original.get(CONTENTS[index]).unwrap()).unwrap(),
            CONTENTS[index]
        );
    }
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    assert_eq!(case.exports.protects.lock().unwrap().len(), 4);
    // 消费本次完整结果事件后再关闭，保持真实接收入口的完成边界。
    let source = case.source_id.clone();
    next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == source),
    )
    .await;
    let source_entry = report.entry_id.clone();
    let mut previous_attempt = if repeat_complete {
        sender_completed(
            &case.source,
            &source_entry,
            &member_id(&case.receiver).await,
            None,
        )
        .await
        .attempt_id
    } else {
        None
    };
    if reopen != Reopen::None {
        case.source.shutdown_until_complete().await.unwrap();
        let config = EngineConfig::new("1.2.3")
            .with_rendezvous_base_url(case._server.uri())
            .with_test_relay_fallback(false);
        if reopen == Reopen::Both {
            case.receiver.shutdown_until_complete().await.unwrap();
            let (receiver, events) = Engine::start(
                config.clone(),
                persistent_engine_host(case.receiver_root.path(), case.receiver_storage.clone()),
            )
            .await
            .unwrap();
            case.receiver = receiver;
            case.events = events;
        }
        let host = HostCapabilities::new(
            HostDirectories::new(
                case._source_root.path().join("private"),
                case._source_root.path().join("cache"),
                case._source_root.path().join("temporary"),
                case._source_root.path().join("logs"),
            ),
            Box::new(case.source_storage.clone()),
            Box::new(ReceiveClipboard(case.clipboard.clone())),
            Box::new(Files),
        );
        let (sender, events) = Engine::start(config, host).await.unwrap();
        case.source = sender;
        case.source_events = events;
        for engine in [&case.source]
            .into_iter()
            .chain((reopen == Reopen::Both).then_some(&case.receiver))
        {
            engine
                .execute(Operation::RecoverSession(RecoverSessionInput {
                    allow_secure_storage_unlock: true,
                }))
                .await
                .unwrap();
        }
        let receiver = member_id(&case.receiver).await;
        timeout(Duration::from_secs(30), async {
            loop {
                if matches!(case.source.execute(Operation::QueryPeerConnections).await.unwrap(),
                    OperationResult::PeerConnections(peers)
                        if peers.iter().any(|peer| peer.peer_id == receiver && peer.connected))
                {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
    for repetition in 0..if repeat_complete { 2 } else { 0 } {
        let receiver = member_id(&case.receiver).await;
        let OperationResult::EntrySent(report) = case
            .source
            .execute(Operation::SendFiles(SendFilesInput {
                files: (0..3)
                    .map(|index| HostFileHandle::new(index.to_string()))
                    .collect(),
                target_devices: vec![receiver],
            }))
            .await
            .unwrap()
        else {
            panic!("file send expected")
        };
        next_engine_event_matching(&mut case.events,
            |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == source),
        ).await;
        assert_eq!(
            report.total_accepted + report.total_duplicate,
            1,
            "完整成果须被接收或认作快速重复：{report:?}"
        );
        assert_eq!(
            report.entry_id, source_entry,
            "重复发送复用同一本机来源身份"
        );
        assert_eq!(report.total_errored, 0);
        assert_eq!(report.total_pending, 0);
        let view = sender_completed(
            &case.source,
            &report.entry_id,
            &member_id(&case.receiver).await,
            previous_attempt.as_deref(),
        )
        .await;
        assert_eq!(view.freshness, OutboundProgressFreshness::Settled);
        assert_eq!(view.items_completed, 3);
        assert!(view
            .items
            .iter()
            .all(|item| item.state == OutboundProgressState::Completed));
        assert!(view.attempt_id.is_some());
        previous_attempt = view.attempt_id.clone();

        let repeated = progress(&case.receiver, &entry, "completed").await;
        assert_eq!(repeated.attempt_id, complete.attempt_id);
        assert_eq!(
            saved_files(&case.receiver, &entry, case.receiver_root.path()).await,
            files
        );
        assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
        assert_eq!(case.exports.protects.lock().unwrap().len(), 4);
        if let Ok(path) = std::env::var("UC_HELD_REDELIVERY_EVIDENCE") {
            std::fs::write(
                path,
                serde_json::to_vec_pretty(&serde_json::json!({
                    "scenario": "completed_redelivery",
                    "redeliveries": repetition + 1,
                    "reopened": reopen != Reopen::None,
                    "receiver_reopened": reopen == Reopen::Both,
                    "sender_completed": view.items_completed,
                    "sender_settled": view.freshness == OutboundProgressFreshness::Settled,
                    "receiver_attempt_unchanged": repeated.attempt_id == complete.attempt_id,
                    "exports_before_repeat": 4,
                    "exports_after_repeat": case.exports.exports.lock().unwrap().len(),
                    "saved_files_unchanged": true
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }
    case.shutdown().await;
}

async fn sender_completed(
    source: &Engine,
    entry: &str,
    peer: &str,
    previous_attempt: Option<&str>,
) -> OutboundProgressSummary {
    let mut last = None;
    timeout(Duration::from_secs(10), async {
        loop {
            if let OperationResult::OutboundProgress(Some(view)) = source
                .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
                    entry_id: entry.into(),
                    target_device_id: peer.into(),
                }))
                .await
                .unwrap()
            {
                if view.state == OutboundProgressBatchState::Completed
                    && view.attempt_id.as_deref() != previous_attempt
                {
                    break view;
                }
                last = Some(view);
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("fully held redelivery must complete on the sender: {last:?}"))
}
