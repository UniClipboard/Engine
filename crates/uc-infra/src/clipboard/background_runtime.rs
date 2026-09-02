//! Clipboard spool/blob 的具体 Infra runtime adapter。
//!
//! Application 决定启动顺序；本 adapter 只实现具体磁盘恢复与 worker。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use uc_application::facade::{ClipboardBackgroundError, ClipboardBackgroundPort};
use uc_core::ids::RepresentationId;
use uc_core::ports::clipboard::{
    ClipboardRepresentationStore, ThumbnailGeneratorPort, ThumbnailRepositoryPort,
};
use uc_core::ports::{ClockPort, ContentHashPort};
use uc_core::TaskRegistry;

use crate::blob::BlobWriterPort;

use super::{
    BackgroundBlobWorker, RepresentationCache, SpoolJanitor, SpoolManager, SpoolScanner,
    StagedReconciler,
};

const SPOOL_JANITOR_INTERVAL: Duration = Duration::from_secs(60 * 60);

pub struct ClipboardBackgroundRuntime {
    representation_cache: Arc<RepresentationCache>,
    spool_manager: Arc<SpoolManager>,
    worker_rx: Mutex<Option<mpsc::Receiver<RepresentationId>>>,
    spool_dir: PathBuf,
    spool_ttl_days: u64,
    worker_retry_max_attempts: u32,
    worker_retry_backoff: Duration,
    representation_repo: Arc<dyn ClipboardRepresentationStore>,
    worker_tx: mpsc::Sender<RepresentationId>,
    blob_writer: Arc<dyn BlobWriterPort>,
    hasher: Arc<dyn ContentHashPort>,
    clock: Arc<dyn ClockPort>,
    thumbnail_repo: Arc<dyn ThumbnailRepositoryPort>,
    thumbnail_generator: Arc<dyn ThumbnailGeneratorPort>,
}

impl ClipboardBackgroundRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        representation_cache: Arc<RepresentationCache>,
        spool_manager: Arc<SpoolManager>,
        worker_rx: mpsc::Receiver<RepresentationId>,
        spool_dir: PathBuf,
        spool_ttl_days: u64,
        worker_retry_max_attempts: u32,
        worker_retry_backoff_ms: u64,
        representation_repo: Arc<dyn ClipboardRepresentationStore>,
        worker_tx: mpsc::Sender<RepresentationId>,
        blob_writer: Arc<dyn BlobWriterPort>,
        hasher: Arc<dyn ContentHashPort>,
        clock: Arc<dyn ClockPort>,
        thumbnail_repo: Arc<dyn ThumbnailRepositoryPort>,
        thumbnail_generator: Arc<dyn ThumbnailGeneratorPort>,
    ) -> Self {
        Self {
            representation_cache,
            spool_manager,
            worker_rx: Mutex::new(Some(worker_rx)),
            spool_dir,
            spool_ttl_days,
            worker_retry_max_attempts,
            worker_retry_backoff: Duration::from_millis(worker_retry_backoff_ms),
            representation_repo,
            worker_tx,
            blob_writer,
            hasher,
            clock,
            thumbnail_repo,
            thumbnail_generator,
        }
    }
}

#[async_trait]
impl ClipboardBackgroundPort for ClipboardBackgroundRuntime {
    async fn start(
        &self,
        task_registry: Arc<TaskRegistry>,
    ) -> Result<(), ClipboardBackgroundError> {
        let worker_rx = self
            .worker_rx
            .lock()
            .await
            .take()
            .ok_or(ClipboardBackgroundError::AlreadyStarted)?;

        let scanner = SpoolScanner::new(
            self.spool_dir.clone(),
            Arc::clone(&self.representation_repo),
            self.worker_tx.clone(),
        );
        let recovered = scanner
            .scan_and_recover()
            .await
            .map_err(|source| ClipboardBackgroundError::SpoolRecovery { source })?;
        if recovered > 0 {
            info!(
                recovered,
                "recovered staged clipboard representations from spool"
            );
        }

        let reconciler = StagedReconciler::new(
            Arc::clone(&self.representation_repo),
            Arc::clone(&self.spool_manager),
        );
        let demoted = reconciler
            .run_once()
            .await
            .map_err(|source| ClipboardBackgroundError::SpoolRecovery { source })?;
        if demoted > 0 {
            info!(demoted, "demoted orphaned staged clipboard representations");
        }

        let worker = BackgroundBlobWorker::new(
            worker_rx,
            Arc::clone(&self.representation_cache),
            Arc::clone(&self.spool_manager),
            Arc::clone(&self.representation_repo),
            Arc::clone(&self.blob_writer),
            Arc::clone(&self.hasher),
            Arc::clone(&self.thumbnail_repo),
            Arc::clone(&self.thumbnail_generator),
            Arc::clone(&self.clock),
            self.worker_retry_max_attempts,
            self.worker_retry_backoff,
        );
        task_registry
            .spawn("blob_worker", |cancel| async move {
                tokio::select! {
                    _ = cancel.cancelled() => info!("background clipboard blob worker stopped"),
                    _ = worker.run() => info!("background clipboard blob worker completed"),
                }
            })
            .await;

        let janitor = SpoolJanitor::new(
            Arc::clone(&self.spool_manager),
            Arc::clone(&self.representation_repo),
            Arc::clone(&self.clock),
            self.spool_ttl_days,
        );
        task_registry
            .spawn("spool_janitor", |cancel| async move {
                let mut interval = tokio::time::interval(SPOOL_JANITOR_INTERVAL);
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => return,
                        _ = interval.tick() => match janitor.run_once().await {
                            Ok(removed) if removed > 0 => info!(removed, "removed expired spool entries"),
                            Ok(_) => {}
                            Err(error) => warn!(error = %error, "spool janitor sweep failed"),
                        }
                    }
                }
            })
            .await;
        Ok(())
    }
}
