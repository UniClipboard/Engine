//! Receiver-side file-transfer session entry point.

use std::sync::Arc;

use uc_core::file_transfer::{FileTransferEventPublisherPort, FileTransferEventStorePort};
use uc_core::ports::file_transfer::{
    PendingInboundTransfer, ProvisionalInboundTransfer, UpdateProvisionalReceivePathPort,
};
use uc_core::ports::{
    ClockPort, FinalizeProvisionalReceivePort, ProvisionalReceiveAction, ProvisionalReceiveError,
    RecordReceiverTransferPort, SeedProvisionalReceivePort,
};
use uc_core::FileTransferEvent;

use crate::transfer::file::lifecycle::{FileTransferLifecycle, FileTransferLifecycleDeps};
use crate::transfer::file::FileTransferApplicationError;
use crate::transfer::receive::reconciliation::{
    EnsureReceiveReadyPort, ReceiveReadinessError, ReceiveReadinessStatus,
};

use super::session::{FileTransferSession, FileTransferSessionRegistry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiverTransferRegistration {
    Entry {
        entry_id: String,
        attempt_id: Option<String>,
        cached_path: String,
    },
    Provisional,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginReceiverTransfer {
    pub transfer_id: String,
    pub peer_id: String,
    pub filename: String,
    pub file_size: Option<u64>,
    pub registration: ReceiverTransferRegistration,
}

/// Dependencies shared by every receiver-side transfer session.
pub struct FileTransferFacadeDeps {
    pub store: Arc<dyn FileTransferEventStorePort>,
    pub publisher: Arc<dyn FileTransferEventPublisherPort>,
    pub repo: Arc<dyn RecordReceiverTransferPort>,
    pub provisional_seed: Arc<dyn SeedProvisionalReceivePort>,
    pub provisional_path: Arc<dyn UpdateProvisionalReceivePathPort>,
    pub provisional_finalize: Arc<dyn FinalizeProvisionalReceivePort>,
    pub clock: Arc<dyn ClockPort>,
    /// Receiver-side lifecycle dependencies: receive readiness, startup
    /// recovery, timeout sweep and privacy maintenance. The facade owns the
    /// lifecycle and exposes its lifecycle actions (ADR-018 stage 2).
    pub lifecycle: FileTransferLifecycleDeps,
}

/// Owns active receiver sessions and closes them as one process-wide lifecycle.
pub struct FileTransferFacade {
    repo: Arc<dyn RecordReceiverTransferPort>,
    provisional_seed: Arc<dyn SeedProvisionalReceivePort>,
    provisional_path: Arc<dyn UpdateProvisionalReceivePathPort>,
    provisional_finalize: Arc<dyn FinalizeProvisionalReceivePort>,
    clock: Arc<dyn ClockPort>,
    store: Arc<dyn FileTransferEventStorePort>,
    publisher: Arc<dyn FileTransferEventPublisherPort>,
    sessions: Arc<FileTransferSessionRegistry>,
    lifecycle: Arc<FileTransferLifecycle>,
}

impl FileTransferFacade {
    pub fn new(deps: FileTransferFacadeDeps) -> Self {
        let store = Arc::clone(&deps.store);
        let publisher = Arc::clone(&deps.publisher);
        let lifecycle = Arc::new(FileTransferLifecycle::new(deps.lifecycle));

        Self {
            repo: deps.repo,
            provisional_seed: deps.provisional_seed,
            provisional_path: deps.provisional_path,
            provisional_finalize: deps.provisional_finalize,
            clock: deps.clock,
            store,
            publisher,
            sessions: Arc::new(FileTransferSessionRegistry::new()),
            lifecycle,
        }
    }

    pub async fn begin_receiver_transfer(
        &self,
        input: BeginReceiverTransfer,
    ) -> Result<Arc<FileTransferSession>, FileTransferApplicationError> {
        let _creation = self.sessions.lock_creation().await;
        self.sessions.ensure_open().await?;
        if let Some(existing) = self.sessions.get(&input.transfer_id).await {
            return if existing.matches(&input) {
                Ok(existing)
            } else {
                Err(FileTransferApplicationError::SessionConflict {
                    transfer_id: input.transfer_id,
                })
            };
        }

        let timeline =
            crate::transfer::file::timeline::load_timeline(self.store.as_ref(), &input.transfer_id)
                .await?;
        if timeline.is_finished() {
            return Err(FileTransferApplicationError::TransferAlreadyFinished {
                transfer_id: input.transfer_id,
            });
        }
        if timeline.started {
            timeline.ensure_active(&input.transfer_id, &input.peer_id)?;
        }

        self.register_receiver(&input).await?;
        let session = Arc::new(FileTransferSession::new(
            input.clone(),
            Arc::clone(&self.store),
            Arc::clone(&self.publisher),
            Arc::downgrade(&self.sessions),
            timeline.last_progress_bytes,
        ));

        if timeline.started {
            self.sessions.insert(Arc::clone(&session)).await;
            return Ok(session);
        }

        let started = FileTransferEvent::started(
            input.transfer_id,
            input.peer_id,
            input.filename,
            input.file_size,
        );
        self.store
            .append(started.clone())
            .await
            .map_err(|error| FileTransferApplicationError::Store(error.to_string()))?;
        self.sessions.insert(Arc::clone(&session)).await;
        self.publisher
            .publish(started)
            .await
            .map_err(|error| FileTransferApplicationError::Publish(error.to_string()))?;
        Ok(session)
    }

    pub async fn active_session(&self, transfer_id: &str) -> Option<Arc<FileTransferSession>> {
        self.sessions.get(transfer_id).await
    }

    pub async fn close(&self) -> Result<(), FileTransferApplicationError> {
        let _creation = self.sessions.lock_creation().await;
        let sessions = self.sessions.close_and_snapshot().await;
        Self::cancel_sessions(sessions, uc_core::FileTransferCancellationReason::Unknown).await
    }

    pub async fn cancel_active_sessions(
        &self,
        reason: uc_core::FileTransferCancellationReason,
    ) -> Result<(), FileTransferApplicationError> {
        let _creation = self.sessions.lock_creation().await;
        let sessions = self.sessions.snapshot().await;
        Self::cancel_sessions(sessions, reason).await
    }

    async fn cancel_sessions(
        sessions: Vec<Arc<FileTransferSession>>,
        reason: uc_core::FileTransferCancellationReason,
    ) -> Result<(), FileTransferApplicationError> {
        let mut first_error = None;
        for session in sessions {
            match session.cancel(reason).await {
                Ok(_) | Err(FileTransferApplicationError::TransferAlreadyFinished { .. }) => {}
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn register_receiver(
        &self,
        input: &BeginReceiverTransfer,
    ) -> Result<(), FileTransferApplicationError> {
        let file_size = input
            .file_size
            .map(i64::try_from)
            .transpose()
            .map_err(|_| {
                FileTransferApplicationError::Repository(
                    "inbound file size exceeds the receiver projection range".to_owned(),
                )
            })?;
        match &input.registration {
            ReceiverTransferRegistration::Entry {
                entry_id,
                attempt_id,
                cached_path,
            } => self
                .repo
                .upsert_pending_transfer(&PendingInboundTransfer {
                    transfer_id: input.transfer_id.clone(),
                    entry_id: entry_id.clone(),
                    attempt_id: attempt_id.clone(),
                    origin_device_id: input.peer_id.clone(),
                    filename: input.filename.clone(),
                    file_size,
                    cached_path: cached_path.clone(),
                    created_at_ms: self.clock.now_ms(),
                })
                .await
                .map_err(|error| FileTransferApplicationError::Repository(error.to_string())),
            ReceiverTransferRegistration::Provisional => self
                .provisional_seed
                .seed_provisional_receive(&ProvisionalInboundTransfer {
                    transfer_id: input.transfer_id.clone(),
                    origin_device_id: input.peer_id.clone(),
                    filename: input.filename.clone(),
                    file_size,
                    created_at_ms: self.clock.now_ms(),
                })
                .await
                .map_err(|error| FileTransferApplicationError::Repository(error.to_string())),
        }
    }

    pub async fn finalize_provisional_receive(
        &self,
        transfer_id: &str,
        action: ProvisionalReceiveAction,
    ) -> Result<(), ProvisionalReceiveError> {
        self.provisional_finalize
            .finalize_provisional_receive(transfer_id, action, self.clock.now_ms())
            .await
    }

    pub async fn record_provisional_receive_path(
        &self,
        transfer_id: &str,
        cached_path: &str,
    ) -> Result<(), ProvisionalReceiveError> {
        self.provisional_path
            .update_provisional_receive_path(transfer_id, cached_path, self.clock.now_ms())
            .await
    }

    /// Lifecycle action: run receive readiness recovery (privacy
    /// maintenance, receive-attempt reconciliation, startup recovery and
    /// staging sweep) and open the receive gate.
    pub async fn ensure_receive_ready(&self) -> Result<(), ReceiveReadinessError> {
        self.lifecycle.ensure_receive_ready().await
    }

    /// Lifecycle action: close the receive gate so inbound transfers are
    /// rejected until the next `ensure_receive_ready`.
    pub fn close_receive_gate(&self) {
        self.lifecycle.close_receive_gate();
    }

    /// Read the current receive-gate status.
    pub fn receive_readiness_status(&self) -> ReceiveReadinessStatus {
        self.lifecycle.receive_readiness_status()
    }

    /// Lifecycle action: spawn the periodic timeout sweep that fails stalled
    /// transfers and cleans partial cache artifacts.
    pub fn spawn_timeout_sweep(
        &self,
        cancel: tokio::sync::watch::Receiver<bool>,
        blob_transfer: Arc<crate::transfer::blob::facade::BlobTransferFacade>,
    ) -> tokio::task::JoinHandle<()> {
        self.lifecycle.spawn_timeout_sweep(cancel, blob_transfer)
    }

    /// Lifecycle action: mark orphaned in-flight transfers as failed after a
    /// restart and clean their cache artifacts.
    pub async fn reconcile_on_startup(&self) -> anyhow::Result<()> {
        self.lifecycle.reconcile_on_startup().await
    }
}

#[async_trait::async_trait]
impl EnsureReceiveReadyPort for FileTransferFacade {
    async fn ensure_receive_ready(&self) -> Result<(), ReceiveReadinessError> {
        self.lifecycle.ensure_receive_ready().await
    }

    fn close_receive_gate(&self) {
        self.lifecycle.close_receive_gate();
    }

    fn receive_readiness_status(&self) -> ReceiveReadinessStatus {
        self.lifecycle.receive_readiness_status()
    }
}
