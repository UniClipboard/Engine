//! Durable staging and post-commit notification for clipboard payloads.
//!
//! `persist()` writes and syncs bytes through `SpoolManager` without waking
//! the worker. Capture owns the database commit and calls `notify_committed()`
//! only after the representation and its owner are visible together.
//!
//! Spool write failure stops capture before its entry commit. A committed
//! record keeps its durable bytes even if the bounded notification channel
//! is full or the process exits: the existing startup scanner recovers it.
//! Uncommitted files keep the existing orphan-recovery policy.

use anyhow::Context;
use std::sync::Arc;

use tokio::sync::mpsc;

use uc_core::ids::RepresentationId;
use uc_core::ports::clipboard::{SpoolQueuePort, SpoolRequest};
use uc_observability_contract::{error_source::io_error_kind, log_fields::log_id, uc_warn};

use crate::clipboard::SpoolManager;

/// Durable payload staging with an explicit committed-work notification.
pub struct DurableSpoolQueue {
    spool_manager: Arc<SpoolManager>,
    worker_tx: mpsc::Sender<RepresentationId>,
}

impl DurableSpoolQueue {
    pub fn new(
        spool_manager: Arc<SpoolManager>,
        worker_tx: mpsc::Sender<RepresentationId>,
    ) -> Self {
        Self {
            spool_manager,
            worker_tx,
        }
    }
}

#[async_trait::async_trait]
impl SpoolQueuePort for DurableSpoolQueue {
    async fn persist(&self, request: SpoolRequest) -> anyhow::Result<()> {
        // Write bytes to disk first — this is the durability guarantee.
        // If this fails, we return Err so the caller knows bytes are not safe.
        self.spool_manager
            .write(&request.rep_id, &request.bytes)
            .await
            .context("failed to write spool file")?;

        Ok(())
    }

    fn notify_committed(&self, rep_id: &RepresentationId) {
        // Notify only after the representation and its owner have committed.
        // A failure here is non-fatal: the spool file is on disk and will be
        // recovered by SpoolScanner on the next application startup.
        if let Err(err) = self.worker_tx.try_send(rep_id.clone()) {
            uc_warn!(
                representation_id = log_id(rep_id),
                error_kind = "worker_notify",
                io_error_kind = io_error_kind(&err),
                "Failed to notify worker after capture commit; will be recovered on next startup"
            );
        }
    }
}
