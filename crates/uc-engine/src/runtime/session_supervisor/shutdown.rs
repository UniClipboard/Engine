use std::time::Duration;

use tokio::time::Instant;
use tracing::info;
use uc_application::facade::LifecycleError;
use uc_core::FileTransferCancellationReason;

use super::{lifecycle_error, ProductionSession};
use crate::runtime::task_shutdown::shutdown_tasks;
use crate::EngineError;

impl ProductionSession {
    pub(super) async fn shutdown(
        self,
        transfer_reason: FileTransferCancellationReason,
        deadline: Option<Instant>,
    ) -> Result<(), LifecycleError> {
        let deadline = deadline.or_else(|| Instant::now().checked_add(Duration::from_millis(500)));
        let mut errors = Vec::new();
        info!("Engine session 开始关闭");
        #[cfg(feature = "lan-compat")]
        if let Err(error) = self.mobile_sync.shutdown_mobile_file_uploads().await {
            errors.push(anyhow::Error::new(error).context("stop mobile file uploads"));
        }
        if let Err(error) = shutdown_tasks(&self.tasks, deadline).await.into_result() {
            errors.push(error.into());
        }
        info!("Engine session 网络观测任务已停止");
        if let Err(error) = self.application.shutdown(deadline).await.into_result() {
            errors.push(error.into());
        }
        info!("Engine session Application runtime 已停止");
        if let Err(error) = self.sync_engine.shutdown(transfer_reason, deadline).await {
            errors.push(error.into());
        }
        info!("Engine session Iroh 网络已停止");
        LifecycleError::from_errors(errors)
    }

    pub(super) async fn shutdown_after_failure(self, primary: EngineError) -> EngineError {
        let additional = self
            .shutdown(FileTransferCancellationReason::Unknown, None)
            .await
            .err()
            .map(anyhow::Error::new)
            .into_iter()
            .collect();
        lifecycle_error(LifecycleError {
            primary: primary.into(),
            additional,
        })
    }
}
