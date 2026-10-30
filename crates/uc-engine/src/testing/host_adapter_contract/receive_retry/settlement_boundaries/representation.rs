//! 实际SDK接收完成后暂停交付密文，原AEAD验证决定表示成果。
use super::super::{
    member_id, next_engine_event_matching, progress, receive_case, ReceiveCase, ReleaseOnDrop,
    ENGINE_TEST_LOCK,
};
use super::assert_processing;
use crate::{
    BlobResourceInput, EngineEvent, HistoryEntryInput, Operation, OperationResult,
    OutboundProgressFreshness, OutboundProgressInput, OutboundProgressState,
    ReceiveItemFailureReason, SendImageInput,
};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::time::timeout;
use uc_core::file_transfer::{
    ProgressFailureReason, ProgressIdentity, ProgressItemKind, ProgressTerminalOutcome,
};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_representation_waits_for_actual_decryption_then_acks_the_verified_result() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    for corrupt in [false, true] {
        assert_decryption_boundary(corrupt).await;
    }
}

async fn assert_decryption_boundary(corrupt: bool) {
    let mut case: ReceiveCase = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    case.exports.pause_read.store(true, Ordering::SeqCst);
    case.exports.corrupt_read.store(corrupt, Ordering::SeqCst);
    let receiver = member_id(&case.receiver).await;
    let bytes = vec![0x55; 96 * 1024];
    let OperationResult::EntrySent(report) = case
        .source
        .execute(Operation::SendImage(SendImageInput {
            bytes: bytes.clone(),
            mime_type: "image/png".into(),
            target_devices: vec![receiver],
        }))
        .await
        .unwrap()
    else {
        panic!("real image send")
    };
    let EngineEvent::IncomingPending(pending) = next_engine_event_matching(&mut case.events,
        |event| matches!(event, EngineEvent::IncomingPending(pending) if pending.entry_id != case.first.entry_id),
    ).await else { panic!("new representation receive") };
    timeout(Duration::from_secs(20), case.exports.read_paused.notified())
        .await
        .unwrap();
    let receiving = progress(&case.receiver, &pending.entry_id, "receiving").await;
    let before = assert_processing(&case, &receiving, 0).await;
    assert_eq!(before.entry_id, report.entry_id);
    assert_eq!(before.items_total, 1);
    assert_eq!(before.items_completed, 0);
    assert_eq!(before.items_failed, 0);
    let total = before.items[0]
        .total_bytes
        .expect("SDK established the protected wire size");
    assert!(total > 0);
    assert_eq!(before.items[0].completed_bytes, Some(total));
    case.exports.read_release.add_permits(1);
    let settled = progress(
        &case.receiver,
        &pending.entry_id,
        if corrupt { "failed" } else { "completed" },
    )
    .await;
    if corrupt {
        assert_eq!(
            settled.items[0].failure_reason,
            Some(ReceiveItemFailureReason::IntegrityCheckFailed)
        );
    } else {
        let OperationResult::HistoryEntryResource(resource) = case
            .receiver
            .execute(Operation::GetHistoryEntryResource(HistoryEntryInput {
                entry_id: pending.entry_id.clone(),
            }))
            .await
            .unwrap()
        else {
            panic!("saved image resource")
        };
        let actual = match (resource.inline_data, resource.blob_id) {
            (Some(bytes), _) => bytes,
            (None, Some(blob_id)) => {
                let OperationResult::BlobRead(blob) = case
                    .receiver
                    .execute(Operation::ReadBlob(BlobResourceInput { blob_id }))
                    .await
                    .unwrap()
                else {
                    panic!("actual saved image bytes")
                };
                blob.bytes
            }
            _ => panic!("saved image content"),
        };
        assert_eq!(actual, bytes);
    }
    let source = ProgressIdentity::new(*Uuid::parse_str(&before.entry_id).unwrap().as_bytes());
    let attempt =
        ProgressIdentity::new(*Uuid::parse_str(&receiving.attempt_id).unwrap().as_bytes());
    let acks = case.exports.matching_acks(source, attempt, 1).await;
    assert_eq!(acks.len(), 1);
    assert_eq!(acks[0].result.item.kind, ProgressItemKind::Representation);
    assert_eq!(acks[0].result.item.index, 0);
    assert_eq!(
        matches!(acks[0].result.outcome, ProgressTerminalOutcome::Completed),
        !corrupt
    );
    if corrupt {
        assert_eq!(
            acks[0].result.outcome,
            ProgressTerminalOutcome::Failed(ProgressFailureReason::IntegrityCheckFailed)
        );
    }
    let OperationResult::OutboundProgress(Some(after)) = case
        .source
        .execute(Operation::QueryOutboundProgress(OutboundProgressInput {
            entry_id: before.entry_id,
            target_device_id: before.target_device_id,
        }))
        .await
        .unwrap()
    else {
        panic!("adopted actual representation outcome")
    };
    assert_eq!(after.freshness, OutboundProgressFreshness::Settled);
    assert_eq!(
        after.items[0].state,
        if corrupt {
            OutboundProgressState::Failed
        } else {
            OutboundProgressState::Completed
        }
    );
    assert_eq!(after.items[0].completed_bytes, Some(acks[0].result.bytes));
    assert_eq!(after.items[0].total_bytes, acks[0].result.total);
    assert_eq!(after.items[0].round, Some(acks[0].result.round));
    assert_eq!(
        case.exports.exports.lock().unwrap().len(),
        3,
        "表示接收不重新导出原文件"
    );
    case.shutdown().await;
}
