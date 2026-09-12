use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::{timeout_at, Instant};

use super::{operation_cancelled_error, Engine};
use crate::{EngineError, EngineErrorCategory, EngineEvent, EngineState, OperationTerminal};

const SHUTDOWN_COMPLETION_MARGIN: Duration = Duration::from_millis(100);

impl Engine {
    pub async fn shutdown(&self, deadline: Duration) -> Result<(), EngineError> {
        let deadline_at = Instant::now()
            .checked_add(deadline)
            .ok_or_else(|| EngineError::new(1003, EngineErrorCategory::InvalidInput, false))?;
        self.stop_requested.store(true, Ordering::Release);
        let shutdown_gate = Arc::clone(&self.shutdown_gate);
        let lifecycle_gate = Arc::clone(&self.lifecycle_gate);
        let state = Arc::clone(&self.state);
        let runtime = Arc::clone(&self.runtime);
        let events = self.events.clone();
        let operations = Arc::clone(&self.operations);
        // 排队也由执行任务持有；等待方离开不会撤销已接受的关闭意图。
        let task = tokio::spawn(async move {
            let _shutdown = shutdown_gate.lock().await;
            let lifecycle_guard = lifecycle_gate.lock().await;
            if *state.lock().await == EngineState::Stopped {
                return Ok(());
            }
            *state.lock().await = EngineState::ShuttingDown;
            events.send(EngineEvent::StateChanged {
                state: EngineState::ShuttingDown,
            });
            drop(lifecycle_guard);
            if !operations
                .wait_until_empty(remaining_until(deadline_at))
                .await
            {
                for operation_id in operations.cancel_all().await {
                    events.send(EngineEvent::OperationFinished {
                        operation_id,
                        terminal: OperationTerminal::Cancelled,
                    });
                }
            }
            // 取消通知不代表调用 future 已释放资源；任务持有等待，宿主预算只限制宿主等待。
            operations.wait_empty().await;
            let budget = remaining_until(deadline_at).saturating_sub(SHUTDOWN_COMPLETION_MARGIN);
            let result = runtime.shutdown(budget).await;
            if let Err(error) = &result {
                if !error.is_retryable() {
                    events.send(EngineEvent::Fatal {
                        error: error.clone(),
                    });
                }
                return result;
            }
            *state.lock().await = EngineState::Stopped;
            events.send(EngineEvent::StateChanged {
                state: EngineState::Stopped,
            });
            events.close();
            result
        });
        timeout_at(deadline_at, task)
            .await
            .map_err(|_| operation_cancelled_error())?
            .map_err(|_| EngineError::new(1108, EngineErrorCategory::Internal, true))?
    }
}

fn remaining_until(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}
