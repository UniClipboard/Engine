use std::sync::Arc;
use std::time::Duration;

use tokio::task::{JoinError, JoinHandle};
use tokio::time::{timeout_at, Instant};
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};

use super::Engine;
use crate::{
    EngineError, EngineErrorCategory, EngineEvent, EngineState, OperationTerminal,
    ResourceReleaseOutcome,
};

const SHUTDOWN_COMPLETION_MARGIN: Duration = Duration::from_millis(100);

#[derive(Default)]
pub(super) struct ShutdownProgress {
    first_error: Option<EngineError>,
    completed: Option<ResourceReleaseOutcome>,
}

impl Engine {
    pub async fn shutdown_until_complete(&self) -> Result<(), EngineError> {
        match self.shutdown_until_released().await {
            ResourceReleaseOutcome::Released { .. } => Ok(()),
            outcome => outcome.into_result(),
        }
    }

    pub async fn shutdown_until_released(&self) -> ResourceReleaseOutcome {
        match self.start_shutdown(None).await {
            Ok(outcome) => outcome,
            Err(error) => ResourceReleaseOutcome::Failed {
                error: shutdown_task_failed(error),
            },
        }
    }

    pub async fn shutdown(&self, deadline: Duration) -> Result<(), EngineError> {
        match self.shutdown_with_outcome(deadline).await {
            ResourceReleaseOutcome::Released { .. } => Ok(()),
            outcome => outcome.into_result(),
        }
    }

    pub async fn shutdown_with_outcome(&self, deadline: Duration) -> ResourceReleaseOutcome {
        let Some(deadline_at) = Instant::now().checked_add(deadline) else {
            return ResourceReleaseOutcome::Failed {
                error: EngineError::new(1003, EngineErrorCategory::InvalidInput, false),
            };
        };
        match timeout_at(deadline_at, self.start_shutdown(Some(deadline_at))).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => ResourceReleaseOutcome::Failed {
                error: shutdown_task_failed(error),
            },
            Err(_) => ResourceReleaseOutcome::Pending,
        }
    }

    pub(super) fn start_shutdown(
        &self,
        deadline_at: Option<Instant>,
    ) -> JoinHandle<ResourceReleaseOutcome> {
        self.lifecycle_requests
            .accept_shutdown(&self.stop_requested);
        let shutdown_gate = Arc::clone(&self.shutdown_gate);
        let progress = Arc::clone(&self.shutdown_progress);
        let lifecycle_gate = Arc::clone(&self.lifecycle_gate);
        let lifecycle_requests = Arc::clone(&self.lifecycle_requests);
        let state = Arc::clone(&self.state);
        let runtime = Arc::clone(&self.runtime);
        let events = self.events.clone();
        let operations = Arc::clone(&self.operations);
        let log_activity = self.log_activity.clone();
        tokio::spawn(async move {
            let _shutdown = shutdown_gate.lock().await;
            let mut progress = progress.lock().await;
            if let Some(outcome) = &progress.completed {
                return outcome.clone();
            }
            let lifecycle_guard = lifecycle_gate.lock().await;
            *state.lock().await = EngineState::ShuttingDown;
            events.send(EngineEvent::StateChanged {
                state: EngineState::ShuttingDown,
            });
            drop(lifecycle_guard);
            lifecycle_requests.wait_empty().await;
            if !operations
                .wait_until_empty(deadline_at.map_or(Duration::ZERO, remaining_until))
                .await
            {
                for operation_id in operations.cancel_all().await {
                    events.send(EngineEvent::OperationFinished {
                        operation_id,
                        terminal: OperationTerminal::Cancelled,
                    });
                }
            }
            operations.wait_empty().await;
            let runtime_deadline =
                deadline_at.map(|end| end.checked_sub(SHUTDOWN_COMPLETION_MARGIN).unwrap_or(end));
            let runtime_result = match runtime.shutdown(runtime_deadline).await {
                Err(error)
                    if deadline_at.is_some()
                        && error.category() == EngineErrorCategory::DeadlineExceeded =>
                {
                    // 等待期限只结束等待；原负责人继续无期限完成已接收的清理。
                    runtime.shutdown(None).await
                }
                result => result,
            };
            let result = match runtime_result {
                Ok(()) => log_activity.end().await,
                Err(error) => Err(error),
            };
            if let Err(error) = result {
                progress.first_error.get_or_insert_with(|| error.clone());
                if !error.is_retryable() {
                    events.send(EngineEvent::Fatal {
                        error: error.clone(),
                    });
                }
                return ResourceReleaseOutcome::Failed { error };
            }
            let outcome = ResourceReleaseOutcome::Released {
                error: progress.first_error.clone(),
            };
            progress.completed = Some(outcome.clone());
            *state.lock().await = EngineState::Stopped;
            events.send(EngineEvent::StateChanged {
                state: EngineState::Stopped,
            });
            events.close();
            outcome
        })
    }
}

/// 关闭任务在等待方处异常退出；任务类别即完整分类，宿主只收到稳定错误码。
fn shutdown_task_failed(_: JoinError) -> EngineError {
    record_task_join_failure(DiagnosticTaskKind::EngineLifecycleTransition);
    EngineError::new(1108, EngineErrorCategory::Internal, true)
}

fn remaining_until(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}
