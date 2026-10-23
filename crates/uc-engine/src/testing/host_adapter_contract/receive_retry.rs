//! 稳定Engine操作驱动真实网络、SQLite与文件成果；测试控制本地导出或保存故障。
mod database;
mod lifecycle;
mod observation;
mod redelivery;
mod save_failure;
#[cfg(target_os = "linux")]
mod stream_isolation;
#[cfg(target_os = "linux")]
use super::blob_resources::ReceiveResourceBaseline;
use super::{
    empty_engine_host, mount_engine_rendezvous, next_engine_event_matching,
    MemoryHostSecureStorage, ENGINE_TEST_LOCK,
};
use crate::testing::receive_exports::{ReceiveExports, ReleaseOnDrop};
use crate::{
    BlobResourceInput, CancelEntryReceiveInput, CreateSpaceInput, DeviceMembershipSummary, Engine,
    EngineConfig, EngineErrorCategory, EngineEvent, EntryReceiveCancellationOutcome,
    EntryReceiveProgressInput, EntryReceiveRetryOutcome, EventStream, HistoryEntryInput,
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostClipboard,
    HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata,
    JoinSpaceInput, Operation, OperationResult, OperationTerminal, QueryHistoryInput,
    ReceiveItemFailureReason, ReceiveProgressSummary, ReceiveRetryUnavailableReason,
    RemoveMemberInput, RetryEntryReceiveInput, SecretString, SendFilesInput, SendTextInput,
    SettingsPatch, SyncSettingsPatch,
};
use database::open_control_connection;
use diesel::sql_types::{Binary, Text};
use diesel::{QueryableByName, RunQueryDsl, SqliteConnection};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};
use url::Url;
use wiremock::MockServer;

const NAME: &str = "engine-retry-filename-probe.bin";
const CONTENTS: [&[u8]; 3] = [b"one-data", b"two-data", b"tri-data"];

struct Files;
fn unavailable() -> HostCapabilityError {
    HostCapabilityError::new(
        HostCapabilityErrorCategory::Unavailable,
        "test file unavailable",
    )
}
impl HostFileAccess for Files {
    fn metadata(&self, handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        let index: usize = handle.as_str().parse().map_err(|_| unavailable())?;
        let bytes = CONTENTS.get(index).ok_or_else(unavailable)?;
        Ok(HostFileMetadata {
            display_name: NAME.into(),
            size_bytes: bytes.len() as u64,
            mime_type: Some("application/octet-stream".into()),
        })
    }
    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        let index: usize = handle.as_str().parse().map_err(|_| unavailable())?;
        let bytes = CONTENTS.get(index).ok_or_else(unavailable)?;
        let start = usize::try_from(offset)
            .map_err(|_| unavailable())?
            .min(bytes.len());
        let end = start.saturating_add(max_bytes as usize).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }
    fn write_chunk(&self, _: &HostFileHandle, _: u64, _: &[u8]) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
    fn finish_write(&self, _: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
}

pub(super) async fn member_id(engine: &Engine) -> String {
    timeout(Duration::from_secs(30), async {
        loop {
            match engine.execute(Operation::QueryDeviceGroupChoices).await {
                Ok(OperationResult::DeviceGroupChoices(choices))
                    if choices.device_trust.local_membership == DeviceMembershipSummary::Active
                        && choices.device_trust.devices.iter().any(|device| {
                            !device.is_local && device.membership == DeviceMembershipSummary::Active
                        }) =>
                {
                    return choices.device_trust.local_device_id
                }
                Ok(OperationResult::DeviceGroupChoices(_)) => {}
                // 原稳定查询允许暂态不可用；只在原30秒准备期限内继续查询。
                Err(error)
                    if error.code() == 1103
                        || (error.category() == EngineErrorCategory::Unavailable
                            && error.is_retryable()) => {}
                other => panic!("membership readiness failed: {other:?}"),
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

async fn progress(engine: &Engine, entry: &str, state: &str) -> ReceiveProgressSummary {
    let mut last = None;
    timeout(Duration::from_secs(20), async {
        loop {
            let result = engine
                .execute(Operation::QueryEntryReceiveProgress(
                    EntryReceiveProgressInput {
                        entry_id: entry.into(),
                    },
                ))
                .await
                .unwrap();
            if let OperationResult::EntryReceiveProgress(Some(progress)) = result {
                if progress.state == state {
                    return progress;
                }
                last = Some(progress);
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("expected receive state {state}, last: {last:?}"))
}

async fn saved_files(engine: &Engine, entry: &str, root: &Path) -> BTreeMap<Vec<u8>, PathBuf> {
    let OperationResult::HistoryEntryResource(resource) = engine
        .execute(Operation::GetHistoryEntryResource(HistoryEntryInput {
            entry_id: entry.into(),
        }))
        .await
        .unwrap()
    else {
        panic!("file representation expected")
    };
    let bytes = match (resource.inline_data, resource.blob_id) {
        (Some(bytes), _) => bytes,
        (None, Some(blob_id)) => {
            let OperationResult::BlobRead(blob) = engine
                .execute(Operation::ReadBlob(BlobResourceInput { blob_id }))
                .await
                .unwrap()
            else {
                panic!("file URI payload expected")
            };
            blob.bytes
        }
        _ => panic!("file URI representation missing"),
    };
    let mut files = BTreeMap::new();
    for line in std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
    {
        let uri = Url::parse(line).unwrap();
        if uri.scheme() == "file" {
            let path = uri.to_file_path().unwrap().canonicalize().unwrap();
            assert!(path.starts_with(root.canonicalize().unwrap()));
            assert!(files.insert(std::fs::read(&path).unwrap(), path).is_none());
        } else {
            assert_eq!(uri.scheme(), "uniclip-missing");
        }
    }
    files
}

struct ReceiveClipboard(Arc<Mutex<HostClipboardSnapshot>>);
impl HostClipboard for ReceiveClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

struct ReceiveCase {
    #[cfg(target_os = "linux")]
    resource_baseline: Option<ReceiveResourceBaseline>,
    source: Engine,
    receiver: Engine,
    source_id: String,
    receiver_root: TempDir,
    exports: Arc<ReceiveExports>,
    first: ReceiveProgressSummary,
    events: EventStream,
    saved: BTreeMap<Vec<u8>, PathBuf>,
    clipboard: Arc<Mutex<HostClipboardSnapshot>>,
    _source_root: TempDir,
    _server: MockServer,
}
impl ReceiveCase {
    fn retry_input(&self) -> RetryEntryReceiveInput {
        RetryEntryReceiveInput {
            entry_id: self.first.entry_id.clone(),
            expected_attempt_id: self.first.attempt_id.clone(),
            item_ids: vec![self.first.items[1].item_id.clone()],
        }
    }
    async fn shutdown(self) {
        self.receiver.shutdown_until_complete().await.unwrap();
        self.source.shutdown_until_complete().await.unwrap();
        #[cfg(target_os = "linux")]
        if let Some(mut baseline) = self.resource_baseline.clone() {
            baseline.record_before_case_drop();
            drop(self);
            baseline.verify_after_shutdown().await;
        }
    }
}

async fn receive_case(pause_first: bool) -> ReceiveCase {
    receive_case_with_progress(pause_first, false, 1).await
}

async fn receive_case_with_progress(
    pause_first: bool,
    prove_progress: bool,
    fail_export_at: usize,
) -> ReceiveCase {
    #[cfg(target_os = "linux")]
    let resource_baseline = ReceiveResourceBaseline::capture();
    // 服务由本次fixture独占，基线先于创建；关闭验收也覆盖其线程和FD，避免进程池冷启动竞态。
    let server = MockServer::builder().start().await;
    mount_engine_rendezvous(&server).await;
    let source_root = tempfile::tempdir().unwrap();
    let receiver_root = tempfile::tempdir().unwrap();
    let exports = ReceiveExports::install(
        receiver_root.path(),
        fail_export_at,
        if prove_progress {
            2
        } else if pause_first {
            0
        } else {
            3
        },
    );
    let clipboard = Arc::new(Mutex::new(HostClipboardSnapshot {
        observed_at_ms: 0,
        representations: Vec::new(),
    }));
    let host = HostCapabilities::new(
        HostDirectories::new(
            source_root.path().join("private"),
            source_root.path().join("cache"),
            source_root.path().join("temporary"),
            source_root.path().join("logs"),
        ),
        Box::new(MemoryHostSecureStorage::default()),
        Box::new(ReceiveClipboard(clipboard.clone())),
        Box::new(Files),
    );
    let config = EngineConfig::new("1.2.3")
        .with_rendezvous_base_url(server.uri())
        .with_test_relay_fallback(false);
    let (source, _source_events) = Engine::start(config.clone(), host).await.unwrap();
    let (receiver, mut events) = Engine::start(config, empty_engine_host(receiver_root.path()))
        .await
        .unwrap();
    source
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("Source".into()),
            passphrase: SecretString::new("real retry fixture"),
            passphrase_confirmation: SecretString::new("real retry fixture"),
        }))
        .await
        .unwrap();
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
            passphrase: SecretString::new("real retry fixture"),
            preserve_unreadable_history: false,
        }))
        .await
        .unwrap();
    let receiver_id = member_id(&receiver).await;
    let source_id = member_id(&source).await;
    for (engine, automatic) in [(&source, true), (&receiver, false)] {
        engine
            .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
                sync: Some(SyncSettingsPatch {
                    auto_sync_enabled: Some(automatic),
                    sync_on_restore: Some(false),
                    ..Default::default()
                }),
                ..Default::default()
            })))
            .await
            .unwrap();
    }
    timeout(Duration::from_secs(30), async {
        loop {
            if matches!(source.execute(Operation::QueryPeerConnections).await.unwrap(),
                OperationResult::PeerConnections(peers)
                    if peers.iter().any(|peer| peer.peer_id == receiver_id && peer.connected))
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let OperationResult::EntrySent(report) = source
        .execute(Operation::SendFiles(SendFilesInput {
            files: (0..3)
                .map(|index| HostFileHandle::new(index.to_string()))
                .collect(),
            target_devices: vec![receiver_id],
        }))
        .await
        .unwrap()
    else {
        panic!("file send expected")
    };
    if pause_first || prove_progress {
        assert_eq!(report.total_accepted + report.total_pending, 1);
    } else {
        assert_eq!(report.total_accepted, 1);
    }
    let EngineEvent::IncomingPending(pending) = next_engine_event_matching(&mut events, |event| {
        matches!(event, EngineEvent::IncomingPending(_))
    })
    .await
    else {
        panic!("pending receive expected")
    };
    if prove_progress {
        let _release = ReleaseOnDrop(exports.clone());
        timeout(Duration::from_secs(20), exports.paused.notified())
            .await
            .unwrap();
        let receiving = progress(&receiver, &pending.entry_id, "receiving").await;
        assert_eq!(
            receiving
                .items
                .iter()
                .map(|item| item.state.as_str())
                .collect::<Vec<_>>(),
            vec!["completed", "failed", "transferring"]
        );
        let committed = saved_files(&receiver, &pending.entry_id, receiver_root.path()).await;
        assert_eq!(
            committed.len(),
            1,
            "后项未结束前，首项必须已经通过实际保存入口可读"
        );
        assert!(committed.contains_key(CONTENTS[0]));
        assert_eq!(exports.exports.lock().unwrap().len(), 3);
        exports.release.add_permits(1);
    }
    let (first, saved) = if pause_first {
        timeout(Duration::from_secs(20), exports.paused.notified())
            .await
            .unwrap();
        let first = progress(&receiver, &pending.entry_id, "receiving").await;
        assert_eq!(
            receiver
                .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                    entry_id: first.entry_id.clone()
                }))
                .await
                .unwrap_err()
                .category(),
            EngineErrorCategory::NotFound
        );
        (first, BTreeMap::new())
    } else {
        let first = progress(&receiver, &pending.entry_id, "failed").await;
        assert_eq!(
            first
                .items
                .iter()
                .map(|item| item.state.as_str())
                .collect::<Vec<_>>(),
            vec!["completed", "failed", "completed"]
        );
        assert_eq!(
            first.items[1].failure_reason,
            Some(ReceiveItemFailureReason::StorageUnavailable)
        );
        assert_eq!(exports.exports.lock().unwrap().len(), 3);
        let saved = saved_files(&receiver, &first.entry_id, receiver_root.path()).await;
        assert_eq!(saved.len(), 2);
        assert!(saved.contains_key(CONTENTS[0]) && saved.contains_key(CONTENTS[2]));
        (first, saved)
    };
    ReceiveCase {
        #[cfg(target_os = "linux")]
        resource_baseline,
        source,
        receiver,
        source_id,
        receiver_root,
        exports,
        first,
        saved,
        events,
        clipboard,
        _source_root: source_root,
        _server: server,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_file_is_durably_saved_while_batch_and_later_file_are_still_receiving() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case_with_progress(false, true, 1).await;
    assert_eq!(case.saved.len(), 2);
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_partial_file_retry_preserves_saved_files_and_only_fetches_failed_item() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let receiver = &case.receiver;
    let first = &case.first;
    let exports = &case.exports;
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("single retry must be accepted")
    };
    assert_ne!(attempt_id, first.attempt_id);
    timeout(Duration::from_secs(20), exports.paused.notified())
        .await
        .unwrap();
    for _ in 0..4 {
        assert_eq!(
            receiver
                .execute(Operation::RetryEntryReceive(case.retry_input()))
                .await
                .unwrap(),
            OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::AlreadyReceiving)
        );
    }
    assert_eq!(
        receiver
            .execute(Operation::CancelEntryReceive(CancelEntryReceiveInput {
                entry_id: first.entry_id.clone(),
                attempt_id: first.attempt_id.clone(),
            }))
            .await
            .unwrap(),
        OperationResult::EntryReceiveCancellation(EntryReceiveCancellationOutcome::Superseded)
    );
    assert_eq!(
        saved_files(receiver, &first.entry_id, case.receiver_root.path()).await,
        case.saved
    );
    exports.release.add_permits(1);
    let completed = progress(&receiver, &first.entry_id, "completed").await;
    assert_eq!(completed.attempt_id, attempt_id);
    assert_eq!(completed.items.len(), 3);
    assert!(completed.items.iter().all(|item| item.state == "completed"));
    let final_files = saved_files(receiver, &first.entry_id, case.receiver_root.path()).await;
    assert_eq!(final_files.len(), 3);
    for (bytes, path) in &case.saved {
        assert_eq!(final_files.get(bytes), Some(path));
    }
    for bytes in CONTENTS {
        assert!(final_files.contains_key(bytes));
    }
    for calls in [&exports.protects, &exports.exports] {
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 4, "成功项不能重新进入获取或导出");
        assert_eq!(calls[3], calls[1]);
    }
    // 提交后还有受监督的后续动作；旧请求须在完整负责人退出后明确过期。
    timeout(Duration::from_secs(10), async {
        loop {
            match receiver
                .execute(Operation::RetryEntryReceive(case.retry_input()))
                .await
                .unwrap()
            {
                OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Superseded) => break,
                OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::AlreadyReceiving) => {}
                other => panic!("old attempt must not start another retry: {other:?}"),
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_retry_rechecks_revoked_sender_before_reusing_cached_blob() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    case.receiver
        .execute(Operation::RemoveMember(RemoveMemberInput {
            device_id: case.source_id.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(
        case.receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap(),
        OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Unavailable {
            reason: ReceiveRetryUnavailableReason::AccessDenied,
        })
    );
    let current = progress(&case.receiver, &case.first.entry_id, "failed").await;
    assert_eq!(current.attempt_id, case.first.attempt_id);
    assert_eq!(case.exports.exports.lock().unwrap().len(), 3);
    assert_eq!(case.exports.protects.lock().unwrap().len(), 3);
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_cancelled_retry_waits_for_export_and_keeps_previous_successes() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let receiver = &case.receiver;
    let OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { attempt_id }) =
        receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap()
    else {
        panic!("retry must be accepted")
    };
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let output = case.exports.paths.lock().unwrap()[3].clone();
    assert!(output.is_file(), "真实导出已经写入，但尚未交回完成权");
    let cancellation = CancelEntryReceiveInput {
        entry_id: case.first.entry_id.clone(),
        attempt_id: attempt_id.clone(),
    };
    // 丢弃调用者等待不能丢弃已接受的停止与本地清理责任。
    assert!(timeout(
        Duration::from_millis(50),
        receiver.execute(Operation::CancelEntryReceive(cancellation.clone()))
    )
    .await
    .is_err());
    let cancelling = progress(receiver, &case.first.entry_id, "cancelling").await;
    assert_eq!(cancelling.attempt_id, attempt_id);
    assert_eq!(
        receiver
            .execute(Operation::CancelEntryReceive(cancellation))
            .await
            .unwrap(),
        OperationResult::EntryReceiveCancellation(
            EntryReceiveCancellationOutcome::CancellationIncomplete
        )
    );
    assert_eq!(
        receiver
            .execute(Operation::RetryEntryReceive(RetryEntryReceiveInput {
                entry_id: case.first.entry_id.clone(),
                expected_attempt_id: attempt_id.clone(),
                item_ids: vec![cancelling.items[1].item_id.clone()],
            }))
            .await
            .unwrap(),
        OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::CancellationIncomplete)
    );
    assert_eq!(case.exports.exports.lock().unwrap().len(), 4);
    assert_eq!(
        saved_files(receiver, &case.first.entry_id, case.receiver_root.path()).await,
        case.saved
    );
    case.exports.release.add_permits(1);
    let cancelled = progress(receiver, &case.first.entry_id, "cancelled").await;
    assert_eq!(cancelled.attempt_id, attempt_id);
    assert_eq!(
        cancelled
            .items
            .iter()
            .map(|item| item.state.as_str())
            .collect::<Vec<_>>(),
        vec!["completed", "cancelled", "completed"]
    );
    assert!(!output.exists(), "本次输出须清理后才能报告取消终态");
    assert_eq!(
        saved_files(receiver, &case.first.entry_id, case.receiver_root.path()).await,
        case.saved
    );
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_abandoned_deletion_stops_retry_and_removes_all_receive_state() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let mut case = receive_case(false).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let receiver = &case.receiver;
    assert!(matches!(
        receiver
            .execute(Operation::RetryEntryReceive(case.retry_input()))
            .await
            .unwrap(),
        OperationResult::EntryReceiveRetry(EntryReceiveRetryOutcome::Accepted { .. })
    ));
    timeout(Duration::from_secs(20), case.exports.paused.notified())
        .await
        .unwrap();
    let output = case.exports.paths.lock().unwrap()[3].clone();
    assert!(output.is_file());
    assert!(timeout(
        Duration::from_millis(50),
        receiver.execute(Operation::DeleteHistoryEntry(HistoryEntryInput {
            entry_id: case.first.entry_id.clone(),
        }))
    )
    .await
    .is_err());
    // 等待者已经离开；受管导出未结算时，删除不能越过停止屏障。
    assert_eq!(
        saved_files(receiver, &case.first.entry_id, case.receiver_root.path()).await,
        case.saved
    );
    case.exports.release.add_permits(1);
    timeout(Duration::from_secs(20), async {
        loop {
            let deleted = matches!(receiver.execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: case.first.entry_id.clone(),
            })).await, Err(error) if error.category() == EngineErrorCategory::NotFound);
            let no_receive = receiver
                .execute(Operation::QueryEntryReceiveProgress(
                    EntryReceiveProgressInput {
                        entry_id: case.first.entry_id.clone(),
                    },
                ))
                .await
                .unwrap()
                == OperationResult::EntryReceiveProgress(None);
            if deleted
                && no_receive
                && !output.exists()
                && case.saved.values().all(|path| !path.exists())
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // 数据消失早于搜索清理及删除scope退出。本例只丢弃了这一项操作的等待；
    // 等它的Cancelled完成事件后再断言重复删除，避免与仍持有封口的原操作竞争。
    next_engine_event_matching(&mut case.events, |event| {
        matches!(
            event,
            EngineEvent::OperationFinished {
                terminal: OperationTerminal::Cancelled,
                ..
            }
        )
    })
    .await;
    assert_eq!(
        receiver
            .execute(Operation::DeleteHistoryEntry(HistoryEntryInput {
                entry_id: case.first.entry_id.clone(),
            }))
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::NotFound
    );
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_clear_without_saved_entry_waits_for_export_and_removes_receive_state() {
    let _lock = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(true).await;
    let _release = ReleaseOnDrop(case.exports.clone());
    let output = case.exports.paths.lock().unwrap()[0].clone();
    assert!(output.is_file());
    assert!(
        timeout(
            Duration::from_millis(50),
            case.receiver.execute(Operation::ClearHistory)
        )
        .await
        .is_err(),
        "无历史条目也不能绕过实际停止与清理"
    );
    case.exports.release.add_permits(1);
    timeout(Duration::from_secs(20), async {
        loop {
            let no_receive = case
                .receiver
                .execute(Operation::QueryEntryReceiveProgress(
                    EntryReceiveProgressInput {
                        entry_id: case.first.entry_id.clone(),
                    },
                ))
                .await
                .unwrap()
                == OperationResult::EntryReceiveProgress(None);
            if no_receive && !output.exists() {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        case.receiver
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: case.first.entry_id.clone()
            }))
            .await
            .unwrap_err()
            .category(),
        EngineErrorCategory::NotFound
    );
    let text = "receive after completed clear fixture";
    let OperationResult::EntrySent(report) = case
        .source
        .execute(Operation::SendText(SendTextInput {
            text: text.into(),
            target_devices: vec![member_id(&case.receiver).await],
        }))
        .await
        .unwrap()
    else {
        panic!("new send expected")
    };
    assert_eq!(report.total_accepted, 1);
    timeout(Duration::from_secs(20), async {
        loop {
            if let OperationResult::HistoryPage { entries, .. } = case
                .receiver
                .execute(Operation::QueryHistory(QueryHistoryInput {
                    cursor: None,
                    limit: 10,
                    query: None,
                }))
                .await
                .unwrap()
            {
                if entries.len() == 1 && entries[0].preview.as_deref() == Some(text) {
                    assert_ne!(entries[0].entry_id, case.first.entry_id);
                    break;
                }
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    case.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_engine_delete_refuses_unauthenticated_file_references_and_preserves_retry_material() {
    let _guard = ENGINE_TEST_LOCK.lock().await;
    let case = receive_case(false).await;
    let entry = &case.first.entry_id;
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
    #[derive(QueryableByName)]
    struct Inline {
        #[diesel(sql_type = Text)]
        id: String,
        #[diesel(sql_type = Binary)]
        inline_data: Vec<u8>,
    }
    let original = diesel::sql_query(
        "SELECT r.id, r.inline_data FROM clipboard_snapshot_representation r \
         JOIN clipboard_entry e ON r.event_id = e.event_id \
         WHERE e.entry_id = ? AND r.inline_data IS NOT NULL ORDER BY r.id LIMIT 1",
    )
    .bind::<Text, _>(entry)
    .get_result::<Inline>(&mut connection)
    .unwrap();
    assert!(!original
        .inline_data
        .windows(NAME.len())
        .any(|bytes| bytes == NAME.as_bytes()));
    // 真实已加密文件引用被破坏；不能用旧降级返回的密文完成删除。
    let mut corrupted = original.inline_data.clone();
    let last = corrupted.last_mut().unwrap();
    *last ^= 1;
    assert_eq!(
        diesel::sql_query(
            "UPDATE clipboard_snapshot_representation SET inline_data = ? WHERE id = ?"
        )
        .bind::<Binary, _>(&corrupted)
        .bind::<Text, _>(&original.id)
        .execute(&mut connection)
        .unwrap(),
        1
    );
    let failure = case
        .receiver
        .execute(Operation::DeleteHistoryEntry(HistoryEntryInput {
            entry_id: entry.clone(),
        }))
        .await
        .unwrap_err();
    assert_eq!(failure.category(), EngineErrorCategory::Internal);
    let clear_failure = case
        .receiver
        .execute(Operation::ClearHistory)
        .await
        .unwrap_err();
    assert_eq!(clear_failure.category(), EngineErrorCategory::Internal);
    let retained = diesel::sql_query(
        "SELECT r.id, r.inline_data FROM clipboard_snapshot_representation r \
         JOIN clipboard_entry e ON r.event_id = e.event_id \
         WHERE e.entry_id = ? AND r.id = ?",
    )
    .bind::<Text, _>(entry)
    .bind::<Text, _>(&original.id)
    .get_result::<Inline>(&mut connection)
    .unwrap();
    assert_eq!(retained.inline_data, corrupted);
    assert_eq!(
        progress(&case.receiver, entry, "failed").await.attempt_id,
        case.first.attempt_id
    );
    for (bytes, path) in &case.saved {
        assert_eq!(&std::fs::read(path).unwrap(), bytes);
    }
    // 恢复原密文后，原动作仍可读取原成果并完成清空，不需要新建接收尝试。
    diesel::sql_query("UPDATE clipboard_snapshot_representation SET inline_data = ? WHERE id = ?")
        .bind::<Binary, _>(&original.inline_data)
        .bind::<Text, _>(&original.id)
        .execute(&mut connection)
        .unwrap();
    assert_eq!(
        saved_files(&case.receiver, entry, case.receiver_root.path()).await,
        case.saved
    );
    case.receiver
        .execute(Operation::ClearHistory)
        .await
        .unwrap();
    for path in case.saved.values() {
        assert!(!path.exists());
    }
    drop(connection);
    case.shutdown().await;
}
