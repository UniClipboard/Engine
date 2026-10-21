use super::super::{BlobExchangeError, BlobExchangeFailure};
use super::owner::{shutdown_failure, task_error, BlobShutdownWait, BlobTransport};
use crate::transfer::policy::CANCEL_WAIT_TIMEOUT;
use std::future::Future;
use std::sync::{Arc, PoisonError};
use tokio::sync::watch;
use tokio::time::timeout;
use tracing::instrument::WithSubscriber;
use tracing::{Instrument, Span};

impl BlobTransport {
    /// 重复关闭接入同一协调者；调用等待结束不会丢弃真正关闭任务。
    pub fn shutdown(
        self: &Arc<Self>,
        wait: BlobShutdownWait,
    ) -> impl Future<Output = Result<(), BlobExchangeError>> + Send + 'static {
        let mut receive = {
            let mut closing = self.closing.lock().unwrap_or_else(PoisonError::into_inner);
            match closing.as_ref() {
                Some(receive) => receive.clone(),
                None => {
                    self.stop.cancel();
                    self.downloads.close();
                    let (send, receive) = watch::channel(None);
                    let owner = self.clone();
                    tokio::spawn(
                        async move {
                            let worker = tokio::spawn(
                                async move { owner.finish_shutdown().await }
                                    .instrument(Span::none())
                                    .with_current_subscriber(),
                            );
                            let result = match worker.await {
                                Ok(result) => result,
                                Err(source) => Err(task_error(source)),
                            }
                            .map_err(Arc::new);
                            send.send_replace(Some(result));
                        }
                        .instrument(Span::none())
                        .with_current_subscriber(),
                    );
                    *closing = Some(receive.clone());
                    receive
                }
            }
        };
        async move {
            let completed = async {
                loop {
                    if let Some(result) = receive.borrow_and_update().clone() {
                        return result
                            .map_err(|source| BlobExchangeError::new(source.kind, source));
                    }
                    receive.changed().await.map_err(|source| {
                        BlobExchangeError::new(BlobExchangeFailure::Settlement, source)
                    })?;
                }
            };
            match wait {
                BlobShutdownWait::Complete => completed.await,
                BlobShutdownWait::Bounded => timeout(CANCEL_WAIT_TIMEOUT, completed)
                    .await
                    .map_err(|source| {
                        BlobExchangeError::new(BlobExchangeFailure::ShutdownIncomplete, source)
                    })?,
            }
        }
    }
    async fn finish_shutdown(&self) -> Result<(), BlobExchangeError> {
        let draining = async {
            let mut jobs = {
                let mut owned = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
                std::mem::take(&mut *owned)
            };
            let mut failure = self
                .failure
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .map(|source| BlobExchangeError::new(source.kind, source));
            while let Some(result) = jobs.join_next().await {
                let result = match result {
                    Ok(result) => {
                        result.map_err(|source| BlobExchangeError::new(source.kind, source))
                    }
                    Err(source) => Err(task_error(source)),
                };
                if let Err(error) = result {
                    if shutdown_failure(&error) {
                        failure.get_or_insert(error);
                    }
                }
            }
            match failure {
                Some(error) => Err(error),
                None => Ok(()),
            }
        };
        let (network, work) = tokio::join!(self.manager.shutdown(), draining);
        let network = network
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Connectivity, source));
        let stopped = combine(network, work);
        // 网络和所有实际工作已结算后，才关闭共享存储。
        combine(stopped, self.lifecycle.shutdown().await)
    }
}
fn combine(
    first: Result<(), BlobExchangeError>,
    second: Result<(), BlobExchangeError>,
) -> Result<(), BlobExchangeError> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(first.with_settlement(second)),
    }
}
