use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_application::facade::{
    BeginReceiverTransfer, FileTransferFacade, FileTransferFacadeDeps, ReceiverTransferRegistration,
};
use uc_application::file_transfer::FileTransferApplicationError;
use uc_core::file_transfer::{FileTransferEventPublisherPort, FileTransferEventStorePort};
use uc_core::ports::file_transfer::{
    FileTransferProjectionError, PendingInboundTransfer, ProvisionalInboundTransfer,
    UpdateProvisionalReceivePathPort,
};
use uc_core::ports::{
    ClockPort, FinalizeProvisionalReceivePort, ProvisionalReceiveAction, ProvisionalReceiveError,
    RecordReceiverTransferPort, SeedProvisionalReceivePort,
};
use uc_core::{FileTransferCancellationReason, FileTransferEvent, FileTransferFailureReason};
use uc_infra::file_transfer::{InMemoryEventPublisher, InMemoryEventStore};

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

    assert_eq!(
        error,
        FileTransferApplicationError::ProgressWentBackwards {
            transfer_id: "transfer-1".into(),
            previous_bytes: 64,
            new_bytes: 32,
        }
    );
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
    assert_eq!(
        ctx.facade
            .begin_receiver_transfer(entry_transfer("transfer-3"))
            .await
            .unwrap_err(),
        FileTransferApplicationError::LifecycleClosed,
    );
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
    });

    let error = facade
        .begin_receiver_transfer(entry_transfer("transfer-1"))
        .await
        .unwrap_err();
    assert!(matches!(error, FileTransferApplicationError::Publish(_)));

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
