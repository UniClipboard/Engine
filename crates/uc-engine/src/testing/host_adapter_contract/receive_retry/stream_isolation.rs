//! 用稳定 Engine 操作验证真实单连接双流与同内容等待者，所有数据仍经原 SDK 校验与落盘。
use super::super::StaticHostClipboard;
use super::{
    empty_engine_host, member_id, mount_engine_rendezvous, progress, saved_files,
    MemoryHostSecureStorage, ENGINE_TEST_LOCK,
};
use crate::testing::receive_streams::{ReceiveStream, ReceiveStreams, ReleaseStreamsOnDrop};
use crate::{
    CancelEntryReceiveInput, ClipboardRestoreMode, ClipboardRestoreOutcome, CreateSpaceInput,
    Engine, EngineConfig, EntryReceiveCancellationOutcome, HostCapabilities, HostCapabilityError,
    HostCapabilityErrorCategory, HostClipboardSnapshot, HostDirectories, HostFileAccess,
    HostFileHandle, HostFileMetadata, JoinSpaceInput, Operation, OperationResult,
    ReceiveProgressSummary, ResendEntryInput, ResendEntryOutcome, RestoreClipboardInput,
    SecretString, SendFilesInput, SendTextInput, SettingsPatch, SyncSettingsPatch,
};
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use uc_infra_p2p::network::iroh::test_support::BlobNativeObservation;
use wiremock::MockServer;

const LARGE_BYTES: usize = 8 * 1024 * 1024;
const TAIL_BYTES: usize = 16 * 1024;
const PASSPHRASE: &str = "engine streams fixture";

#[path = "resource_reads.rs"]
mod resource_reads;

struct StreamFiles;
impl HostFileAccess for StreamFiles {
    fn metadata(&self, handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        let size_bytes = match handle.as_str() {
            "0" | "1" => LARGE_BYTES,
            "2" | "3" | "4" => TAIL_BYTES,
            _ => return Err(unavailable()),
        };
        Ok(HostFileMetadata {
            display_name: format!("stream-{}.bin", handle.as_str()),
            size_bytes: size_bytes as u64,
            mime_type: Some("application/octet-stream".into()),
        })
    }
    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        maximum: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        let (byte, size) = match handle.as_str() {
            "0" => (0x11, LARGE_BYTES),
            "1" => (0x33, LARGE_BYTES),
            "2" => (0x55, TAIL_BYTES),
            "3" => (0x66, TAIL_BYTES),
            "4" => (0x77, TAIL_BYTES),
            _ => return Err(unavailable()),
        };
        let offset = usize::try_from(offset)
            .map_err(|_| unavailable())?
            .min(size);
        Ok(vec![byte; (size - offset).min(maximum as usize)])
    }
    fn write_chunk(&self, _: &HostFileHandle, _: u64, _: &[u8]) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
    fn finish_write(&self, _: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
}
fn unavailable() -> HostCapabilityError {
    HostCapabilityError::new(
        HostCapabilityErrorCategory::Unavailable,
        "fixture file unavailable",
    )
}

async fn receiving_entry(receiver: &Engine, previous: Option<&str>) -> ReceiveProgressSummary {
    // 通知允许宿主重新查询；测试身份取自公开进度权威，不依赖准备期间的瞬时事件。
    let entry = timeout(Duration::from_secs(30), async {
        loop {
            let OperationResult::EntryReceiveProgressList(rows) = receiver
                .execute(Operation::ListEntryReceiveProgress)
                .await
                .unwrap()
            else {
                panic!("receive progress list expected")
            };
            let mut candidates = rows
                .into_iter()
                .filter(|row| row.state == "receiving" && Some(row.entry_id.as_str()) != previous);
            if let Some(row) = candidates.next() {
                assert!(candidates.next().is_none(), "只能有一个新增接收尝试");
                assert_eq!(row.items_total, 3);
                return row;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("file receive must establish public progress");
    let current = progress(receiver, &entry.entry_id, "receiving").await;
    assert_eq!(current.attempt_id, entry.attempt_id);
    current
}

fn content_streams(streams: &ReceiveStreams) -> Vec<Arc<ReceiveStream>> {
    streams
        .receives()
        .into_iter()
        .filter(|stream| !stream.injected_failure.load(Ordering::SeqCst))
        .filter(|stream| {
            stream.total.load(Ordering::SeqCst) == LARGE_BYTES as u64
                || (stream.total.load(Ordering::SeqCst) == 0
                    && !stream.finished.load(Ordering::SeqCst))
        })
        .collect()
}

async fn paused_stream(
    streams: &ReceiveStreams,
    index: usize,
    stage: &'static str,
) -> Arc<ReceiveStream> {
    timeout(Duration::from_secs(30), async {
        loop {
            if let Some(stream) = content_streams(streams).get(index) {
                if stream.paused.load(Ordering::SeqCst) {
                    assert!(stream.validated.load(Ordering::SeqCst) >= 64 * 1024);
                    assert!(stream.validated.load(Ordering::SeqCst) < LARGE_BYTES as u64);
                    assert!(
                        stream.read.load(Ordering::SeqCst) < LARGE_BYTES as u64,
                        "暂停时必须仍有内容未从真实流读取: stage={stage}, read={}, validated={}",
                        stream.read.load(Ordering::SeqCst),
                        stream.validated.load(Ordering::SeqCst),
                    );
                    assert!(!stream.finished.load(Ordering::SeqCst));
                    return stream.clone();
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let states: Vec<_> = streams.receives().iter().map(|stream| (
            stream.total.load(Ordering::SeqCst),
            stream.read.load(Ordering::SeqCst),
            stream.validated.load(Ordering::SeqCst),
            stream.paused.load(Ordering::SeqCst),
            stream.stopped.load(Ordering::SeqCst),
            stream.finished.load(Ordering::SeqCst),
            *stream.failure.lock().unwrap(),
        )).collect();
        panic!("SDK暂停屏障超时: stage={stage}, index={index}, total/read/validated/paused/stopped/finished/failure={states:?}")
    })
}

async fn stage_files(source: &Engine, handles: &[&str]) -> String {
    let OperationResult::EntrySent(sent) = source
        .execute(Operation::SendFiles(SendFilesInput {
            files: handles
                .iter()
                .map(|handle| HostFileHandle::new(*handle))
                .collect(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("locally saved file entry expected")
    };
    sent.entry_id
}

async fn prove_engine_streams(shared_content: bool, recover_second: bool, observe_resources: bool) {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let native = BlobNativeObservation::start();
    let server = MockServer::builder().start().await;
    mount_engine_rendezvous(&server).await;
    let source_root = tempfile::tempdir().unwrap();
    let receiver_root = tempfile::tempdir().unwrap();
    let provider = ReceiveStreams::install(source_root.path(), false);
    let receiving = ReceiveStreams::install(receiver_root.path(), false);
    let _release = ReleaseStreamsOnDrop(receiving.clone());
    let source_host = HostCapabilities::new(
        HostDirectories::new(
            source_root.path().join("private"),
            source_root.path().join("cache"),
            source_root.path().join("temporary"),
            source_root.path().join("logs"),
        ),
        Box::new(MemoryHostSecureStorage::default()),
        Box::new(StaticHostClipboard {
            snapshot: HostClipboardSnapshot {
                observed_at_ms: 0,
                representations: Vec::new(),
            },
        }),
        Box::new(StreamFiles),
    );
    let config = EngineConfig::new("1.2.3")
        .with_rendezvous_base_url(server.uri())
        .with_test_relay_fallback(false);
    let (source, _) = Engine::start(config.clone(), source_host).await.unwrap();
    let source = Arc::new(source);
    source
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("Source".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    source
        .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
            sync: Some(SyncSettingsPatch {
                auto_sync_enabled: Some(true),
                sync_on_restore: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        })))
        .await
        .unwrap();
    // 配对前准备历史记录。发送时只重发旧记录，不同时改变当前剪贴板。
    // 首个小文件先持久交付，让原接收负责人交接进度后继续监督大文件。
    let original_a = stage_files(&source, &["2", "0", "3"]).await;
    let original_b =
        stage_files(&source, &["2", if shared_content { "0" } else { "1" }, "4"]).await;
    let OperationResult::EntrySent(current) = source
        .execute(Operation::SendText(SendTextInput {
            text: "stream-fixture-current".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("local text entry expected")
    };
    assert_eq!(
        source
            .execute(Operation::RestoreClipboard(RestoreClipboardInput {
                entry_id: current.entry_id,
                mode: ClipboardRestoreMode::Standard,
            }))
            .await
            .unwrap(),
        OperationResult::ClipboardRestored(ClipboardRestoreOutcome::Restored)
    );
    let (receiver, _receiver_events) =
        Engine::start(config, empty_engine_host(receiver_root.path()))
            .await
            .unwrap();
    let receiver = Arc::new(receiver);
    let OperationResult::InvitationIssued {
        invitation_code, ..
    } = source.execute(Operation::IssueInvitation).await.unwrap()
    else {
        panic!("invitation expected")
    };
    receiver
        .execute(Operation::JoinSpace(JoinSpaceInput {
            invitation_code,
            device_name: Some("Receiver".into()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }))
        .await
        .unwrap();
    let receiver_id = member_id(&receiver).await;
    member_id(&source).await;
    receiver
        .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
            sync: Some(SyncSettingsPatch {
                auto_sync_enabled: Some(true),
                sync_on_restore: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        })))
        .await
        .unwrap();
    timeout(Duration::from_secs(30), async {
        loop {
            if matches!(source.execute(Operation::QueryPeerConnections).await.unwrap(),
                OperationResult::PeerConnections(peers) if peers.iter().any(|peer| peer.peer_id == receiver_id && peer.connected)) { break; }
            sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    assert!(
        native.counts().0 <= 2,
        "准备过程只能使用同一对端的一条blob连接"
    );
    assert!(
        content_streams(&receiving).is_empty(),
        "准备阶段不能提前获取待测文件"
    );
    receiving.arm();
    let a = {
        let source = source.clone();
        let receiver_id = receiver_id.clone();
        tokio::spawn(async move {
            source
                .execute(Operation::ResendEntry(ResendEntryInput {
                    entry_id: original_a,
                    target_devices: vec![receiver_id],
                }))
                .await
                .unwrap()
        })
    };
    let entry_a = receiving_entry(&receiver, None).await;
    let stream_a = paused_stream(&receiving, 0, "first_transfer").await;
    let preserved = saved_files(&receiver, &entry_a.entry_id, receiver_root.path()).await;
    assert_eq!(preserved.len(), 1);
    assert!(preserved.contains_key(&vec![0x55; TAIL_BYTES]));
    if recover_second {
        receiving.fail_other_content_once(stream_a.digest, io::ErrorKind::ConnectionReset);
    }
    // 普通入站循环按完整接收串行处理；第二个意图走既有独立主动拉取入口。
    let b = {
        let source = source.clone();
        tokio::spawn(async move {
            source
                .execute(Operation::RestoreClipboard(RestoreClipboardInput {
                    entry_id: original_b,
                    mode: ClipboardRestoreMode::FilePaths,
                }))
                .await
                .unwrap()
        })
    };
    let entry_b = receiving_entry(&receiver, Some(&entry_a.entry_id)).await;
    assert!(
        entry_a.entry_id != entry_b.entry_id,
        "两个意图必须对应不同条目"
    );
    let stream_b = timeout(Duration::from_secs(30), async {
        loop {
            if let Some(stream) = content_streams(&receiving).get(1) {
                if provider
                    .provides
                    .lock()
                    .unwrap()
                    .contains(&stream.stream_id)
                    && (shared_content || stream.paused.load(Ordering::SeqCst))
                {
                    return stream.clone();
                }
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    if shared_content {
        assert!(
            stream_a.digest == stream_b.digest,
            "两个条目的大文件必须共享相同内容"
        );
    } else {
        paused_stream(&receiving, 1, "sibling_before_cancel").await;
        assert!(stream_a.digest != stream_b.digest, "独立文件应使用不同内容");
    }
    assert_ne!(stream_a.stream_id, stream_b.stream_id);
    assert_eq!(native.counts(), (2, 2), "两流只能使用一对原生连接句柄");
    let observer = if observe_resources {
        let prefix = saved_files(&receiver, &entry_b.entry_id, receiver_root.path()).await;
        assert_eq!(prefix.len(), 1);
        Some(resource_reads::ResourceReads::start(
            receiver.clone(),
            entry_b.entry_id.clone(),
        ))
    } else {
        None
    };
    let OperationResult::EntryReceiveCancellation(outcome) = receiver
        .execute(Operation::CancelEntryReceive(CancelEntryReceiveInput {
            entry_id: entry_a.entry_id.clone(),
            attempt_id: entry_a.attempt_id.clone(),
        }))
        .await
        .unwrap()
    else {
        panic!("cancellation result expected")
    };
    assert_eq!(outcome, EntryReceiveCancellationOutcome::Cancelled);
    assert!(stream_a.stopped.load(Ordering::SeqCst));
    assert!(
        stream_a.finished.load(Ordering::SeqCst),
        "取消成功必须在原SDK交换结束之后"
    );
    paused_stream(&receiving, 1, "sibling_after_cancel").await;
    assert!(
        !stream_b.finished.load(Ordering::SeqCst),
        "取消A不能结束仍暂停的B"
    );
    assert!(
        !stream_b.stopped.load(Ordering::SeqCst),
        "不能停止兄弟接收流"
    );
    assert_eq!(native.counts(), (2, 2), "取消不能关闭共用原生连接");
    assert_eq!(content_streams(&receiving).len(), 2, "B不能被迫重试");
    assert!(
        saved_files(&receiver, &entry_a.entry_id, receiver_root.path()).await == preserved,
        "取消必须保留首项已交付文件"
    );
    stream_b.release();
    let completed = progress(&receiver, &entry_b.entry_id, "completed").await;
    assert!(
        completed.attempt_id == entry_b.attempt_id,
        "B保持原业务尝试"
    );
    let saved = saved_files(&receiver, &entry_b.entry_id, receiver_root.path()).await;
    assert_eq!(saved.len(), 3);
    assert!(saved.contains_key(&vec![if shared_content { 0x11 } else { 0x33 }; LARGE_BYTES]));
    assert!(saved.contains_key(&vec![0x55; TAIL_BYTES]));
    assert!(saved.contains_key(&vec![0x77; TAIL_BYTES]));
    let calls = receiving.receives();
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.digest == stream_b.digest)
            .count(),
        if shared_content || recover_second {
            2
        } else {
            1
        },
        "B的大文件不得包含被迫重传"
    );
    let failures: Vec<_> = calls
        .iter()
        .filter(|call| call.injected_failure.load(Ordering::SeqCst))
        .collect();
    assert_eq!(failures.len(), usize::from(recover_second));
    if let Some(failed) = failures.first() {
        assert!(failed.digest == stream_b.digest);
        assert_ne!(failed.stream_id, stream_b.stream_id);
        assert!(failed.validated.load(Ordering::SeqCst) >= 64 * 1024);
        assert!(failed.stopped.load(Ordering::SeqCst));
        assert!(failed.finished.load(Ordering::SeqCst));
    }
    if !shared_content {
        assert_eq!(
            calls
                .iter()
                .filter(|call| call.digest == stream_a.digest)
                .count(),
            1
        );
    }
    for stream in [&stream_a, &stream_b] {
        assert_eq!(
            provider
                .provides
                .lock()
                .unwrap()
                .iter()
                .filter(|id| **id == stream.stream_id)
                .count(),
            1,
            "每条真实流必须只提供一次"
        );
    }
    assert_eq!(native.counts(), (2, 2));
    assert!(saved_files(&receiver, &entry_a.entry_id, receiver_root.path()).await == preserved);
    {
        let OperationResult::EntryResent(ResendEntryOutcome::Completed(report)) =
            timeout(Duration::from_secs(30), a).await.unwrap().unwrap()
        else {
            panic!("resend report expected")
        };
        assert_eq!(
            report.accepted + report.pending + report.errored + report.duplicate + report.offline,
            1
        );
    }
    assert_eq!(
        timeout(Duration::from_secs(30), b).await.unwrap().unwrap(),
        OperationResult::ClipboardRestored(ClipboardRestoreOutcome::Restored),
    );
    if let Some(observer) = observer {
        observer.finish().await;
    }
    receiver.shutdown_until_complete().await.unwrap();
    source.shutdown_until_complete().await.unwrap();
    assert_eq!(native.counts(), (2, 0), "完整关闭必须释放共用原生连接");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_cancels_one_blob_stream_without_retrying_its_sibling() {
    prove_engine_streams(false, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_cancels_one_shared_content_stream_without_retrying_its_sibling() {
    prove_engine_streams(true, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_recovers_a_transient_blob_read_then_completes_the_original_attempt() {
    prove_engine_streams(false, true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_keeps_partial_resources_readable_during_item_commits() {
    prove_engine_streams(false, false, true).await;
}
