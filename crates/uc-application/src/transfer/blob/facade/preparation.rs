//! 下载取得原许可后才准备本地 lifecycle；等待方退出不丢弃已开始的准备。
use super::{BlobTransferError, BlobTransferFacade, FetchTransferContext, InflightFetch};
use crate::transfer::blob::transport::{
    BlobDownloadPreparation, BlobExchangeError, BlobExchangeFailure, BlobPreparationStop,
    DownloadTask,
};
use crate::transfer::blob::BlobProgressSink;
use crate::transfer::file::session::ReceiverTransferHandle;
use crate::transfer::file::FileTransferApplicationError;
use futures::future::BoxFuture;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{FileTransferCancellationReason, FileTransferFailureReason};
use uc_observability_contract::diagnostics::ObservationContext;

#[derive(Default)]
pub(super) struct FetchPreparationState {
    pub(super) session: Arc<Mutex<Option<Arc<ReceiverTransferHandle>>>>,
    pub(super) progress: Mutex<Option<Arc<dyn BlobProgressSink>>>,
    pub(super) stop: Arc<Mutex<Option<BlobPreparationStop>>>,
    pub(super) stop_attempted: AtomicBool,
    pub(super) start_failed: AtomicBool,
}

impl BlobTransferFacade {
    pub(super) fn prepare_fetch(
        &self,
        context: Option<FetchTransferContext>,
        cached_path: BoxFuture<'static, Result<String, BlobExchangeError>>,
        cancel: CancellationToken,
        state: Arc<FetchPreparationState>,
    ) -> BlobDownloadPreparation {
        let owner = self.clone();
        let stop = state.stop.clone();
        let observation = ObservationContext::capture();
        let work = Box::pin(observation.scope(async move {
            if cancel.is_cancelled() {
                return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled));
            }
            let cached_path = cached_path.await?;
            if cancel.is_cancelled() {
                return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled));
            }
            let session = match context.as_ref() {
                Some(context)
                    if context.individual_lifecycle || context.batch_position.is_first() =>
                {
                    owner.begin_lifecycle(context, cached_path).await
                }
                Some(context) => owner.existing_lifecycle(context).await,
                None => Ok(None),
            };
            let session = match session {
                Ok(session) => session,
                Err(source) => {
                    // 仅接回本次已登记的本地准备失败；业务拒绝不能接管别人的 session。
                    let local_start_failure = match &source {
                        BlobTransferError::Persistence(cause) => cause.chain().any(|cause| {
                            matches!(
                                cause.downcast_ref::<FileTransferApplicationError>(),
                                Some(
                                    FileTransferApplicationError::Store(_)
                                        | FileTransferApplicationError::Publish(_)
                                )
                            )
                        }),
                        _ => false,
                    };
                    if local_start_failure {
                        if let (Some(context), Some(facade)) =
                            (context.as_ref(), owner.file_transfer.as_ref())
                        {
                            let session = facade.active_session(&context.transfer_id).await;
                            state
                                .start_failed
                                .store(session.is_some(), Ordering::Release);
                            *state.session.lock().unwrap_or_else(PoisonError::into_inner) = session;
                        }
                    }
                    return Err(preparation_error(source));
                }
            };
            *state.session.lock().unwrap_or_else(PoisonError::into_inner) = session.clone();
            let sink = owner
                .build_progress_sink(context.as_ref(), session.clone())
                .await;
            *state
                .progress
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = sink.clone();
            if !cancel.is_cancelled() {
                if let (Some(context), Some(sink)) = (context.as_ref(), sink.as_ref()) {
                    if context.individual_lifecycle || context.batch_position.is_first() {
                        sink.report(0, context.total_bytes).await;
                    }
                }
            }
            if cancel.is_cancelled() {
                if let Some(session) = session {
                    state.stop_attempted.store(true, Ordering::Release);
                    let cause = state
                        .stop
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .unwrap_or(BlobPreparationStop::Cancelled(
                            FileTransferCancellationReason::LocalUser,
                        ));
                    match cause {
                        BlobPreparationStop::Cancelled(reason) => session.cancel(reason).await,
                        BlobPreparationStop::Deadline => {
                            session
                                .fail(FileTransferFailureReason::TimedOut, None)
                                .await
                        }
                    }
                    .map_err(|source| {
                        BlobExchangeError::new(BlobExchangeFailure::Settlement, source)
                    })?;
                }
                return Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled));
            }
            Ok(sink)
        }));
        BlobDownloadPreparation { work, stop }
    }

    /// 检查旧登记后才启动任务，重复意图不能先修改原 session。
    pub(super) fn start_fetch<T>(
        &self,
        context: Option<&FetchTransferContext>,
        token: CancellationToken,
        prepared: &Arc<FetchPreparationState>,
        start: impl FnOnce() -> DownloadTask<T>,
    ) -> Result<DownloadTask<T>, BlobTransferError> {
        let Some(context) = context else {
            return Ok(start());
        };
        let mut registry = self
            .inflight_fetches
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        registry.retain(|_, previous| previous.is_running());
        if registry
            .get(&context.transfer_id)
            .is_some_and(InflightFetch::is_running)
        {
            token.cancel();
            return Err(BlobTransferError::Fetch(
                BlobExchangeError::refused(BlobExchangeFailure::Busy).into(),
            ));
        }
        let operation = start();
        registry.insert(
            context.transfer_id.clone(),
            InflightFetch {
                completion: operation.completion(),
                cancelling: Arc::new(Mutex::new(None)),
                token,
                prepared: prepared.clone(),
                attempt_id: context.attempt_id.clone(),
                local: context
                    .batch
                    .as_ref()
                    .and_then(|batch| batch.file_completion(&context.transfer_id)),
            },
        );
        Ok(operation)
    }
}

impl InflightFetch {
    pub(super) fn is_running(&self) -> bool {
        self.completion.borrow().is_none()
            || self.local.as_ref().is_some_and(|local| local.is_pending())
            || self
                .cancelling
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .is_some_and(|completion| completion.borrow().is_none())
    }
}

/// 保留准备动作的完整原因；业务拒绝不应污染传输负责人的关闭结果。
fn preparation_error(source: BlobTransferError) -> BlobExchangeError {
    let kind = match &source {
        BlobTransferError::Persistence(cause) => cause
            .chain()
            .find_map(|cause| cause.downcast_ref::<FileTransferApplicationError>())
            .map(|cause| match cause {
                FileTransferApplicationError::LifecycleClosed => BlobExchangeFailure::Closed,
                FileTransferApplicationError::SessionConflict { .. }
                | FileTransferApplicationError::TransferAlreadyStarted { .. } => {
                    BlobExchangeFailure::Busy
                }
                FileTransferApplicationError::TransferAlreadyFinished { .. } => {
                    BlobExchangeFailure::AlreadyCompleted
                }
                FileTransferApplicationError::TransferNotStarted { .. }
                | FileTransferApplicationError::PeerMismatch { .. } => {
                    BlobExchangeFailure::InvalidRequest
                }
                FileTransferApplicationError::InvalidHistory { .. }
                | FileTransferApplicationError::ProgressWentBackwards { .. } => {
                    BlobExchangeFailure::Integrity
                }
                FileTransferApplicationError::Store(_)
                | FileTransferApplicationError::Repository(_) => BlobExchangeFailure::Store,
                FileTransferApplicationError::Cleanup(_)
                | FileTransferApplicationError::Publish(_) => BlobExchangeFailure::Settlement,
            })
            .unwrap_or(BlobExchangeFailure::Settlement),
        BlobTransferError::Cancelled => BlobExchangeFailure::Cancelled,
        BlobTransferError::Publish(_)
        | BlobTransferError::Fetch(_)
        | BlobTransferError::CancellationIncomplete(_) => BlobExchangeFailure::Settlement,
    };
    BlobExchangeError::new(kind, source)
}

#[cfg(test)]
mod tests {
    use super::{preparation_error, BlobTransferError};
    use crate::transfer::blob::test_support::RejectUnexpectedNetwork;
    use crate::transfer::blob::transport::{BlobExchangeFailure, BlobShutdownWait, BlobTransport};
    use crate::transfer::file::FileTransferApplicationError;
    use std::io::{Error, ErrorKind};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    #[tokio::test]
    async fn preparation_refusals_preserve_sources_without_poisoning_shutdown() {
        for (cause, expected) in [
            (
                FileTransferApplicationError::LifecycleClosed,
                BlobExchangeFailure::Closed,
            ),
            (
                FileTransferApplicationError::SessionConflict {
                    transfer_id: "conflict".into(),
                },
                BlobExchangeFailure::Busy,
            ),
            (
                FileTransferApplicationError::TransferAlreadyFinished {
                    transfer_id: "finished".into(),
                },
                BlobExchangeFailure::AlreadyCompleted,
            ),
        ] {
            let transport = BlobTransport::new(
                Arc::new(RejectUnexpectedNetwork),
                Arc::new(RejectUnexpectedNetwork),
                Arc::new(RejectUnexpectedNetwork),
                Arc::new(RejectUnexpectedNetwork),
            );
            let error = transport
                .supervise::<(), _>(CancellationToken::new(), move |_| async move {
                    Err(preparation_error(BlobTransferError::Persistence(
                        cause.into(),
                    )))
                })
                .await
                .unwrap_err();
            assert!(error
                .source
                .chain()
                .any(|source| source.is::<FileTransferApplicationError>()));
            assert!(
                transport.shutdown(BlobShutdownWait::Complete).await.is_ok(),
                "normal refusal must not poison shutdown"
            );
            assert_eq!(error.kind, expected);
        }
    }

    #[tokio::test]
    async fn preparation_storage_failure_remains_visible_to_shutdown() {
        let transport = BlobTransport::new(
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
        );
        let error = transport
            .supervise::<(), _>(CancellationToken::new(), |_| async {
                Err(preparation_error(BlobTransferError::Persistence(
                    FileTransferApplicationError::Store(Error::from(ErrorKind::StorageFull).into())
                        .into(),
                )))
            })
            .await
            .unwrap_err();
        assert_eq!(error.kind, BlobExchangeFailure::Store);
        let shutdown = transport
            .shutdown(BlobShutdownWait::Complete)
            .await
            .unwrap_err();
        assert_eq!(shutdown.kind, BlobExchangeFailure::Store);
        assert_eq!(
            shutdown
                .source
                .chain()
                .find_map(|source| source.downcast_ref::<Error>())
                .unwrap()
                .kind(),
            ErrorKind::StorageFull
        );
    }
}
