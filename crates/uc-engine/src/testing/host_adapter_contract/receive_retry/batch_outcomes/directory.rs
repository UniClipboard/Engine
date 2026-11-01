//! 实际目录包含多个成员或没有blob时，源端都只汇总一个完整逻辑单元。
use super::super::{
    member_id, next_engine_event_matching, open_control_connection, progress, receive_case,
    ReleaseOnDrop, ENGINE_TEST_LOCK,
};
use super::reconciled_for_input;
use crate::{
    flush_test_tracing, init_test_tracing, BlobResourceInput, EngineEvent, HistoryEntryInput,
    HostClipboardRepresentation, HostClipboardSnapshot, ObserveClipboardChangeInput, Operation,
    OperationResult, OutboundProgressBatchState, OutboundProgressFreshness, OutboundProgressInput,
    OutboundProgressReconciliation, OutboundProgressState, ReceiveItemFailureReason,
    ReceiveItemKind, ReconcileOutboundProgressInput,
};
use diesel::RunQueryDsl;
use std::time::Duration;
use tokio::time::timeout;
use uc_core::file_transfer::{ProgressIdentity, ProgressItemKind, ProgressTerminalOutcome};
use url::Url;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_directory_batch_counts_only_one_complete_unit_after_commit() {
    // 显式本机后端验收复用原实际目录流程，不让普通测试默认远程导出。
    if std::env::var_os("UC_TEST_OTLP_TRACE_ENDPOINT").is_some() {
        init_test_tracing();
    }
    assert_directory(false, false).await;
    flush_test_tracing();
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_failed_directory_batch_does_not_count_successful_children_as_delivered_units()
{
    assert_directory(true, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_empty_directory_without_blobs_is_one_confirmed_completed_unit() {
    assert_directory(false, true).await;
}

async fn assert_directory(fail: bool, empty: bool) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let generation = std::fs::read_dir(
        case.receiver_root
            .path()
            .join("private/profile-data-generations"),
    )
    .unwrap()
    .map(|entry| entry.unwrap())
    .find(|entry| entry.file_type().unwrap().is_dir())
    .unwrap();
    let mut connection =
        open_control_connection(&generation.path().join("v3-payloads/profile.sqlite"));
    if fail {
        let old_entry = case.first.entry_id.replace('\'', "''");
        diesel::sql_query(format!("CREATE TRIGGER reject_directory_batch_commit BEFORE INSERT ON clipboard_event WHEN EXISTS (SELECT 1 FROM file_transfer WHERE entry_id != '{old_entry}' AND item_role = 'file' AND status = 'completed') BEGIN SELECT RAISE(ABORT, 'injected directory batch commit'); END"))
            .execute(&mut connection).unwrap();
    }
    let directory = case._source_root.path().join("directory-source");
    std::fs::create_dir(&directory).unwrap();
    if !empty {
        std::fs::write(directory.join("a.bin"), vec![0x61; 96 * 1024]).unwrap();
        std::fs::write(directory.join("b.bin"), vec![0x62; 96 * 1024]).unwrap();
    }
    *case.clipboard.lock().unwrap() = HostClipboardSnapshot {
        observed_at_ms: 1_700_000_100_000,
        representations: vec![HostClipboardRepresentation::Inline {
            format: "files".into(),
            mime_type: Some("text/uri-list".into()),
            bytes: Url::from_directory_path(&directory)
                .unwrap()
                .to_string()
                .into_bytes(),
        }],
    };
    let OperationResult::ClipboardChangeObserved {
        report: Some(report),
    } = case
        .source
        .execute(Operation::ObserveClipboardChange(
            ObserveClipboardChangeInput { dispatch: true },
        ))
        .await
        .unwrap()
    else {
        panic!("actual directory dispatch report")
    };
    let target = member_id(&case.receiver).await;
    let EngineEvent::IncomingPending(pending) = next_engine_event_matching(&mut case.events,
        |event| matches!(event, EngineEvent::IncomingPending(pending) if pending.entry_id != case.first.entry_id),
    ).await else { panic!("actual new directory receive") };
    if !empty {
        timeout(Duration::from_secs(20), case.exports.paused.notified())
            .await
            .unwrap();
        let receiving = progress(&case.receiver, &pending.entry_id, "receiving").await;
        let running = reconciled_for_input(
            &case,
            &receiving,
            OutboundProgressInput {
                entry_id: report.entry_id.clone(),
                target_device_id: target.clone(),
            },
            OutboundProgressReconciliation::Running,
        )
        .await;
        assert_eq!(running.items_total, 1);
        assert_eq!(running.items_completed, 0);
        assert_eq!(running.items_running, 1);
        assert_eq!(running.items_failed, 0);
        assert_eq!(running.items[0].kind, ReceiveItemKind::DirectoryUnit);
        case.exports.release.add_permits(1);
    }
    let received = progress(
        &case.receiver,
        &pending.entry_id,
        if fail { "failed" } else { "completed" },
    )
    .await;
    let received = if fail {
        // 首轮未保存条目，另一条已接受入站可开始新尝试；旧快照须明确过期。
        let EngineEvent::IncomingPending(next) = next_engine_event_matching(&mut case.events,
            |event| matches!(event, EngineEvent::IncomingPending(next) if next.entry_id != case.first.entry_id && next.entry_id != pending.entry_id),
        ).await else { panic!("new accepted directory attempt") };
        assert_ne!(
            next.attempt_id.as_deref(),
            Some(received.attempt_id.as_str())
        );
        let current = progress(&case.receiver, &next.entry_id, "failed").await;
        assert_eq!(
            case.source
                .execute(Operation::ReconcileOutboundProgress(
                    ReconcileOutboundProgressInput {
                        entry_id: report.entry_id.clone(),
                        target_device_id: target.clone(),
                        expected_attempt_id: Some(received.attempt_id),
                    }
                ))
                .await
                .unwrap(),
            OperationResult::OutboundProgressReconciliation(
                OutboundProgressReconciliation::Superseded
            )
        );
        current
    } else {
        received
    };
    let query = serde_json::to_value(&received).unwrap();
    assert_eq!(
        query["items"][0]["content"],
        serde_json::json!({"kind": "directory_unit"}),
        "directory query: {query}"
    );
    assert!(query["snapshot_version"].is_object());
    super::super::item_progress::record_evidence("directory", &query);

    let settled = reconciled_for_input(
        &case,
        &received,
        OutboundProgressInput {
            entry_id: report.entry_id,
            target_device_id: target,
        },
        OutboundProgressReconciliation::Settled,
    )
    .await;
    assert_eq!(settled.items.len(), 1);
    assert_eq!(settled.items[0].kind, ReceiveItemKind::DirectoryUnit);
    assert_eq!(settled.items_total, 1);
    assert_eq!(
        (
            settled.items_waiting,
            settled.items_running,
            settled.items_cancelled,
            settled.items_unconfirmed
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(settled.freshness, OutboundProgressFreshness::Settled);
    assert_eq!(
        settled.state,
        if fail {
            OutboundProgressBatchState::Failed
        } else {
            OutboundProgressBatchState::Completed
        }
    );
    assert_eq!(
        (settled.items_completed, settled.items_failed),
        if fail { (0, 1) } else { (1, 0) }
    );
    assert_eq!(
        settled.items[0].state,
        if fail {
            OutboundProgressState::Failed
        } else {
            OutboundProgressState::Completed
        }
    );
    assert_eq!(
        settled.items[0].failure_reason,
        received.items[0].failure_reason
    );
    assert_eq!(
        received.items[0].failure_reason,
        fail.then_some(ReceiveItemFailureReason::Unknown)
    );
    let exports = case.exports.exports.lock().unwrap().clone();
    if fail {
        // 失败后的成员可保持未开始；两次尝试都只能输出原目录计划，不能重做旧成功文件。
        assert!((5..=7).contains(&exports.len()));
        assert_ne!(exports[3], exports[4]);
        assert!(exports[3..]
            .iter()
            .all(|digest| !exports[..3].contains(digest)));
        assert!(exports[5..]
            .iter()
            .all(|digest| exports[3..5].contains(digest)));
    } else {
        assert_eq!(exports.len(), if empty { 3 } else { 5 });
    }
    if fail {
        assert!(case.exports.paths.lock().unwrap()[3..]
            .iter()
            .all(|path| !path.exists()));
    } else {
        let OperationResult::HistoryEntryResource(resource) = case
            .receiver
            .execute(Operation::GetHistoryEntryResource(HistoryEntryInput {
                entry_id: pending.entry_id.clone(),
            }))
            .await
            .unwrap()
        else {
            panic!("saved directory URI")
        };
        let bytes = match (resource.inline_data, resource.blob_id) {
            (Some(bytes), _) => bytes,
            (None, Some(blob_id)) => {
                let OperationResult::BlobRead(blob) = case
                    .receiver
                    .execute(Operation::ReadBlob(BlobResourceInput { blob_id }))
                    .await
                    .unwrap()
                else {
                    panic!("actual saved directory URI bytes")
                };
                blob.bytes
            }
            _ => panic!("saved directory resource"),
        };
        let text = std::str::from_utf8(&bytes).unwrap();
        let paths = text
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| Url::parse(line).unwrap().to_file_path().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].is_dir());
        assert!(paths[0]
            .canonicalize()
            .unwrap()
            .starts_with(case.receiver_root.path().canonicalize().unwrap()));
        assert_eq!(
            std::fs::read_dir(&paths[0]).unwrap().count(),
            if empty { 0 } else { 2 }
        );
        if !empty {
            assert_eq!(
                std::fs::read(paths[0].join("a.bin")).unwrap(),
                vec![0x61; 96 * 1024]
            );
            assert_eq!(
                std::fs::read(paths[0].join("b.bin")).unwrap(),
                vec![0x62; 96 * 1024]
            );
        }
    }
    let source = ProgressIdentity::new(*Uuid::parse_str(&settled.entry_id).unwrap().as_bytes());
    let attempt = ProgressIdentity::new(*Uuid::parse_str(&received.attempt_id).unwrap().as_bytes());
    let acks = case.exports.matching_acks(source, attempt, 1).await;
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].result.item.kind, ProgressItemKind::DirectoryUnit);
    assert_eq!(acks[0].result.item.index, 0);
    assert_eq!(Some(acks[0].result.bytes), settled.items[0].completed_bytes);
    assert_eq!(acks[0].result.total, settled.items[0].total_bytes);
    assert_eq!(Some(acks[0].result.round), settled.items[0].round);
    if fail {
        assert!(matches!(
            acks[0].result.outcome,
            ProgressTerminalOutcome::Failed(_)
        ));
    } else {
        assert_eq!(acks[0].result.outcome, ProgressTerminalOutcome::Completed);
    }
    for (bytes, path) in &case.saved {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    if fail {
        diesel::sql_query("DROP TRIGGER reject_directory_batch_commit")
            .execute(&mut connection)
            .unwrap();
    }
    drop(connection);
    case.shutdown().await;
}
