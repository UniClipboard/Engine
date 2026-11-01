//! 真实资料关闭后重开：丢失原来源材料时必须拒绝重试，并保留既有部分成果。
use super::super::{
    next_engine_event_matching, persistent_engine_host, progress, receive_case, saved_files,
    Engine, EngineConfig, EngineEvent, EntryReceiveProgressInput, EntryReceiveRetryOutcome,
    Operation, OperationResult, ReceiveRetryUnavailableReason, ReleaseOnDrop, SecretString,
    ENGINE_TEST_LOCK,
};
use crate::{RecoverSessionInput, UnlockSpaceInput};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_restart_refuses_retry_without_original_material_and_preserves_saved_files() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let original = case.first.clone();
    let original_files = case.saved.clone();
    let source = case.source_id.clone();
    next_engine_event_matching(
        &mut case.events,
        |event| matches!(event, EngineEvent::InboundNotice(notice) if notice.from_device == source),
    )
    .await;
    // 先停止发送端，防止新入站投递为重开的接收负责人重新提供来源材料。
    case.source.shutdown_until_complete().await.unwrap();
    case.receiver.shutdown_until_complete().await.unwrap();
    let config = EngineConfig::new("1.2.3")
        .with_rendezvous_base_url(case._server.uri())
        .with_test_relay_fallback(false);
    let (receiver, events) = Engine::start(
        config,
        persistent_engine_host(case.receiver_root.path(), case.receiver_storage.clone()),
    )
    .await
    .unwrap();
    case.receiver = receiver;
    case.events = events;
    case.receiver
        .execute(Operation::RecoverSession(RecoverSessionInput {
            allow_secure_storage_unlock: true,
        }))
        .await
        .unwrap();
    let OperationResult::EncryptionState(encryption) = case
        .receiver
        .execute(Operation::QueryEncryptionState)
        .await
        .unwrap()
    else {
        panic!("重开后必须能够查询当前加密会话")
    };
    if !encryption.session_ready {
        case.receiver
            .execute(Operation::UnlockSpace(UnlockSpaceInput {
                passphrase: SecretString::new("real retry fixture"),
            }))
            .await
            .unwrap();
    }
    let recovered = progress(&case.receiver, &original.entry_id, "failed").await;
    let mut outcomes = Vec::new();
    for _ in 0..2 {
        outcomes.push(
            case.receiver
                .execute(Operation::RetryEntryReceive(case.retry_input()))
                .await
                .unwrap(),
        );
    }
    // 即使实现错误地启动了新工作，也先释放原fixture屏障并完成真实关闭，再断言。
    case.exports.release.add_permits(1);
    let after = case
        .receiver
        .execute(Operation::QueryEntryReceiveProgress(
            EntryReceiveProgressInput {
                entry_id: original.entry_id.clone(),
            },
        ))
        .await
        .unwrap();
    let files = saved_files(
        &case.receiver,
        &original.entry_id,
        case.receiver_root.path(),
    )
    .await;
    case.receiver.shutdown_until_complete().await.unwrap();
    let exports = case.exports.exports.lock().unwrap().len();
    case.shutdown().await;
    for outcome in outcomes {
        assert_eq!(
            outcome,
            OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Unavailable {
                reason: ReceiveRetryUnavailableReason::SourceUnavailable,
            })
        );
    }
    assert_eq!(recovered.attempt_id, original.attempt_id);
    let mut durable_items = original.items.clone();
    for item in &mut durable_items {
        item.content = None;
        item.transfer = None;
        item.history_file_index = None;
    }
    assert!(recovered.snapshot_version.is_none());
    assert_eq!(recovered.items, durable_items);
    let OperationResult::EntryReceiveProgress(Some(after)) = after else {
        panic!("重试不可用不能移除原部分结果")
    };
    assert_eq!(after.attempt_id, original.attempt_id);
    assert_eq!(after.state, "failed");
    assert!(after.snapshot_version.is_none());
    assert_eq!(after.items, durable_items);
    assert_eq!(files, original_files);
    assert_eq!(exports, 3, "没有原始来源时不得重新导出或伪造新尝试");
}
