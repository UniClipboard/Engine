use std::error::Error as StdError;
use std::future::Future;
use std::io;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use anyhow::Error as SourceError;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio::sync::{oneshot, OnceCell};
use tokio::task::JoinHandle;
use tracing::Instrument;
use uc_observability_contract::diagnostics::ObservationContext;

use crate::runtime_lifecycle::LifecycleError;

#[path = "shutdown/owners.rs"]
mod owners;
#[cfg(test)]
#[path = "shutdown/tests.rs"]
mod tests;

type ShutdownOutcome = Result<(), Arc<LifecycleError>>;

struct ShutdownTask {
    action: &'static str,
    handle: JoinHandle<Result<(), SourceError>>,
}

fn begin<E>(
    action: &'static str,
    cleanup: impl Future<Output = Result<(), E>> + Send + 'static,
) -> ShutdownTask
where
    E: StdError + Send + Sync + 'static,
{
    let observation = ObservationContext::capture();
    let handle = tokio::spawn(
        observation.scope(async move { cleanup.await.map_err(SourceError::new) }.in_current_span()),
    );
    ShutdownTask { action, handle }
}

async fn finish(tasks: Vec<ShutdownTask>) -> Result<(), LifecycleError> {
    let mut errors = Vec::new();
    for task in tasks {
        let outcome = task
            .handle
            .await
            .map_err(SourceError::new)
            .and_then(|result| result);
        if let Err(source) = outcome {
            errors.push(source.context(task.action));
        }
    }
    LifecycleError::from_errors(errors)
}

#[derive(Debug, thiserror::Error)]
enum ShutdownWorkerError {
    #[error("application shutdown worker creation failed")]
    Thread(#[source] io::Error),
    #[error("application shutdown executor creation failed")]
    Executor(#[source] io::Error),
    #[error("application shutdown worker panicked")]
    Panicked,
    #[error("application shutdown worker result lost")]
    ResultLost(#[source] oneshot::error::RecvError),
}

fn worker_error(error: ShutdownWorkerError) -> Arc<LifecycleError> {
    Arc::new(LifecycleError {
        primary: error.into(),
        additional: Vec::new(),
    })
}

#[derive(Default)]
pub(super) struct ApplicationShutdown {
    result: OnceCell<Shared<BoxFuture<'static, ShutdownOutcome>>>,
}

impl ApplicationShutdown {
    pub(super) async fn run(
        &self,
        cleanup: impl Future<Output = Result<(), LifecycleError>> + Send + 'static,
    ) -> ShutdownOutcome {
        self.result
            .get_or_init(|| async move {
                let observation = ObservationContext::capture();
                let cleanup = observation.scope(cleanup.in_current_span());
                let (sender, receiver) = oneshot::channel();
                // 已接收的关闭工作不能依赖提交方执行器的生命周期。
                let spawned = std::thread::Builder::new()
                    .name("uc-application-shutdown".to_owned())
                    .spawn(move || {
                        let result = catch_unwind(AssertUnwindSafe(|| {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(|source| {
                                    worker_error(ShutdownWorkerError::Executor(source))
                                })?;
                            let result = runtime.block_on(async move {
                                let task = tokio::spawn(cleanup);
                                task.await
                                    .map_err(|source| {
                                        Arc::new(LifecycleError {
                                            primary: source.into(),
                                            additional: Vec::new(),
                                        })
                                    })?
                                    .map_err(Arc::new)
                            });
                            drop(runtime);
                            result
                        }))
                        .unwrap_or_else(|_| Err(worker_error(ShutdownWorkerError::Panicked)));
                        let _ = sender.send(result);
                    });
                async move {
                    if let Err(source) = spawned {
                        return Err(worker_error(ShutdownWorkerError::Thread(source)));
                    }
                    receiver
                        .await
                        .map_err(|source| worker_error(ShutdownWorkerError::ResultLost(source)))?
                }
                .boxed()
                .shared()
            })
            .await
            .clone()
            .await
    }
}
