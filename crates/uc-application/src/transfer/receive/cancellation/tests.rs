use crate::transfer::receive::work::{ReceiveAction, ReceiveWorkError, ReceiveWorkRegistry};
use std::sync::{Arc, Mutex};

use anyhow::Error as SourceError;

use uc_core::file_transfer::FileTransferCancellationReason;
use uc_core::ids::EntryId;
use uc_core::ports::{
    CancelDirectoryAttemptTransfersPort, CleanupDirectoryStagingPort, ClockPort,
    CommitInboundReceivePort, DirectoryStagingCleanupError, GetDirectoryPublishRecordPort,
    GetEntryAttemptPort, InboundReceiveSettlement, PublishLogError,
    RequestReceiveCancellationOutcome, RequestReceiveCancellationPort,
};

use crate::facade::HostEventBus;
use crate::transfer::blob::facade::{BlobTransferError, TransferCancellationWait};
use crate::transfer::blob::transport::BlobTransport;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use uc_core::ports::{
    AttemptError, AttemptState, DirectoryPublishRecord, EntryReceiveAttempt,
    InboundReceiveCommitError, PublishPhase,
};

use super::*;

struct AttemptMock {
    state: AttemptState,
    cancel_outcome: RequestReceiveCancellationOutcome,
    cancel_calls: AtomicUsize,
}

#[async_trait]
impl GetEntryAttemptPort for AttemptMock {
    async fn get_entry_attempt(
        &self,
        entry_id: &str,
    ) -> Result<Option<EntryReceiveAttempt>, AttemptError> {
        Ok(Some(EntryReceiveAttempt {
            entry_id: entry_id.to_owned(),
            current_attempt_id: "attempt-1".into(),
            state: self.state,
            updated_at_ms: 1,
        }))
    }
}

#[async_trait]
impl RequestReceiveCancellationPort for AttemptMock {
    async fn request_receive_cancellation(
        &self,
        _entry_id: &str,
        _attempt_id: &str,
        _now_ms: i64,
    ) -> Result<RequestReceiveCancellationOutcome, AttemptError> {
        self.cancel_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.cancel_outcome)
    }
}

#[derive(Default)]
struct CommitMock {
    calls: AtomicUsize,
}

#[async_trait]
impl CommitInboundReceivePort for CommitMock {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> Result<(), InboundReceiveCommitError> {
        assert!(matches!(
            settlement,
            InboundReceiveSettlement::NoEntry { .. }
        ));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct PublishMock;

#[async_trait]
impl GetDirectoryPublishRecordPort for PublishMock {
    async fn get_publish_record(
        &self,
        entry_id: &str,
        attempt_id: &str,
    ) -> Result<Option<DirectoryPublishRecord>, PublishLogError> {
        Ok(Some(DirectoryPublishRecord {
            entry_id: entry_id.to_owned(),
            attempt_id: attempt_id.to_owned(),
            phase: PublishPhase::Staging,
            root_map: vec![(
                PathBuf::from("/tmp/.uniclip-incoming-entry/root"),
                PathBuf::from("/tmp/final/root"),
            )],
            partial_visible_roots: None,
            landed: false,
        }))
    }
}

struct NoPublishMock;

#[async_trait]
impl GetDirectoryPublishRecordPort for NoPublishMock {
    async fn get_publish_record(
        &self,
        _entry_id: &str,
        _attempt_id: &str,
    ) -> Result<Option<DirectoryPublishRecord>, PublishLogError> {
        Ok(None)
    }
}

#[derive(Default)]
struct CleanerMock {
    calls: AtomicUsize,
}

#[async_trait]
impl CleanupDirectoryStagingPort for CleanerMock {
    async fn cleanup_staging_roots(
        &self,
        staged_roots: &[PathBuf],
    ) -> Result<(), DirectoryStagingCleanupError> {
        assert_eq!(
            staged_roots,
            &[PathBuf::from("/tmp/.uniclip-incoming-entry/root")]
        );
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(Default)]
struct TransferMock {
    calls: AtomicUsize,
}

#[async_trait]
impl AttemptTransferCancellation for TransferMock {
    async fn cancel_inbound_attempt(
        &self,
        attempt_id: &str,
        reason: FileTransferCancellationReason,
        _wait: TransferCancellationWait,
    ) -> Result<usize, BlobTransferError> {
        assert_eq!(attempt_id, "attempt-1");
        assert_eq!(reason, FileTransferCancellationReason::LocalUser);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(2)
    }
}

#[derive(Default)]
struct ProjectionMock {
    calls: AtomicUsize,
}

#[async_trait]
impl CancelDirectoryAttemptTransfersPort for ProjectionMock {
    async fn cancel_attempt_transfers(
        &self,
        _entry_id: &str,
        attempt_id: &str,
        reason: FileTransferCancellationReason,
        _now_ms: i64,
    ) -> Result<u32, uc_core::ports::FileTransferProjectionError> {
        assert_eq!(attempt_id, "attempt-1");
        assert_eq!(reason, FileTransferCancellationReason::LocalUser);
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(3)
    }
}

struct ClockMock;

impl ClockPort for ClockMock {
    fn now_ms(&self) -> i64 {
        42
    }
}

fn use_case(
    attempts: Arc<AttemptMock>,
    cleaner: Arc<CleanerMock>,
    transfers: Arc<TransferMock>,
    projection: Arc<ProjectionMock>,
) -> Arc<CancelEntryReceiveUseCase> {
    CancelEntryReceiveUseCase::new(
        attempts.clone(),
        attempts,
        Arc::new(CommitMock::default()),
        Arc::new(PublishMock),
        cleaner,
        projection,
        transfers,
        Arc::new(ClockMock),
        Arc::new(HostEventBus::new()),
        BlobTransport::new(
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
        ),
        Arc::new(ReceiveWorkRegistry::default()),
        Arc::default(),
    )
}

#[tokio::test]
async fn active_attempt_is_cancelled_before_transfers_and_staging_cleanup() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let cleaner = Arc::new(CleanerMock::default());
    let transfers = Arc::new(TransferMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let result = use_case(
        attempts.clone(),
        cleaner.clone(),
        transfers.clone(),
        projection.clone(),
    )
    .execute(&EntryId::new(), "attempt-1")
    .await;

    assert!(matches!(result, Ok(CancelEntryReceiveOutcome::Cancelled)));
    assert_eq!(attempts.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.calls.load(Ordering::SeqCst), 1);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancelled_attempt_retries_transfer_and_staging_compensation() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Cancelling,
        cancel_outcome: RequestReceiveCancellationOutcome::AlreadyCancelling,
        cancel_calls: AtomicUsize::new(0),
    });
    let cleaner = Arc::new(CleanerMock::default());
    let transfers = Arc::new(TransferMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let result = use_case(
        attempts.clone(),
        cleaner.clone(),
        transfers.clone(),
        projection.clone(),
    )
    .execute(&EntryId::new(), "attempt-1")
    .await;

    assert!(matches!(result, Ok(CancelEntryReceiveOutcome::Cancelled)));
    assert_eq!(attempts.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.calls.load(Ordering::SeqCst), 1);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn active_file_receive_is_terminally_cancelled() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let commits = Arc::new(CommitMock::default());
    let cleaner = Arc::new(CleanerMock::default());
    let transfers = Arc::new(TransferMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let use_case = CancelEntryReceiveUseCase::new(
        attempts.clone(),
        attempts,
        commits.clone(),
        Arc::new(NoPublishMock),
        cleaner.clone(),
        projection.clone(),
        transfers.clone(),
        Arc::new(ClockMock),
        Arc::new(HostEventBus::new()),
        BlobTransport::new(
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
        ),
        Arc::new(ReceiveWorkRegistry::default()),
        Arc::default(),
    );

    let result = use_case.execute(&EntryId::new(), "attempt-1").await;

    assert!(matches!(result, Ok(CancelEntryReceiveOutcome::Cancelled)));
    assert_eq!(commits.calls.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.calls.load(Ordering::SeqCst), 1);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn promotion_winner_blocks_cancellation_side_effects() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::TooLate,
        cancel_calls: AtomicUsize::new(0),
    });
    let cleaner = Arc::new(CleanerMock::default());
    let transfers = Arc::new(TransferMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let result = use_case(
        attempts.clone(),
        cleaner.clone(),
        transfers.clone(),
        projection.clone(),
    )
    .execute(&EntryId::new(), "attempt-1")
    .await;

    assert!(matches!(result, Ok(CancelEntryReceiveOutcome::TooLate)));
    assert_eq!(attempts.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(transfers.calls.load(Ordering::SeqCst), 0);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 0);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stale_cancellation_does_not_touch_newer_attempt() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let cleaner = Arc::new(CleanerMock::default());
    let transfers = Arc::new(TransferMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let result = use_case(
        attempts,
        cleaner.clone(),
        transfers.clone(),
        projection.clone(),
    )
    .execute(&EntryId::new(), "stale-attempt")
    .await;

    assert!(matches!(result, Ok(CancelEntryReceiveOutcome::Superseded)));
    assert_eq!(transfers.calls.load(Ordering::SeqCst), 0);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 0);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 0);
}

struct FailingTransfers;

#[async_trait]
impl AttemptTransferCancellation for FailingTransfers {
    async fn cancel_inbound_attempt(
        &self,
        _attempt_id: &str,
        _reason: FileTransferCancellationReason,
        _wait: TransferCancellationWait,
    ) -> Result<usize, BlobTransferError> {
        Err(BlobTransferError::Fetch(SourceError::from(
            std::io::Error::other("transport gone"),
        )))
    }
}

struct Gate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
}
impl Gate {
    async fn wait(&self) {
        self.entered.add_permits(1);
        self.release.acquire().await.expect("gate release").forget();
    }
}
struct GatedTransfers {
    gate: Arc<Gate>,
    calls: AtomicUsize,
}
#[async_trait]
impl AttemptTransferCancellation for GatedTransfers {
    async fn cancel_inbound_attempt(
        &self,
        _: &str,
        _: FileTransferCancellationReason,
        wait: TransferCancellationWait,
    ) -> Result<usize, BlobTransferError> {
        assert!(matches!(wait, TransferCancellationWait::Complete));
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.gate.wait().await;
        Ok(2)
    }
}
#[tokio::test(start_paused = true)]
async fn dropped_and_repeated_waiters_do_not_abandon_cleanup_or_restart_the_cancel_workflow() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let gate = Arc::new(Gate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let transfers = Arc::new(GatedTransfers {
        gate: gate.clone(),
        calls: AtomicUsize::new(0),
    });
    let commit = Arc::new(CommitMock::default());
    let cleaner = Arc::new(CleanerMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let transport = BlobTransport::new(
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
    );
    let usecase = CancelEntryReceiveUseCase::new(
        attempts.clone(),
        attempts.clone(),
        commit.clone(),
        Arc::new(PublishMock),
        cleaner.clone(),
        projection.clone(),
        transfers.clone(),
        Arc::new(ClockMock),
        Arc::new(HostEventBus::new()),
        transport.clone(),
        Arc::new(ReceiveWorkRegistry::default()),
        Arc::default(),
    );
    let entry = EntryId::new();
    drop(usecase.execute(&entry, "attempt-1"));
    gate.entered
        .acquire()
        .await
        .expect("actual stop entered")
        .forget();
    for _ in 0..2 {
        assert_eq!(
            usecase.execute(&entry, "attempt-1").await.expect("pending"),
            CancelEntryReceiveOutcome::CancellationIncomplete
        );
    }
    assert_eq!(transfers.calls.load(Ordering::SeqCst), 1);
    assert_eq!(attempts.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(commit.calls.load(Ordering::SeqCst), 0);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 0);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 0);
    gate.release.add_permits(1);
    // 不再发取消命令，生命周期关闭仍须收集完整清理与最终 CAS。
    transport
        .shutdown(crate::transfer::blob::transport::BlobShutdownWait::Complete)
        .await
        .expect("full shutdown");
    assert_eq!(commit.calls.load(Ordering::SeqCst), 1);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        usecase
            .execute(&entry, "attempt-1")
            .await
            .expect("same result"),
        CancelEntryReceiveOutcome::Cancelled
    );
}

struct SlowRead {
    inner: Arc<AttemptMock>,
    gate: Arc<Gate>,
}
#[async_trait]
impl GetEntryAttemptPort for SlowRead {
    async fn get_entry_attempt(
        &self,
        entry: &str,
    ) -> Result<Option<EntryReceiveAttempt>, AttemptError> {
        self.gate.wait().await;
        self.inner.get_entry_attempt(entry).await
    }
}
struct SlowCommit {
    inner: Arc<CommitMock>,
    gate: Arc<Gate>,
}
#[async_trait]
impl CommitInboundReceivePort for SlowCommit {
    async fn commit_inbound_receive(
        &self,
        settlement: &InboundReceiveSettlement,
    ) -> Result<(), InboundReceiveCommitError> {
        self.gate.wait().await;
        self.inner.commit_inbound_receive(settlement).await
    }
}
#[tokio::test(start_paused = true)]
async fn caller_deadline_covers_initial_authority_read_and_final_commit_without_abandoning_either()
{
    for slow_read in [true, false] {
        let attempts = Arc::new(AttemptMock {
            state: AttemptState::Receiving,
            cancel_outcome: RequestReceiveCancellationOutcome::Requested,
            cancel_calls: AtomicUsize::new(0),
        });
        let gate = Arc::new(Gate {
            entered: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
        });
        let commit = Arc::new(CommitMock::default());
        let cleaner = Arc::new(CleanerMock::default());
        let projection = Arc::new(ProjectionMock::default());
        let transfer = Arc::new(TransferMock::default());
        let read: Arc<dyn GetEntryAttemptPort> = if slow_read {
            Arc::new(SlowRead {
                inner: attempts.clone(),
                gate: gate.clone(),
            })
        } else {
            attempts.clone()
        };
        let writer: Arc<dyn CommitInboundReceivePort> = if slow_read {
            commit.clone()
        } else {
            Arc::new(SlowCommit {
                inner: commit.clone(),
                gate: gate.clone(),
            })
        };
        let transport = BlobTransport::new(
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
        );
        let usecase = CancelEntryReceiveUseCase::new(
            read,
            attempts.clone(),
            writer,
            Arc::new(PublishMock),
            cleaner.clone(),
            projection.clone(),
            transfer.clone(),
            Arc::new(ClockMock),
            Arc::new(HostEventBus::new()),
            transport.clone(),
            Arc::new(ReceiveWorkRegistry::default()),
            Arc::default(),
        );
        let entry = EntryId::new();
        let start = tokio::time::Instant::now();
        assert_eq!(
            usecase
                .execute(&entry, "attempt-1")
                .await
                .expect("incomplete"),
            CancelEntryReceiveOutcome::CancellationIncomplete
        );
        assert_eq!(start.elapsed(), std::time::Duration::from_secs(5));
        gate.entered
            .acquire()
            .await
            .expect("actual phase entered")
            .forget();
        assert_eq!(commit.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            attempts.cancel_calls.load(Ordering::SeqCst),
            usize::from(!slow_read)
        );
        let closing =
            transport.shutdown(crate::transfer::blob::transport::BlobShutdownWait::Complete);
        tokio::pin!(closing);
        assert!(
            futures::poll!(closing.as_mut()).is_pending(),
            "shutdown must retain the real slow phase"
        );
        gate.release.add_permits(if slow_read { 2 } else { 1 });
        closing.await.expect("real close");
        assert_eq!(commit.calls.load(Ordering::SeqCst), 1);
        assert_eq!(cleaner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(projection.calls.load(Ordering::SeqCst), 1);
        assert_eq!(transfer.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            usecase
                .execute(&entry, "attempt-1")
                .await
                .expect("terminal"),
            CancelEntryReceiveOutcome::Cancelled
        );
    }
}

#[tokio::test(start_paused = true)]
async fn cancelled_entry_requests_have_a_bounded_registry_without_hidden_read_waiters() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let gate = Arc::new(Gate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    let commit = Arc::new(CommitMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let transfer = Arc::new(TransferMock::default());
    let transport = BlobTransport::new(
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
    );
    let usecase = CancelEntryReceiveUseCase::new(
        Arc::new(SlowRead {
            inner: attempts.clone(),
            gate: gate.clone(),
        }),
        attempts.clone(),
        commit.clone(),
        Arc::new(NoPublishMock),
        Arc::new(CleanerMock::default()),
        projection,
        transfer,
        Arc::new(ClockMock),
        Arc::new(HostEventBus::new()),
        transport.clone(),
        Arc::new(ReceiveWorkRegistry::default()),
        Arc::default(),
    );
    for _ in 0..68 {
        drop(usecase.execute(&EntryId::new(), "attempt-1"));
    }
    gate.entered
        .acquire_many(68)
        .await
        .expect("all accepted reads")
        .forget();
    let error = usecase
        .execute(&EntryId::new(), "attempt-1")
        .await
        .expect_err("overload must be immediate");
    assert!(matches!(error, CancelEntryReceiveError::Busy(_)));
    assert_eq!(gate.entered.available_permits(), 0);
    assert_eq!(attempts.cancel_calls.load(Ordering::SeqCst), 0);
    gate.release.add_permits(136);
    transport
        .shutdown(crate::transfer::blob::transport::BlobShutdownWait::Complete)
        .await
        .expect("all accepted work settled");
    assert_eq!(commit.calls.load(Ordering::SeqCst), 68);
    assert_eq!(attempts.cancel_calls.load(Ordering::SeqCst), 68);
}

struct PanickingTransfers;
#[async_trait]
impl AttemptTransferCancellation for PanickingTransfers {
    async fn cancel_inbound_attempt(
        &self,
        _: &str,
        _: FileTransferCancellationReason,
        _: TransferCancellationWait,
    ) -> Result<usize, BlobTransferError> {
        panic!("injected cancel worker failure")
    }
}
#[tokio::test]
async fn panicking_cancel_worker_keeps_the_join_source_and_never_runs_cleanup() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let commit = Arc::new(CommitMock::default());
    let cleaner = Arc::new(CleanerMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let transport = BlobTransport::new(
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
        Arc::new(NoNetwork),
    );
    let usecase = CancelEntryReceiveUseCase::new(
        attempts.clone(),
        attempts,
        commit.clone(),
        Arc::new(PublishMock),
        cleaner.clone(),
        projection.clone(),
        Arc::new(PanickingTransfers),
        Arc::new(ClockMock),
        Arc::new(HostEventBus::new()),
        transport.clone(),
        Arc::new(ReceiveWorkRegistry::default()),
        Arc::default(),
    );
    let error = usecase
        .execute(&EntryId::new(), "attempt-1")
        .await
        .expect_err("panic must fail");
    let CancelEntryReceiveError::Supervision(error) = error else {
        panic!("typed supervision failure required")
    };
    assert!(error.source.chain().any(|source| source
        .downcast_ref::<tokio::task::JoinError>()
        .is_some_and(|source| source.is_panic())));
    assert_eq!(commit.calls.load(Ordering::SeqCst), 0);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 0);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 0);
    let error = transport
        .shutdown(crate::transfer::blob::transport::BlobShutdownWait::Complete)
        .await
        .expect_err("close keeps actual task failure");
    assert_eq!(
        error.kind,
        crate::transfer::blob::transport::BlobExchangeFailure::Task
    );
}

struct FailingProjection;

#[async_trait]
impl CancelDirectoryAttemptTransfersPort for FailingProjection {
    async fn cancel_attempt_transfers(
        &self,
        _entry_id: &str,
        _attempt_id: &str,
        _reason: FileTransferCancellationReason,
        _now_ms: i64,
    ) -> Result<u32, uc_core::ports::FileTransferProjectionError> {
        Err(uc_core::ports::FileTransferProjectionError::Backend(
            "projection gone".into(),
        ))
    }
}

#[tokio::test]
async fn failed_transfer_stop_keeps_its_source_and_prevents_following_side_effects() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let use_case = CancelEntryReceiveUseCase::new(
        attempts.clone(),
        attempts,
        Arc::new(CommitMock::default()),
        Arc::new(NoPublishMock),
        Arc::new(CleanerMock::default()),
        Arc::new(FailingProjection),
        Arc::new(FailingTransfers),
        Arc::new(ClockMock),
        Arc::new(HostEventBus::new()),
        BlobTransport::new(
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
            Arc::new(NoNetwork),
        ),
        Arc::new(ReceiveWorkRegistry::default()),
        Arc::default(),
    );

    let error = use_case
        .execute(&EntryId::new(), "attempt-1")
        .await
        .expect_err("both transfer cancellations fail");

    let CancelEntryReceiveError::Shared(shared) = &error else {
        panic!("shared result required")
    };
    let CancelEntryReceiveError::Transfer(source) = shared.as_ref() else {
        panic!("expected transfer failure, got {error:?}");
    };
    assert!(source
        .chain()
        .any(|cause| cause.downcast_ref::<std::io::Error>().is_some()));
    assert!(!error.to_string().contains("transport gone"));
    assert!(!error.to_string().contains("projection gone"));
}
struct NoNetwork;
#[async_trait]
impl crate::connectivity::ConnectivityBackend for NoNetwork {
    async fn connect(
        &self,
        _: uc_core::ids::DeviceId,
        _: Option<Vec<u8>>,
        _: tokio_util::sync::CancellationToken,
    ) -> Result<Arc<dyn crate::connectivity::PeerConnection>, crate::connectivity::ConnectivityError>
    {
        panic!("cancellation cannot dial")
    }
}
#[async_trait]
impl crate::connectivity::SessionConnectionAdmissionPort for NoNetwork {
    fn begin(&self, _: uc_core::ids::DeviceId) -> Box<dyn crate::connectivity::AdmissionAttempt> {
        panic!("cancellation cannot acquire a connection")
    }
    async fn adopt_inbound(
        &self,
        _: Arc<dyn crate::connectivity::PeerConnection>,
        _: &dyn crate::connectivity::InboundIdentityReservation,
    ) -> Result<Box<dyn crate::connectivity::AdmissionGrant>, crate::connectivity::ConnectivityError>
    {
        panic!("cancellation cannot admit a connection")
    }
}
#[async_trait]
impl crate::transfer::blob::transport::BlobExchangePort for NoNetwork {
    async fn receive(
        &self,
        _: crate::connectivity::StreamLease,
        _: uc_core::ports::blob::BlobDigest,
        _: tokio_util::sync::CancellationToken,
        _: Arc<dyn crate::transfer::blob::transport::BlobValidatedProgress>,
    ) -> Result<
        crate::transfer::blob::transport::BlobReceived,
        crate::transfer::blob::transport::BlobExchangeError,
    > {
        panic!("cancellation cannot get")
    }
    async fn provide(
        &self,
        _: crate::connectivity::StreamLease,
        _: tokio::time::Instant,
        _: tokio_util::sync::CancellationToken,
    ) -> Result<(), crate::transfer::blob::transport::BlobExchangeError> {
        panic!("cancellation cannot provide")
    }
}
#[async_trait]
impl crate::transfer::blob::transport::BlobStoreLifecyclePort for NoNetwork {
    async fn shutdown(&self) -> Result<(), crate::transfer::blob::transport::BlobExchangeError> {
        Ok(())
    }
}

#[tokio::test(start_paused = true)]
async fn entry_cancellation_waits_for_the_plan_before_touching_projection_or_staging() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let cleaner = Arc::new(CleanerMock::default());
    let transfers = Arc::new(TransferMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let owner = use_case(
        attempts,
        cleaner.clone(),
        transfers.clone(),
        projection.clone(),
    );
    let entry = EntryId::new();
    let action = owner.receive_work.action();
    action.register(&entry, "attempt-1").unwrap();
    assert_eq!(
        owner.execute(&entry, "attempt-1").await.unwrap(),
        CancelEntryReceiveOutcome::CancellationIncomplete
    );
    assert!(action.cancellation().is_cancelled());
    assert_eq!(transfers.calls.load(Ordering::SeqCst), 1);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 0);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 0);
    action.finish(Ok(()));
    assert_eq!(
        owner.execute(&entry, "attempt-1").await.unwrap(),
        CancelEntryReceiveOutcome::Cancelled
    );
    assert_eq!(projection.calls.load(Ordering::SeqCst), 1);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn failed_receive_settlement_prevents_cancellation_cleanup_and_preserves_source() {
    let attempts = Arc::new(AttemptMock {
        state: AttemptState::Receiving,
        cancel_outcome: RequestReceiveCancellationOutcome::Requested,
        cancel_calls: AtomicUsize::new(0),
    });
    let cleaner = Arc::new(CleanerMock::default());
    let transfers = Arc::new(TransferMock::default());
    let projection = Arc::new(ProjectionMock::default());
    let owner = use_case(attempts, cleaner.clone(), transfers, projection.clone());
    let entry = EntryId::new();
    let action = owner.receive_work.action();
    action.register(&entry, "attempt-1").unwrap();
    assert_eq!(
        owner.execute(&entry, "attempt-1").await.unwrap(),
        CancelEntryReceiveOutcome::CancellationIncomplete
    );
    action.finish(Err(ReceiveWorkError::Action(
        std::io::Error::from(std::io::ErrorKind::PermissionDenied).into(),
    )));
    let error = owner.execute(&entry, "attempt-1").await.unwrap_err();
    let source = std::iter::successors(Some(&error as &dyn std::error::Error), |source| {
        source.source()
    })
    .find_map(|source| source.downcast_ref::<std::io::Error>())
    .unwrap();
    assert_eq!(source.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(projection.calls.load(Ordering::SeqCst), 0);
    assert_eq!(cleaner.calls.load(Ordering::SeqCst), 0);
}

struct ReceiveSettledLedger {
    done: std::sync::atomic::AtomicBool,
    retire: Mutex<Option<Arc<ReceiveAction>>>,
}

#[async_trait]
impl GetEntryAttemptPort for ReceiveSettledLedger {
    async fn get_entry_attempt(
        &self,
        entry_id: &str,
    ) -> Result<Option<EntryReceiveAttempt>, AttemptError> {
        Ok(Some(EntryReceiveAttempt {
            entry_id: entry_id.to_owned(),
            current_attempt_id: "attempt-1".into(),
            state: if self.done.load(Ordering::SeqCst) {
                AttemptState::Cancelled
            } else {
                AttemptState::Receiving
            },
            updated_at_ms: 1,
        }))
    }
}
#[async_trait]
impl RequestReceiveCancellationPort for ReceiveSettledLedger {
    async fn request_receive_cancellation(
        &self,
        _: &str,
        _: &str,
        _: i64,
    ) -> Result<RequestReceiveCancellationOutcome, AttemptError> {
        if let Some(action) = self.retire.lock().unwrap().take() {
            self.done.store(true, Ordering::SeqCst);
            action.finish(Ok(()));
        }
        Ok(RequestReceiveCancellationOutcome::Requested)
    }
}

#[tokio::test(start_paused = true)]
async fn plan_settled_partial_cancellation_is_never_overwritten_by_no_entry_cleanup() {
    for finishes_before_lookup in [false, true] {
        let attempts = Arc::new(ReceiveSettledLedger {
            done: std::sync::atomic::AtomicBool::new(false),
            retire: Mutex::new(None),
        });
        let cleaner = Arc::new(CleanerMock::default());
        let transfers = Arc::new(TransferMock::default());
        let projection = Arc::new(ProjectionMock::default());
        let commit = Arc::new(CommitMock::default());
        let work = Arc::new(ReceiveWorkRegistry::default());
        let owner = CancelEntryReceiveUseCase::new(
            attempts.clone(),
            attempts.clone(),
            commit.clone(),
            Arc::new(PublishMock),
            cleaner.clone(),
            projection.clone(),
            transfers,
            Arc::new(ClockMock),
            Arc::new(HostEventBus::new()),
            BlobTransport::new(
                Arc::new(NoNetwork),
                Arc::new(NoNetwork),
                Arc::new(NoNetwork),
                Arc::new(NoNetwork),
            ),
            work.clone(),
            Arc::default(),
        );
        let entry = EntryId::new();
        let action = work.action();
        action.register(&entry, "attempt-1").unwrap();
        if finishes_before_lookup {
            *attempts.retire.lock().unwrap() = Some(action);
        } else {
            assert_eq!(
                owner.execute(&entry, "attempt-1").await.unwrap(),
                CancelEntryReceiveOutcome::CancellationIncomplete
            );
            attempts.done.store(true, Ordering::SeqCst);
            action.finish(Ok(()));
        }
        assert_eq!(
            owner.execute(&entry, "attempt-1").await.unwrap(),
            CancelEntryReceiveOutcome::Cancelled
        );
        assert_eq!(projection.calls.load(Ordering::SeqCst), 1);
        assert_eq!(commit.calls.load(Ordering::SeqCst), 0);
        assert_eq!(cleaner.calls.load(Ordering::SeqCst), 0);
    }
}
