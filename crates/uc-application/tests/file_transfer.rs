use std::error::Error as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_application::facade::file_transfer::FileTransferApplicationError;
use uc_application::facade::HostEventBus;
use uc_application::facade::{
    BeginReceiverTransfer, FileTransferFacade, FileTransferFacadeDeps, FileTransferLifecycleDeps,
    ReceiverTransferRegistration,
};
use uc_core::file_transfer::{FileTransferEventPublisherPort, FileTransferEventStorePort};
use uc_core::ports::file_transfer::{
    FileTransferProjectionError, PendingInboundTransfer, ProvisionalInboundTransfer,
    UpdateProvisionalReceivePathPort,
};
use uc_core::ports::{
    AttemptError, BeginReceiveFailureOutcome, CleanupReceiveArtifactsPort, ClockPort,
    CommitInboundReceivePort, EntryReceiveAttempt, FileTransferPrivacyMaintenanceError,
    FinalizeProvisionalReceivePort, GetDirectoryPublishRecordPort, GetEntryAttemptPort,
    InboundReceiveSettlement, ListNonTerminalAttemptsPort, ListProvisionalReceivesPort,
    ListUnsettledReceiveArtifactsPort, ProvisionalReceiveAction, ProvisionalReceiveError,
    ProvisionalReceiveRecovery, ReceiveArtifact, ReceiveArtifactLogError, ReceiveArtifactRecord,
    RecordReceiverTransferPort, SeedProvisionalReceivePort,
};
use uc_core::{FileTransferCancellationReason, FileTransferEvent, FileTransferFailureReason};
#[derive(Default)]
struct InMemoryEventStore {
    events: std::sync::RwLock<std::collections::HashMap<String, Vec<FileTransferEvent>>>,
}

impl InMemoryEventStore {
    fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl FileTransferEventStorePort for InMemoryEventStore {
    async fn load(&self, transfer_id: &str) -> anyhow::Result<Vec<FileTransferEvent>> {
        Ok(self
            .events
            .read()
            .map_err(|_| anyhow::anyhow!("event store lock poisoned"))?
            .get(transfer_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn append(&self, event: FileTransferEvent) -> anyhow::Result<()> {
        let transfer_id = match &event {
            FileTransferEvent::Started { transfer_id, .. }
            | FileTransferEvent::Progress { transfer_id, .. }
            | FileTransferEvent::Completed { transfer_id, .. }
            | FileTransferEvent::Failed { transfer_id, .. }
            | FileTransferEvent::Cancelled { transfer_id, .. } => transfer_id.clone(),
        };
        self.events
            .write()
            .map_err(|_| anyhow::anyhow!("event store lock poisoned"))?
            .entry(transfer_id)
            .or_default()
            .push(event);
        Ok(())
    }
}

#[derive(Default)]
struct InMemoryEventPublisher {
    published: std::sync::RwLock<Vec<FileTransferEvent>>,
}

impl InMemoryEventPublisher {
    fn new() -> Self {
        Self::default()
    }

    fn published_events(&self) -> anyhow::Result<Vec<FileTransferEvent>> {
        Ok(self
            .published
            .read()
            .map_err(|_| anyhow::anyhow!("event publisher lock poisoned"))?
            .clone())
    }
}

#[async_trait]
impl FileTransferEventPublisherPort for InMemoryEventPublisher {
    async fn publish(&self, event: FileTransferEvent) -> anyhow::Result<()> {
        self.published
            .write()
            .map_err(|_| anyhow::anyhow!("event publisher lock poisoned"))?
            .push(event);
        Ok(())
    }
}

/// No-op lifecycle ports for session tests that never drive readiness.
#[derive(Default)]
struct NoopLifecyclePorts;

#[async_trait]
impl uc_core::ports::ListExpiredInflightTransfersPort for NoopLifecyclePorts {
    async fn list_expired_inflight(
        &self,
        _pending_cutoff: i64,
        _transferring_cutoff: i64,
    ) -> Result<
        Vec<uc_core::ports::file_transfer::ExpiredInflightTransfer>,
        FileTransferProjectionError,
    > {
        Ok(vec![])
    }
}

#[async_trait]
impl uc_core::ports::FailInflightTransfersPort for NoopLifecyclePorts {
    async fn mark_failed(
        &self,
        _transfer_id: &str,
        _reason: &str,
        _now_ms: i64,
    ) -> Result<(), FileTransferProjectionError> {
        Ok(())
    }
    async fn bulk_fail_inflight(
        &self,
        _reason: &str,
        _now_ms: i64,
    ) -> Result<
        Vec<uc_core::ports::file_transfer::ExpiredInflightTransfer>,
        FileTransferProjectionError,
    > {
        Ok(vec![])
    }
}

#[async_trait]
impl uc_core::ports::EnsureFileTransferPrivacyMaintenancePort for NoopLifecyclePorts {
    async fn ensure_file_transfer_privacy_maintenance(
        &self,
    ) -> Result<(), uc_core::ports::FileTransferPrivacyMaintenanceError> {
        Ok(())
    }
}

#[async_trait]
impl uc_core::ports::inbound_file_target::ResolveInboundSaveDirPort for NoopLifecyclePorts {
    async fn resolve_save_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
}

#[derive(Default)]
struct NoopReceiveAttemptPorts;

#[async_trait]
impl GetEntryAttemptPort for NoopReceiveAttemptPorts {
    async fn get_entry_attempt(
        &self,
        _entry_id: &str,
    ) -> Result<Option<EntryReceiveAttempt>, AttemptError> {
        Ok(None)
    }
}

#[async_trait]
impl ListNonTerminalAttemptsPort for NoopReceiveAttemptPorts {
    async fn list_non_terminal_attempts(&self) -> Result<Vec<EntryReceiveAttempt>, AttemptError> {
        Ok(vec![])
    }
}

#[async_trait]
impl ListUnsettledReceiveArtifactsPort for NoopReceiveAttemptPorts {
    async fn list_unsettled_receive_artifacts(
        &self,
    ) -> Result<Vec<ReceiveArtifactRecord>, ReceiveArtifactLogError> {
        Ok(vec![])
    }
}

#[async_trait]
impl GetDirectoryPublishRecordPort for NoopReceiveAttemptPorts {
    async fn get_publish_record(
        &self,
        _entry_id: &str,
        _attempt_id: &str,
    ) -> Result<Option<uc_core::ports::DirectoryPublishRecord>, uc_core::ports::PublishLogError>
    {
        Ok(None)
    }
}

#[async_trait]
impl uc_core::ports::BeginReceiveFailurePort for NoopReceiveAttemptPorts {
    async fn begin_receive_failure(
        &self,
        _entry_id: &str,
        _attempt_id: &str,
        _now_ms: i64,
    ) -> Result<BeginReceiveFailureOutcome, AttemptError> {
        Ok(BeginReceiveFailureOutcome::Begun)
    }
}

#[async_trait]
impl CleanupReceiveArtifactsPort for NoopReceiveAttemptPorts {
    async fn cleanup_receive_artifacts(
        &self,
        _artifacts: &[ReceiveArtifact],
    ) -> Result<(), ReceiveArtifactLogError> {
        Ok(())
    }
}

#[async_trait]
impl CommitInboundReceivePort for NoopReceiveAttemptPorts {
    async fn commit_inbound_receive(
        &self,
        _settlement: &InboundReceiveSettlement,
    ) -> Result<(), uc_core::ports::InboundReceiveCommitError> {
        Ok(())
    }
}

#[async_trait]
impl ListProvisionalReceivesPort for NoopReceiveAttemptPorts {
    async fn list_provisional_receives(
        &self,
    ) -> Result<Vec<ProvisionalReceiveRecovery>, ProvisionalReceiveError> {
        Ok(vec![])
    }
}

#[async_trait]
impl uc_core::ports::FinalizeProvisionalReceivePort for NoopReceiveAttemptPorts {
    async fn finalize_provisional_receive(
        &self,
        _transfer_id: &str,
        _action: ProvisionalReceiveAction,
        _now_ms: i64,
    ) -> Result<(), ProvisionalReceiveError> {
        Ok(())
    }
}

fn noop_lifecycle_deps() -> FileTransferLifecycleDeps {
    let noop = Arc::new(NoopLifecyclePorts);
    let attempts = Arc::new(NoopReceiveAttemptPorts);
    FileTransferLifecycleDeps {
        list_expired: Arc::clone(&noop) as _,
        fail_inflight: Arc::clone(&noop) as _,
        get_receive_attempt: Arc::clone(&attempts) as _,
        list_receive_attempts: Arc::clone(&attempts) as _,
        list_unsettled_artifacts: Arc::clone(&attempts) as _,
        get_directory_publish: Arc::clone(&attempts) as _,
        begin_receive_failure: Arc::clone(&attempts) as _,
        cleanup_artifacts: Arc::clone(&attempts) as _,
        commit_inbound: Arc::clone(&attempts) as _,
        list_provisional: Arc::clone(&attempts) as _,
        finalize_provisional: Arc::clone(&attempts) as _,
        privacy_maintenance: Arc::clone(&noop) as _,
        save_dir_resolver: Arc::clone(&noop) as _,
        file_cache_dir: std::path::PathBuf::new(),
        clock: Arc::new(FixedClock),
        host_event_bus: Arc::new(HostEventBus::new()),
        receive_readiness: Arc::new(uc_application::facade::ReceiveReadinessCoordinator::new()),
    }
}

#[derive(Default)]
struct ReceiverStore {
    pending: Mutex<Vec<PendingInboundTransfer>>,
    provisional: Mutex<Vec<ProvisionalInboundTransfer>>,
}

impl ReceiverStore {
    fn pending_transfer_ids(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|items| items.iter().map(|item| item.transfer_id.clone()).collect())
            .unwrap_or_default()
    }

    fn provisional_transfer_ids(&self) -> Vec<String> {
        self.provisional
            .lock()
            .map(|items| items.iter().map(|item| item.transfer_id.clone()).collect())
            .unwrap_or_default()
    }
}

#[async_trait]
impl RecordReceiverTransferPort for ReceiverStore {
    async fn upsert_pending_transfer(
        &self,
        transfer: &PendingInboundTransfer,
    ) -> Result<(), FileTransferProjectionError> {
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| FileTransferProjectionError::Backend("pending lock poisoned".into()))?;
        if let Some(existing) = pending
            .iter_mut()
            .find(|item| item.transfer_id == transfer.transfer_id)
        {
            *existing = transfer.clone();
        } else {
            pending.push(transfer.clone());
        }
        Ok(())
    }
}

#[async_trait]
impl SeedProvisionalReceivePort for ReceiverStore {
    async fn seed_provisional_receive(
        &self,
        transfer: &ProvisionalInboundTransfer,
    ) -> Result<(), ProvisionalReceiveError> {
        self.provisional
            .lock()
            .map_err(|_| ProvisionalReceiveError::Backend("provisional lock poisoned".into()))?
            .push(transfer.clone());
        Ok(())
    }
}

#[async_trait]
impl UpdateProvisionalReceivePathPort for ReceiverStore {
    async fn update_provisional_receive_path(
        &self,
        _provisional_transfer_id: &str,
        _cached_path: &str,
        _now_ms: i64,
    ) -> Result<(), ProvisionalReceiveError> {
        Ok(())
    }
}

#[async_trait]
impl FinalizeProvisionalReceivePort for ReceiverStore {
    async fn finalize_provisional_receive(
        &self,
        _provisional_transfer_id: &str,
        _action: ProvisionalReceiveAction,
        _now_ms: i64,
    ) -> Result<(), ProvisionalReceiveError> {
        Ok(())
    }
}

struct FixedClock;

impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        42
    }
}

struct FailFirstPublisher {
    calls: AtomicUsize,
}

struct FailingEventStore;

#[async_trait]
impl FileTransferEventStorePort for FailingEventStore {
    async fn load(&self, _transfer_id: &str) -> anyhow::Result<Vec<FileTransferEvent>> {
        Err(anyhow::anyhow!("event history unavailable"))
    }

    async fn append(&self, _event: FileTransferEvent) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("event append unavailable"))
    }
}

struct FailingPrivacyMaintenance;

#[async_trait]
impl uc_core::ports::EnsureFileTransferPrivacyMaintenancePort for FailingPrivacyMaintenance {
    async fn ensure_file_transfer_privacy_maintenance(
        &self,
    ) -> Result<(), FileTransferPrivacyMaintenanceError> {
        Err(FileTransferPrivacyMaintenanceError::Backend(
            "privacy database unavailable".to_owned(),
        ))
    }
}

#[async_trait]
impl FileTransferEventPublisherPort for FailFirstPublisher {
    async fn publish(&self, _event: FileTransferEvent) -> anyhow::Result<()> {
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Err(anyhow::anyhow!("publisher unavailable"))
        } else {
            Ok(())
        }
    }
}

struct TestContext {
    facade: Arc<FileTransferFacade>,
    store: Arc<InMemoryEventStore>,
    publisher: Arc<InMemoryEventPublisher>,
    receiver: Arc<ReceiverStore>,
}

fn build_context() -> TestContext {
    let store = Arc::new(InMemoryEventStore::new());
    let publisher = Arc::new(InMemoryEventPublisher::new());
    let receiver = Arc::new(ReceiverStore::default());
    let store_port: Arc<dyn FileTransferEventStorePort> = store.clone();
    let publisher_port: Arc<dyn FileTransferEventPublisherPort> = publisher.clone();
    let repo: Arc<dyn RecordReceiverTransferPort> = receiver.clone();
    let provisional_seed: Arc<dyn SeedProvisionalReceivePort> = receiver.clone();
    let provisional_path: Arc<dyn UpdateProvisionalReceivePathPort> = receiver.clone();
    let provisional_finalize: Arc<dyn FinalizeProvisionalReceivePort> = receiver.clone();

    TestContext {
        facade: Arc::new(FileTransferFacade::new(FileTransferFacadeDeps {
            store: store_port,
            publisher: publisher_port,
            repo,
            provisional_seed,
            provisional_path,
            provisional_finalize,
            clock: Arc::new(FixedClock),
            lifecycle: noop_lifecycle_deps(),
        })),
        store,
        publisher,
        receiver,
    }
}

fn entry_transfer(transfer_id: &str) -> BeginReceiverTransfer {
    BeginReceiverTransfer {
        transfer_id: transfer_id.into(),
        peer_id: "peer-1".into(),
        filename: "report.pdf".into(),
        file_size: Some(128),
        registration: ReceiverTransferRegistration::Entry {
            entry_id: "entry-1".into(),
            attempt_id: Some("attempt-1".into()),
            cached_path: String::new(),
        },
    }
}

async fn history(ctx: &TestContext, transfer_id: &str) -> Vec<FileTransferEvent> {
    ctx.store.load(transfer_id).await.unwrap()
}

#[tokio::test]
async fn beginning_receiver_transfer_records_context_and_started_event() {
    let ctx = build_context();

    let session = ctx
        .facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap();

    assert_eq!(session.transfer_id(), "transfer-1");
    assert_eq!(ctx.receiver.pending_transfer_ids(), vec!["transfer-1"]);
    assert_eq!(
        history(&ctx, "transfer-1").await,
        vec![FileTransferEvent::started(
            "transfer-1",
            "peer-1",
            "report.pdf",
            Some(128),
        )]
    );
    assert_eq!(ctx.publisher.published_events().unwrap().len(), 1);
}

#[tokio::test]
async fn beginning_provisional_receiver_transfer_records_context_and_started_event() {
    let ctx = build_context();
    let mut input = entry_transfer("transfer-1");
    input.registration = ReceiverTransferRegistration::Provisional;

    ctx.facade.begin_receiver_transfer(input).await.unwrap();

    assert_eq!(ctx.receiver.provisional_transfer_ids(), vec!["transfer-1"]);
    assert!(matches!(
        history(&ctx, "transfer-1").await.as_slice(),
        [FileTransferEvent::Started { .. }]
    ));
}

#[tokio::test]
async fn progress_is_monotonic_inside_one_session() {
    let ctx = build_context();
    let session = ctx
        .facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap();

    session.report_progress(64, Some(128)).await.unwrap();
    let error = session.report_progress(32, Some(128)).await.unwrap_err();

    assert!(matches!(
        error,
        FileTransferApplicationError::ProgressWentBackwards {
            ref transfer_id,
            previous_bytes: 64,
            new_bytes: 32,
        } if transfer_id == "transfer-1"
    ));
    assert_eq!(history(&ctx, "transfer-1").await.len(), 2);
}

#[tokio::test]
async fn concurrent_terminal_calls_append_only_one_terminal_event() {
    let ctx = build_context();
    let session = ctx
        .facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap();

    let (completed, failed) = tokio::join!(
        session.complete(),
        session.fail(FileTransferFailureReason::TimedOut, None),
    );

    assert_ne!(completed.is_ok(), failed.is_ok());
    let terminal_count = history(&ctx, "transfer-1")
        .await
        .iter()
        .filter(|event| {
            matches!(
                event,
                FileTransferEvent::Completed { .. }
                    | FileTransferEvent::Failed { .. }
                    | FileTransferEvent::Cancelled { .. }
            )
        })
        .count();
    assert_eq!(terminal_count, 1);
}

#[tokio::test]
async fn repeating_same_terminal_call_is_idempotent() {
    let ctx = build_context();
    let session = ctx
        .facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap();

    let first = session.complete().await.unwrap();
    let repeated = session.complete().await.unwrap();

    assert_eq!(first, repeated);
    assert_eq!(history(&ctx, "transfer-1").await.len(), 2);
    assert_eq!(ctx.publisher.published_events().unwrap().len(), 2);
}

#[tokio::test]
async fn active_batch_reuses_the_same_session() {
    let ctx = build_context();

    let first = ctx
        .facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap();
    let repeated = ctx
        .facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap();

    assert!(Arc::ptr_eq(&first, &repeated));
    assert_eq!(history(&ctx, "transfer-1").await.len(), 1);
}

#[tokio::test]
async fn closing_facade_cancels_every_active_session_and_rejects_new_sessions() {
    let ctx = build_context();
    ctx.facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap();
    ctx.facade
        .begin_receiver_transfer(entry_transfer("transfer-2"))
        .await
        .unwrap();

    ctx.facade.close().await.unwrap();

    for transfer_id in ["transfer-1", "transfer-2"] {
        assert!(matches!(
            history(&ctx, transfer_id).await.as_slice(),
            [
                FileTransferEvent::Started { .. },
                FileTransferEvent::Cancelled {
                    reason: FileTransferCancellationReason::Unknown,
                    ..
                }
            ]
        ));
    }
    assert!(matches!(
        ctx.facade
            .begin_receiver_transfer(entry_transfer("transfer-3"))
            .await
            .unwrap_err(),
        FileTransferApplicationError::LifecycleClosed
    ));
}

#[tokio::test]
async fn persisted_start_is_not_forgotten_when_publishing_fails() {
    let store = Arc::new(InMemoryEventStore::new());
    let receiver = Arc::new(ReceiverStore::default());
    let store_port: Arc<dyn FileTransferEventStorePort> = store.clone();
    let publisher_port: Arc<dyn FileTransferEventPublisherPort> = Arc::new(FailFirstPublisher {
        calls: AtomicUsize::new(0),
    });
    let repo: Arc<dyn RecordReceiverTransferPort> = receiver.clone();
    let provisional_seed: Arc<dyn SeedProvisionalReceivePort> = receiver.clone();
    let provisional_path: Arc<dyn UpdateProvisionalReceivePathPort> = receiver.clone();
    let provisional_finalize: Arc<dyn FinalizeProvisionalReceivePort> = receiver;
    let facade = FileTransferFacade::new(FileTransferFacadeDeps {
        store: store_port,
        publisher: publisher_port,
        repo,
        provisional_seed,
        provisional_path,
        provisional_finalize,
        clock: Arc::new(FixedClock),
        lifecycle: noop_lifecycle_deps(),
    });

    let error = facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap_err();
    assert!(matches!(error, FileTransferApplicationError::Publish(_)));
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("publisher unavailable")
    );

    let persisted_session = facade
        .active_session("transfer-1")
        .await
        .expect("persisted session must remain active");
    let retried_session = facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap();

    assert!(Arc::ptr_eq(&persisted_session, &retried_session));
    assert_eq!(store.load("transfer-1").await.unwrap().len(), 1);
}

#[tokio::test]
async fn event_store_failure_preserves_its_source() {
    let receiver = Arc::new(ReceiverStore::default());
    let repo: Arc<dyn RecordReceiverTransferPort> = receiver.clone();
    let provisional_seed: Arc<dyn SeedProvisionalReceivePort> = receiver.clone();
    let provisional_path: Arc<dyn UpdateProvisionalReceivePathPort> = receiver.clone();
    let provisional_finalize: Arc<dyn FinalizeProvisionalReceivePort> = receiver;
    let facade = FileTransferFacade::new(FileTransferFacadeDeps {
        store: Arc::new(FailingEventStore),
        publisher: Arc::new(InMemoryEventPublisher::new()),
        repo,
        provisional_seed,
        provisional_path,
        provisional_finalize,
        clock: Arc::new(FixedClock),
        lifecycle: noop_lifecycle_deps(),
    });

    let error = facade
        .begin_receiver_transfer(entry_transfer("transfer-store-error"))
        .await
        .unwrap_err();

    assert!(matches!(error, FileTransferApplicationError::Store(_)));
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("event history unavailable")
    );
}

#[tokio::test]
async fn readiness_recovery_failure_preserves_its_source() {
    let store = Arc::new(InMemoryEventStore::new());
    let receiver = Arc::new(ReceiverStore::default());
    let repo: Arc<dyn RecordReceiverTransferPort> = receiver.clone();
    let provisional_seed: Arc<dyn SeedProvisionalReceivePort> = receiver.clone();
    let provisional_path: Arc<dyn UpdateProvisionalReceivePathPort> = receiver.clone();
    let provisional_finalize: Arc<dyn FinalizeProvisionalReceivePort> = receiver;
    let mut lifecycle = noop_lifecycle_deps();
    lifecycle.privacy_maintenance = Arc::new(FailingPrivacyMaintenance);
    let facade = FileTransferFacade::new(FileTransferFacadeDeps {
        store,
        publisher: Arc::new(InMemoryEventPublisher::new()),
        repo,
        provisional_seed,
        provisional_path,
        provisional_finalize,
        clock: Arc::new(FixedClock),
        lifecycle,
    });

    let error = facade.ensure_receive_ready().await.unwrap_err();

    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("file transfer privacy maintenance failed: privacy database unavailable")
    );
}
