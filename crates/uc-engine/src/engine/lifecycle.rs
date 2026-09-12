use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::{timeout_at, Instant};

use super::event_stream::EventSender;
use super::in_flight::InFlightOperations;
use super::{invalid_state_error, operation_cancelled_error, Engine, EngineRuntime};
use crate::{
    EngineError, EngineErrorCategory, EngineEvent, EngineState, LifecycleAction, OperationTerminal,
};

const SUSPEND_OPERATION_DRAIN: Duration = Duration::from_secs(2);

enum Request {
    Quiesce(Instant),
    Suspend(Option<Instant>),
    Resume,
}

/// 持有宿主入口的状态发布与排空；参与者顺序仍由 Application 完整负责。
struct Transition {
    gate: Arc<Mutex<()>>,
    state: Arc<Mutex<EngineState>>,
    runtime: Arc<dyn EngineRuntime>,
    operations: Arc<InFlightOperations>,
    events: EventSender,
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
        let deadline = Instant::now()
            .checked_add(deadline)
            .ok_or_else(|| EngineError::new(1003, EngineErrorCategory::InvalidInput, false))?;
        self.transition(Request::Suspend(Some(deadline))).await
    }

    pub async fn resume(&self) -> Result<(), EngineError> {
        self.transition(Request::Resume).await
    }

    async fn transition(&self, request: Request) -> Result<(), EngineError> {
        let deadline = match request {
            Request::Suspend(deadline) => deadline,
            _ => None,
        };
        let transition = Transition {
            gate: Arc::clone(&self.lifecycle_gate),
            state: Arc::clone(&self.state),
            runtime: Arc::clone(&self.runtime),
            operations: Arc::clone(&self.operations),
            events: self.events.clone(),
        };
        // 接受请求后，排队、执行和状态发布都不依赖调用方继续等待。
        let task = tokio::spawn(async move { transition.execute(request).await });
        let result = match deadline {
            Some(deadline) => timeout_at(deadline, task)
                .await
                .map_err(|_| operation_cancelled_error())?,
            None => task.await,
        };
        result.map_err(|_| EngineError::new(1108, EngineErrorCategory::Internal, true))?
    }
}

impl Transition {
    async fn execute(&self, request: Request) -> Result<(), EngineError> {
        let _gate = self.gate.lock().await;
        match request {
            Request::Quiesce(deadline) => {
                self.quiesce(deadline.saturating_duration_since(Instant::now()))
                    .await
            }
            Request::Suspend(deadline) => self.suspend(deadline).await,
            Request::Resume => self.resume().await,
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
        let budget = deadline.map_or(SUSPEND_OPERATION_DRAIN, |deadline| {
            deadline
                .saturating_duration_since(Instant::now())
                .min(SUSPEND_OPERATION_DRAIN)
        });
        let result = if self.operations.wait_until_empty(budget).await {
            self.runtime.suspend(deadline).await
        } else {
            Err(operation_cancelled_error())
        };
        self.report_result(LifecycleAction::Suspend, result)?;
        self.publish(EngineState::Suspended).await;
        Ok(())
    }

    async fn resume(&self) -> Result<(), EngineError> {
        match *self.state.lock().await {
            EngineState::Running => return Ok(()),
            EngineState::Suspended => {}
            _ => return Err(invalid_state_error()),
        }
        self.report_result(LifecycleAction::Resume, self.runtime.resume().await)?;
        self.publish(EngineState::Running).await;
        Ok(())
    }

    async fn quiesce(&self, budget: Duration) -> Result<(), EngineError> {
        if *self.state.lock().await != EngineState::Running {
            return Err(invalid_state_error());
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
