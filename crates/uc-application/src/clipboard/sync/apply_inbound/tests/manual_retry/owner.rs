use super::super::{
    fixture_input, FakeAvailability, InboundApplyCommonDeps, InboundReceiveAttemptDeps,
    MockBlobMaterializer, MockCapture, MockEntryRepo, NoopReceiveArtifactCleanup,
    RecordingLiveIndex, StoreOnlyPullDeps,
};
use super::{AttemptGate, Fixture};
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::clipboard::sync::apply_inbound::{ApplyInboundClipboardUseCase, ApplyOutcome};
use crate::facade::HostEventBus;
use crate::transfer::receive::materialization::ReceiveReuseRefusal;
use crate::transfer::receive::query::ReceiveProgressQuery;
use crate::transfer::receive::retry::{
    ReceiveCancellationBarrier, ReceiveRedeliveryRefusal, ReceiveRetryPreparation,
    RetryEntryReceiveError, RetryEntryReceiveOutcome, RetryEntryReceiveUseCase,
    RetrySnapshotReader,
};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashSet;
use std::io::{Error as IoError, ErrorKind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, Semaphore};
use uc_core::blob::ports::BlobReaderPort;
use uc_core::clipboard::{
    ClipboardEntry, ClipboardRepositoryError, PersistedClipboardRepresentation,
};
use uc_core::ids::{DeviceId, EntryId, EventId, RepresentationId};
use uc_core::ports::clipboard::{
    ClipboardPayloadResolverPort, GetClipboardEntryPort, GetRepresentationPort,
    PayloadResolveError, ResolvedClipboardPayload,
};
use uc_core::ports::{
    AttemptError, EntryReceiveAttempt, EntryReceiveProgress, FileTransferProjectionError,
    GetEntryReceiveProgressPort, ListNonTerminalAttemptsPort,
};
use uc_core::{BlobId, SystemClipboardSnapshot};
use uc_observability_contract::diagnostics::connectivity::{
    ClipboardReceiveFailure, ClipboardReceiveObservation, ClipboardRetryOutcome,
};

struct Stored {
    snapshot: SystemClipboardSnapshot,
    attempts: Arc<AttemptGate>,
    lost: AtomicBool,
    query_failure: Mutex<Option<ErrorKind>>,
    slow: AtomicBool,
    entered: Notify,
    release: Semaphore,
}
#[async_trait]
impl GetClipboardEntryPort for Stored {
    async fn get_entry(
        &self,
        entry: &EntryId,
    ) -> Result<Option<ClipboardEntry>, ClipboardRepositoryError> {
        Ok(Some(ClipboardEntry::new(
            entry.clone(),
            EventId::from("event"),
            1,
            0,
        )))
    }
}
#[async_trait]
impl GetRepresentationPort for Stored {
    async fn get_representation(
        &self,
        _: &EventId,
        id: &RepresentationId,
    ) -> Result<Option<PersistedClipboardRepresentation>, ClipboardRepositoryError> {
        if self.slow.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        Ok(self
            .snapshot
            .representations
            .iter()
            .find(|rep| &rep.id == id)
            .map(|rep| {
                PersistedClipboardRepresentation::new(
                    rep.id.clone(),
                    rep.format_id.clone(),
                    rep.mime.clone(),
                    rep.inline_bytes().unwrap().len() as i64,
                    Some(rep.inline_bytes().unwrap().to_vec()),
                    None,
                )
            }))
    }
}
#[async_trait]
impl ClipboardPayloadResolverPort for Stored {
    async fn resolve(
        &self,
        rep: &PersistedClipboardRepresentation,
    ) -> Result<ResolvedClipboardPayload, PayloadResolveError> {
        if self.lost.load(Ordering::SeqCst) {
            return Err(PayloadResolveError::Lost {
                rep_id: rep.id.clone(),
                reason: "PRIVATE_LOST_BODY".into(),
            });
        }
        Ok(ResolvedClipboardPayload::Inline {
            mime: "text/uri-list".into(),
            bytes: rep.inline_data.clone().unwrap(),
        })
    }
}
#[async_trait]
impl BlobReaderPort for Stored {
    async fn get(&self, _: &BlobId) -> Result<Vec<u8>> {
        panic!("inline fixture must not fetch a different body")
    }
}
#[async_trait]
impl GetEntryReceiveProgressPort for Stored {
    async fn get_entry_receive_progress(
        &self,
        entry: &str,
    ) -> Result<Option<EntryReceiveProgress>, FileTransferProjectionError> {
        if let Some(kind) = *self.query_failure.lock().unwrap() {
            return Err(FileTransferProjectionError::Backend(Box::new(
                IoError::new(kind, "PRIVATE_RETRY_PROGRESS_READ"),
            )));
        }
        Ok(Some(EntryReceiveProgress {
            snapshot_version: None,
            entry_id: entry.into(),
            attempt_id: self.attempts.attempt_id.lock().unwrap().clone(),
            state: *self.attempts.state.lock().unwrap(),
            total_bytes: 21,
            completed_bytes: 7,
            items_total: 0,
            items_completed: 0,
            items: Vec::new(),
        }))
    }
}
#[async_trait]
impl ListNonTerminalAttemptsPort for Stored {
    async fn list_non_terminal_attempts(&self) -> Result<Vec<EntryReceiveAttempt>, AttemptError> {
        Ok(Vec::new())
    }
}
struct Barrier(AtomicBool);
impl ReceiveCancellationBarrier for Barrier {
    fn is_settling(&self, _: &EntryId, _: &str) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}
fn owner(fixture: &Fixture) -> (Arc<RetryEntryReceiveUseCase>, Arc<Stored>, Arc<Barrier>) {
    let stored = Arc::new(Stored {
        snapshot: fixture.snapshot.clone(),
        attempts: fixture.attempts.clone(),
        lost: AtomicBool::new(false),
        query_failure: Mutex::new(None),
        slow: AtomicBool::new(false),
        entered: Notify::new(),
        release: Semaphore::new(0),
    });
    let barrier = Arc::new(Barrier(AtomicBool::new(false)));
    (
        Arc::new(RetryEntryReceiveUseCase {
            preparation: Arc::new(ReceiveRetryPreparation {
                begin: fixture.begin.clone(),
                cache_dir: fixture._cache.path().to_owned(),
                query: Arc::new(ReceiveProgressQuery::new(
                    stored.clone(),
                    stored.clone(),
                    fixture.work.clone(),
                )),
                work: fixture.work.clone(),
                cancellation: barrier.clone(),
                scope: fixture.checks.clone(),
                snapshots: RetrySnapshotReader {
                    entry: stored.clone(),
                    representations: stored.clone(),
                    payload: stored.clone(),
                    blobs: stored.clone(),
                    identity: fixture.checks.clone(),
                },
            }),
            apply: fixture.apply.clone(),
            requests: Arc::new(Mutex::new(HashSet::new())),
        }),
        stored,
        barrier,
    )
}
fn item(index: usize) -> Vec<String> {
    vec![format!("receiver:attempt:old:member:{index}")]
}

#[tokio::test]
async fn retry_owner_accepts_one_set_and_refuses_different_set_until_full_capture_finishes() {
    let fixture = Fixture::new(1).await;
    fixture.capture.slow.store(true, Ordering::SeqCst);
    let (owner, _, barrier) = owner(&fixture);
    let result = owner
        .execute(EntryId::from("receiver"), "old".into(), item(1))
        .await
        .unwrap();
    let RetryEntryReceiveOutcome::Accepted { attempt_id } = result else {
        panic!("not accepted")
    };
    fixture.capture.entered.notified().await;
    assert_eq!(
        owner
            .execute(EntryId::from("receiver"), "old".into(), item(2))
            .await
            .unwrap(),
        RetryEntryReceiveOutcome::AlreadyReceiving
    );
    barrier.0.store(true, Ordering::SeqCst);
    assert_eq!(
        owner
            .execute(EntryId::from("receiver"), attempt_id.clone(), item(1))
            .await
            .unwrap(),
        RetryEntryReceiveOutcome::CancellationIncomplete,
        "防重入仍持有时也必须区分正在停止的完整动作"
    );
    barrier.0.store(false, Ordering::SeqCst);
    let current = owner
        .preparation
        .query
        .get(&EntryId::from("receiver"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.attempt_id, attempt_id);
    assert_eq!(current.items.len(), 3);
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 1);
    let waiting = fixture
        .work
        .stop(&EntryId::from("receiver"), &attempt_id)
        .unwrap();
    fixture.capture.release.add_permits(1);
    waiting.wait().await.unwrap();
    assert_eq!(fixture.capture.snapshots.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn retry_owner_rejects_invalid_selection_and_incomplete_cancellation_without_new_attempt() {
    let fixture = Fixture::new(0).await;
    let (owner, _, barrier) = owner(&fixture);
    for items in [
        Vec::new(),
        item(0),
        vec!["unknown".into()],
        vec![item(1)[0].clone(), item(1)[0].clone()],
    ] {
        assert!(matches!(
            owner
                .execute(EntryId::from("receiver"), "old".into(), items)
                .await,
            Err(RetryEntryReceiveError::InvalidInput)
        ));
    }
    barrier.0.store(true, Ordering::SeqCst);
    assert_eq!(
        owner
            .execute(EntryId::from("receiver"), "old".into(), item(1))
            .await
            .unwrap(),
        RetryEntryReceiveOutcome::CancellationIncomplete
    );
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn retry_owner_preflight_caller_drop_does_not_leave_a_reservation_or_create_attempt() {
    let fixture = Fixture::new(0).await;
    let (owner, stored, _) = owner(&fixture);
    stored.slow.store(true, Ordering::SeqCst);
    let worker = owner.clone();
    let waiter = tokio::spawn(async move {
        worker
            .execute(EntryId::from("receiver"), "old".into(), item(1))
            .await
    });
    stored.entered.notified().await;
    assert!(owner
        .requests
        .lock()
        .unwrap()
        .contains(&EntryId::from("receiver")));
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(owner.requests.lock().unwrap().is_empty());
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn retry_owner_changed_saved_file_is_rejected_before_cas_with_original_reason() {
    let fixture = Fixture::new(0).await;
    let (owner, _, _) = owner(&fixture);
    std::fs::write(&fixture.saved, b"changed").unwrap();
    let error = owner
        .execute(EntryId::from("receiver"), "old".into(), item(1))
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE"));
    let RetryEntryReceiveError::Unavailable(source) = error else {
        panic!("wrong failure")
    };
    assert!(source.downcast_ref::<ReceiveReuseRefusal>().is_some());
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retry_owner_lost_stored_payload_preserves_source_and_never_starts_a_new_attempt() {
    let fixture = Fixture::new(0).await;
    let (owner, stored, _) = owner(&fixture);
    stored.lost.store(true, Ordering::SeqCst);
    let error = owner
        .execute(EntryId::from("receiver"), "old".into(), item(1))
        .await
        .unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE_LOST_BODY"));
    assert!(!error.to_string().contains("PRIVATE_LOST_BODY"));
    let RetryEntryReceiveError::Unavailable(source) = error else {
        panic!("wrong failure")
    };
    assert!(matches!(
        source.downcast_ref::<PayloadResolveError>(),
        Some(PayloadResolveError::Lost { .. })
    ));
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn ordinary_redelivery_prepares_original_positions_and_all_failed_items_without_new_work() {
    let fixture = Fixture::new(0).await;
    let (owner, _, _) = owner(&fixture);
    let action = fixture.work.action();
    let prepared = owner
        .preparation
        .prepare_redelivery(
            &EntryId::from("receiver"),
            "original-hash",
            DeviceId::new("source"),
            action.clone(),
        )
        .await
        .unwrap();
    let selection = prepared.selection.as_ref().unwrap();
    assert_eq!(selection.selected, HashSet::from([1, 2]));
    assert_eq!(
        selection.landed.keys().copied().collect::<Vec<_>>(),
        vec![0]
    );
    assert!(selection.unselected.is_empty());
    assert_eq!(prepared.source.blob_refs.len(), 3);
    assert!(prepared
        .source
        .blob_refs
        .iter()
        .all(|blob| blob.ticket.as_bytes() == [1]));
    assert_eq!(
        prepared.snapshot.representations[0].id,
        fixture.snapshot.representations[0].id
    );
    assert!(Arc::ptr_eq(&prepared.action, &action));
    assert!(prepared.accepted.is_none());
    assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
}

#[tokio::test]
async fn ordinary_redelivery_refuses_unavailable_material_or_changed_output_without_new_attempt() {
    for failure in ["identity", "output", "payload"] {
        let fixture = Fixture::new(0).await;
        let (owner, stored, _) = owner(&fixture);
        if failure == "output" {
            std::fs::write(&fixture.saved, b"changed").unwrap();
        }
        if failure == "payload" {
            stored.lost.store(true, Ordering::SeqCst);
        }
        let result = owner
            .preparation
            .prepare_redelivery(
                &EntryId::from("receiver"),
                if failure == "identity" {
                    "different-hash"
                } else {
                    "original-hash"
                },
                DeviceId::new("source"),
                fixture.work.action(),
            )
            .await;
        let Err(RetryEntryReceiveError::Unavailable(source)) = result else {
            panic!("original source must be required")
        };
        match failure {
            "output" => assert!(source.downcast_ref::<ReceiveReuseRefusal>().is_some()),
            "payload" => assert!(matches!(
                source.downcast_ref::<PayloadResolveError>(),
                Some(PayloadResolveError::Lost { .. })
            )),
            _ => {}
        }
        assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
        assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
        assert!(fixture
            .work
            .retry_material(&EntryId::from("receiver"), "old")
            .is_some());
    }
}

#[tokio::test]
async fn ordinary_redelivery_preserves_access_and_cleanup_refusals_before_new_attempt() {
    for denied in [false, true] {
        let fixture = Fixture::new(0).await;
        let (owner, _, barrier) = owner(&fixture);
        if !denied {
            barrier.0.store(true, Ordering::SeqCst);
        }
        let result = owner
            .preparation
            .prepare_redelivery(
                &EntryId::from("receiver"),
                "original-hash",
                DeviceId::new(if denied { "different-sender" } else { "source" }),
                fixture.work.action(),
            )
            .await;
        let Err(RetryEntryReceiveError::Unavailable(source)) = result else {
            panic!("authorization and cleanup must precede retry")
        };
        let refusal = source.downcast_ref::<ReceiveRedeliveryRefusal>().unwrap();
        assert_eq!(
            refusal.0,
            if denied {
                RetryEntryReceiveOutcome::AccessDenied
            } else {
                RetryEntryReceiveOutcome::CancellationIncomplete
            }
        );
        assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
        assert_eq!(std::fs::read(&fixture.saved).unwrap(), b"success");
    }
}

#[tokio::test]
async fn ordinary_inline_redelivery_upgrades_without_requiring_blob_retry_material() {
    let fixture = Fixture::new(0).await;
    let (owner, _, _) = owner(&fixture);
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .times(1)
        .returning(|_| Ok(Some(EntryId::from("inline-partial"))));
    let mut capture = MockCapture::new();
    capture
        .expect_capture()
        .times(1)
        .withf(|entry, _, _| entry.as_ref() == "inline-partial")
        .returning(|entry, _, _| Ok(Some(entry)));
    let apply = ApplyInboundClipboardUseCase::store_only_pull(StoreOnlyPullDeps {
        common: InboundApplyCommonDeps {
            receive_retry: Some(owner.preparation.clone()),
            receive_work: fixture.work.clone(),
            entry_repo: Arc::new(repo),
            capture: Arc::new(capture),
            blob_materializer: Arc::new(MockBlobMaterializer::new()),
            receive_attempts: InboundReceiveAttemptDeps {
                get: fixture.attempts.clone(),
                begin: fixture.attempts.clone(),
                claim_commit: fixture.attempts.clone(),
                request_cancel: fixture.attempts.clone(),
                begin_failure: fixture.attempts.clone(),
                commit: fixture.attempts.clone(),
                clock: fixture.attempts.clone(),
            },
            receive_artifact_cleanup: Arc::new(NoopReceiveArtifactCleanup),
            receive_readiness: fixture.readiness.clone(),
            host_event_emitter: Arc::new(HostEventBus::new()),
            search_live_index: Arc::new(RecordingLiveIndex {
                calls: AtomicUsize::new(0),
            }),
            availability: Arc::new(FakeAvailability { available: false }),
            entry_identity_coordinator: Arc::new(EntryIdentityCoordinator::new()),
        },
    });
    let (input, _) = fixture_input("repair inline payload");
    assert_eq!(
        apply.execute(input).await.unwrap(),
        ApplyOutcome::Applied {
            entry_id: EntryId::from("inline-partial")
        }
    );
    assert_eq!(
        fixture.attempts.retry_calls.load(Ordering::SeqCst),
        1,
        "纯inline修复仍应登记正常重投尝试"
    );
    assert!(
        fixture.begin.plans.lock().unwrap().is_empty(),
        "纯inline修复不能创建blob重试集合"
    );
}

/// 原完整重试在准入前失败时，完成记录保留同一source的固定存储分类。
#[tokio::test]
async fn retry_preflight_completion_preserves_each_storage_source_without_accepting_work() {
    for (kind, expected) in [
        (
            ErrorKind::PermissionDenied,
            ClipboardReceiveFailure::ApplyPermissionDenied,
        ),
        (
            ErrorKind::StorageFull,
            ClipboardReceiveFailure::ApplyStorageFull,
        ),
        (
            ErrorKind::ReadOnlyFilesystem,
            ClipboardReceiveFailure::ApplyReadOnly,
        ),
        (ErrorKind::Other, ClipboardReceiveFailure::ApplyIoFailed),
    ] {
        let fixture = Fixture::new(0).await;
        let (owner, stored, _) = owner(&fixture);
        *stored.query_failure.lock().unwrap() = Some(kind);
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        let recorded = outcomes.clone();
        let observation = ClipboardReceiveObservation::with_retry_completion(move |outcome| {
            recorded.lock().unwrap().push(outcome);
        });
        let error = observation
            .scope(owner.execute(EntryId::from("receiver"), "old".into(), item(1)))
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("PRIVATE_RETRY_PROGRESS_READ"));
        let RetryEntryReceiveError::Source(source) = error else {
            panic!("真实进度读取失败必须保留原Source分类")
        };
        assert!(source.chain().any(|source| source
            .downcast_ref::<IoError>()
            .is_some_and(|source| source.kind() == kind)));
        assert!(owner.requests.lock().unwrap().is_empty());
        assert_eq!(fixture.attempts.retry_calls.load(Ordering::SeqCst), 0);
        assert!(fixture.capture.snapshots.lock().unwrap().is_empty());
        assert!(outcomes.lock().unwrap().is_empty());
        drop(observation);
        assert_eq!(
            *outcomes.lock().unwrap(),
            vec![ClipboardRetryOutcome::Failed(expected)],
            "准入未交接的完成记录也必须保留原固定失败类别",
        );
    }
}
