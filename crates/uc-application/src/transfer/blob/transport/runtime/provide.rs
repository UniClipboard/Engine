use super::super::{BlobExchangeError, BlobExchangeFailure};
use super::owner::{task_error, BlobTransport};
use crate::connectivity::{ConnectionLease, ConnectivityError, StreamLease};
use crate::transfer::policy::INBOUND_REQUEST_TIMEOUT;
use tokio::task::{JoinError, JoinSet};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tracing::instrument::WithSubscriber;
use tracing::{Instrument, Span};
use uc_core::error_class::ErrorClass;
use uc_observability_contract::{error_source::io_error_kind, uc_debug, uc_warn};

impl BlobTransport {
    pub(super) async fn run_connection(
        &self,
        lease: ConnectionLease,
        cancel: CancellationToken,
    ) -> Result<(), BlobExchangeError> {
        let mut work = JoinSet::new();
        let mut failure = None;
        let retained =
            self.manager.retain(&lease).await.map_err(|source| {
                BlobExchangeError::new(BlobExchangeFailure::Connectivity, source)
            });
        match retained {
            Ok(true) => {
                loop {
                    let accepting = lease.accept_bi();
                    tokio::pin!(accepting);
                    // 收集兄弟结果时仍保留同一个 accept future，不能丢弃已开始的准入读取。
                    let accepted = loop {
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => {
                                lease.close();
                                settle_listener(&lease, accepting.await, &mut failure);
                                break None;
                            }
                            _ = lease.closed() => {
                                cancel.cancel();
                                settle_listener(&lease, accepting.await, &mut failure);
                                break None;
                            }
                            result = work.join_next(), if !work.is_empty() => {
                                if let Some(result) = result { record(result, &mut failure); }
                                if failure.as_ref().is_some_and(|error| error.kind == BlobExchangeFailure::Task) {
                                    cancel.cancel(); lease.close();
                                    settle_listener(&lease, accepting.await, &mut failure);
                                    break None;
                                }
                            }
                            result = &mut accepting => break Some(result),
                        }
                    };
                    let Some(accepted) = accepted else { break };
                    let stream = match accepted {
                        Ok(stream) => stream,
                        Err(ConnectivityError::Busy(_)) => continue,
                        Err(source) => {
                            if source.is_admission_read_failure()
                                || matches!(source, ConnectivityError::Settlement { .. })
                                || !lease.is_closed()
                            {
                                failure.get_or_insert_with(|| {
                                    BlobExchangeError::new(
                                        BlobExchangeFailure::Connectivity,
                                        source,
                                    )
                                });
                            }
                            break;
                        }
                    };
                    let deadline =
                        stream.received_at.unwrap_or_else(Instant::now) + INBOUND_REQUEST_TIMEOUT;
                    let exchange = self.exchange.clone();
                    let stopped = cancel.clone();
                    work.spawn(
                        async move { exchange.provide(stream, deadline, stopped).await }
                            .instrument(Span::none())
                            .with_current_subscriber(),
                    );
                }
            }
            Ok(false) => {
                failure = Some(BlobExchangeError::refused(BlobExchangeFailure::Busy));
            }
            Err(source) => {
                failure = Some(source);
            }
        }
        // 先同时封口全部交换，再收集实际 export 完成；不逐项串行发送停止意图。
        cancel.cancel();
        lease.close();
        self.manager.retire(&lease);
        while let Some(result) = work.join_next().await {
            record(result, &mut failure);
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
fn settle_listener(
    lease: &ConnectionLease,
    result: Result<StreamLease, ConnectivityError>,
    failure: &mut Option<BlobExchangeError>,
) {
    let error = match result {
        Ok(stream) => stream.stop().err(),
        Err(ConnectivityError::Stopped) => None,
        Err(source @ ConnectivityError::Stream(_)) if lease.is_closed() => {
            // discarded-source[business-outcome]: 等待新流因连接关闭结束；显式半流停止失败走下面的完整错误分支。
            uc_debug!(
                error_kind = "blob_listener_closed",
                error_class = source.class(),
                io_error_kind = io_error_kind(&source),
                "blob listener ended during connection close"
            );
            None
        }
        Err(source) => Some(source),
    };
    if let Some(source) = error {
        let error = BlobExchangeError::new(BlobExchangeFailure::Settlement, source);
        match failure.take() {
            Some(primary) => *failure = Some(primary.with_settlement(error)),
            None => *failure = Some(error),
        }
    }
}
fn record(
    result: Result<Result<(), BlobExchangeError>, JoinError>,
    failure: &mut Option<BlobExchangeError>,
) {
    let error = match result {
        Ok(Ok(())) => return,
        Ok(Err(error)) if error.kind == BlobExchangeFailure::Cancelled => return,
        Ok(Err(error)) => error,
        Err(source) => task_error(source),
    };
    if matches!(
        error.kind,
        BlobExchangeFailure::Store | BlobExchangeFailure::Settlement | BlobExchangeFailure::Task
    ) && failure.is_none()
    {
        *failure = Some(error);
    } else {
        // discarded-source[business-outcome]: 已按固定分类记录该次服务结果，保持连接以服务兄弟交换。
        uc_warn!(
            error_kind = "blob_service_exchange",
            error_class = error.class(),
            io_error_kind = io_error_kind(&error),
            "blob service exchange failed"
        );
    }
}
