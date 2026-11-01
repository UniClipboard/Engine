//! 通过真实任务填满原目录；淘汰只使运行期进度不可用，不改变已保存文件。
use super::super::{
    member_id, next_engine_event_matching, progress, receive_case, saved_files, ReleaseOnDrop,
    ENGINE_TEST_LOCK,
};
use crate::{EngineEvent, HostFileHandle, Operation, OperationResult, SendFilesInput};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_evicted_receive_snapshot_preserves_saved_results_without_fabricated_counters()
{
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let before = serde_json::to_value(&case.first).unwrap();
    assert!(before["snapshot_version"].is_object());
    case.exports.paused_exports.lock().unwrap().clear();
    let peer = member_id(&case.receiver).await;
    for index in 0..64 {
        let send = case.source.execute(Operation::SendFiles(SendFilesInput {
            files: vec![HostFileHandle::new((1000 + index).to_string())],
            target_devices: vec![peer.clone()],
        }));
        let pending = next_engine_event_matching(
            &mut case.events,
            |event| matches!(event, EngineEvent::IncomingPending(value) if value.entry_id != case.first.entry_id),
        );
        let (sent, pending) = tokio::join!(send, pending);
        assert!(matches!(sent.unwrap(), OperationResult::EntrySent(_)));
        let EngineEvent::IncomingPending(pending) = pending else {
            panic!("real receive task")
        };
        let completed = progress(&case.receiver, &pending.entry_id, "completed").await;
        assert!(completed.snapshot_version.is_some());
    }
    let evicted = progress(&case.receiver, &case.first.entry_id, "failed").await;
    assert!(evicted.snapshot_version.is_none());
    assert!(evicted.items.iter().all(|item| item.content.is_none()
        && item.transfer.is_none()
        && item.history_file_index.is_none()));
    assert_eq!(
        saved_files(
            &case.receiver,
            &case.first.entry_id,
            case.receiver_root.path()
        )
        .await,
        case.saved
    );
    assert_eq!(evicted.items_completed, case.first.items_completed);
    super::record_evidence("retention", &json!({"before": before, "evicted": evicted}));
    case.shutdown().await;
}
