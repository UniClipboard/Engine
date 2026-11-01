//! 完整 Engine 查询验证逐项关联、保存围栏和选择性重试；工件仅含合成资料。
mod retention;

use super::{progress, receive_case, ReleaseOnDrop, CONTENTS, ENGINE_TEST_LOCK};
use crate::{EntryReceiveRetryOutcome, Operation, OperationResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::time::timeout;

fn verify(snapshot: &Value) {
    assert!(
        snapshot["snapshot_version"]["producer_id"].is_string(),
        "query must expose original owner version: {snapshot}"
    );
    for (index, item) in snapshot["items"].as_array().unwrap().iter().enumerate() {
        assert_eq!(item["content"], json!({"kind": "file", "index": index}));
        assert_eq!(
            item["transfer"]["total_bytes"],
            json!(CONTENTS[index].len())
        );
        assert!(item["transfer"]["received_bytes"].is_u64());
        assert!(item["transfer"]["round"].is_u64());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_file_addressable_query_survives_reconnect_and_selective_retry() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(true).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let first = serde_json::to_value(&case.first).unwrap();
    verify(&first);
    assert_eq!(first["items"][0]["state"], "transferring");
    assert_eq!(
        first["items"][0]["transfer"]["received_bytes"],
        first["items"][0]["transfer"]["total_bytes"]
    );
    assert_eq!(first["completed_bytes"], 0);
    // 新宿主事件订阅不回放过去的网络事件；重新查询必须自足。
    // 宿主重新打开详情，只调用查询，不读取旧事件。
    let again = progress(&case.receiver, &case.first.entry_id, "receiving").await;
    assert_eq!(serde_json::to_value(again).unwrap(), first);
    case.exports.release.add_permits(1);
    let failed = progress(&case.receiver, &case.first.entry_id, "failed").await;
    let failed_json = serde_json::to_value(&failed).unwrap();
    verify(&failed_json);
    assert_eq!(
        failed_json["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["history_file_index"].clone())
            .collect::<Vec<_>>(),
        vec![json!(0), json!(2), json!(1)]
    );
    let late_event = super::next_engine_event_matching(&mut case.events, |event| matches!(event, crate::EngineEvent::TransferProgress(value) if value.attempt_id.as_deref() == Some(failed.attempt_id.as_str()))).await;
    case.exports.paused_exports.lock().unwrap().insert(3);
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("selective retry accepted")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let retry = progress(&case.receiver, &case.first.entry_id, "receiving").await;
    let retry_json = serde_json::to_value(&retry).unwrap();
    verify(&retry_json);
    assert_eq!(retry.attempt_id, attempt_id);
    case.receiver.probe_emit_host_event(late_event.clone());
    super::next_engine_event_matching(&mut case.events, |event| *event == late_event).await;
    let after_late = progress(&case.receiver, &case.first.entry_id, "receiving").await;
    assert_eq!(
        after_late, retry,
        "late previous-attempt event cannot mutate the current snapshot"
    );
    for index in [0, 2] {
        assert_eq!(retry.items[index].state, "completed");
        assert_eq!(
            retry_json["items"][index]["content"],
            failed_json["items"][index]["content"]
        );
    }
    assert!(
        retry_json["snapshot_version"]["attempt_revision"]
            .as_u64()
            .unwrap()
            > failed_json["snapshot_version"]["attempt_revision"]
                .as_u64()
                .unwrap()
    );
    case.exports.release.add_permits(1);
    let completed = progress(&case.receiver, &case.first.entry_id, "completed").await;
    let completed_json = serde_json::to_value(&completed).unwrap();
    verify(&completed_json);
    let files = super::saved_files(
        &case.receiver,
        &case.first.entry_id,
        case.receiver_root.path(),
    )
    .await;
    let late_event = match late_event {
        crate::EngineEvent::TransferProgress(event) => serde_json::to_value(event).unwrap(),
        _ => unreachable!("captured transfer event"),
    };
    let hashes = CONTENTS
        .iter()
        .map(|bytes| {
            let actual = std::fs::read(files.get(*bytes).unwrap()).unwrap();
            assert_eq!(&actual, bytes);
            format!("{:x}", Sha256::digest(&actual))
        })
        .collect::<Vec<_>>();
    if let Some(path) = std::env::var_os("UC_RECEIVE_PROGRESS_EVIDENCE") {
        std::fs::write(path, serde_json::to_vec_pretty(&json!({"first": first, "failed": failed_json, "retry": retry_json, "completed": completed_json, "sha256": hashes, "late_event": late_event, "after_late_event": after_late})).unwrap()).unwrap();
    }
    case.receiver.shutdown_until_complete().await.unwrap();
    let (receiver, events) = crate::Engine::start(
        crate::EngineConfig::new("1.2.3")
            .with_rendezvous_base_url(case._server.uri())
            .with_test_relay_fallback(false),
        super::persistent_engine_host(case.receiver_root.path(), case.receiver_storage.clone()),
    )
    .await
    .unwrap();
    case.receiver = receiver;
    case.events = events;
    case.receiver
        .execute(Operation::RecoverSession(crate::RecoverSessionInput {
            allow_secure_storage_unlock: true,
        }))
        .await
        .unwrap();
    let restarted = progress(&case.receiver, &case.first.entry_id, "completed").await;
    let restarted = serde_json::to_value(restarted).unwrap();
    assert!(restarted["snapshot_version"].is_null());
    assert!(restarted["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["transfer"].is_null() && item["content"].is_null()));
    if let Some(path) = std::env::var_os("UC_RECEIVE_PROGRESS_EVIDENCE") {
        let mut evidence: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        evidence["restarted"] = restarted;
        std::fs::write(path, serde_json::to_vec_pretty(&evidence).unwrap()).unwrap();
    }
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_zero_byte_file_has_known_zero_total() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    case.exports.release.add_permits(1);
    let file = case._source_root.path().join("zero-synthetic.bin");
    std::fs::write(&file, b"").unwrap();
    *case.clipboard.lock().unwrap() = crate::HostClipboardSnapshot {
        observed_at_ms: 1_700_000_300_000,
        representations: vec![crate::HostClipboardRepresentation::Inline {
            format: "files".into(),
            mime_type: Some("text/uri-list".into()),
            bytes: url::Url::from_file_path(file)
                .unwrap()
                .to_string()
                .into_bytes(),
        }],
    };
    case.source
        .execute(Operation::ObserveClipboardChange(
            crate::ObserveClipboardChangeInput { dispatch: true },
        ))
        .await
        .unwrap();
    let crate::EngineEvent::IncomingPending(pending) = super::next_engine_event_matching(&mut case.events,
        |event| matches!(event, crate::EngineEvent::IncomingPending(pending) if pending.entry_id != case.first.entry_id)
    ).await else { panic!("new zero file entry") };
    let completed = progress(&case.receiver, &pending.entry_id, "completed").await;
    let query = serde_json::to_value(&completed).unwrap();
    assert_eq!(
        query["items"][0]["content"],
        json!({"kind": "file", "index": 0})
    );
    assert_eq!(query["items"][0]["transfer"]["received_bytes"], 0);
    assert_eq!(query["items"][0]["transfer"]["total_bytes"], 0);
    let files =
        super::saved_files(&case.receiver, &pending.entry_id, case.receiver_root.path()).await;
    assert!(files.contains_key(b"".as_slice()));
    record_evidence("zero", &query);
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_mixed_content_keeps_file_and_representation_namespaces_and_unknown_total() {
    use crate::{HostClipboardRepresentation, HostClipboardSnapshot, ObserveClipboardChangeInput};
    use std::sync::atomic::Ordering;
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    case.exports.pause_read.store(true, Ordering::SeqCst);
    case.exports.release.add_permits(1);
    let file = case._source_root.path().join("mixed-synthetic.bin");
    std::fs::write(&file, b"mixed-file").unwrap();
    *case.clipboard.lock().unwrap() = HostClipboardSnapshot {
        observed_at_ms: 1_700_000_200_000,
        representations: vec![
            HostClipboardRepresentation::Inline {
                format: "jpeg".into(),
                mime_type: Some("image/jpeg".into()),
                bytes: vec![b'a'; 96 * 1024],
            },
            HostClipboardRepresentation::Inline {
                format: "image".into(),
                mime_type: Some("image/png".into()),
                bytes: vec![0x55; 96 * 1024],
            },
            HostClipboardRepresentation::Inline {
                format: "files".into(),
                mime_type: Some("text/uri-list".into()),
                bytes: url::Url::from_file_path(file)
                    .unwrap()
                    .to_string()
                    .into_bytes(),
            },
        ],
    };
    case.source
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: true },
        ))
        .await
        .unwrap();
    let crate::EngineEvent::IncomingPending(pending) = super::next_engine_event_matching(&mut case.events,
        |event| matches!(event, crate::EngineEvent::IncomingPending(pending) if pending.entry_id != case.first.entry_id)
    ).await else { panic!("new mixed entry") };
    timeout(Duration::from_secs(20), case.exports.read_paused.notified())
        .await
        .unwrap();
    let receiving = progress(&case.receiver, &pending.entry_id, "receiving").await;
    let query = serde_json::to_value(&receiving).unwrap();
    let items = query["items"].as_array().unwrap();
    let first = items
        .iter()
        .find(|item| item["content"] == json!({"kind": "representation", "index": 0}))
        .unwrap_or_else(|| panic!("original mixed plan: {query}"));
    let second = items
        .iter()
        .find(|item| item["content"] == json!({"kind": "representation", "index": 1}))
        .unwrap();
    let file = items
        .iter()
        .find(|item| item["content"] == json!({"kind": "file", "index": 0}))
        .unwrap();
    assert!(first["transfer"]["total_bytes"].is_u64());
    assert_eq!(first["state"], "transferring");
    assert!(second["transfer"]["total_bytes"].is_null());
    assert_eq!(second["transfer"]["received_bytes"], 0);
    assert_eq!(file["transfer"]["total_bytes"], 10);
    case.exports.pause_read.store(false, Ordering::SeqCst);
    case.exports.read_release.add_permits(1);
    let completed = progress(&case.receiver, &pending.entry_id, "completed").await;
    assert!(completed.items.iter().all(|item| item.state == "completed"));
    record_evidence(
        "mixed",
        &json!({"receiving": query, "completed": completed}),
    );
    case.shutdown().await;
}

pub(super) fn record_evidence(name: &str, evidence: &Value) {
    if let Some(path) = std::env::var_os("UC_RECEIVE_PROGRESS_EVIDENCE") {
        let path = std::path::PathBuf::from(path).with_extension(format!("{name}.json"));
        std::fs::write(path, serde_json::to_vec_pretty(evidence).unwrap()).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_equal_content_and_basenames_keep_distinct_file_positions() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    case.exports.paused_exports.lock().unwrap().clear();
    case.source
        .execute(Operation::SendFiles(crate::SendFilesInput {
            files: vec![
                crate::HostFileHandle::new("0"),
                crate::HostFileHandle::new("0"),
                crate::HostFileHandle::new("2"),
            ],
            target_devices: vec![super::member_id(&case.receiver).await],
        }))
        .await
        .unwrap();
    let crate::EngineEvent::IncomingPending(pending) = super::next_engine_event_matching(&mut case.events,
        |event| matches!(event, crate::EngineEvent::IncomingPending(pending) if pending.entry_id != case.first.entry_id)
    ).await else { panic!("new duplicate-content entry") };
    let completed = progress(&case.receiver, &pending.entry_id, "completed").await;
    assert_eq!(completed.items.len(), 3);
    let query = serde_json::to_value(&completed).unwrap();
    for index in 0..3 {
        assert_eq!(
            query["items"][index]["content"],
            json!({"kind": "file", "index": index})
        );
        assert_eq!(query["items"][index]["history_file_index"], index);
    }
    assert_ne!(completed.items[0].item_id, completed.items[1].item_id);
    assert_eq!(completed.items[0].transfer, completed.items[1].transfer);
    let OperationResult::HistoryEntryResource(resource) = case
        .receiver
        .execute(Operation::GetHistoryEntryResource(
            crate::HistoryEntryInput {
                entry_id: pending.entry_id,
            },
        ))
        .await
        .unwrap()
    else {
        panic!("saved files")
    };
    let bytes = match (resource.inline_data, resource.blob_id) {
        (Some(bytes), _) => bytes,
        (None, Some(blob_id)) => {
            let OperationResult::BlobRead(blob) = case
                .receiver
                .execute(Operation::ReadBlob(crate::BlobResourceInput { blob_id }))
                .await
                .unwrap()
            else {
                panic!("saved URI payload")
            };
            blob.bytes
        }
        _ => panic!("saved URI payload"),
    };
    let paths = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| url::Url::parse(line).unwrap().to_file_path().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 3);
    assert_ne!(paths[0], paths[1]);
    let hashes = paths
        .iter()
        .map(|path| format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())))
        .collect::<Vec<_>>();
    assert_eq!(hashes[0], hashes[1]);
    record_evidence(
        "duplicates",
        &json!({"completed": completed, "sha256": hashes}),
    );
    case.shutdown().await;
}
