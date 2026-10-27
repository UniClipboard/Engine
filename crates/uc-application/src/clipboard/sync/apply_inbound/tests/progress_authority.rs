//! 真实接收用例的CAS先后证明；存储port采用受控fixture，不冒充SQLite重启。
use super::{
    file_blob_input, ApplyInboundClipboardUseCase, AttemptGate, FakeAvailability, HostEventBus,
    InboundApplyCommonDeps, InboundReceiveAttemptDeps, MockBlobMaterializer, MockCapture,
    MockEntryRepo, NoopReceiveArtifactCleanup, RecordingLiveIndex, StoreOnlyPullDeps,
};
use crate::clipboard::entry_identity::EntryIdentityCoordinator;
use crate::transfer::progress::ProgressTaskAuthorityPort;
use crate::transfer::receive::reconciliation::ReceiveReadinessCoordinator;
use crate::transfer::receive::work::ReceiveWorkRegistry;
use async_trait::async_trait;
use std::io;
use std::sync::{atomic::AtomicUsize, Arc};
use tokio::sync::{Notify, Semaphore};
use uc_core::file_transfer::ProgressIdentity;
use uc_core::ids::EntryId;
use uc_core::ports::{AttemptError, BeginReceiveAttemptPort, BeginReceiveOutcome};
use uuid::Uuid;

struct HeldAcceptance {
    ledger: Arc<AttemptGate>,
    reject: bool,
    entered: Notify,
    release: Semaphore,
}
#[async_trait]
impl BeginReceiveAttemptPort for HeldAcceptance {
    async fn begin_first_receive(
        &self,
        entry: &str,
        attempt: &str,
        now: i64,
    ) -> Result<BeginReceiveOutcome, AttemptError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        if self.reject {
            return Ok(BeginReceiveOutcome::Superseded);
        }
        self.ledger.begin_first_receive(entry, attempt, now).await
    }
    async fn begin_redelivery(
        &self,
        entry: &str,
        expected: &str,
        attempt: &str,
        now: i64,
    ) -> Result<BeginReceiveOutcome, AttemptError> {
        self.entered.notify_one();
        self.release.acquire().await.unwrap().forget();
        if self.reject {
            return Ok(BeginReceiveOutcome::Superseded);
        }
        self.ledger
            .begin_redelivery(entry, expected, attempt, now)
            .await
    }
}

#[tokio::test]
async fn real_receive_handoffs_only_after_cas_begun_and_before_materialization() {
    for reject in [false, true] {
        let work = Arc::new(ReceiveWorkRegistry::default());
        let attempts = AttemptGate::empty();
        let acceptance = Arc::new(HeldAcceptance {
            ledger: attempts.clone(),
            reject,
            entered: Notify::new(),
            release: Semaphore::new(0),
        });
        let mut input = file_blob_input();
        let source = EntryId::from("00000000-0000-4000-8000-000000000001");
        let id = ProgressIdentity::new(*Uuid::parse_str(source.as_ref()).unwrap().as_bytes());
        input.source_entry = Some(source);
        let peer = input.from_device;
        let mut repo = MockEntryRepo::new();
        repo.expect_find_entry_id_by_snapshot_hash()
            .returning(|_| Ok(None));
        let capture = MockCapture::new();
        let mut materializer = MockBlobMaterializer::new();
        if !reject {
            let registry = work.clone();
            let ledger = attempts.clone();
            materializer
                .expect_materialize_plan()
                .times(1)
                .returning(move |_| {
                    let accepted = registry
                        .current_task(&peer, id)
                        .expect("authoritative before download");
                    let current = ledger.attempt_id.lock().unwrap();
                    assert_eq!(
                        accepted.descriptor.attempt.bytes(),
                        *Uuid::parse_str(&current).unwrap().as_bytes()
                    );
                    assert_eq!(accepted.descriptor.item_count, 1);
                    Err(io::Error::new(
                        io::ErrorKind::ConnectionAborted,
                        "fixture ends after accepted handoff",
                    )
                    .into())
                });
        }
        let readiness = Arc::new(ReceiveReadinessCoordinator::new());
        readiness.mark_ready();
        let receiver = Arc::new(ApplyInboundClipboardUseCase::store_only_pull(
            StoreOnlyPullDeps {
                common: InboundApplyCommonDeps {
                    receive_retry: None,
                    receive_work: work.clone(),
                    entry_repo: Arc::new(repo),
                    capture: Arc::new(capture),
                    blob_materializer: Arc::new(materializer),
                    receive_attempts: InboundReceiveAttemptDeps {
                        get: attempts.clone(),
                        begin: acceptance.clone(),
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
        ));
        let owner = receiver.clone();
        let applying = tokio::spawn(async move { owner.execute(input).await });
        acceptance.entered.notified().await;
        assert!(work.current_task(&peer, id).is_none());
        acceptance.release.add_permits(1);
        assert!(applying.await.unwrap().is_err());
        assert_eq!(work.current_task(&peer, id).is_none(), reject);
        receiver.shutdown().await.unwrap();
    }
}
