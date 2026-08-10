//! File-transfer lifecycle owner: receive readiness, startup recovery,
//! timeout sweep and privacy maintenance all belong to the transfer domain
//! (ADR-018 stage 2). Engine callers reach these through `FileTransferFacade`
//! lifecycle actions only.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::{info, info_span, warn, Instrument};

use uc_core::file_transfer::FileTransferCancellationReason;
use uc_core::ports::file_transfer::TrackedFileTransferStatus;
use uc_core::ports::inbound_file_target::ResolveInboundSaveDirPort;
use uc_core::ports::EnsureFileTransferPrivacyMaintenancePort;
use uc_core::ports::{ClockPort, FailInflightTransfersPort, ListExpiredInflightTransfersPort};

use crate::facade::blob_transfer::{BlobTransferFacade, InboundCancelOutcome};
use crate::facade::host_event::{HostEvent, HostEventBus, TransferHostEvent};
use crate::transfer::receive::reconciliation::{
    EnsureReceiveReadyPort, ReceiveReadinessCoordinator, ReceiveReadinessError,
    ReceiveReadinessStatus, ReconcileReceiveAttemptsUseCase,
};

/// Pending rows abandoned for longer than this are considered stalled and
/// force-failed by the sweep.
const PENDING_TIMEOUT_MS: i64 = 60_000;
/// Transferring rows with no new activity within this window are force-failed.
const TRANSFERRING_TIMEOUT_MS: i64 = 5 * 60_000;
/// Sweep frequency.
const SWEEP_INTERVAL: Duration = Duration::from_secs(15);

/// Dependencies for the receiver-side file-transfer lifecycle.
pub struct FileTransferLifecycleDeps {
    pub list_expired: Arc<dyn ListExpiredInflightTransfersPort>,
    pub fail_inflight: Arc<dyn FailInflightTransfersPort>,
    pub get_receive_attempt: Arc<dyn uc_core::ports::GetEntryAttemptPort>,
    pub list_receive_attempts: Arc<dyn uc_core::ports::ListNonTerminalAttemptsPort>,
    pub list_unsettled_artifacts: Arc<dyn uc_core::ports::ListUnsettledReceiveArtifactsPort>,
    pub get_directory_publish: Arc<dyn uc_core::ports::GetDirectoryPublishRecordPort>,
    pub begin_receive_failure: Arc<dyn uc_core::ports::BeginReceiveFailurePort>,
    pub cleanup_artifacts: Arc<dyn uc_core::ports::CleanupReceiveArtifactsPort>,
    pub commit_inbound: Arc<dyn uc_core::ports::CommitInboundReceivePort>,
    pub list_provisional: Arc<dyn uc_core::ports::ListProvisionalReceivesPort>,
    pub finalize_provisional: Arc<dyn uc_core::ports::FinalizeProvisionalReceivePort>,
    pub privacy_maintenance: Arc<dyn EnsureFileTransferPrivacyMaintenancePort>,
    pub save_dir_resolver: Arc<dyn ResolveInboundSaveDirPort>,
    pub file_cache_dir: PathBuf,
    pub clock: Arc<dyn ClockPort>,
    pub host_event_bus: Arc<HostEventBus>,
}

/// Wraps receiver-side projection and the periodic health tasks.
///
/// ## Sweep / reconcile path
///
/// The sweep branches on the row's tracked status:
///
/// - **Transferring** rows route through
///   [`FileTransferFacade::cancel_inbound_transfer`]: that tears down the
///   receiver-side iroh-blobs fetch task + QUIC connection AND appends a
///   `Cancelled { reason: Timeout }` domain event whose projection flips
///   the row to `cancelled`. This is the path that actually closes the
///   sender → receiver tap (the original bug — receiver "timed out"
///   locally while the sender provider kept streaming).
/// - **Pending** rows (no `Started` event yet, no `peer_id` available)
///   stay on the legacy `mark_failed` + manual host-event path: appending
///   a peer-less `Cancelled`/`Failed` to the timeline is a domain-model
///   change that belongs to the Phase 5 cleanup, not P1.
///
/// `reconcile_on_startup` always uses the legacy path: by definition the
/// runtime is not yet up, so there is no in-flight fetch to cancel.
pub struct FileTransferLifecycle {
    /// Shared host-event bus.
    ///
    /// Exposed so receiver-side workers can publish UI-facing `pending` status
    /// events directly after seeding the receiver projection — this bypasses
    /// the domain event bus on purpose, since `pending` is a presentation-layer
    /// preview, not a domain fact (there is no `Announced` event in the
    /// timeline).
    pub host_event_bus: Arc<HostEventBus>,

    list_expired: Arc<dyn ListExpiredInflightTransfersPort>,
    fail_inflight: Arc<dyn FailInflightTransfersPort>,
    receive_reconcile: Arc<ReconcileReceiveAttemptsUseCase>,
    receive_readiness: Arc<ReceiveReadinessCoordinator>,
    privacy_maintenance: Arc<dyn EnsureFileTransferPrivacyMaintenancePort>,
    save_dir_resolver: Arc<dyn ResolveInboundSaveDirPort>,
    file_cache_dir: PathBuf,
    clock: Arc<dyn ClockPort>,
}

impl FileTransferLifecycle {
    pub(crate) fn new(deps: FileTransferLifecycleDeps) -> Self {
        let FileTransferLifecycleDeps {
            list_expired,
            fail_inflight,
            get_receive_attempt,
            list_receive_attempts,
            list_unsettled_artifacts,
            get_directory_publish,
            begin_receive_failure,
            cleanup_artifacts,
            commit_inbound,
            list_provisional,
            finalize_provisional,
            privacy_maintenance,
            save_dir_resolver,
            file_cache_dir,
            clock,
            host_event_bus,
        } = deps;
        Self {
            host_event_bus,
            list_expired,
            fail_inflight,
            receive_reconcile: Arc::new(ReconcileReceiveAttemptsUseCase::new(
                get_receive_attempt,
                list_receive_attempts,
                list_unsettled_artifacts,
                get_directory_publish,
                begin_receive_failure,
                cleanup_artifacts,
                list_provisional,
                finalize_provisional,
                commit_inbound,
                Arc::clone(&clock),
            )),
            receive_readiness: Arc::new(ReceiveReadinessCoordinator::new()),
            privacy_maintenance,
            save_dir_resolver,
            file_cache_dir,
            clock,
        }
    }

    pub async fn ensure_receive_ready(&self) -> Result<(), ReceiveReadinessError> {
        self.receive_readiness
            .ensure_ready(|| async {
                self.privacy_maintenance
                    .ensure_file_transfer_privacy_maintenance()
                    .await
                    .map_err(|error| anyhow::anyhow!(error.to_string()))?;
                self.receive_reconcile.execute().await?;
                self.reconcile_on_startup().await?;
                sweep_inbound_staging(Arc::clone(&self.save_dir_resolver), &self.file_cache_dir)
                    .await;
                Ok(())
            })
            .await
            .map_err(|error| ReceiveReadinessError::Recovery(error.to_string()))
    }

    /// Spawn a periodic timeout sweep.
    ///
    /// Runs every 15 seconds. Fails stalled pending (>60s) and transferring
    /// (>5min) rows, emits `TransferHostEvent::StatusChanged`, and cleans the
    /// partial cache artifacts on disk.
    pub fn spawn_timeout_sweep(
        &self,
        cancel: tokio::sync::watch::Receiver<bool>,
        blob_transfer: Arc<BlobTransferFacade>,
    ) -> JoinHandle<()> {
        let list_expired = Arc::clone(&self.list_expired);
        let fail_inflight = Arc::clone(&self.fail_inflight);
        let clock = Arc::clone(&self.clock);
        let bus = Arc::clone(&self.host_event_bus);
        let readiness = Arc::clone(&self.receive_readiness);

        tokio::spawn(
            async move {
                readiness.wait_ready().await;
                let mut interval = tokio::time::interval(SWEEP_INTERVAL);
                let mut cancel = cancel;

                loop {
                    tokio::select! {
                        _ = interval.tick() => {},
                        _ = cancel.changed() => {
                            if *cancel.borrow() {
                                info!("File transfer timeout sweep shutting down");
                                return;
                            }
                        }
                    }

                    let now_ms = clock.now_ms();
                    let pending_cutoff = now_ms - PENDING_TIMEOUT_MS;
                    let transferring_cutoff = now_ms - TRANSFERRING_TIMEOUT_MS;

                    let expired = match list_expired
                        .list_expired_inflight(pending_cutoff, transferring_cutoff)
                        .await
                    {
                        Ok(list) => list,
                        Err(err) => {
                            warn!(error = %err, "Timeout sweep query failed");
                            continue;
                        }
                    };

                    if expired.is_empty() {
                        continue;
                    }

                    info!(
                        count = expired.len(),
                        "Timeout sweep found expired in-flight transfers"
                    );

                    for t in &expired {
                        if matches!(t.status, TrackedFileTransferStatus::Transferring) {
                            match blob_transfer
                                .cancel_inbound_transfer(
                                    &t.transfer_id,
                                    FileTransferCancellationReason::Timeout,
                                )
                                .await
                            {
                                Ok(InboundCancelOutcome::Cancelled) => {
                                    cleanup_cached_path(&t.cached_path).await;
                                    continue;
                                }
                                Ok(InboundCancelOutcome::NotInflight) => {
                                    // fall through to mark_failed
                                }
                                Err(err) => {
                                    warn!(
                                        error = %err,
                                        transfer_id = %t.transfer_id,
                                        "Timeout sweep: cancel_inbound_transfer failed, falling back to mark_failed"
                                    );
                                }
                            }
                        }

                        let reason = timeout_reason_for(t.status);

                        if let Err(err) = fail_inflight.mark_failed(&t.transfer_id, reason, now_ms).await {
                            warn!(
                                error = %err,
                                transfer_id = %t.transfer_id,
                                "Failed to mark expired transfer as failed"
                            );
                            continue;
                        }

                        cleanup_cached_path(&t.cached_path).await;

                        bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::StatusChanged {
                            transfer_id: t.transfer_id.clone(),
                            entry_id: t.entry_id.clone(),
                            attempt_id: None,
                            status: "failed".to_string(),
                            reason: Some(reason.to_string()),
                        }));
                    }
                }
            }
            .instrument(info_span!("file_transfer.timeout_sweep")),
        )
    }

    /// Run startup reconciliation: mark orphaned in-flight transfers as
    /// failed and clean their cache artifacts.
    ///
    /// A storage failure is returned so the receive readiness gate remains
    /// closed and a later lifecycle retry can run the same idempotent pass.
    pub async fn reconcile_on_startup(&self) -> anyhow::Result<()> {
        let now_ms = self.clock.now_ms();
        let reason = "orphaned: app restarted while transfer was in-flight";

        let cleanup_targets = match self
            .fail_inflight
            .bulk_fail_inflight(reason, now_ms)
            .instrument(info_span!("file_transfer.startup_reconcile"))
            .await
        {
            Ok(targets) => targets,
            Err(err) => {
                warn!(error = %err, "Startup reconciliation failed");
                return Err(anyhow::anyhow!(err.to_string()));
            }
        };

        if cleanup_targets.is_empty() {
            info!("No orphaned in-flight transfers found at startup");
            return Ok(());
        }

        info!(
            count = cleanup_targets.len(),
            "Reconciled orphaned in-flight transfers at startup"
        );

        for t in &cleanup_targets {
            cleanup_cached_path(&t.cached_path).await;

            self.host_event_bus.emit_or_warn(HostEvent::Transfer(
                TransferHostEvent::StatusChanged {
                    transfer_id: t.transfer_id.clone(),
                    entry_id: t.entry_id.clone(),
                    attempt_id: None,
                    status: "failed".to_string(),
                    reason: Some(reason.to_string()),
                },
            ));
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl EnsureReceiveReadyPort for FileTransferLifecycle {
    async fn ensure_receive_ready(&self) -> Result<(), ReceiveReadinessError> {
        Self::ensure_receive_ready(self).await
    }

    fn close_receive_gate(&self) {
        self.receive_readiness.mark_not_ready();
    }

    fn receive_readiness_status(&self) -> ReceiveReadinessStatus {
        self.receive_readiness.status()
    }
}

fn timeout_reason_for(status: TrackedFileTransferStatus) -> &'static str {
    match status {
        TrackedFileTransferStatus::Pending => "timeout: no data received within 60 seconds",
        TrackedFileTransferStatus::Transferring => {
            "timeout: no new chunk received within 5 minutes"
        }
        _ => "timeout: stalled transfer",
    }
}

/// Startup sweep of the hidden areas where inbound directories are assembled.
///
/// A directory receive builds its tree in a hidden area beside where the roots
/// will land, then publishes them in one atomic step. A process that dies
/// mid-receive never reaches that step, so the area survives with a partial
/// tree in it and no entry referring to it.
///
/// Debris can therefore sit in two places, and both are scanned: the user's
/// configured save directory, and the managed per-entry cache parents used
/// when there is no save directory (or its volume cannot publish atomically).
///
/// Governance only: nothing here is fatal, because a leftover area costs disk
/// space rather than correctness — it is hidden, and unreachable from any
/// entry.
async fn sweep_inbound_staging(
    save_dir_resolver: Arc<dyn ResolveInboundSaveDirPort>,
    file_cache_dir: &Path,
) {
    let mut scan_dirs: Vec<PathBuf> = Vec::new();

    if let Some(save_dir) = save_dir_resolver.resolve_save_dir().await {
        scan_dirs.push(save_dir);
    }

    // The managed layout puts each entry's roots in its own directory, so the
    // areas are one level down rather than directly here.
    let managed = file_cache_dir.join("iroh-blobs");
    match tokio::fs::read_dir(&managed).await {
        Ok(mut entries) => loop {
            match entries.next_entry().await {
                Ok(Some(entry)) => {
                    if entry
                        .file_type()
                        .await
                        .map(|kind| kind.is_dir())
                        .unwrap_or(false)
                    {
                        scan_dirs.push(entry.path());
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    warn!(
                        error = %err,
                        "could not enumerate managed cache while sweeping staging areas"
                    );
                    break;
                }
            }
        },
        // No cache dir yet (first run) is the normal case, not a problem.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            warn!(
                error = %err,
                "could not open managed cache while sweeping staging areas"
            );
        }
    }

    crate::clipboard::sync::sweep_inbound_staging(&scan_dirs).await;
}

/// Best-effort cleanup of a cached file or transfer directory.
async fn cleanup_cached_path(cached_path: &str) {
    if cached_path.is_empty() {
        return;
    }

    let path = std::path::Path::new(cached_path);

    if path.is_file() {
        if let Err(err) = tokio::fs::remove_file(path).await {
            warn!(error = %err, "Failed to remove cached file");
        }
    }

    if let Some(parent) = path.parent() {
        // Only remove parent if it looks like a per-transfer directory — avoid
        // accidentally deleting the shared cache root. The heuristic matches
        // the previous orchestrator behavior.
        if parent.is_dir() {
            if let Ok(mut entries) = tokio::fs::read_dir(parent).await {
                if entries.next_entry().await.ok().flatten().is_none() {
                    if let Err(err) = tokio::fs::remove_dir(parent).await {
                        warn!(
                            error = %err,
                            "Failed to remove empty transfer directory"
                        );
                    }
                }
            }
        }
    }
}
