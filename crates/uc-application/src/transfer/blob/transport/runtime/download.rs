//! 下载获取及时间预算；许可和内容保护交给同一 transfer 模块的输出结算持有。
use super::super::{
    BlobExchangeError, BlobExchangeFailure, BlobReadStorePort, BlobReceived, BlobSource,
    BlobSourcePort, BlobValidatedProgress,
};
use super::owner::BlobTransport;
use crate::connectivity::{ConnectionLease, ConnectionRequest, ConnectionRequestScope};
use crate::transfer::policy::PROGRESS_REPORT_INTERVAL;
use crate::transfer::receive::timing::{ReceiveTiming, TimingError};
use crate::transfer::receive::{scheduler::DownloadReservation, DownloadSettlement};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{watch, Notify};
use tokio::time::{sleep_until, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::FileTransferCancellationReason;
use uc_core::ports::blob::{BlobDigest, BlobProgressSink, BlobTicket};

#[derive(Clone, Copy)]
pub(in super::super::super::super) enum BlobPreparationStop {
    Cancelled(FileTransferCancellationReason),
    Deadline,
}

pub(in super::super::super::super) struct BlobDownloadPreparation {
    pub work: BoxFuture<'static, Result<Option<Arc<dyn BlobProgressSink>>, BlobExchangeError>>,
    pub stop: Arc<Mutex<Option<BlobPreparationStop>>>,
}

pub struct BlobDownloadResources {
    pub transport: Arc<BlobTransport>,
    pub source: Arc<dyn BlobSourcePort>,
    pub store: Arc<dyn BlobReadStorePort>,
}

pub(in super::super::super::super) struct PreparedDownload {
    scope: ConnectionRequestScope,
    source: BlobSource,
    queued: DownloadReservation,
}

pub(in super::super::super::super) struct DownloadedBlob {
    pub digest: BlobDigest,
    pub request: ConnectionRequest,
    _settlement: DownloadSettlement,
}
struct Progress {
    timing: Mutex<ReceiveTiming>,
    changed: Notify,
    latest: watch::Sender<Option<(u64, u64)>>,
}
impl BlobValidatedProgress for Progress {
    fn validated_through(&self, end: u64, total: u64) -> Result<(), BlobExchangeError> {
        if self
            .timing
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .validated_through(end, Instant::now())
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::RequestExpired, source))?
        {
            self.latest.send_replace(Some((end, total)));
            self.changed.notify_one();
        }
        Ok(())
    }
}
impl BlobDownloadResources {
    pub(in super::super::super::super) fn prepare(
        &self,
        ticket: &BlobTicket,
        cancel: &CancellationToken,
    ) -> Result<PreparedDownload, BlobExchangeError> {
        let scope = self.transport.manager.freeze_request();
        let source = self.source.inspect(ticket)?;
        let queued = self
            .transport
            .downloads
            .reserve(source.source_key, cancel)
            .map_err(BlobExchangeError::from)?;
        Ok(PreparedDownload {
            scope,
            source,
            queued,
        })
    }
    pub(in super::super::super::super) async fn obtain(
        &self,
        prepared: PreparedDownload,
        cancel: &CancellationToken,
        preparing: BlobDownloadPreparation,
        settlement: DownloadSettlement,
    ) -> Result<DownloadedBlob, BlobExchangeError> {
        let PreparedDownload {
            scope,
            source,
            queued,
        } = prepared;
        let permit = queued.wait(cancel).await.map_err(BlobExchangeError::from)?;
        let started_at = permit.started_at;
        settlement
            .retain_permit(permit)
            .map_err(BlobExchangeError::from)?;
        let (latest, receive) = watch::channel(None);
        let progress = Arc::new(Progress {
            timing: Mutex::new(ReceiveTiming::new(started_at)),
            changed: Notify::new(),
            latest,
        });
        let reports_done = CancellationToken::new();
        let report_stop = reports_done.clone();
        let BlobDownloadPreparation {
            work: preparing,
            stop,
        } = preparing;
        let mut reports = None;
        let result = {
            let work = async {
                let sink = preparing.await?;
                if cancel.is_cancelled() {
                    return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled));
                }
                progress
                    .timing
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .check(Instant::now())
                    .map_err(|source| {
                        BlobExchangeError::new(BlobExchangeFailure::RequestExpired, source)
                    })?;
                reports = Some(tokio::spawn(async move {
                    if let Some(sink) = sink {
                        report_progress(receive, sink, report_stop).await;
                    }
                }));
                let peer = self.source.resolve_peer(&source.source_key).await?;
                let request = scope
                    .bind(peer, source.address_hint)
                    .map_err(|source| self.transport.exchange.connection_failure(source))?;
                let content = self
                    .obtain_active(&request, source.digest, cancel, progress.clone())
                    .await?;
                Ok::<_, BlobExchangeError>((content, request))
            };
            tokio::pin!(work);
            loop {
                let deadline = progress
                    .timing
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .deadline();
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        // 接受取消后不再用后来的数据期限覆盖原结果；已开始的本地工作仍实际结算。
                        let cause = *stop.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(BlobPreparationStop::Cancelled(FileTransferCancellationReason::LocalUser));
                        let primary = match cause {
                            BlobPreparationStop::Cancelled(_) => BlobExchangeError::refused(BlobExchangeFailure::Cancelled),
                            BlobPreparationStop::Deadline => BlobExchangeError::new(BlobExchangeFailure::RequestExpired, TimingError(deadline.reason)),
                        };
                        break match work.await {
                            Err(error) if error.settled_cancellation() => Err(error),
                            Err(error) => Err(primary.with_settlement(error)),
                            Ok(_) => Err(primary),
                        };
                    }
                    result = &mut work => break result,
                    _ = progress.changed.notified() => {},
                    _ = sleep_until(deadline.at) => {
                        let cause = *stop.lock().unwrap_or_else(PoisonError::into_inner).get_or_insert(BlobPreparationStop::Deadline);
                        cancel.cancel();
                        let primary = match cause {
                            BlobPreparationStop::Cancelled(_) => BlobExchangeError::refused(BlobExchangeFailure::Cancelled),
                            BlobPreparationStop::Deadline => BlobExchangeError::new(BlobExchangeFailure::RequestExpired, TimingError(deadline.reason)),
                        };
                        break match work.await {
                            Err(error) if !error.settled_cancellation() => Err(primary.with_settlement(error)),
                            _ => Err(primary),
                        };
                    }
                }
            }
        };
        reports_done.cancel();
        let reported = match reports {
            Some(reports) => reports.await.map_err(super::owner::task_error),
            None => Ok(()),
        };
        let (content, request) = match (result, reported) {
            (Ok(received), Ok(())) => received,
            (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
            (Err(error), Err(additional)) => return Err(error.with_settlement(additional)),
        };
        let digest = content.digest;
        settlement
            .retain_content(content.protection)
            .map_err(BlobExchangeError::from)?;
        Ok(DownloadedBlob {
            digest,
            request,
            _settlement: settlement,
        })
    }
    async fn obtain_active(
        &self,
        request: &ConnectionRequest,
        digest: BlobDigest,
        cancel: &CancellationToken,
        progress: Arc<Progress>,
    ) -> Result<BlobReceived, BlobExchangeError> {
        let deadline = || {
            progress
                .timing
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .deadline()
                .at
        };
        request
            .validate(deadline(), cancel)
            .await
            .map_err(BlobExchangeError::from)?;
        if let Some(content) = self.store.protect(digest).await? {
            request
                .validate(deadline(), cancel)
                .await
                .map_err(BlobExchangeError::from)?;
            return Ok(content);
        }
        let mut connection: Option<ConnectionLease> = None;
        loop {
            if cancel.is_cancelled() || self.transport.stop.is_cancelled() {
                return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled));
            }
            let result = {
                let exchange = async {
                    let lease = match connection.take() {
                        Some(previous) if previous.is_closed() => self
                            .transport
                            .manager
                            .refresh(previous, deadline(), cancel)
                            .await
                            .map_err(|source| self.transport.exchange.connection_failure(source))?,
                        Some(previous) => previous,
                        None => request
                            .connect(deadline(), cancel)
                            .await
                            .map_err(|source| self.transport.exchange.connection_failure(source))?,
                    };
                    connection = Some(lease);
                    let lease = connection
                        .as_ref()
                        .ok_or_else(|| BlobExchangeError::refused(BlobExchangeFailure::Closed))?;
                    self.transport
                        .manager
                        .retain(lease)
                        .await
                        .map_err(|source| self.transport.exchange.connection_failure(source))?;
                    let stream = lease
                        .open_bi_with_cancel(deadline(), cancel)
                        .await
                        .map_err(|source| self.transport.exchange.connection_failure(source))?;
                    self.transport
                        .exchange
                        .receive(stream, digest, cancel.clone(), progress.clone())
                        .await
                };
                exchange.await
            };
            match result {
                Ok(content) => {
                    progress
                        .timing
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .check(Instant::now())
                        .map_err(|source| {
                            BlobExchangeError::new(BlobExchangeFailure::RequestExpired, source)
                        })?;
                    return Ok(content);
                }
                Err(error) if error.kind == BlobExchangeFailure::TransientNetwork => {
                    let next = progress
                        .timing
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .schedule_recovery(Instant::now())
                        .map_err(|source| BlobExchangeError {
                            kind: BlobExchangeFailure::RequestExpired,
                            source: source.into(),
                            additional: vec![error],
                        })?;
                    progress.changed.notify_one();
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled)),
                        _ = self.transport.stop.cancelled() => return Err(BlobExchangeError::refused(BlobExchangeFailure::Closed)),
                        _ = sleep_until(deadline()) => return Err(BlobExchangeError::refused(BlobExchangeFailure::RequestExpired)),
                        _ = sleep_until(next) => {},
                    }
                    progress
                        .timing
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .start_retry(Instant::now())
                        .map_err(|source| {
                            BlobExchangeError::new(BlobExchangeFailure::RequestExpired, source)
                        })?;
                    // 下一轮仍正常时只重开本交换；真实失活才消费旧租约并等待方向名额。
                }
                Err(error) => return Err(error),
            }
        }
    }
}
async fn report_progress(
    mut latest: watch::Receiver<Option<(u64, u64)>>,
    sink: Arc<dyn BlobProgressSink>,
    stop: CancellationToken,
) {
    let mut next = Instant::now();
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            result = latest.changed() => { if result.is_err() { break; } },
        }
        tokio::select! { biased; _ = stop.cancelled() => break, _ = sleep_until(next) => {} }
        let value = *latest.borrow_and_update();
        if let Some((end, total)) = value {
            sink.report(end, Some(total)).await;
        }
        next = Instant::now() + PROGRESS_REPORT_INTERVAL;
    }
}
