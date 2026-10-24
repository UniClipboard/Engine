use super::{
    test_materializer, ApplyInboundClipboardUseCase, ApplyInboundInput, AttemptGate,
    FakeAvailability, HostEventBus, InboundApplyCommonDeps, InboundReceiveAttemptDeps,
    MockBlobFetcher, MockEntryRepo, NoopReceiveArtifactCleanup, RecordingLiveIndex,
    StoreOnlyPullDeps,
};
use crate::clipboard::capture::InboundCaptureCommitContext;
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::clipboard::sync::apply_inbound::InboundCapture;
use crate::clipboard::sync::payload_codec::{
    encode_snapshot_with_blob_refs_to_v3_bytes, V3BlobRef,
};
use crate::facade::blob_transfer::FetchBlobToPathResult;
use crate::transfer::receive::query::ReceiveProgressQuery;
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use anyhow::Result;
use async_trait::async_trait;
use std::io;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::blob::{BlobDigest, BlobTicket, PlaintextHash};
use uc_core::ports::{
    AttemptError, AttemptState, BeginReceiveAttemptPort, BeginReceiveOutcome, EntryReceiveAttempt,
    EntryReceiveProgress, FileTransferProjectionError, GetEntryReceiveProgressPort,
    ListNonTerminalAttemptsPort, TrackedFileTransferStatus,
};
use uc_core::{MimeType, ObservedClipboardRepresentation, SnapshotHash, SystemClipboardSnapshot};

struct Capture {
    entry: Mutex<Option<EntryId>>,
    attempts: Arc<AttemptGate>,
    fail: bool,
}
#[async_trait]
impl InboundCapture for Capture {
    async fn capture(
        &self,
        _: EntryId,
        _: DeviceId,
        _: SystemClipboardSnapshot,
    ) -> Result<Option<EntryId>> {
        panic!("receive must use its atomic persistence contract")
    }
    async fn capture_inbound_with_identity(
        &self,
        entry: EntryId,
        _: DeviceId,
        snapshot: SystemClipboardSnapshot,
        _: Option<SnapshotHash>,
        commit: InboundCaptureCommitContext,
    ) -> Result<Option<EntryId>> {
        *self.entry.lock().unwrap() = Some(entry.clone());
        if self.fail {
            return Err(io::Error::from(io::ErrorKind::StorageFull).into());
        }
        assert_eq!(snapshot.file_content_digests.len(), 1);
        if !matches!(commit, InboundCaptureCommitContext::Progress { .. }) {
            *self.attempts.state.lock().unwrap() = AttemptState::Failed;
        }
        Ok(Some(entry))
    }
    async fn replace_inbound_with_identity(
        &self,
        entry: EntryId,
        source: DeviceId,
        snapshot: SystemClipboardSnapshot,
        hash: Option<SnapshotHash>,
        commit: InboundCaptureCommitContext,
    ) -> Result<Option<EntryId>> {
        self.capture_inbound_with_identity(entry, source, snapshot, hash, commit)
            .await
    }
}
struct Progress(Arc<AttemptGate>);
#[async_trait]
impl GetEntryReceiveProgressPort for Progress {
    async fn get_entry_receive_progress(
        &self,
        entry: &str,
    ) -> Result<Option<EntryReceiveProgress>, FileTransferProjectionError> {
        Ok(Some(EntryReceiveProgress {
            entry_id: entry.into(),
            attempt_id: self.0.attempt_id.lock().unwrap().clone(),
            state: *self.0.state.lock().unwrap(),
            total_bytes: 0,
            completed_bytes: 0,
            items_total: 0,
            items_completed: 0,
            items: Vec::new(),
        }))
    }
}
#[async_trait]
impl ListNonTerminalAttemptsPort for Progress {
    async fn list_non_terminal_attempts(&self) -> Result<Vec<EntryReceiveAttempt>, AttemptError> {
        Ok(Vec::new())
    }
}

// attempt已写入、创建端口尚未返回时，同一个业务查询也必须看见完整未启动计划。
struct QueryBeforeBeginReturns {
    inner: Arc<AttemptGate>,
    query: Arc<ReceiveProgressQuery>,
}
impl QueryBeforeBeginReturns {
    async fn assert_original_plan(&self, entry: &str) {
        let progress = self
            .query
            .get(&EntryId::from(entry))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            progress.items_total, 2,
            "attempt一旦可见，未启动原始项也必须计入总量"
        );
        assert_eq!(progress.total_bytes, 14);
        assert_eq!(progress.items.len(), 2);
        assert_eq!(progress.items_completed, 0);
        assert_eq!(progress.completed_bytes, 0);
        assert!(progress
            .items
            .iter()
            .all(|item| item.status == TrackedFileTransferStatus::Pending));
    }
}
#[async_trait]
impl BeginReceiveAttemptPort for QueryBeforeBeginReturns {
    async fn begin_first_receive(
        &self,
        entry: &str,
        attempt: &str,
        now_ms: i64,
    ) -> Result<BeginReceiveOutcome, AttemptError> {
        let outcome = self
            .inner
            .begin_first_receive(entry, attempt, now_ms)
            .await?;
        if outcome == BeginReceiveOutcome::Begun {
            self.assert_original_plan(entry).await;
        }
        Ok(outcome)
    }
    async fn begin_redelivery(
        &self,
        entry: &str,
        expected: &str,
        attempt: &str,
        now_ms: i64,
    ) -> Result<BeginReceiveOutcome, AttemptError> {
        let outcome = self
            .inner
            .begin_redelivery(entry, expected, attempt, now_ms)
            .await?;
        if outcome == BeginReceiveOutcome::Begun {
            self.assert_original_plan(entry).await;
        }
        Ok(outcome)
    }
}

#[tokio::test]
async fn real_partial_materialization_query_only_completes_files_that_were_actually_saved() {
    for fail in [false, true] {
        let cache = tempfile::tempdir().unwrap();
        let mut fetcher = MockBlobFetcher::new();
        fetcher
            .expect_fetch_blob_to_path()
            .times(2)
            .returning(|command| {
                if command
                    .transfer_context
                    .as_ref()
                    .unwrap()
                    .transfer_id
                    .ends_with("member:1")
                {
                    return Err(io::Error::from(io::ErrorKind::ConnectionReset).into());
                }
                std::fs::write(&command.target_path, b"success").unwrap();
                Ok(FetchBlobToPathResult {
                    entry_id: command.entry_id,
                    bytes_written: 7,
                    plaintext_hash: PlaintextHash::from_bytes([3; 32]),
                    digest: BlobDigest::from_bytes([4; 32]),
                })
            });
        let materializer = test_materializer(Arc::new(fetcher), cache.path().to_owned());
        let snapshot = SystemClipboardSnapshot {
            ts_ms: 1,
            representations: vec![ObservedClipboardRepresentation::new(
                RepresentationId::new(),
                FormatId::from("files"),
                Some(MimeType("text/uri-list".into())),
                b"file:///PRIVATE/a/same.bin\r\nfile:///PRIVATE/b/same.bin\r\n".to_vec(),
            )],
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        };
        let refs = (0..2)
            .map(|_| V3BlobRef {
                ticket: BlobTicket::from_bytes(vec![1]),
                entry_id: EntryId::from("PRIVATE_SOURCE"),
                filename: Some("same.bin".into()),
                mime: None,
                size_bytes: 7,
                representation_index: None,
            })
            .collect::<Vec<_>>();
        let (wire, hash) = encode_snapshot_with_blob_refs_to_v3_bytes(&snapshot, &refs).unwrap();
        let attempts = AttemptGate::empty();
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        readiness.ensure_ready(|| async { Ok(()) }).await.unwrap();
        let projection = Arc::new(Progress(attempts.clone()));
        let initial_query = Arc::new(ReceiveProgressQuery::new(
            projection.clone(),
            projection,
            readiness.work_registry(),
        ));
        let observed_begin = Arc::new(QueryBeforeBeginReturns {
            inner: attempts.clone(),
            query: initial_query,
        });
        let capture = Arc::new(Capture {
            entry: Mutex::new(None),
            attempts: attempts.clone(),
            fail,
        });
        let mut repo = MockEntryRepo::new();
        repo.expect_find_entry_id_by_snapshot_hash()
            .returning(|_| Ok(None));
        let receiver = ApplyInboundClipboardUseCase::store_only_pull(StoreOnlyPullDeps {
            common: InboundApplyCommonDeps {
                receive_retry: None,
                receive_work: readiness.work_registry(),
                entry_repo: Arc::new(repo),
                capture: capture.clone(),
                blob_materializer: Arc::new(materializer),
                receive_attempts: InboundReceiveAttemptDeps {
                    get: attempts.clone(),
                    begin: observed_begin,
                    claim_commit: attempts.clone(),
                    request_cancel: attempts.clone(),
                    begin_failure: attempts.clone(),
                    commit: attempts.clone(),
                    clock: attempts.clone(),
                },
                receive_artifact_cleanup: Arc::new(NoopReceiveArtifactCleanup),
                receive_readiness: readiness.clone(),
                host_event_emitter: Arc::new(HostEventBus::new()),
                search_live_index: Arc::new(RecordingLiveIndex {
                    calls: AtomicUsize::new(0),
                }),
                availability: Arc::new(FakeAvailability { available: false }),
                entry_identity_coordinator: Arc::new(EntryIdentityCoordinator::new()),
            },
        });
        let result = receiver
            .execute(ApplyInboundInput {
                source_entry: None,
                from_device: DeviceId::new("PRIVATE_DEVICE"),
                snapshot_hash: hash,
                plaintext: wire,
                ..super::file_blob_input()
            })
            .await;
        assert_eq!(result.is_err(), fail);
        let entry = capture.entry.lock().unwrap().clone().unwrap();
        let projection = Arc::new(Progress(attempts));
        let query =
            ReceiveProgressQuery::new(projection.clone(), projection, readiness.work_registry());
        let progress = query.get(&entry).await.unwrap().unwrap();
        assert_eq!(progress.items.len(), 2);
        assert_eq!(
            progress.items[0].status,
            if fail {
                TrackedFileTransferStatus::Failed
            } else {
                TrackedFileTransferStatus::Completed
            }
        );
        assert_eq!(progress.items[1].status, TrackedFileTransferStatus::Failed);
        assert!(!format!("{progress:?}").contains("PRIVATE"));
    }
}
