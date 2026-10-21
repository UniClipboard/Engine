use super::{
    file_blob_input, ApplyInboundClipboardUseCase, AttemptGate, FakeAvailability, HostEventBus,
    InboundApplyCommonDeps, InboundReceiveAttemptDeps, MockBlobMaterializer, MockEntryRepo,
    NoopReceiveArtifactCleanup, RecordingLiveIndex, StoreOnlyPullDeps,
};
use super::{test_materializer, MockBlobFetcher};
use crate::clipboard::capture::InboundCaptureCommitContext;
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::clipboard::sync::apply_inbound::InboundCapture;
use crate::clipboard::sync::payload_codec::V3BlobRef;
use crate::facade::blob_transfer::FetchBlobToPathResult;
use crate::transfer::receive::materialization::MaterializeResult;
use crate::transfer::receive::materialization::{
    InboundBlobMaterializer, MaterializeOutcome, ReceiveWorkPlan,
};
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use crate::transfer::receive::work::{ReceiveWorkRegistry, StoppedReceive};
use anyhow::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;
use uc_core::ids::{DeviceId, EntryId, FormatId, RepresentationId};
use uc_core::ports::blob::{BlobDigest, BlobTicket, PlaintextHash};
use uc_core::ports::inbound_file_target::ReserveInboundFileTargetPort;
use uc_core::ports::{
    AttemptError, AttemptState, ClaimReceiveCommitPort, PartialReceiveTerminal,
    RequestReceiveCancellationPort,
};
use uc_core::SnapshotHash;
use uc_core::{MimeType, ObservedClipboardRepresentation, SystemClipboardSnapshot};

struct SlowSecondReservation {
    directory: PathBuf,
    calls: AtomicUsize,
    entered: Notify,
    release: Semaphore,
}

#[async_trait]
impl ReserveInboundFileTargetPort for SlowSecondReservation {
    async fn reserve_target(&self, filename: &str) -> Option<PathBuf> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 1 {
            self.entered.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        let path = self.directory.join(filename);
        tokio::fs::write(&path, []).await.unwrap();
        Some(path)
    }
}

#[tokio::test]
async fn cancellation_during_target_reservation_retains_success_and_never_starts_later_files() {
    let directory = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let reserver = Arc::new(SlowSecondReservation {
        directory: directory.path().to_owned(),
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
        release: Semaphore::new(0),
    });
    let mut fetcher = MockBlobFetcher::new();
    fetcher
        .expect_fetch_blob_to_path()
        .times(1)
        .returning(|command| {
            assert!(!command.cancellation.is_cancelled());
            std::fs::write(&command.target_path, b"complete first file").unwrap();
            Ok(FetchBlobToPathResult {
                entry_id: command.entry_id,
                plaintext_hash: PlaintextHash::from_bytes([1; 32]),
                digest: BlobDigest::from_bytes([2; 32]),
                bytes_written: 19,
            })
        });
    let materializer = test_materializer(Arc::new(fetcher), cache.path().to_owned())
        .with_target_reserver(reserver.clone());
    let refs = ["first.bin", "second.bin", "third.bin"]
        .into_iter()
        .map(|name| V3BlobRef {
            ticket: BlobTicket::from_bytes(vec![1]),
            entry_id: EntryId::from("source"),
            filename: Some(name.to_owned()),
            mime: None,
            size_bytes: 19,
            representation_index: None,
        })
        .collect();
    let snapshot = SystemClipboardSnapshot {
        ts_ms: 1,
        representations: vec![ObservedClipboardRepresentation::new(RepresentationId::new(), FormatId::from("files"), Some(MimeType("text/uri-list".into())), b"file:///source/first.bin\r\nfile:///source/second.bin\r\nfile:///source/third.bin\r\n".to_vec())],
        file_content_digests: Vec::new(),
        file_set_v1_component: None,
    };
    let cancel = CancellationToken::new();
    let plan = ReceiveWorkPlan::new(
        DeviceId::new("source"),
        EntryId::from("receiver"),
        snapshot,
        refs,
        None,
        Some("attempt".into()),
        None,
    )
    .with_cancellation(cancel.clone());
    let running = tokio::spawn(async move { materializer.materialize_plan(plan).await });
    reserver.entered.notified().await;
    cancel.cancel();
    assert!(!running.is_finished());
    reserver.release.add_permits(1);
    let result = running.await.unwrap().unwrap();
    assert_eq!(result.outcome(), MaterializeOutcome::PartialCancelled);
    assert_eq!(result.missing.len(), 2);
    assert_eq!(result.receive_artifacts.len(), 1);
    assert_eq!(reserver.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::fs::read(directory.path().join("first.bin")).unwrap(),
        b"complete first file"
    );
    assert!(!directory.path().join("second.bin").exists());
    assert!(!directory.path().join("third.bin").exists());
}

struct CancellationWinsCommit {
    attempts: Arc<AttemptGate>,
    work: Arc<ReceiveWorkRegistry>,
    waiting: Mutex<Option<StoppedReceive>>,
}

#[async_trait]
impl ClaimReceiveCommitPort for CancellationWinsCommit {
    async fn claim_receive_commit(
        &self,
        entry: &str,
        attempt: &str,
        now: i64,
    ) -> std::result::Result<bool, AttemptError> {
        self.attempts
            .request_receive_cancellation(entry, attempt, now)
            .await?;
        *self.waiting.lock().unwrap() = self.work.stop(&EntryId::from(entry), attempt);
        Ok(false)
    }
}

struct HeldPartialCapture {
    attempts: Arc<AttemptGate>,
    entered: Notify,
    release: Semaphore,
    calls: AtomicUsize,
    failure: bool,
}

#[async_trait]
impl InboundCapture for HeldPartialCapture {
    async fn capture(
        &self,
        _: EntryId,
        _: DeviceId,
        _: SystemClipboardSnapshot,
    ) -> Result<Option<EntryId>> {
        panic!("attempt capture must use the atomic receive contract")
    }

    async fn capture_inbound_with_identity(
        &self,
        entry: EntryId,
        _: DeviceId,
        snapshot: SystemClipboardSnapshot,
        _: Option<SnapshotHash>,
        commit: InboundCaptureCommitContext,
    ) -> Result<Option<EntryId>> {
        assert!(matches!(
            commit,
            InboundCaptureCommitContext::Partial {
                terminal: PartialReceiveTerminal::Cancelled,
                ..
            }
        ));
        assert!(!snapshot.representations.is_empty());
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        if self.failure {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied).into());
        }
        *self.attempts.state.lock().unwrap() = AttemptState::Cancelled;
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(entry))
    }
}

#[tokio::test]
async fn commit_race_keeps_partial_settlement_owned_and_preserves_capture_failure() {
    for abandon_waiter in [true, false] {
        let work = Arc::new(ReceiveWorkRegistry::default());
        let attempts = AttemptGate::empty();
        let claim = Arc::new(CancellationWinsCommit {
            attempts: attempts.clone(),
            work: work.clone(),
            waiting: Mutex::new(None),
        });
        let capture = Arc::new(HeldPartialCapture {
            failure: !abandon_waiter,
            attempts: attempts.clone(),
            entered: Notify::new(),
            release: Semaphore::new(0),
            calls: AtomicUsize::new(0),
        });
        let mut repo = MockEntryRepo::new();
        repo.expect_find_entry_id_by_snapshot_hash()
            .returning(|_| Ok(None));
        let mut materializer = MockBlobMaterializer::new();
        materializer
            .expect_materialize_plan()
            .times(1)
            .returning(|plan| {
                let (_, _, snapshot, _, _, _, _, _) = plan.into_parts();
                Ok(MaterializeResult::complete(snapshot))
            });
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        readiness.mark_ready();
        let receiver = Arc::new(ApplyInboundClipboardUseCase::store_only_pull(
            StoreOnlyPullDeps {
                common: InboundApplyCommonDeps {
                    receive_work: work,
                    entry_repo: Arc::new(repo),
                    capture: capture.clone(),
                    blob_materializer: Arc::new(materializer),
                    receive_attempts: InboundReceiveAttemptDeps {
                        get: attempts.clone(),
                        begin: attempts.clone(),
                        claim_commit: claim.clone(),
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
        ));
        let owner = receiver.clone();
        let calling = tokio::spawn(async move { owner.execute(file_blob_input()).await });
        capture.entered.notified().await;
        let stopped = claim
            .waiting
            .lock()
            .unwrap()
            .take()
            .expect("registered before attempt visibility");
        let mut stopped = Box::pin(stopped.wait());
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), stopped.as_mut())
                .await
                .is_err()
        );
        if abandon_waiter {
            calling.abort();
            assert!(calling.await.unwrap_err().is_cancelled());
            capture.release.add_permits(1);
            stopped.await.unwrap();
            assert_eq!(capture.calls.load(Ordering::SeqCst), 1);
        } else {
            capture.release.add_permits(1);
            let caller_error = calling.await.unwrap().unwrap_err();
            let stopped_error = stopped.await.unwrap_err();
            for error in [
                &caller_error as &dyn std::error::Error,
                stopped_error.as_ref(),
            ] {
                assert!(
                    std::iter::successors(Some(error), |source| source.source()).any(|source| {
                        source
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|source| {
                                source.kind() == std::io::ErrorKind::PermissionDenied
                            })
                    })
                );
            }
        }
        receiver.shutdown().await.unwrap();
        assert_eq!(*attempts.state.lock().unwrap(), AttemptState::Cancelled);
    }
}
