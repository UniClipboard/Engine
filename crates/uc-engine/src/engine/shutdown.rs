use std::error::Error;
use std::future::pending;
use std::io;
#[cfg(feature = "dev-tools")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use tokio::task::{JoinError, JoinHandle};
use tokio::time::{timeout_at, Instant};
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_observability_runtime::{
    confirm_engine_closed, retain_engine_closer, update_engine_closer, RetainedEngineCloser,
    RetainedEngineResult, RetainedEngineState, RetainedEngineToken,
};

use super::owned_executor::ExecutorStop;
use super::Engine;
use crate::{
    EngineError, EngineErrorCategory, EngineEvent, EngineState, OperationTerminal,
    ResourceReleaseOutcome,
};

const SHUTDOWN_COMPLETION_MARGIN: Duration = Duration::from_millis(100);
#[cfg(feature = "dev-tools")]
pub(super) static PROBE_ORPHAN_RETRY_EXECUTOR_FAILURE: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
pub(super) struct ShutdownProgress {
    first_error: Option<EngineError>,
    completed: Option<ResourceReleaseOutcome>,
}

#[derive(Default)]
pub(super) struct OrphanRetention {
    state: StdMutex<OrphanState>,
}

#[derive(Default)]
struct OrphanState {
    completed: bool,
    token: Option<RetainedEngineToken>,
    in_flight: usize,
    terminal: bool,
    last_error: Option<Arc<EngineError>>,
}

impl OrphanRetention {
    pub(super) fn retain(&self, engine: &Engine) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if (state.completed && !state.terminal && state.in_flight == 0) || state.token.is_some() {
            return;
        }
        let closer = Arc::new(OrphanedEngine {
            engine: engine.retained_copy(),
            _worker_error: StdMutex::new(None),
        });
        let status = if state.in_flight > 0 {
            RetainedEngineState::Pending
        } else if state.terminal {
            RetainedEngineState::Terminal
        } else {
            RetainedEngineState::Retryable
        };
        state.token = Some(retain_engine_closer(closer, status));
    }

    fn begin(self: &Arc<Self>) -> ShutdownAttempt {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.in_flight = state.in_flight.saturating_add(1);
        if let Some(token) = &state.token {
            update_engine_closer(token, RetainedEngineState::Pending);
        }
        ShutdownAttempt {
            owner: Arc::clone(self),
            settled: false,
            started: false,
        }
    }

    fn is_terminal(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .terminal
    }

    fn finish(&self, outcome: AttemptOutcome) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let last = state.in_flight == 1;
        state.in_flight = state.in_flight.saturating_sub(1);
        match outcome {
            AttemptOutcome::Released => state.completed = true,
            AttemptOutcome::Failed(error) => state.last_error = Some(error),
            AttemptOutcome::Terminated => state.terminal = true,
            AttemptOutcome::AbandonedWaiter if last && !state.completed => state.terminal = true,
            AttemptOutcome::AbandonedWaiter => {}
        }
        if state.completed && !state.terminal && state.in_flight == 0 {
            if let Some(token) = state.token.take() {
                confirm_engine_closed(&token);
            }
        } else if let Some(token) = &state.token {
            let status = if state.in_flight > 0 {
                RetainedEngineState::Pending
            } else if state.terminal {
                RetainedEngineState::Terminal
            } else {
                RetainedEngineState::Retryable
            };
            update_engine_closer(token, status);
        }
    }
}

enum AttemptOutcome {
    Released,
    Failed(Arc<EngineError>),
    Terminated,
    AbandonedWaiter,
}

struct ShutdownAttempt {
    owner: Arc<OrphanRetention>,
    settled: bool,
    started: bool,
}

impl ShutdownAttempt {
    fn settle(&mut self, outcome: AttemptOutcome) {
        self.owner.finish(outcome);
        self.settled = true;
    }
}

impl Drop for ShutdownAttempt {
    fn drop(&mut self) {
        if !self.settled {
            self.owner.finish(if self.started {
                AttemptOutcome::Terminated
            } else {
                AttemptOutcome::AbandonedWaiter
            });
        }
    }
}

struct OrphanedEngine {
    engine: Engine,
    _worker_error: StdMutex<Option<Arc<io::Error>>>,
}

impl RetainedEngineCloser for OrphanedEngine {
    fn errors(&self) -> Vec<Arc<dyn Error + Send + Sync>> {
        if let Some(error) = self
            ._worker_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return vec![error as Arc<dyn Error + Send + Sync>];
        }
        self.engine
            .orphan_retention
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last_error
            .clone()
            .map(|error| vec![error as Arc<dyn Error + Send + Sync>])
            .unwrap_or_default()
    }
    fn wait_accepted_work(&self) {
        self.engine.runtime.wait_shutdown_obligations_blocking();
        if let Some(executor) = &self.engine.owned_executor {
            executor.wait_if_stopping(Duration::MAX);
        }
    }

    fn retry_close(&self) -> RetainedEngineResult {
        let build = || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
        };
        #[cfg(feature = "dev-tools")]
        let created = if PROBE_ORPHAN_RETRY_EXECUTOR_FAILURE.swap(false, Ordering::SeqCst) {
            Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "controlled retry executor failure",
                ),
            ))
        } else {
            build()
        };
        #[cfg(not(feature = "dev-tools"))]
        let created = build();
        let runtime = match created {
            Ok(runtime) => runtime,
            Err(source) => {
                *self
                    ._worker_error
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(source));
                return RetainedEngineResult::TerminalFailure;
            }
        };
        runtime.block_on(async {
            let (completed, failed) = {
                let progress = self.engine.shutdown_progress.lock().await;
                (progress.completed.is_some(), progress.first_error.is_some())
            };
            if self.engine.orphan_retention.is_terminal() {
                return RetainedEngineResult::TerminalFailure;
            }
            if self.engine.owned_executor.as_ref().is_some_and(|executor| {
                executor.wait_if_stopping(Duration::MAX) == Some(ExecutorStop::Failed)
            }) {
                return RetainedEngineResult::TerminalFailure;
            }
            if completed {
                return RetainedEngineResult::Released;
            }
            if failed && !self.engine.runtime.supports_shutdown_retry().await {
                return RetainedEngineResult::TerminalFailure;
            }
            match self.engine.shutdown_until_released().await {
                ResourceReleaseOutcome::Released { .. } => RetainedEngineResult::Released,
                _ if self.engine.runtime.supports_shutdown_retry().await => {
                    RetainedEngineResult::RetryableFailure
                }
                _ => RetainedEngineResult::TerminalFailure,
            }
        })
    }
}

impl Engine {
    fn retained_copy(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            lifecycle_gate: Arc::clone(&self.lifecycle_gate),
            lifecycle_requests: Arc::clone(&self.lifecycle_requests),
            shutdown_gate: Arc::clone(&self.shutdown_gate),
            shutdown_progress: Arc::clone(&self.shutdown_progress),
            orphan_retention: Arc::clone(&self.orphan_retention),
            retain_on_drop: false,
            stop_requested: Arc::clone(&self.stop_requested),
            runtime: Arc::clone(&self.runtime),
            owned_executor: self.owned_executor.clone(),
            events: self.events.clone(),
            operations: Arc::clone(&self.operations),
            log_activity: self.log_activity.clone(),
            file_roots: Arc::clone(&self.file_roots),
        }
    }

    async fn wait_shutdown_obligations(&self) {
        self.runtime.wait_shutdown_obligations().await;
        if let Some(executor) = self.owned_executor.clone() {
            let _ =
                tokio::task::spawn_blocking(move || executor.wait_if_stopping(Duration::MAX)).await;
        }
    }

    pub async fn shutdown_until_complete(&self) -> Result<(), EngineError> {
        match self.shutdown_until_released().await {
            ResourceReleaseOutcome::Released { .. } => Ok(()),
            outcome => outcome.into_result(),
        }
    }

    pub async fn shutdown_until_released(&self) -> ResourceReleaseOutcome {
        match self.start_shutdown(None, None).await {
            Ok(outcome) => outcome,
            Err(error) => {
                self.wait_shutdown_obligations().await;
                ResourceReleaseOutcome::Failed {
                    error: shutdown_task_failed(error),
                }
            }
        }
    }

    /// 接收一次关闭，并保留在途操作的等待期限，直到该次关闭结束。
    pub async fn shutdown_until_released_with_grace(
        &self,
        grace: Duration,
    ) -> ResourceReleaseOutcome {
        let Some(deadline) = Instant::now().checked_add(grace) else {
            return ResourceReleaseOutcome::Failed {
                error: EngineError::new(1003, EngineErrorCategory::InvalidInput, false),
            };
        };
        match self.start_shutdown(Some(deadline), None).await {
            Ok(outcome) => outcome,
            Err(error) => {
                self.wait_shutdown_obligations().await;
                ResourceReleaseOutcome::Failed {
                    error: shutdown_task_failed(error),
                }
            }
        }
    }

    /// 关闭工作线程终止后，等待已接收的工作，再返回未确认的关闭结果。
    pub async fn settle_terminated_shutdown(&self) -> ResourceReleaseOutcome {
        self.lifecycle_requests.wait_empty().await;
        self.operations.wait_empty().await;
        self.wait_shutdown_obligations().await;
        if let Some(executor) = self.owned_executor.clone() {
            if tokio::task::spawn_blocking(move || executor.stop(Duration::MAX))
                .await
                .is_err()
            {
                pending::<()>().await;
            }
        }
        ResourceReleaseOutcome::Failed {
            error: EngineError::new(1108, EngineErrorCategory::Internal, false),
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
        let attempt = self.start_shutdown(Some(deadline_at), Some(deadline_at));
        match timeout_at(deadline_at, async {
            match attempt.await {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.wait_shutdown_obligations().await;
                    ResourceReleaseOutcome::Failed {
                        error: shutdown_task_failed(error),
                    }
                }
            }
        })
        .await
        {
            Ok(outcome) => outcome,
            Err(_) => ResourceReleaseOutcome::Pending,
        }
    }

    pub(super) fn start_shutdown(
        &self,
        deadline_at: Option<Instant>,
        cleanup_deadline: Option<Instant>,
    ) -> JoinHandle<ResourceReleaseOutcome> {
        self.lifecycle_requests
            .accept_shutdown(&self.stop_requested);
        let shutdown_gate = Arc::clone(&self.shutdown_gate);
        let progress = Arc::clone(&self.shutdown_progress);
        let lifecycle_gate = Arc::clone(&self.lifecycle_gate);
        let lifecycle_requests = Arc::clone(&self.lifecycle_requests);
        let state = Arc::clone(&self.state);
        let runtime = Arc::clone(&self.runtime);
        let owned_executor = self.owned_executor.clone();
        let events = self.events.clone();
        let operations = Arc::clone(&self.operations);
        let log_activity = self.log_activity.clone();
        let attempt = self.orphan_retention.begin();
        tokio::spawn(async move {
            let _shutdown = shutdown_gate.lock().await;
            let mut attempt = attempt;
            attempt.started = true;
            if attempt.owner.is_terminal() {
                runtime.wait_shutdown_obligations().await;
                if let Some(executor) = owned_executor.clone() {
                    let _ = tokio::task::spawn_blocking(move || {
                        executor.wait_if_stopping(Duration::MAX)
                    })
                    .await;
                }
                let error = EngineError::new(1108, EngineErrorCategory::Internal, false);
                attempt.settle(AttemptOutcome::Failed(Arc::new(error.clone())));
                return ResourceReleaseOutcome::Failed { error };
            }
            let mut progress = progress.lock().await;
            if let Some(outcome) = &progress.completed {
                attempt.settle(AttemptOutcome::Released);
                return outcome.clone();
            }
            if let Some(executor) = owned_executor.clone() {
                let status =
                    tokio::task::spawn_blocking(move || executor.wait_if_stopping(Duration::MAX))
                        .await;
                if !matches!(status, Ok(None)) {
                    // 执行器已停止或终止后，不能重新执行其仍依赖的关闭器。
                    attempt.settle(AttemptOutcome::Terminated);
                    return ResourceReleaseOutcome::Failed {
                        error: EngineError::new(1108, EngineErrorCategory::Internal, false),
                    };
                }
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
            let runtime_deadline = cleanup_deadline
                .map(|end| end.checked_sub(SHUTDOWN_COMPLETION_MARGIN).unwrap_or(end));
            let runtime_result = match runtime.shutdown(runtime_deadline).await {
                Err(error)
                    if cleanup_deadline.is_some()
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
                attempt.settle(AttemptOutcome::Failed(Arc::new(error.clone())));
                return ResourceReleaseOutcome::Failed { error };
            }
            if let Some(executor) = owned_executor {
                let stopped =
                    tokio::task::spawn_blocking(move || executor.stop(Duration::MAX)).await;
                if !matches!(stopped, Ok(ExecutorStop::Stopped)) {
                    let error = EngineError::new(1108, EngineErrorCategory::Internal, false);
                    progress.first_error.get_or_insert_with(|| error.clone());
                    attempt.settle(AttemptOutcome::Failed(Arc::new(error.clone())));
                    return ResourceReleaseOutcome::Failed { error };
                }
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
            attempt.settle(AttemptOutcome::Released);
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
