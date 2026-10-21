use super::{
    file_blob_input, ApplyInboundClipboardUseCase, ApplyInboundError, AttemptGate,
    FakeAvailability, HostEventBus, InboundApplyCommonDeps, InboundReceiveAttemptDeps,
    MockBlobMaterializer, MockCapture, NoopReceiveArtifactCleanup, RecordingLiveIndex,
    StoreOnlyPullDeps,
};
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::{Notify, Semaphore};
use uc_core::clipboard::ClipboardRepositoryError;
use uc_core::ids::EntryId;
use uc_core::ports::clipboard::FindEntryIdBySnapshotHashPort;
use uc_core::ports::{AttemptError, EntryReceiveAttempt, GetEntryAttemptPort};

struct HeldLookup {
    entered: Notify,
    release: Semaphore,
    at_attempt: bool,
    attempt_reads: AtomicUsize,
}

#[async_trait]
impl FindEntryIdBySnapshotHashPort for HeldLookup {
    async fn find_entry_id_by_snapshot_hash(
        &self,
        _: &str,
    ) -> Result<Option<EntryId>, ClipboardRepositoryError> {
        if !self.at_attempt {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        Ok(None)
    }
}

#[async_trait]
impl GetEntryAttemptPort for HeldLookup {
    async fn get_entry_attempt(
        &self,
        _: &str,
    ) -> Result<Option<EntryReceiveAttempt>, AttemptError> {
        self.attempt_reads.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        Ok(None)
    }
}

#[tokio::test]
async fn closing_and_reopening_gate_during_authority_reads_rejects_late_attempt_creation() {
    for at_attempt in [false, true] {
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        readiness.ensure_ready(|| async { Ok(()) }).await.unwrap();
        let attempts = AttemptGate::empty();
        let lookup = Arc::new(HeldLookup {
            entered: Notify::new(),
            release: Semaphore::new(0),
            at_attempt,
            attempt_reads: AtomicUsize::new(0),
        });
        let mut capture = MockCapture::new();
        capture.expect_capture().times(0);
        let mut materializer = MockBlobMaterializer::new();
        materializer.expect_materialize_plan().times(0);
        let receiver = Arc::new(ApplyInboundClipboardUseCase::store_only_pull(
            StoreOnlyPullDeps {
                common: InboundApplyCommonDeps {
                    receive_work: readiness.work_registry(),
                    entry_repo: lookup.clone(),
                    capture: Arc::new(capture),
                    blob_materializer: Arc::new(materializer),
                    receive_attempts: InboundReceiveAttemptDeps {
                        get: lookup.clone(),
                        begin: attempts.clone(),
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
            },
        ));
        let receiving = tokio::spawn(async move { receiver.execute(file_blob_input()).await });
        lookup.entered.notified().await;
        readiness.mark_not_ready();
        readiness.ensure_ready(|| async { Ok(()) }).await.unwrap();
        lookup.release.add_permits(1);
        assert!(matches!(
            receiving.await.unwrap(),
            Err(ApplyInboundError::Stopped)
        ));
        assert_eq!(
            lookup.attempt_reads.load(Ordering::SeqCst),
            usize::from(at_attempt)
        );
        assert_eq!(attempts.begin_calls.load(Ordering::SeqCst), 0);
        assert_eq!(attempts.retry_calls.load(Ordering::SeqCst), 0);
        assert_eq!(attempts.commit_calls.load(Ordering::SeqCst), 0);
    }
}
