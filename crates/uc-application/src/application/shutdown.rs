use std::error::Error as StdError;
use std::future::Future;
use std::sync::Arc;

use anyhow::Error as SourceError;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio::sync::OnceCell;
use tokio::task::JoinHandle;
use tracing::Instrument;
use uc_observability_contract::diagnostics::ObservationContext;

use crate::runtime_lifecycle::LifecycleError;

mod owners;
#[cfg(test)]
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
                // 先启动真实清理；共享 future 只保存结果，不承担驱动清理的责任。
                let task = tokio::spawn(observation.scope(cleanup.in_current_span()));
                async move {
                    task.await
                        .map_err(|source| {
                            Arc::new(LifecycleError {
                                primary: source.into(),
                                additional: Vec::new(),
                            })
                        })?
                        .map_err(Arc::new)
                }
                .boxed()
                .shared()
            })
            .await
            .clone()
            .await
    }
}
