//! 完整下载工作归同一负责人持有；调用等待退出只发送停止意图。
use super::super::{BlobExchangeError, BlobExchangeFailure};
use super::owner::{task_error, BlobTransport};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, PoisonError};
use std::task::{Context, Poll};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;
use tracing::instrument::WithSubscriber;
use tracing::{Instrument, Span};
use uc_observability_contract::diagnostics::ObservationContext;

pub(in super::super::super::super) type DownloadCompletion =
    watch::Receiver<Option<Result<(), Arc<BlobExchangeError>>>>;
pub(in super::super::super::super) struct DownloadTask<T> {
    cancel: CancellationToken,
    result: oneshot::Receiver<Result<T, Arc<BlobExchangeError>>>,
    completion: DownloadCompletion,
}
impl<T> DownloadTask<T> {
    pub fn completion(&self) -> DownloadCompletion {
        self.completion.clone()
    }
    pub fn rejected(error: BlobExchangeError, cancel: CancellationToken) -> Self {
        let error = Arc::new(error);
        let (send, result) = oneshot::channel();
        let (_, completion) = watch::channel(Some(Err(error.clone())));
        // discarded-source[channel]: 同步拒绝的结果只交给本次等待方，不启动任务。
        let _ = send.send(Err(error));
        Self {
            cancel,
            result,
            completion,
        }
    }
}
impl<T> Drop for DownloadTask<T> {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl<T> Future for DownloadTask<T> {
    type Output = Result<T, BlobExchangeError>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().result).poll(cx).map(|result| {
            result
                .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Settlement, source))?
                .map_err(|source| BlobExchangeError::new(source.kind, source))
        })
    }
}
impl BlobTransport {
    pub(in super::super::super::super) fn supervise<T: Send + 'static>(
        self: &Arc<Self>,
        cancel: CancellationToken,
        work: impl Future<Output = Result<T, BlobExchangeError>> + Send + 'static,
    ) -> DownloadTask<T> {
        let (send, receive) = oneshot::channel();
        let (settled, completion) = watch::channel(None);
        let stop = self.stop.clone();
        let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
        self.reap_completed(&mut jobs);
        if stop.is_cancelled() {
            let error = Arc::new(BlobExchangeError::refused(BlobExchangeFailure::Closed));
            settled.send_replace(Some(Err(error.clone())));
            // discarded-source[channel]: 未接受工作；等待方可读取同一拒绝结果。
            let _ = send.send(Err(error));
        } else {
            let stopping = cancel.clone();
            let observation = ObservationContext::capture();
            jobs.spawn(
                async move {
                    let mut worker = tokio::spawn(
                        observation
                            .scope(work)
                            .instrument(Span::none())
                            .with_current_subscriber(),
                    );
                    let result = tokio::select! {
                        biased;
                        result = &mut worker => result,
                        _ = stop.cancelled() => { stopping.cancel(); worker.await },
                    };
                    let result = match result {
                        Ok(result) => result,
                        Err(source) => Err(task_error(source)),
                    }
                    .map_err(Arc::new);
                    let completed = result.as_ref().map(|_| ()).map_err(Arc::clone);
                    settled.send_replace(Some(completed.clone()));
                    // discarded-source[channel]: 等待方退出不撤销负责人持有的结果和真实工作。
                    let _ = send.send(result);
                    completed
                }
                .instrument(Span::none())
                .with_current_subscriber(),
            );
        }
        DownloadTask {
            cancel,
            result: receive,
            completion,
        }
    }
}
