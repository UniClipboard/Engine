//! 普通同内容再次投递复用既有成功文件，不把整批当成全新下载。
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_partial_redelivery_reuses_saved_outputs_and_only_fetches_missing_file() {
    verify_redelivery(vec![0, 1, 2], false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_reordered_partial_redelivery_keeps_original_success_positions() {
    verify_redelivery(vec![1, 2, 0], false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_completed_redelivery_keeps_held_files_without_new_attempt() {
    verify_redelivery(vec![0, 1, 2], true).await;
}

async fn verify_redelivery(order: Vec<usize>, repeat_complete: bool) {
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
    if repeat_complete {
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
        assert_eq!(report.total_errored, 0);
        assert_eq!(report.total_pending, 0);
        let repeated = progress(&case.receiver, &entry, "completed").await;
        assert_eq!(repeated.attempt_id, complete.attempt_id);
        assert_eq!(
            saved_files(&case.receiver, &entry, case.receiver_root.path()).await,
            files
        );
        assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
        assert_eq!(case.exports.protects.lock().unwrap().len(), 4);
    }
    case.shutdown().await;
}
