mod queue;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{oneshot, Mutex};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_infra_local::fs::open_files::open_lockable_files;
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};
use uc_observability_contract::{uc_info, uc_warn};

use super::event_stream::EventSender;
use super::in_flight::InFlightOperations;
use super::log_activity::LogActivity;
use super::{invalid_state_error, operation_cancelled_error, Engine, EngineRuntime};
use crate::{
    EngineError, EngineErrorCategory, EngineEvent, EngineState, LifecycleAction, OperationTerminal,
    ResourceReleaseOutcome,
};

pub(super) use queue::TransitionQueue;

pub(super) enum Request {
    Quiesce(Instant),
    Suspend(Option<Instant>),
    Resume,
}

impl Request {
    pub(super) fn suspend_with_deadline(deadline: Duration) -> Result<Self, EngineError> {
        let deadline = Instant::now()
            .checked_add(deadline)
            .ok_or_else(|| EngineError::new(1003, EngineErrorCategory::InvalidInput, false))?;
        Ok(Self::Suspend(Some(deadline)))
    }
}

/// 持有宿主入口的状态发布与排空；参与者顺序仍由 Application 完整负责。
struct Transition {
    gate: Arc<Mutex<()>>,
    stop_requested: Arc<AtomicBool>,
    state: Arc<Mutex<EngineState>>,
    runtime: Arc<dyn EngineRuntime>,
    operations: Arc<InFlightOperations>,
    events: EventSender,
    log_activity: LogActivity,
    file_roots: Arc<[PathBuf]>,
}

impl Engine {
    pub async fn quiesce(&self, deadline: Duration) -> Result<(), EngineError> {
        let deadline = Instant::now()
            .checked_add(deadline)
            .ok_or_else(|| EngineError::new(1003, EngineErrorCategory::InvalidInput, false))?;
        self.transition(Request::Quiesce(deadline)).await
    }

    pub async fn suspend(&self) -> Result<(), EngineError> {
        self.transition(Request::Suspend(None)).await
    }

    /// 期限包含排队；超时只结束本次等待，实际收尾继续，不能据此认定已安全暂停。
    pub async fn suspend_with_deadline(&self, deadline: Duration) -> Result<(), EngineError> {
        self.transition(Request::suspend_with_deadline(deadline)?)
            .await
    }

    pub async fn resume(&self) -> Result<(), EngineError> {
        self.transition(Request::Resume).await
    }

    /// 关闭已接管生命周期时，等待同一实例的关闭结果。
    pub async fn suspend_with_outcome(&self, deadline: Duration) -> ResourceReleaseOutcome {
        if self.stop_requested.load(Ordering::Acquire) {
            return self.shutdown_with_outcome(deadline).await;
        }
        match self.suspend_with_deadline(deadline).await {
            Ok(()) => ResourceReleaseOutcome::Released { error: None },
            Err(error) if error.category() == EngineErrorCategory::DeadlineExceeded => {
                ResourceReleaseOutcome::Pending
            }
            Err(error) => ResourceReleaseOutcome::Failed { error },
        }
    }

    async fn transition(&self, request: Request) -> Result<(), EngineError> {
        submit(&self.lifecycle_requests, request).await
    }

    pub(super) fn bind_lifecycle(&self, announce_start: bool) -> oneshot::Receiver<()> {
        let transition = Transition {
            gate: Arc::clone(&self.lifecycle_gate),
            stop_requested: Arc::clone(&self.stop_requested),
            state: Arc::clone(&self.state),
            runtime: Arc::clone(&self.runtime),
            operations: Arc::clone(&self.operations),
            events: self.events.clone(),
            log_activity: self.log_activity.clone(),
            file_roots: Arc::clone(&self.file_roots),
        };
        self.lifecycle_requests.bind(transition, announce_start)
    }
}

pub(super) async fn submit(
    requests: &Arc<TransitionQueue>,
    request: Request,
) -> Result<(), EngineError> {
    let deadline = match request {
        Request::Suspend(deadline) => deadline,
        _ => None,
    };
    // 接受请求后，启动排队、执行和状态发布都不依赖调用方继续等待。
    let completion = requests.enqueue(request);
    let result = match deadline {
        Some(deadline) => timeout_at(deadline, completion)
            .await
            // discarded-source[timeout]: `tokio::time::error::Elapsed`: the timeout itself is the classification
            .map_err(|_| operation_cancelled_error())?,
        None => completion.await,
    };
    // discarded-source[channel]: `tokio::sync::oneshot::error::RecvError`: the error only means the peer is gone or carries the unsent payload, which must not outlive it
    result.map_err(|_| EngineError::new(1108, EngineErrorCategory::Internal, true))?
}

impl Transition {
    async fn execute(
        &self,
        request: Request,
        cancellation: CancellationToken,
        requests: &TransitionQueue,
    ) -> Result<(), EngineError> {
        let _gate = self.gate.lock().await;
        if self.stop_requested.load(Ordering::Acquire) {
            return Err(invalid_state_error());
        }
        match request {
            Request::Quiesce(deadline) => {
                self.quiesce(deadline.saturating_duration_since(Instant::now()))
                    .await
            }
            Request::Suspend(deadline) => self.suspend(deadline).await,
            Request::Resume => self.resume(cancellation, requests).await,
        }
    }

    async fn suspend(&self, deadline: Option<Instant>) -> Result<(), EngineError> {
        let lifecycle = *self.state.lock().await;
        match lifecycle {
            EngineState::Running => self.quiesce(Duration::ZERO).await?,
            EngineState::Quiesced => {}
            EngineState::Suspended => return Ok(()),
            _ => return Err(invalid_state_error()),
        }
        // 等待期限只约束调用方；已接受的暂停必须在真实读写结束后继续收尾。
        self.operations.wait_empty().await;
        let result = match self.runtime.suspend(deadline).await {
            // 期限内未完成的参与者调用已按共同期限结束；由本次已接受的暂停再完整执行一次，
            // 等在途工作实际退出后交还本地资源，宿主无需剩余时间发出第二次请求。
            Err(error)
                if deadline.is_some()
                    && error.category() == EngineErrorCategory::DeadlineExceeded =>
            {
                self.runtime.suspend(None).await
            }
            result => result,
        };
        self.report_result(LifecycleAction::Suspend, result)?;
        // 进程网络日志在发布暂停前交还文件：宿主读到 Suspended 即可结束后台任务。
        self.log_activity.end().await?;
        self.record_open_files().await;
        self.publish(EngineState::Suspended).await;
        Ok(())
    }

    async fn resume(
        &self,
        cancellation: CancellationToken,
        requests: &TransitionQueue,
    ) -> Result<(), EngineError> {
        let state = *self.state.lock().await;
        match state {
            EngineState::Running => return Ok(()),
            EngineState::Suspended | EngineState::Quiesced => {}
            _ => return Err(invalid_state_error()),
        }
        if cancellation.is_cancelled() {
            return self.report_result(LifecycleAction::Resume, Err(invalid_state_error()));
        }
        // 恢复开始后旧暂停证明已失效；失败时仍关闭入口，并允许完整负责人重试收尾。
        if state == EngineState::Suspended {
            self.publish(EngineState::Quiesced).await;
        }
        // 先重开网络日志，恢复中的网络启动记录才能落盘；恢复失败时保持登记，直到下一次暂停成功交还。
        self.log_activity.begin().await?;
        self.report_result(
            LifecycleAction::Resume,
            self.runtime.resume(cancellation.clone()).await,
        )?;
        let mut state = self.state.lock().await;
        let result = requests.publish_resume(
            &cancellation,
            &self.stop_requested,
            &mut state,
            &self.events,
        );
        drop(state);
        self.report_result(LifecycleAction::Resume, result)
    }

    async fn quiesce(&self, budget: Duration) -> Result<(), EngineError> {
        match *self.state.lock().await {
            EngineState::Quiesced => return Ok(()),
            EngineState::Running => {}
            _ => return Err(invalid_state_error()),
        }
        self.publish(EngineState::Quiescing).await;
        if !self.operations.wait_until_empty(budget).await {
            for operation_id in self.operations.cancel_all().await {
                self.events.send(EngineEvent::OperationFinished {
                    operation_id,
                    terminal: OperationTerminal::Cancelled,
                });
            }
        }
        self.publish(EngineState::Quiesced).await;
        Ok(())
    }

    /// 暂停成功的核对记录：宿主目录内仍被本进程打开的数据库与租约文件数，预期为零。
    /// 只含计数；无法枚举描述符的平台不写计数。宿主自己的文件（例如运行期所有权锁）不在宿主目录内时不计入。
    async fn record_open_files(&self) {
        let roots = Arc::clone(&self.file_roots);
        let census = match tokio::task::spawn_blocking(move || open_lockable_files(&roots)).await {
            Ok(census) => census,
            // discarded-source[contract-boundary]: `tokio::task::JoinError`: the task kind is the full classification; the record then carries no counts
            Err(_) => {
                record_task_join_failure(DiagnosticTaskKind::EngineLifecycleTransition);
                None
            }
        };
        let databases = census.map(|census| census.databases);
        let leases = census.map(|census| census.leases);
        if census.is_some_and(|census| census.total() > 0) {
            uc_warn!(
                open_database_files = databases,
                open_lease_files = leases,
                "engine suspended while local files remain open"
            );
        } else {
            uc_info!(
                open_database_files = databases,
                open_lease_files = leases,
                "engine suspended with local files released"
            );
        }
    }

    async fn publish(&self, state: EngineState) {
        *self.state.lock().await = state;
        self.events.send(EngineEvent::StateChanged { state });
    }

    fn report_result(
        &self,
        action: LifecycleAction,
        result: Result<(), EngineError>,
    ) -> Result<(), EngineError> {
        if let Err(error) = &result {
            self.events.send(EngineEvent::LifecycleFailed {
                action,
                error: error.clone(),
            });
        }
        result
    }
}
