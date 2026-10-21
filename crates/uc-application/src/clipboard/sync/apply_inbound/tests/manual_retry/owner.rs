use super::{AttemptGate, Fixture};
use crate::transfer::receive::materialization::ReceiveReuseRefusal;
use crate::transfer::receive::query::ReceiveProgressQuery;
use crate::transfer::receive::retry::{
    ReceiveCancellationBarrier, RetryEntryReceiveError, RetryEntryReceiveOutcome,
    RetryEntryReceiveUseCase, RetrySnapshotReader,
};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, Semaphore};
use uc_core::blob::ports::BlobReaderPort;
use uc_core::clipboard::{
    ClipboardEntry, ClipboardRepositoryError, PersistedClipboardRepresentation,
};
use uc_core::ids::{EntryId, EventId, RepresentationId};
use uc_core::ports::clipboard::{
    ClipboardPayloadResolverPort, GetClipboardEntryPort, GetRepresentationPort,
    PayloadResolveError, ResolvedClipboardPayload,
};
use uc_core::ports::{
    AttemptError, EntryReceiveAttempt, EntryReceiveProgress, FileTransferProjectionError,
    GetEntryReceiveProgressPort, ListNonTerminalAttemptsPort,
};
use uc_core::{BlobId, SystemClipboardSnapshot};

struct Stored {
    snapshot: SystemClipboardSnapshot,
    attempts: Arc<AttemptGate>,
    lost: AtomicBool,
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
        Ok(Some(EntryReceiveProgress {
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
        slow: AtomicBool::new(false),
        entered: Notify::new(),
        release: Semaphore::new(0),
    });
    let barrier = Arc::new(Barrier(AtomicBool::new(false)));
    (
        Arc::new(RetryEntryReceiveUseCase {
            cache_dir: fixture._cache.path().to_owned(),
            apply: fixture.apply.clone(),
            query: Arc::new(ReceiveProgressQuery::new(
                stored.clone(),
                stored.clone(),
                fixture.work.clone(),
            )),
            work: fixture.work.clone(),
            cancellation: barrier.clone(),
            scope: fixture.checks.clone(),
            requests: Arc::new(Mutex::new(HashSet::new())),
            snapshots: RetrySnapshotReader {
                entry: stored.clone(),
                representations: stored.clone(),
                payload: stored.clone(),
                blobs: stored.clone(),
                identity: fixture.checks.clone(),
            },
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
    let (owner, _, _) = owner(&fixture);
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
    let current = owner
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
    assert_eq!(fixture.capture.snapshots.lock().unwrap().len(), 1);
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
