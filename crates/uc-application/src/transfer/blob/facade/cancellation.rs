//! 完整单项／尝试取消；原下载及本地结算均实际结束后才交接结果。
use super::{
    cancellation_result, BlobTransferError, BlobTransferFacade, InboundCancelOutcome,
    TransferCancellationWait,
};
use crate::transfer::blob::transport::{
    BlobExchangeError, BlobExchangeFailure, BlobPreparationStop, DownloadCompletion,
};
use crate::transfer::policy::CANCEL_WAIT_TIMEOUT;
use crate::transfer::receive::{LocalSettlement, LocalSettlementFailure, LocalSettlementOutcome};
use std::sync::PoisonError;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::FileTransferCancellationReason;
use uc_observability_contract::{error_source::io_error_kind, log_fields::log_id, uc_warn};

impl BlobTransferFacade {
    /// 先停止自身，再接入唯一收尾协调者；五秒只限制调用等待。
    pub(crate) async fn cancel_inbound_transfer(
        &self,
        transfer_id: &str,
        reason: FileTransferCancellationReason,
        wait: TransferCancellationWait,
    ) -> Result<InboundCancelOutcome, BlobTransferError> {
        let deadline = Instant::now() + CANCEL_WAIT_TIMEOUT;
        let entry = self
            .inflight_fetches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(transfer_id)
            .cloned();
        let Some(entry) = entry else {
            return Ok(InboundCancelOutcome::NotInflight);
        };
        if entry
            .completion
            .borrow()
            .as_ref()
            .is_some_and(Result::is_ok)
            && entry
                .local
                .as_ref()
                .is_none_or(LocalSettlement::is_committed)
        {
            return Ok(InboundCancelOutcome::NotInflight);
        }
        let downloaded = entry.completion.borrow().clone();
        if let Some(downloaded) = downloaded {
            if entry
                .cancelling
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_none()
            {
                match entry.local.as_ref().and_then(LocalSettlement::result) {
                    Some(Err(source)) => {
                        // 已结算失败只返回原事实；观察终态不再创建会污染关闭的停止任务。
                        let local = BlobExchangeError::new(
                            BlobExchangeFailure::Settlement,
                            LocalSettlementFailure(source),
                        );
                        let source = match downloaded {
                            Err(primary) => {
                                BlobExchangeError::new(primary.kind, primary).with_settlement(local)
                            }
                            Ok(()) => local,
                        };
                        return Err(BlobTransferError::Fetch(source.into()));
                    }
                    Some(Ok(LocalSettlementOutcome::Cancelled)) => {
                        if let Err(source) = downloaded {
                            if !source.settled_cancellation() {
                                return Err(BlobTransferError::Fetch(
                                    BlobExchangeError::new(source.kind, source).into(),
                                ));
                            }
                        }
                        return Ok(InboundCancelOutcome::Cancelled);
                    }
                    _ => {}
                }
            }
        }
        if let Some(local) = &entry.local {
            local.cancel();
        }
        {
            let mut stop = entry
                .prepared
                .stop
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            stop.get_or_insert(BlobPreparationStop::Cancelled(reason));
            entry.token.cancel();
        }
        let completion = {
            let mut cancelling = entry
                .cancelling
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match cancelling.as_ref() {
                Some(completion) => completion.clone(),
                None => {
                    let mut pending = entry.completion.clone();
                    let local = entry.local.clone();
                    let job = self
                        .transport
                        .supervise(CancellationToken::new(), async move {
                            let has_local = local.is_some();
                            let download = wait_download_completion(&mut pending).await;
                            if let Some(local) = local {
                                match local.wait().await {
                                    Ok(LocalSettlementOutcome::Committed) => {
                                        return Err(BlobExchangeError::refused(
                                            BlobExchangeFailure::AlreadyCompleted,
                                        ))
                                    }
                                    Ok(LocalSettlementOutcome::Cancelled) => {}
                                    Err(source) => {
                                        let local = BlobExchangeError::new(
                                            BlobExchangeFailure::Settlement,
                                            LocalSettlementFailure(source),
                                        );
                                        return Err(match download {
                                            Err(primary) => primary.with_settlement(local),
                                            Ok(()) => local,
                                        });
                                    }
                                }
                            }
                            match download {
                                Ok(()) if !has_local => {
                                    return Err(BlobExchangeError::refused(
                                        BlobExchangeFailure::AlreadyCompleted,
                                    ))
                                }
                                Ok(()) => {}
                                Err(error) if error.settled_cancellation() => {}
                                Err(error) => return Err(error),
                            }
                            Ok(())
                        });
                    let completion = job.completion();
                    // 等待方不拥有协调者；即使五秒等待或调用 future 退出，任务仍归 transport。
                    drop(job);
                    *cancelling = Some(completion.clone());
                    completion
                }
            }
        };
        let mut completion = completion;
        let result = match wait {
            TransferCancellationWait::Complete => wait_download_completion(&mut completion).await,
            TransferCancellationWait::Bounded => {
                timeout_at(deadline, wait_download_completion(&mut completion))
                    .await
                    .map_err(BlobTransferError::CancellationIncomplete)?
            }
        };
        match result {
            Ok(()) => {}
            Err(error) if error.kind == BlobExchangeFailure::AlreadyCompleted => {
                return Ok(InboundCancelOutcome::NotInflight)
            }
            Err(error) => return Err(BlobTransferError::Fetch(error.into())),
        }
        Ok(InboundCancelOutcome::Cancelled)
    }

    pub(crate) async fn cancel_inbound_attempt(
        &self,
        attempt_id: &str,
        reason: FileTransferCancellationReason,
        wait: TransferCancellationWait,
    ) -> Result<usize, BlobTransferError> {
        let transfer_ids = self
            .inflight_fetches
            .lock()
            // discarded-source[lock-poisoned]: `std::sync::PoisonError<Guard>`: the poison error holds the guard and cannot be stored across threads
            .map_err(|_| {
                BlobTransferError::Fetch(anyhow::anyhow!("in-flight fetch registry is poisoned"))
            })?
            .iter()
            .filter(|(_, fetch)| fetch.attempt_id.as_deref() == Some(attempt_id))
            .map(|(transfer_id, fetch)| {
                fetch
                    .prepared
                    .stop
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert(BlobPreparationStop::Cancelled(reason));
                if let Some(local) = &fetch.local {
                    local.cancel();
                }
                fetch.token.cancel();
                transfer_id.clone()
            })
            .collect::<Vec<_>>();

        let mut cancelled = 0usize;
        let mut errors = Vec::new();
        let results = futures::future::join_all(
            transfer_ids
                .iter()
                .map(|transfer_id| self.cancel_inbound_transfer(transfer_id, reason, wait)),
        )
        .await;
        for (transfer_id, result) in transfer_ids.into_iter().zip(results) {
            match result {
                Ok(InboundCancelOutcome::Cancelled) => cancelled += 1,
                Ok(InboundCancelOutcome::NotInflight) => {}
                Err(error) => {
                    uc_warn!(
                        transfer_id = log_id(&transfer_id),
                        error_kind = "member_transfer_cancel",
                        io_error_kind = io_error_kind(&error),
                        "failed to cancel directory member transfer"
                    );
                    errors.push(error);
                }
            }
        }
        cancellation_result(cancelled, errors)
    }
}

async fn wait_download_completion(
    completion: &mut DownloadCompletion,
) -> Result<(), BlobExchangeError> {
    loop {
        if let Some(result) = completion.borrow_and_update().clone() {
            return result.map_err(|source| BlobExchangeError::new(source.kind, source));
        }
        completion
            .changed()
            .await
            .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Settlement, source))?;
    }
}
