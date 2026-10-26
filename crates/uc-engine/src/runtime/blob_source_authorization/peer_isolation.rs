//! 三个生产Runtime中不同接收设备的真实SDK读流、取消及反向确认分别结算。
use super::ownership::{assert_saved, outbound};
use super::{dispatch, join, membership, pending, settled, start, PASSPHRASE};
use crate::engine::EngineRuntime;
use crate::runtime::ProductionRuntime;
use crate::testing::{
    mount_engine_rendezvous,
    receive_exports::{ReceiveExports, ReleaseOnDrop},
    receive_streams::{ReceiveStreams, ReleaseStreamsOnDrop},
};
use crate::{
    CancelEntryReceiveInput, CreateSpaceInput, EntryReceiveCancellationOutcome,
    MemberSyncPreferencesPatch, Operation, OperationResult, OutboundProgressBatchState,
    OutboundProgressPhase, SecretString, SettingsPatch, SyncSettingsPatch,
    UpdateMemberSyncPreferencesInput,
};
use std::fs;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tempfile::{tempdir, TempDir};
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use uc_application::facade::{PublishBlobPathCommand, PublishBlobResult};
use uc_core::file_transfer::ProgressIdentity;
use uc_core::ids::DeviceId;
use uuid::Uuid;
use wiremock::MockServer;

struct Case {
    _release: [ReleaseOnDrop; 3],
    _release_streams: ReleaseStreamsOnDrop,
    receiver: ProductionRuntime,
    sender: ProductionRuntime,
    other: ProductionRuntime,
    roots: [TempDir; 3],
    receiver_id: String,
    other_id: String,
    receiver_entry: String,
    other_entry: String,
    streams: Arc<ReceiveStreams>,
    exports: [Arc<ReceiveExports>; 2],
    sender_exports: Arc<ReceiveExports>,
    published: PublishBlobResult,
    content: Vec<u8>,
    _server: MockServer,
}
impl Case {
    async fn start(block_reads: bool) -> Self {
        let server = MockServer::start().await;
        mount_engine_rendezvous(&server).await;
        let roots = [tempdir().unwrap(), tempdir().unwrap(), tempdir().unwrap()];
        let exports = [
            ReceiveExports::install(roots[0].path(), usize::MAX, usize::MAX),
            ReceiveExports::install(roots[2].path(), usize::MAX, usize::MAX),
        ];
        let sender_exports = ReceiveExports::install(roots[1].path(), usize::MAX, usize::MAX);
        let release = [
            ReleaseOnDrop(exports[0].clone()),
            ReleaseOnDrop(exports[1].clone()),
            ReleaseOnDrop(sender_exports.clone()),
        ];
        let streams = ReceiveStreams::install(roots[0].path(), block_reads);
        let release_streams = ReleaseStreamsOnDrop(streams.clone());
        let (receiver, mut receiver_events) = start(roots[0].path(), &server).await;
        let (sender, _) = start(roots[1].path(), &server).await;
        let (other, mut other_events) = start(roots[2].path(), &server).await;
        receiver
            .execute(
                Operation::CreateSpace(CreateSpaceInput {
                    device_name: Some("peer isolation receiver".into()),
                    passphrase: SecretString::new(PASSPHRASE),
                    passphrase_confirmation: SecretString::new(PASSPHRASE),
                }),
                CancellationToken::new(),
            )
            .await
            .unwrap();
        join(&receiver, &sender).await;
        membership(&receiver, 2, "receiver").await;
        membership(&sender, 2, "sender").await;
        join(&receiver, &other).await;
        let receiver_id = membership(&receiver, 3, "receiver").await;
        let sender_id = membership(&sender, 3, "sender").await;
        let other_id = membership(&other, 3, "other").await;
        for runtime in [&receiver, &sender, &other] {
            runtime
                .execute(
                    Operation::UpdateSettings(Box::new(SettingsPatch {
                        sync: Some(SyncSettingsPatch {
                            auto_sync_enabled: Some(false),
                            sync_on_restore: Some(false),
                            ..Default::default()
                        }),
                        ..Default::default()
                    })),
                    CancellationToken::new(),
                )
                .await
                .unwrap();
        }
        // C完成后的活跃剪贴板广播是另一条同步动作；本场景只发B的同一原意图。
        // 关闭C的新内容发送仍须保留C对B的原反向进度与精确ACK。
        for (runtime, peers) in [
            (&other, [&receiver_id, &sender_id]),
            (&receiver, [&other_id, &sender_id]),
        ] {
            for peer in peers {
                runtime
                    .execute(
                        Operation::UpdateMemberSyncPreferences(UpdateMemberSyncPreferencesInput {
                            device_id: peer.clone(),
                            patch: MemberSyncPreferencesPatch {
                                send_enabled: Some(false),
                                ..Default::default()
                            },
                        }),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
            }
        }
        if !block_reads {
            *sender_exports.hold_ack_peer.lock().unwrap() = Some(DeviceId::new(&receiver_id));
        }
        timeout(Duration::from_secs(30), async {
            loop {
                if matches!(sender.execute(Operation::QueryPeerConnections, CancellationToken::new()).await.unwrap(),
                    OperationResult::PeerConnections(peers) if [&receiver_id, &other_id].iter().all(|id|
                        peers.iter().any(|peer| &peer.peer_id == *id && peer.connected))) { break; }
                sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("both actual recipients must be reachable");
        // 文件本体走原明文文件能力，不压缩；足够大以越过SDK的真实64KiB读取屏障。
        let content = vec![0x5a; 256 * 1024];
        let path = roots[1].path().join("peer-body.bin");
        fs::write(&path, &content).unwrap();
        let facade = sender.current_facade().await.unwrap();
        let published = facade
            .publish_blob_path(PublishBlobPathCommand {
                path,
                entry_id: None,
            })
            .await
            .unwrap();
        dispatch(&facade, &receiver_id, &published, content.len()).await;
        let receiver_entry = pending(&mut receiver_events, &sender_id).await;
        let other_entry = pending(&mut other_events, &sender_id).await;
        drop(facade);
        Self {
            _release: release,
            _release_streams: release_streams,
            receiver,
            sender,
            other,
            roots,
            receiver_id,
            other_id,
            receiver_entry,
            other_entry,
            streams,
            exports,
            sender_exports,
            published,
            content,
            _server: server,
        }
    }
    async fn close(self) {
        for runtime in [&self.other, &self.sender, &self.receiver] {
            runtime.shutdown(None).await.unwrap();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paused_native_read_and_local_cancel_do_not_stop_another_receivers_actual_completion() {
    let case = Case::start(true).await;
    let paused = timeout(Duration::from_secs(20), async {
        loop {
            if let Some(stream) = case.streams.receives().into_iter().find(|stream| {
                stream.digest == case.published.digest && stream.paused.load(Ordering::SeqCst)
            }) {
                assert!(stream.validated.load(Ordering::SeqCst) >= 64 * 1024);
                assert!(
                    stream.validated.load(Ordering::SeqCst) < stream.total.load(Ordering::SeqCst)
                );
                assert!(!stream.finished.load(Ordering::SeqCst));
                return stream;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("actual SDK read must be held after verified prefix");
    let other = settled(&case.other, &case.other_entry, "completed").await;
    let source = case.published.entry_id.as_ref();
    let completed = outbound(&case.sender, source, &case.other_id, |view| {
        view.state == OutboundProgressBatchState::Completed
    })
    .await;
    assert_eq!(
        completed.attempt_id.as_deref(),
        Some(other.attempt_id.as_str())
    );
    case.exports[1]
        .matching_acks(
            ProgressIdentity::new(*Uuid::parse_str(source).unwrap().as_bytes()),
            ProgressIdentity::new(*Uuid::parse_str(&other.attempt_id).unwrap().as_bytes()),
            1,
        )
        .await;
    let running = settled(&case.receiver, &case.receiver_entry, "receiving").await;
    let transmitting = outbound(&case.sender, source, &case.receiver_id, |view| {
        view.items[0].last_known_phase == Some(OutboundProgressPhase::Transferring)
    })
    .await;
    assert_eq!(transmitting.items_completed, 0);
    assert_eq!(
        transmitting.attempt_id.as_deref(),
        Some(running.attempt_id.as_str())
    );
    assert_ne!(transmitting.attempt_id, completed.attempt_id);
    assert!(!paused.finished.load(Ordering::SeqCst));
    assert!(!paused.stopped.load(Ordering::SeqCst));
    assert!(case.exports[0].exports.lock().unwrap().is_empty());
    assert_saved(
        &case.other,
        &case.other_entry,
        case.roots[2].path(),
        &case.content,
    )
    .await;
    let result = case
        .receiver
        .execute(
            Operation::CancelEntryReceive(CancelEntryReceiveInput {
                entry_id: case.receiver_entry.clone(),
                attempt_id: running.attempt_id.clone(),
            }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        result,
        OperationResult::EntryReceiveCancellation(EntryReceiveCancellationOutcome::Cancelled)
    ));
    assert!(paused.stopped.load(Ordering::SeqCst));
    assert!(
        paused.finished.load(Ordering::SeqCst),
        "local cancellation waits for the original SDK work"
    );
    let cancelled = settled(&case.receiver, &case.receiver_entry, "cancelled").await;
    assert_eq!(cancelled.attempt_id, running.attempt_id);
    let cancelled_source = outbound(&case.sender, source, &case.receiver_id, |view| {
        view.state == OutboundProgressBatchState::Cancelled
    })
    .await;
    assert_eq!(cancelled_source.attempt_id, transmitting.attempt_id);
    let unchanged = outbound(&case.sender, source, &case.other_id, |view| {
        view.state == OutboundProgressBatchState::Completed
    })
    .await;
    assert_eq!(unchanged.attempt_id, completed.attempt_id);
    assert_eq!(unchanged.items, completed.items);
    assert_eq!(
        case.streams.receives().len(),
        1,
        "cancellation must not retry the original stream"
    );
    for exports in &case.exports {
        assert_eq!(
            exports.cache_misses.lock().unwrap().as_slice(),
            [case.published.digest]
        );
    }
    assert_saved(
        &case.other,
        &case.other_entry,
        case.roots[2].path(),
        &case.content,
    )
    .await;
    case.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adopted_result_waiting_for_its_real_ack_does_not_block_another_receivers_confirmation() {
    let case = Case::start(false).await;
    timeout(
        Duration::from_secs(20),
        case.sender_exports.ack_held.notified(),
    )
    .await
    .expect("actual A terminal is adopted before its ACK is encoded");
    let first = settled(&case.receiver, &case.receiver_entry, "completed").await;
    let other = settled(&case.other, &case.other_entry, "completed").await;
    let source = case.published.entry_id.as_ref();
    let source_identity = ProgressIdentity::new(*Uuid::parse_str(source).unwrap().as_bytes());
    let first_attempt =
        ProgressIdentity::new(*Uuid::parse_str(&first.attempt_id).unwrap().as_bytes());
    let other_attempt =
        ProgressIdentity::new(*Uuid::parse_str(&other.attempt_id).unwrap().as_bytes());
    let first_view = outbound(&case.sender, source, &case.receiver_id, |view| {
        view.state == OutboundProgressBatchState::Completed
    })
    .await;
    let other_view = outbound(&case.sender, source, &case.other_id, |view| {
        view.state == OutboundProgressBatchState::Completed
    })
    .await;
    assert_ne!(first_view.attempt_id, other_view.attempt_id);
    assert_eq!(
        first_view.attempt_id.as_deref(),
        Some(first.attempt_id.as_str())
    );
    assert_eq!(
        other_view.attempt_id.as_deref(),
        Some(other.attempt_id.as_str())
    );
    // C确实收到精确回执；A的结果已真实采用，但其ACK尚未写入原生流。
    case.exports[1]
        .matching_acks(source_identity, other_attempt, 1)
        .await;
    assert_eq!(
        case.exports[0].matching_ack_count(source_identity, first_attempt),
        0
    );
    assert_eq!(
        *case.sender_exports.hold_ack_peer.lock().unwrap(),
        Some(DeviceId::new(&case.receiver_id))
    );
    assert_saved(
        &case.receiver,
        &case.receiver_entry,
        case.roots[0].path(),
        &case.content,
    )
    .await;
    assert_saved(
        &case.other,
        &case.other_entry,
        case.roots[2].path(),
        &case.content,
    )
    .await;
    *case.sender_exports.hold_ack_peer.lock().unwrap() = None;
    case.sender_exports.ack_release.notify_waiters();
    case.exports[0]
        .matching_acks(source_identity, first_attempt, 1)
        .await;
    let first_after = outbound(&case.sender, source, &case.receiver_id, |view| {
        view.state == OutboundProgressBatchState::Completed
    })
    .await;
    let other_after = outbound(&case.sender, source, &case.other_id, |view| {
        view.state == OutboundProgressBatchState::Completed
    })
    .await;
    assert_eq!(first_after.attempt_id, first_view.attempt_id);
    assert_eq!(first_after.items, first_view.items);
    assert_eq!(other_after.attempt_id, other_view.attempt_id);
    assert_eq!(other_after.items, other_view.items);
    for exports in &case.exports {
        assert_eq!(
            exports.cache_misses.lock().unwrap().as_slice(),
            [case.published.digest]
        );
        assert_eq!(
            exports.exports.lock().unwrap().as_slice(),
            [case.published.digest]
        );
    }
    assert_eq!(
        case.streams.receives().len(),
        1,
        "waiting for ACK must not download again"
    );
    case.close().await;
}
