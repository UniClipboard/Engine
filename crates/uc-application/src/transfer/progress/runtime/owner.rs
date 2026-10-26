use super::events::observe_events;
use super::receive::observe_receive;
use crate::connectivity::ConnectivityError;
use crate::facade::host_event::HostEventBus;
use crate::transfer::progress::{
    ProgressExchangeService, ProgressExchangeWorker, ProgressProjectionRegistry,
    ProgressProjectionWorker, ProgressReceiveError, TransferProgressReconciler,
    TransferProgressTransport,
};
use crate::transfer::receive::work::ReceiveWorkRegistry;
use futures::future::{BoxFuture, Shared};
use futures::FutureExt;
use std::sync::Arc;
use tokio::task::{JoinError, JoinSet};
use tokio_util::sync::CancellationToken;
use uc_observability_contract::diagnostics::{
    record_task_join_failure, DiagnosticTaskKind, ObservationContext,
};

/// 仅Application内部持有完整领域能力；宿主和Engine不取得投影或业务步骤。
#[derive(Clone)]
pub(crate) struct ProgressSession {
    pub projections: ProgressProjectionRegistry,
    pub exchanges: ProgressExchangeService,
    pub reconciliation: Arc<TransferProgressReconciler>,
    pub inbound: Arc<TransferProgressTransport>,
}
type Completion = Shared<BoxFuture<'static, Result<(), Arc<ProgressReceiveError>>>>;

pub(crate) struct ProgressRuntime {
    stop: CancellationToken,
    completion: Completion,
}
#[derive(Debug, thiserror::Error)]
#[error("progress runtime worker exited before closure")]
struct UnexpectedWorkerExit;

impl ProgressRuntime {
    pub(super) fn start(
        session: ProgressSession,
        projection: ProgressProjectionWorker,
        exchange: ProgressExchangeWorker,
        receipts: Arc<ReceiveWorkRegistry>,
        host_events: Arc<HostEventBus>,
    ) -> Self {
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let inbound = session.inbound.clone();
        let exchanges = session.exchanges.clone();
        let events = session.projections.subscribe();
        let observation = ObservationContext::capture();
        let task = tokio::spawn(observation.scope(async move {
            let mut jobs = JoinSet::new();
            jobs.spawn(projection.run(stopping.clone()));
            jobs.spawn(exchange.run(stopping.clone()));
            jobs.spawn(observe_receive(receipts, exchanges, stopping.clone()));
            jobs.spawn(observe_events(events, host_events, stopping.clone()));
            let mut failure = None;
            tokio::select! {
                biased;
                _ = stopping.cancelled() => {},
                completed = jobs.join_next() => {
                    stopping.cancel();
                    match completed {
                        Some(Err(source)) => retain_failure(&mut failure, task_failure(source)),
                        Some(Ok(())) | None => retain_failure(&mut failure, ProgressReceiveError::Exchange(Box::new(UnexpectedWorkerExit))),
                    }
                }
            }
            // worker封口、实际网络关闭和本地采用共同结算；不因调用方停止等待而丢弃任务。
            let (network, ()) = tokio::join!(inbound.shutdown(), async {
                while let Some(result) = jobs.join_next().await {
                    if let Err(source) = result { retain_failure(&mut failure, task_failure(source)); }
                }
            });
            if let Err(source) = network { retain_failure(&mut failure, source); }
            match failure { Some(source) => Err(source), None => Ok(()) }
        }));
        let completion = async move {
            match task.await {
                Ok(result) => result.map_err(Arc::new),
                Err(source) => Err(Arc::new(task_failure(source))),
            }
        }
        .boxed()
        .shared();
        Self { stop, completion }
    }
    pub(crate) async fn shutdown(&self) -> Result<(), Arc<ProgressReceiveError>> {
        self.stop.cancel();
        self.completion.clone().await
    }
}
impl Drop for ProgressRuntime {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
fn task_failure(source: JoinError) -> ProgressReceiveError {
    record_task_join_failure(DiagnosticTaskKind::TransferProgressConnectivityWorker);
    ConnectivityError::Task(Arc::new(source)).into()
}
fn retain_failure(failure: &mut Option<ProgressReceiveError>, additional: ProgressReceiveError) {
    *failure = Some(match failure.take() {
        Some(primary) => ProgressReceiveError::Settlement {
            primary: Box::new(primary),
            additional: Box::new(additional),
        },
        None => additional,
    });
}
