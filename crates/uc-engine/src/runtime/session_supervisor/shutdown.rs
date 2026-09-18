use std::time::Duration;

use tokio::time::Instant;
use tracing::info;
#[cfg(feature = "lan-compat")]
use tracing::warn;
use uc_application::deps::LifecycleError;
use uc_core::FileTransferCancellationReason;
use uc_observability_contract::diagnostics::connectivity::{
    LocalWorkObservation, LocalWorkOutcome, LocalWorkStep,
};

use super::{lifecycle_error, ProductionSession, SessionSupervisor};
use crate::runtime::task_shutdown::shutdown_tasks;
use crate::EngineError;

impl ProductionSession {
    pub(super) async fn shutdown(
        self,
        transfer_reason: FileTransferCancellationReason,
        deadline: Option<Instant>,
    ) -> Result<(), LifecycleError> {
        let task_deadline =
            deadline.or_else(|| Instant::now().checked_add(Duration::from_millis(500)));
        let mut errors = Vec::new();
        info!("Engine session 开始关闭");
        #[cfg(feature = "lan-compat")]
        if let Err(error) = self.mobile_sync.shutdown_mobile_file_uploads().await {
            warn!("mobile file upload shutdown finished with an error");
            errors.push(anyhow::Error::new(error).context("stop mobile file uploads"));
        }
        let stopping = LocalWorkObservation::begin(LocalWorkStep::SessionStopTasks);
        let stopped = shutdown_tasks(&self.tasks, task_deadline).await;
        stopping.finish(
            if stopped.timed_out_count > 0 || stopped.join_error_count > 0 {
                LocalWorkOutcome::Error
            } else {
                LocalWorkOutcome::Ok
            },
        );
        if let Err(error) = stopped.into_result() {
            errors.push(error.into());
        }
        info!("Engine session 网络观测任务已停止");
        let stopping = LocalWorkObservation::begin(LocalWorkStep::SessionStopApplication);
        let application_shutdown = self.application.shutdown(deadline).await;
        stopping.finish(if application_shutdown.is_ok() {
            LocalWorkOutcome::Ok
        } else {
            LocalWorkOutcome::Error
        });
        if let Err(error) = application_shutdown {
            errors.push(error.into());
        }
        info!("Engine session Application runtime 已停止");
        let stopping = LocalWorkObservation::begin(LocalWorkStep::SessionStopNetwork);
        let network_shutdown = self.sync_session.shutdown(transfer_reason, deadline).await;
        stopping.finish(if network_shutdown.is_ok() {
            LocalWorkOutcome::Ok
        } else {
            LocalWorkOutcome::Error
        });
        if let Err(error) = network_shutdown {
            errors.push(error.into());
        }
        info!("Engine session 网络会话任务已停止");
        LifecycleError::from_errors(errors)
    }
}

impl SessionSupervisor {
    pub(super) async fn shutdown_after_failure(&self, primary: EngineError) -> EngineError {
        let additional = self
            .stop_current_session(FileTransferCancellationReason::Unknown, None)
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
