use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, Span};

use super::{
    invalid_state_error, operation_cancelled_error, terminal_for_result, Engine, EngineRuntime,
};
#[cfg(feature = "dev-tools")]
use crate::{DevOperation, DevOperationResult};
use crate::{
    EngineError, EngineErrorCategory, EngineEvent, Operation, OperationResult, OperationTerminal,
};

impl Engine {
    pub async fn execute(&self, operation: Operation) -> Result<OperationResult, EngineError> {
        self.run_operation("operation", move |runtime, cancellation| async move {
            runtime.execute(operation, cancellation).await
        })
        .await
    }

    #[cfg(feature = "dev-tools")]
    pub async fn execute_dev(
        &self,
        operation: DevOperation,
    ) -> Result<DevOperationResult, EngineError> {
        self.run_operation("dev-operation", move |runtime, cancellation| async move {
            runtime.execute_dev(operation, cancellation).await
        })
        .await
    }

    async fn run_operation<T, F, Fut>(&self, prefix: &str, invoke: F) -> Result<T, EngineError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<dyn EngineRuntime>, CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, EngineError>> + Send + 'static,
    {
        self.lifecycle_requests.check_admission()?;
        let registered = {
            let _lifecycle = self.lifecycle_gate.lock().await;
            if self.stop_requested.load(Ordering::Acquire)
                || !self.state.lock().await.accepts_operations()
            {
                return Err(invalid_state_error());
            }
            self.lifecycle_requests
                .register_operation(&self.operations, prefix)?
        };
        let cancellation = registered.cancellation.clone();
        let _cancel_on_waiter_drop = cancellation.clone().drop_guard();
        let runtime = Arc::clone(&self.runtime);
        let operations = Arc::clone(&self.operations);
        let events = self.events.clone();
        // 登记许可由执行方保留；取消等待不能让仍在工作的磁盘线程从排空清单消失。
        let task = tokio::spawn(
            async move {
                let work_cancel = registered.cancellation.clone();
                let result = tokio::spawn(
                    async move {
                        if work_cancel.is_cancelled() {
                            return Err(operation_cancelled_error());
                        }
                        invoke(runtime, work_cancel).await
                    }
                    .instrument(Span::current()),
                )
                .await
                .unwrap_or_else(|_| {
                    Err(EngineError::new(1108, EngineErrorCategory::Internal, true))
                });
                let terminal = if registered.cancellation.is_cancelled() {
                    OperationTerminal::Cancelled
                } else {
                    terminal_for_result(&result)
                };
                if operations.finish(&registered.id).await {
                    events.send(EngineEvent::OperationFinished {
                        operation_id: registered.id.clone(),
                        terminal,
                    });
                }
                result
            }
            .instrument(Span::current()),
        );
        await_operation_completion(cancellation, task).await
    }
}

pub(super) async fn await_operation_completion<T>(
    cancellation: CancellationToken,
    task: JoinHandle<Result<T, EngineError>>,
) -> Result<T, EngineError> {
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(operation_cancelled_error()),
        result = task => result.map_err(|_| EngineError::new(1108, EngineErrorCategory::Internal, true))?,
    }
}
