//! 最终本地交接也是原下载任务的一部分；完成通知不得早于实际结算。
use super::preparation::FetchPreparationState;
use super::{
    flip_cancel_reason_perspective, BlobTransferError, BlobTransferFacade, FetchTransferContext,
};
use crate::transfer::blob::fetch_failure_reason;
use crate::transfer::blob::transport::{
    BlobExchangeError, BlobExchangeFailure, BlobPreparationStop,
};
use crate::transfer::receive::DownloadSettlement;
use std::sync::atomic::Ordering;
use std::sync::{Arc, PoisonError};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{FileTransferCancellationReason, OutboundProgressStatus};

pub(super) struct FetchFinalization {
    pub(super) owner: BlobTransferFacade,
    pub(super) context: Option<FetchTransferContext>,
    pub(super) prepared: Arc<FetchPreparationState>,
    pub(super) cancel: CancellationToken,
    pub(super) settlement: DownloadSettlement,
}
impl FetchFinalization {
    pub(super) async fn finish<T>(
        self,
        mut result: Result<(T, u64), BlobExchangeError>,
    ) -> Result<T, BlobExchangeError> {
        // 保留同一保护及许可至最终事件和通知实际结束。
        let _settlement = self.settlement;
        let session = self
            .prepared
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let sink = self
            .prepared
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Ok((_, bytes)) = &result {
            if !self.cancel.is_cancelled() {
                if let (Some(context), Some(sink)) = (&self.context, &sink) {
                    sink.report(*bytes, context.total_bytes.or(Some(*bytes)))
                        .await;
                }
            }
            if self.cancel.is_cancelled() {
                result = Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled));
            }
        }
        match result {
            Ok((value, bytes)) => {
                if let Some(context) = &self.context {
                    if context.individual_lifecycle || context.batch_position.is_last() {
                        if let Some(session) = session
                            .as_ref()
                            .filter(|_| !self.prepared.stop_attempted.load(Ordering::Acquire))
                        {
                            session.complete().await.map_err(|source| {
                                BlobExchangeError::new(BlobExchangeFailure::Settlement, source)
                            })?;
                        }
                    }
                    if context.batch_position.is_last() || context.batch.is_some() {
                        self.owner
                            .report_outbound_terminal(
                                context,
                                bytes,
                                context.total_bytes.or(Some(bytes)),
                                OutboundProgressStatus::Completed,
                            )
                            .await;
                    }
                }
                Ok(value)
            }
            Err(error) => {
                let cancelled = error.settled_cancellation();
                let reason = match *self
                    .prepared
                    .stop
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                {
                    Some(BlobPreparationStop::Cancelled(reason)) => reason,
                    Some(BlobPreparationStop::Deadline) | None => {
                        FileTransferCancellationReason::LocalUser
                    }
                };
                if let Some(session) = session
                    .as_ref()
                    .filter(|_| !self.prepared.stop_attempted.load(Ordering::Acquire))
                {
                    let stopped = if cancelled {
                        session.cancel(reason).await
                    } else {
                        session
                            .fail(fetch_failure_reason(&error.source), None)
                            .await
                    };
                    if let Err(source) = stopped {
                        return Err(error.with_settlement(BlobExchangeError::new(
                            BlobExchangeFailure::Settlement,
                            source,
                        )));
                    }
                } else if self.prepared.stop_attempted.load(Ordering::Acquire)
                    && error.kind == BlobExchangeFailure::Settlement
                {
                    return Err(error);
                }
                if let Some(context) = &self.context {
                    let status = if cancelled {
                        OutboundProgressStatus::Cancelled {
                            reason: flip_cancel_reason_perspective(reason),
                        }
                    } else {
                        OutboundProgressStatus::Failed
                    };
                    self.owner
                        .report_outbound_terminal(context, 0, context.total_bytes, status)
                        .await;
                }
                Err(error)
            }
        }
    }
}

pub(super) fn transfer_error(source: BlobExchangeError) -> BlobTransferError {
    if source.settled_cancellation() {
        BlobTransferError::Cancelled
    } else {
        BlobTransferError::Fetch(source.into())
    }
}
