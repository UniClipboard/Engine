use std::future::Future;
use std::sync::Arc;

use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use tokio::sync::OnceCell;
use tracing::Instrument;
use uc_observability_contract::diagnostics::ObservationContext;

use crate::runtime_lifecycle::LifecycleError;

#[cfg(test)]
mod tests;

type ShutdownOutcome = Result<(), Arc<LifecycleError>>;

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
