mod owner;
use super::{
    file_blob_input, test_materializer, ApplyInboundClipboardUseCase, ApplyInboundError,
    ApplyInboundInput, ApplyOutcome, AttemptGate, FakeAvailability, InboundApplyCommonDeps,
    InboundReceiveAttemptDeps, MockBlobFetcher, MockEntryRepo, NoopReceiveArtifactCleanup,
    RecordingLiveIndex, StoreOnlyPullDeps,
};
use crate::clipboard::capture::InboundCaptureCommitContext;
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::clipboard::sync::apply_inbound::InboundCapture;
use crate::clipboard::sync::payload_codec::{
    encode_snapshot_with_blob_refs_to_v3_bytes, V3BlobRef,
};
use crate::facade::blob_transfer::{BlobTransferError, FetchBlobToPathResult};
use crate::facade::HostEventBus;
use crate::space::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
};
use crate::transfer::receive::materialization::{
    OutputCleanupFailure, ReceiveSelection, ReceivedItem, ReceivedOutput,
};
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use crate::transfer::receive::retry::{
    BeginReceiveRetryInput, BeginReceiveRetryPort, PreparedReceiveRetry,
    ReceiveCancellationBarrier, RetryEntryReceiveOutcome,
};
use crate::transfer::receive::work::{
    ReceiveRegistrationError, ReceiveRepresentation, ReceiveSource, ReceiveWorkRegistry,
};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::{io, path::PathBuf};
use tokio::sync::{oneshot, Notify, Semaphore};
use uc_core::clipboard::ClipboardRepositoryError;
use uc_core::file_transfer::{FileTransferCancellationReason, OutboundProgressStatus};
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::blob::{BlobDigest, BlobTicket, PlaintextHash};
use uc_core::ports::clipboard::GetEntrySnapshotHashPort;
use uc_core::ports::{
    AttemptState, BeginReceiveAttemptPort, BeginReceiveOutcome, EntryReceiveProgress,
    FileTransferProjectionError, ReceiveArtifact, ReceiveArtifactOwnership, ReceiveItemKind,
    ReceiveItemProgress, TrackedFileTransferStatus,
};
use uc_core::{MimeType, ObservedClipboardRepresentation, SnapshotHash, SystemClipboardSnapshot};

#[tokio::test]
async fn manual_retry_cleanup_failure_preserves_sources_and_keeps_attempt_nonterminal() {
    for cancelled in [false, true] {
        let fixture = Fixture::new(0).await;
        let mut fetcher = MockBlobFetcher::new();
        fetcher
            .expect_fetch_blob_to_path()
            .times(1)
            .returning(move |command| {
                // 目录不能由remove_file清理，模拟实际文件系统拒绝而非fake清理结果。
                std::fs::remove_file(&command.target_path).unwrap();
                std::fs::create_dir(&command.target_path).unwrap();
                Err(if cancelled {
                    BlobTransferError::Cancelled.into()
                } else {
                    io::Error::from(io::ErrorKind::StorageFull).into()
                })
            });
        let receiver = fixture
            .apply
            .as_ref()
            .clone()
            .with_blob_materializer(Arc::new(test_materializer(
                Arc::new(fetcher),
                fixture._cache.path().to_owned(),
            )));
        let (prepared, reply) = fixture.prepared();
        let error = receiver.execute_retry(prepared).await.unwrap_err();
        assert!(matches!(
            reply.await.unwrap(),
            RetryEntryReceiveOutcome::Accepted { .. }
        ));
        let ApplyInboundError::Internal(source) = error else {
            panic!("cleanup failure must retain its source")
        };
        let cleanup = source
            .chain()
            .find_map(|error| error.downcast_ref::<OutputCleanupFailure>())
            .unwrap();
        assert_eq!(cleanup.cleanup.kind(), io::ErrorKind::IsADirectory);
        if cancelled {
            assert!(source.chain().any(|error| matches!(
                error.downcast_ref::<BlobTransferError>(),
                Some(BlobTransferError::Cancelled)
            )));
        } else {
            assert_eq!(
                source
                    .chain()
                    .find_map(|error| error.downcast_ref::<io::Error>())
                    .unwrap()
                    .kind(),
                io::ErrorKind::StorageFull
            );
        }
        assert_eq!(
            *fixture.attempts.state.lock().unwrap(),
            if cancelled {
                AttemptState::Cancelling
            } else {
                AttemptState::Failing
            }
        );
        assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
        assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    }
}

#[tokio::test]
async fn discarded_partial_redelivery_preserves_original_saved_recipe_for_manual_retry() {
    for save_fails in [false, true] {
        let fixture = Fixture::new(1).await;
        fixture.capture.fail.store(save_fails, Ordering::SeqCst);
        let mut fetcher = MockBlobFetcher::new();
        fetcher
            .expect_fetch_blob_to_path()
            .times(2)
            .returning(move |command| {
                if !save_fails
                    && command
                        .transfer_context
                        .as_ref()
                        .unwrap()
                        .transfer_id
                        .ends_with("member:0")
                {
                    return Err(io::Error::from(io::ErrorKind::ConnectionReset).into());
                }
                std::fs::write(&command.target_path, b"newfile").unwrap();
                Ok(FetchBlobToPathResult {
                    entry_id: command.entry_id,
                    bytes_written: 7,
                    plaintext_hash: PlaintextHash::from_bytes(*blake3::hash(b"newfile").as_bytes()),
                    digest: BlobDigest::from_bytes([6; 32]),
                })
            });
        let redelivery = receiver(
            fixture.attempts.clone(),
            fixture.capture.clone(),
            fixture.readiness.clone(),
            fetcher,
            fixture._cache.path().to_owned(),
        );
        // 新包的来源、项数、ticket及表示ID都不同，不能把其索引当成旧保存证明。
        let mut incoming = fixture.snapshot.clone();
        incoming.representations[0].id = RepresentationId::new();
        let refs = (0..2)
            .map(|_| V3BlobRef {
                ticket: BlobTicket::from_bytes(vec![6]),
                entry_id: EntryId::from("different-source-entry"),
                filename: Some("incoming.bin".into()),
                mime: None,
                size_bytes: 7,
                representation_index: None,
            })
            .collect::<Vec<_>>();
        let (wire, _) = encode_snapshot_with_blob_refs_to_v3_bytes(&incoming, &refs).unwrap();
        let result = redelivery
            .execute(ApplyInboundInput {
                from_device: DeviceId::new("different-sender"),
                plaintext: wire,
                snapshot_hash: "original-hash".into(),
                ..file_blob_input()
            })
            .await;
        if save_fails {
            let Err(ApplyInboundError::Capture(source)) = result else {
                panic!("save must fail")
            };
            assert_eq!(
                source
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<io::Error>())
                    .unwrap()
                    .kind(),
                io::ErrorKind::StorageFull
            );
        } else {
            assert!(matches!(
                result.unwrap(),
                ApplyOutcome::DuplicateSkipped { .. }
            ));
        }
        fixture.capture.fail.store(false, Ordering::SeqCst);
        assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
        let attempt = fixture.attempts.attempt_id.lock().unwrap().clone();
        assert_ne!(attempt, "old");
        let (source, landed) = fixture
            .work
            .retry_material(&EntryId::from("receiver"), &attempt)
            .unwrap();
        assert_eq!(source.from_device, DeviceId::new("source"));
        assert_eq!(source.blob_refs.len(), 3);
        assert!(source
            .blob_refs
            .iter()
            .all(|blob| blob.ticket.as_bytes() == [1]));
        assert_eq!(
            source.representations[0].id,
            fixture.snapshot.representations[0].id
        );
        assert_eq!(landed.keys().copied().collect::<Vec<_>>(), vec![0]);
        let mut progress = EntryReceiveProgress {
            entry_id: "receiver".into(),
            attempt_id: attempt.clone(),
            state: AttemptState::Failed,
            total_bytes: 0,
            completed_bytes: 0,
            items_total: 0,
            items_completed: 0,
            items: Vec::new(),
        };
        fixture.work.merge_progress(&mut progress);
        assert_eq!(
            progress
                .items
                .iter()
                .map(|item| item.status)
                .collect::<Vec<_>>(),
            vec![
                TrackedFileTransferStatus::Completed,
                TrackedFileTransferStatus::Failed,
                TrackedFileTransferStatus::Cancelled,
            ]
        );
        assert!(progress
            .items
            .iter()
            .all(|item| item.item_id.contains(&attempt)));
        let (accepted, reply) = oneshot::channel();
        fixture
            .apply
            .execute_retry(PreparedReceiveRetry {
                begin: fixture.begin.clone(),
                cancellation: fixture.checks.clone(),
                identity: fixture.checks.clone(),
                scope: fixture.checks.clone(),
                entry: EntryId::from("receiver"),
                expected_attempt: attempt,
                source,
                snapshot: fixture.snapshot.clone(),
                selection: Some(ReceiveSelection {
                    selected: HashSet::from([1]),
                    landed,
                    unselected: HashMap::from([(2, progress.items[2].clone())]),
                }),
                action: fixture.work.action(),
                accepted: Some(accepted),
            })
            .await
            .unwrap();
        assert!(matches!(
            reply.await.unwrap(),
            RetryEntryReceiveOutcome::Accepted { .. }
        ));
        assert_eq!(fixture.capture.snapshots.lock().unwrap().len(), 1);
        assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
        assert_eq!(
            fixture.capture.snapshots.lock().unwrap()[0]
                .file_content_digests
                .len(),
            2
        );
    }
}

struct Checks {
    pause_identity: AtomicBool,
    identity_entered: Notify,
    identity_release: Semaphore,
    locked: AtomicBool,
    settling: AtomicBool,
    allowed: AtomicBool,
    hash: Mutex<Option<String>>,
}
impl ReceiveCancellationBarrier for Checks {
    fn is_settling(&self, _: &EntryId, _: &str) -> bool {
        self.settling.load(Ordering::SeqCst)
    }
}
#[async_trait]
impl CurrentSpaceMemberScopePort for Checks {
    async fn snapshot(
        &self,
    ) -> std::result::Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(CurrentSpaceMemberScopeError::Locked);
        }
        Ok(CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: true,
            usable_peer_device_ids: if self.allowed.load(Ordering::SeqCst) {
                vec![DeviceId::new("source")]
            } else {
                Vec::new()
            },
            paused_peer_devices: Vec::new(),
        })
    }
}
#[async_trait]
impl GetEntrySnapshotHashPort for Checks {
    async fn get_entry_snapshot_hash(
        &self,
        _: &EntryId,
    ) -> std::result::Result<Option<String>, ClipboardRepositoryError> {
        if self.pause_identity.load(Ordering::SeqCst) {
            self.identity_entered.notify_one();
            self.identity_release.acquire().await.unwrap().forget();
        }
        Ok(self.hash.lock().unwrap().clone())
    }
}
struct Capture {
    attempts: Arc<AttemptGate>,
    snapshots: Mutex<Vec<SystemClipboardSnapshot>>,
    deliveries: Mutex<Vec<Vec<String>>>,
    entered: Notify,
    release: Semaphore,
    slow: AtomicBool,
    fail: AtomicBool,
}
#[async_trait]
impl InboundCapture for Capture {
    async fn capture(
        &self,
        _: EntryId,
        _: DeviceId,
        _: SystemClipboardSnapshot,
    ) -> Result<Option<EntryId>> {
        panic!("retry must replace its partial entry atomically")
    }
    async fn replace_inbound_with_identity(
        &self,
        entry: EntryId,
        _: DeviceId,
        snapshot: SystemClipboardSnapshot,
        _: Option<SnapshotHash>,
        commit: InboundCaptureCommitContext,
    ) -> Result<Option<EntryId>> {
        if self.slow.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::StorageFull).into());
        }
        *self.attempts.state.lock().unwrap() = if snapshot.file_content_digests.len() == 3 {
            AttemptState::Completed
        } else {
            AttemptState::Cancelled
        };
        let delivered_item_ids = match commit {
            InboundCaptureCommitContext::Progress {
                delivered_item_ids, ..
            }
            | InboundCaptureCommitContext::Complete {
                delivered_item_ids, ..
            }
            | InboundCaptureCommitContext::Partial {
                delivered_item_ids, ..
            } => delivered_item_ids,
        };
        self.deliveries.lock().unwrap().push(delivered_item_ids);
        self.snapshots.lock().unwrap().push(snapshot);
        Ok(Some(entry))
    }
}
struct RetryStarter {
    attempts: Arc<AttemptGate>,
    plans: Mutex<Vec<BeginReceiveRetryInput>>,
}
#[async_trait]
impl BeginReceiveRetryPort for RetryStarter {
    async fn begin_receive_retry(
        &self,
        input: BeginReceiveRetryInput,
    ) -> std::result::Result<BeginReceiveOutcome, FileTransferProjectionError> {
        input
            .validate()
            .map_err(|source| FileTransferProjectionError::Backend(Box::new(source)))?;
        self.plans.lock().unwrap().push(input.clone());
        self.attempts
            .begin_redelivery(
                &input.entry_id,
                &input.expected_attempt_id,
                &input.attempt_id,
                input.now_ms,
            )
            .await
            .map_err(|source| FileTransferProjectionError::Backend(Box::new(source)))
    }
}

fn receiver(
    attempts: Arc<AttemptGate>,
    capture: Arc<Capture>,
    readiness: Arc<ReceiveReadinessCoordinator>,
    fetcher: MockBlobFetcher,
    cache_dir: PathBuf,
) -> Arc<ApplyInboundClipboardUseCase> {
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .returning(|_| Ok(Some(EntryId::from("receiver"))));
    Arc::new(ApplyInboundClipboardUseCase::store_only_pull(
        StoreOnlyPullDeps {
            common: InboundApplyCommonDeps {
                receive_work: readiness.work_registry(),
                entry_repo: Arc::new(repo),
                capture: capture.clone(),
                blob_materializer: Arc::new(test_materializer(Arc::new(fetcher), cache_dir)),
                receive_attempts: InboundReceiveAttemptDeps {
                    get: attempts.clone(),
                    begin: attempts.clone(),
                    claim_commit: attempts.clone(),
                    request_cancel: attempts.clone(),
                    begin_failure: attempts.clone(),
                    commit: attempts.clone(),
                    clock: attempts.clone(),
                },
                receive_artifact_cleanup: Arc::new(NoopReceiveArtifactCleanup),
                receive_readiness: readiness,
                host_event_emitter: Arc::new(HostEventBus::new()),
                search_live_index: Arc::new(RecordingLiveIndex {
                    calls: AtomicUsize::new(0),
                }),
                availability: Arc::new(FakeAvailability { available: false }),
                entry_identity_coordinator: Arc::new(EntryIdentityCoordinator::new()),
            },
        },
    ))
}

struct Fixture {
    begin: Arc<RetryStarter>,
    _cache: tempfile::TempDir,
    saved: PathBuf,
    apply: Arc<ApplyInboundClipboardUseCase>,
    capture: Arc<Capture>,
    attempts: Arc<AttemptGate>,
    work: Arc<ReceiveWorkRegistry>,
    readiness: Arc<ReceiveReadinessCoordinator>,
    checks: Arc<Checks>,
    snapshot: SystemClipboardSnapshot,
}
impl Fixture {
    async fn new(fetches: usize) -> Self {
        let cache = tempfile::tempdir().unwrap();
        let saved = cache.path().join("iroh-blobs/source/same.bin");
        std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
        std::fs::write(&saved, b"success").unwrap();
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        readiness.ensure_ready(|| async { Ok(()) }).await.unwrap();
        let work = readiness.work_registry();
        let attempts = AttemptGate::new(AttemptState::Failed, None);
        *attempts.attempt_id.lock().unwrap() = "old".into();
        let capture = Arc::new(Capture {
            attempts: attempts.clone(),
            snapshots: Mutex::new(Vec::new()),
            deliveries: Mutex::new(Vec::new()),
            entered: Notify::new(),
            release: Semaphore::new(0),
            slow: AtomicBool::new(false),
            fail: AtomicBool::new(false),
        });
        let mut fetcher = MockBlobFetcher::new();
        fetcher
            .expect_fetch_blob_to_path()
            .times(fetches)
            .withf(move |command| {
                command
                    .transfer_context
                    .as_ref()
                    .unwrap()
                    .transfer_id
                    .ends_with("member:1")
                    || (fetches > 1
                        && command
                            .transfer_context
                            .as_ref()
                            .unwrap()
                            .transfer_id
                            .ends_with("member:2"))
            })
            .returning(|command| {
                std::fs::write(&command.target_path, b"retried").unwrap();
                Ok(FetchBlobToPathResult {
                    entry_id: command.entry_id,
                    bytes_written: 7,
                    plaintext_hash: PlaintextHash::from_bytes(*blake3::hash(b"retried").as_bytes()),
                    digest: BlobDigest::from_bytes([4; 32]),
                })
            });
        let apply = receiver(
            attempts.clone(),
            capture.clone(),
            readiness.clone(),
            fetcher,
            cache.path().to_owned(),
        );
        let snapshot = SystemClipboardSnapshot {
            ts_ms: 1,
            representations: vec![ObservedClipboardRepresentation::new(
                RepresentationId::new(),
                FormatId::from("files"),
                Some(MimeType("text/uri-list".into())),
                Vec::new(),
            )],
            file_content_digests: Vec::new(),
            file_set_v1_component: None,
        };
        let original = work.action();
        original
            .register(&EntryId::from("receiver"), "old")
            .unwrap();
        original.retain_source(ReceiveSource {
            from_device: DeviceId::new("source"),
            expected_hash: "original-hash".into(),
            blob_refs: (0..3)
                .map(|_| V3BlobRef {
                    ticket: BlobTicket::from_bytes(vec![1]),
                    entry_id: EntryId::from("source"),
                    filename: Some("same.bin".into()),
                    mime: None,
                    size_bytes: 7,
                    representation_index: None,
                })
                .collect(),
            manifest: None,
            original_wire: None,
            representations: snapshot
                .representations
                .iter()
                .map(|rep| ReceiveRepresentation {
                    id: rep.id.clone(),
                    format_id: rep.format_id.clone(),
                    mime: rep.mime.clone(),
                })
                .collect(),
        });
        original.batch.item_result(
            "receiver:attempt:old:member:0",
            7,
            OutboundProgressStatus::Completed,
            None,
        );
        original.batch.item_result(
            "receiver:attempt:old:member:1",
            0,
            OutboundProgressStatus::Failed,
            None,
        );
        original.batch.item_result(
            "receiver:attempt:old:member:2",
            0,
            OutboundProgressStatus::Cancelled {
                reason: FileTransferCancellationReason::LocalUser,
            },
            None,
        );
        original.persisted(
            false,
            &[ReceivedItem {
                blob_index: 0,
                plaintext_hash: *blake3::hash(b"success").as_bytes(),
                bytes: 7,
                output: ReceivedOutput::File(ReceiveArtifact {
                    item_id: "receiver:attempt:old:member:0".into(),
                    staged_path: saved.clone(),
                    final_path: saved.clone(),
                    ownership: ReceiveArtifactOwnership::ManagedStaging,
                }),
            }],
        );
        original.finish(Ok(()));
        Self {
            begin: Arc::new(RetryStarter {
                attempts: attempts.clone(),
                plans: Mutex::new(Vec::new()),
            }),
            _cache: cache,
            saved,
            apply,
            capture,
            attempts,
            work,
            readiness,
            checks: Arc::new(Checks {
                pause_identity: AtomicBool::new(false),
                identity_entered: Notify::new(),
                identity_release: Semaphore::new(0),
                locked: AtomicBool::new(false),
                settling: AtomicBool::new(false),
                allowed: AtomicBool::new(true),
                hash: Mutex::new(Some("original-hash".into())),
            }),
            snapshot,
        }
    }
    fn prepared(
        &self,
    ) -> (
        PreparedReceiveRetry,
        oneshot::Receiver<RetryEntryReceiveOutcome>,
    ) {
        let (source, landed) = self
            .work
            .retry_material(&EntryId::from("receiver"), "old")
            .unwrap();
        let (accepted, reply) = oneshot::channel();
        (
            PreparedReceiveRetry {
                begin: self.begin.clone(),
                cancellation: self.checks.clone(),
                entry: EntryId::from("receiver"),
                expected_attempt: "old".into(),
                source,
                snapshot: self.snapshot.clone(),
                action: self.work.action(),
                selection: Some(ReceiveSelection {
                    selected: HashSet::from([1]),
                    landed,
                    unselected: HashMap::from([(
                        2,
                        ReceiveItemProgress {
                            item_id: "receiver:attempt:old:member:2".into(),
                            kind: ReceiveItemKind::File,
                            status: TrackedFileTransferStatus::Cancelled,
                            failure_reason: None,
                        },
                    )]),
                }),
                accepted: Some(accepted),
                identity: self.checks.clone(),
                scope: self.checks.clone(),
            },
            reply,
        )
    }
}

#[tokio::test]
async fn manual_retry_saves_new_partial_success_without_fetching_or_deleting_previous_success() {
    let fixture = Fixture::new(1).await;
    let (prepared, reply) = fixture.prepared();
    fixture.apply.execute_retry(prepared).await.unwrap();
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = reply.await.unwrap() else {
        panic!("retry not accepted")
    };
    assert_ne!(attempt_id, "old");
    assert_eq!(
        fixture.capture.deliveries.lock().unwrap().as_slice(),
        &[vec![
            format!("receiver:attempt:{attempt_id}:member:0"),
            format!("receiver:attempt:{attempt_id}:member:1"),
        ]],
        "原成功与新增成功一起保存，未选取消项不能进入交付证明"
    );
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 1);
    let snapshots = fixture.capture.snapshots.lock().unwrap();
    assert_eq!(snapshots.len(), 1, "a second partial must be saved");
    assert_eq!(
        snapshots[0].file_content_digests,
        vec![
            *blake3::hash(b"success").as_bytes(),
            *blake3::hash(b"retried").as_bytes()
        ]
    );
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    let mut progress = EntryReceiveProgress {
        entry_id: "receiver".into(),
        attempt_id,
        state: AttemptState::Cancelled,
        total_bytes: 0,
        completed_bytes: 0,
        items_total: 0,
        items_completed: 0,
        items: Vec::new(),
    };
    fixture.work.merge_progress(&mut progress);
    assert_eq!(
        progress
            .items
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>(),
        vec![
            TrackedFileTransferStatus::Completed,
            TrackedFileTransferStatus::Completed,
            TrackedFileTransferStatus::Cancelled
        ]
    );
}

#[tokio::test]
async fn manual_retry_rechecks_expected_attempt_membership_and_stored_identity_before_cas() {
    for case in 0..5 {
        let fixture = Fixture::new(0).await;
        let (prepared, reply) = fixture.prepared();
        let expected = match case {
            0 => {
                *fixture.attempts.attempt_id.lock().unwrap() = "newer".into();
                RetryEntryReceiveOutcome::Superseded
            }
            1 => {
                fixture.checks.allowed.store(false, Ordering::SeqCst);
                RetryEntryReceiveOutcome::AccessDenied
            }
            2 => {
                *fixture.checks.hash.lock().unwrap() = Some("different-content".into());
                RetryEntryReceiveOutcome::Unavailable
            }
            3 => {
                fixture.checks.settling.store(true, Ordering::SeqCst);
                RetryEntryReceiveOutcome::CancellationIncomplete
            }
            _ => {
                fixture.checks.locked.store(true, Ordering::SeqCst);
                RetryEntryReceiveOutcome::AccessDenied
            }
        };
        fixture.apply.execute_retry(prepared).await.unwrap();
        assert_eq!(reply.await.unwrap(), expected);
        assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
        assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn manual_retry_waits_for_actual_old_action_settlement_and_respects_deletion_fence() {
    let fixture = Fixture::new(0).await;
    let (prepared, reply) = fixture.prepared();
    let old = fixture.work.action();
    old.register(&EntryId::from("receiver"), "old").unwrap();
    fixture.apply.execute_retry(prepared).await.unwrap();
    assert_eq!(
        reply.await.unwrap(),
        RetryEntryReceiveOutcome::CancellationIncomplete
    );
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
    old.finish(Ok(()));
    let (prepared, reply) = fixture.prepared();
    drop(
        fixture
            .work
            .delete_scope(&EntryId::from("receiver"))
            .await
            .unwrap(),
    );
    fixture.apply.execute_retry(prepared).await.unwrap();
    assert_eq!(reply.await.unwrap(), RetryEntryReceiveOutcome::Unavailable);
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn manual_retry_caller_drop_keeps_full_capture_supervised_until_actual_completion() {
    let fixture = Fixture::new(1).await;
    fixture.capture.slow.store(true, Ordering::SeqCst);
    let (prepared, reply) = fixture.prepared();
    let apply = fixture.apply.clone();
    let waiter = tokio::spawn(async move { apply.execute_retry(prepared).await });
    fixture.capture.entered.notified().await;
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = reply.await.unwrap() else {
        panic!("retry not accepted")
    };
    let mut pending = EntryReceiveProgress {
        entry_id: "receiver".into(),
        attempt_id: attempt_id.clone(),
        state: AttemptState::Receiving,
        total_bytes: 0,
        completed_bytes: 0,
        items_total: 0,
        items_completed: 0,
        items: Vec::new(),
    };
    fixture.work.merge_progress(&mut pending);
    assert_eq!(
        pending
            .items
            .iter()
            .map(|item| item.status)
            .collect::<Vec<_>>(),
        vec![
            TrackedFileTransferStatus::Completed,
            TrackedFileTransferStatus::Transferring,
            TrackedFileTransferStatus::Cancelled,
        ],
        "保存仍阻塞时，继承交付和未选取消已经可见，新下载不能提前交付"
    );
    waiter.abort();
    let stopped = fixture
        .work
        .stop(&EntryId::from("receiver"), &attempt_id)
        .unwrap();
    assert!(fixture.work.owns_entry("receiver"));
    fixture.capture.release.add_permits(1);
    stopped.wait().await.unwrap();
    assert_eq!(fixture.capture.snapshots.lock().unwrap().len(), 1);
    assert!(!fixture.work.owns_entry("receiver"));
}

#[tokio::test]
async fn manual_retry_full_completion_keeps_inherited_item_results_and_releases_all_sources() {
    let fixture = Fixture::new(2).await;
    let (prepared, first_reply) = fixture.prepared();
    fixture.apply.execute_retry(prepared).await.unwrap();
    let RetryEntryReceiveOutcome::Accepted { attempt_id: first } = first_reply.await.unwrap()
    else {
        panic!("not accepted")
    };
    let (source, landed) = fixture
        .work
        .retry_material(&EntryId::from("receiver"), &first)
        .unwrap();
    assert_eq!(landed.len(), 2);
    let (send, reply) = oneshot::channel();
    let snapshot = fixture.capture.snapshots.lock().unwrap()[0].clone();
    fixture
        .apply
        .execute_retry(PreparedReceiveRetry {
            begin: fixture.begin.clone(),
            cancellation: fixture.checks.clone(),
            identity: fixture.checks.clone(),
            scope: fixture.checks.clone(),
            entry: EntryId::from("receiver"),
            expected_attempt: first,
            source,
            snapshot,
            selection: Some(ReceiveSelection {
                selected: HashSet::from([2]),
                landed,
                unselected: HashMap::new(),
            }),
            action: fixture.work.action(),
            accepted: Some(send),
        })
        .await
        .unwrap();
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = reply.await.unwrap() else {
        panic!("not accepted")
    };
    assert_eq!(
        *fixture.attempts.state.lock().unwrap(),
        AttemptState::Completed
    );
    assert!(fixture
        .work
        .retry_material(&EntryId::from("receiver"), &attempt_id)
        .is_none());
    let mut current = EntryReceiveProgress {
        entry_id: "receiver".into(),
        attempt_id,
        state: AttemptState::Completed,
        total_bytes: 0,
        completed_bytes: 0,
        items_total: 0,
        items_completed: 0,
        items: Vec::new(),
    };
    fixture.work.merge_progress(&mut current);
    assert_eq!(current.items.len(), 3);
    assert!(current
        .items
        .iter()
        .all(|item| item.status == TrackedFileTransferStatus::Completed));
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
}

#[tokio::test]
async fn manual_retry_late_cleanup_or_gate_close_has_a_stable_refusal_without_new_attempt() {
    for closing in [false, true] {
        let fixture = Fixture::new(0).await;
        let (prepared, reply) = fixture.prepared();
        fixture.checks.pause_identity.store(true, Ordering::SeqCst);
        let apply = fixture.apply.clone();
        let task = tokio::spawn(async move { apply.execute_retry(prepared).await });
        fixture.checks.identity_entered.notified().await;
        let cleanup = if closing {
            fixture.work.close();
            None
        } else {
            fixture.work.claim_cleanup("receiver")
        };
        if !closing {
            assert!(cleanup.is_some());
        }
        fixture.checks.identity_release.add_permits(1);
        task.await.unwrap().unwrap();
        assert_eq!(
            reply.await.unwrap(),
            if closing {
                RetryEntryReceiveOutcome::Unavailable
            } else {
                RetryEntryReceiveOutcome::CancellationIncomplete
            }
        );
        assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
        assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
        drop(cleanup);
    }
}

#[tokio::test]
async fn ordinary_redelivery_waits_for_previous_terminal_actions_actual_settlement() {
    let fixture = Fixture::new(0).await;
    let previous = fixture.work.action();
    previous
        .register(&EntryId::from("receiver"), "old")
        .unwrap();
    let Err(ApplyInboundError::Internal(source)) = fixture.apply.execute(file_blob_input()).await
    else {
        panic!("previous action must finish first")
    };
    assert!(matches!(
        source.downcast_ref::<ReceiveRegistrationError>(),
        Some(ReceiveRegistrationError::CleanupInProgress)
    ));
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
    previous.finish(Ok(()));
}

#[tokio::test]
async fn actual_download_retains_original_protection_and_capacity_through_owned_capture() {
    use crate::transfer::blob::test_support::local_file_probe;
    use crate::transfer::blob::transport::{BlobExchangeFailure, BlobShutdownWait};
    let fixture = Fixture::new(0).await;
    fixture.capture.slow.store(true, Ordering::SeqCst);
    let (facade, transport, protected) = local_file_probe();
    let apply = Arc::new(
        fixture
            .apply
            .as_ref()
            .clone()
            .with_blob_materializer(Arc::new(test_materializer(
                facade,
                fixture._cache.path().to_owned(),
            ))),
    );
    let (prepared, reply) = fixture.prepared();
    let caller = {
        let apply = apply.clone();
        tokio::spawn(async move { apply.execute_retry(prepared).await })
    };
    fixture.capture.entered.notified().await;
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = reply.await.unwrap() else {
        panic!("retry not accepted");
    };
    let held_during_capture = protected.load(Ordering::SeqCst);
    let incomplete = transport.shutdown(BlobShutdownWait::Bounded).await;
    caller.abort();
    let stopped = fixture
        .work
        .stop(&EntryId::from("receiver"), &attempt_id)
        .unwrap();
    fixture.capture.release.add_permits(1);
    stopped.wait().await.unwrap();
    apply.shutdown().await.unwrap();
    transport
        .shutdown(BlobShutdownWait::Complete)
        .await
        .unwrap();
    assert_eq!(
        held_during_capture, 1,
        "capture still running must retain the original download protection"
    );
    assert!(
        matches!(incomplete, Err(error) if error.kind == BlobExchangeFailure::ShutdownIncomplete),
        "shutdown must wait for actual parent capture completion"
    );
    assert_eq!(protected.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    assert_eq!(fixture.capture.snapshots.lock().unwrap().len(), 1);
}
