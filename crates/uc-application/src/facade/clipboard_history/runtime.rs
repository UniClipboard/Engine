use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::{
    CleanupResultView, ClipboardHistoryError, ClipboardHistoryFacade, ReconcileResultView,
    RetentionEnforcementResultView,
};

const HISTORY_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(300);

#[async_trait]
pub(super) trait HistoryMaintenance: Send + Sync {
    async fn reconcile_missing_files(&self) -> Result<ReconcileResultView, ClipboardHistoryError>;
    async fn cleanup_expired_files(&self) -> Result<CleanupResultView, ClipboardHistoryError>;
    async fn enforce_retention_policy(
        &self,
    ) -> Result<RetentionEnforcementResultView, ClipboardHistoryError>;
}

#[async_trait]
impl HistoryMaintenance for ClipboardHistoryFacade {
    async fn reconcile_missing_files(&self) -> Result<ReconcileResultView, ClipboardHistoryError> {
        ClipboardHistoryFacade::reconcile_missing_files(self).await
    }

    async fn cleanup_expired_files(&self) -> Result<CleanupResultView, ClipboardHistoryError> {
        ClipboardHistoryFacade::cleanup_expired_files(self).await
    }

    async fn enforce_retention_policy(
        &self,
    ) -> Result<RetentionEnforcementResultView, ClipboardHistoryError> {
        ClipboardHistoryFacade::enforce_retention_policy(self).await
    }
}

#[derive(Debug, Error)]
pub enum HistoryMaintenanceRuntimeError {
    #[error("history maintenance task failed: {0}")]
    Task(String),
}

pub struct HistoryMaintenanceRuntime {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl HistoryMaintenanceRuntime {
    pub async fn start(history: Arc<ClipboardHistoryFacade>) -> Self {
        let maintenance: Arc<dyn HistoryMaintenance> = history;
        Self::start_with_interval(maintenance, HISTORY_MAINTENANCE_INTERVAL).await
    }

    pub(super) async fn start_with_interval(
        maintenance: Arc<dyn HistoryMaintenance>,
        interval: Duration,
    ) -> Self {
        run_history_maintenance_once(maintenance.as_ref()).await;
        let cancel = CancellationToken::new();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            run_history_maintenance_loop(maintenance, interval, task_cancel).await;
        });
        Self {
            cancel,
            task: Some(task),
        }
    }

    pub async fn shutdown(mut self) -> Result<(), HistoryMaintenanceRuntimeError> {
        self.cancel.cancel();
        let Some(task) = self.task.take() else {
            return Ok(());
        };
        task.await
            .map_err(|error| HistoryMaintenanceRuntimeError::Task(error.to_string()))
    }
}

impl Drop for HistoryMaintenanceRuntime {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

async fn run_history_maintenance_loop(
    maintenance: Arc<dyn HistoryMaintenance>,
    interval: Duration,
    cancel: CancellationToken,
) {
    info!("history maintenance started");
    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            _ = tokio::time::sleep(interval) => {}
        }
        run_history_maintenance_once(maintenance.as_ref()).await;
    }
    info!("history maintenance stopped");
}

#[derive(Default)]
struct HistoryMaintenanceSummary {
    reconcile: Option<ReconcileResultView>,
    cleanup: Option<CleanupResultView>,
    retention: Option<RetentionEnforcementResultView>,
    reconcile_failed: bool,
    cleanup_failed: bool,
    retention_failed: bool,
}

impl HistoryMaintenanceSummary {
    fn log(&self) {
        let reconcile = self.reconcile.as_ref().cloned().unwrap_or_default();
        let cleanup = self.cleanup.as_ref().cloned().unwrap_or_default();
        let retention = self.retention.as_ref().cloned().unwrap_or_default();
        info!(
            reconcile_failed = self.reconcile_failed,
            cleanup_failed = self.cleanup_failed,
            retention_failed = self.retention_failed,
            entries_scanned = reconcile.entries_scanned,
            missing_entries_deleted = reconcile.entries_deleted,
            cache_files_removed = cleanup.files_removed,
            cache_entries_deleted = cleanup.entries_deleted,
            cache_orphans_removed = cleanup.orphans_removed,
            bytes_reclaimed = cleanup.bytes_reclaimed,
            retention_entries_deleted = retention.entries_deleted,
            errors = reconcile
                .errors
                .saturating_add(cleanup.errors)
                .saturating_add(retention.errors),
            "history maintenance pass finished"
        );
    }
}

async fn run_history_maintenance_once(maintenance: &dyn HistoryMaintenance) {
    let mut summary = HistoryMaintenanceSummary::default();
    match maintenance.reconcile_missing_files().await {
        Ok(result) => summary.reconcile = Some(result),
        Err(_) => {
            summary.reconcile_failed = true;
            warn!("history reconciliation failed; skipping remaining maintenance passes");
            summary.log();
            return;
        }
    }

    match maintenance.cleanup_expired_files().await {
        Ok(result) => summary.cleanup = Some(result),
        Err(_) => {
            summary.cleanup_failed = true;
            warn!("history file cache cleanup failed");
        }
    }

    match maintenance.enforce_retention_policy().await {
        Ok(result) => summary.retention = Some(result),
        Err(_) => {
            summary.retention_failed = true;
            warn!("history retention policy enforcement failed");
        }
    }
    summary.log();
}
