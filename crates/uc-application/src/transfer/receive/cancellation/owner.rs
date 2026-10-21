use super::{AttemptTransferCancellation, CancelEntryReceiveError, CancelEntryReceiveOutcome};
use crate::facade::HostEventBus;
use crate::transfer::blob::transport::{
    BlobExchangeError, BlobExchangeFailure, BlobTransport, DownloadCompletion,
};
use crate::transfer::policy::{ACTIVE_DOWNLOADS, CANCEL_WAIT_TIMEOUT, QUEUED_DOWNLOADS};
use crate::transfer::receive::work::ReceiveWorkRegistry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use tracing::instrument;
use uc_core::error_class::ErrorClass;
use uc_core::ids::EntryId;
use uc_core::ports::{
    CancelDirectoryAttemptTransfersPort, CleanupDirectoryStagingPort, ClockPort,
    CommitInboundReceivePort, GetDirectoryPublishRecordPort, GetEntryAttemptPort,
    RequestReceiveCancellationPort,
};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

type Completion = Option<Result<CancelEntryReceiveOutcome, Arc<CancelEntryReceiveError>>>;
#[derive(Clone)]
struct Job {
    result: watch::Receiver<Completion>,
    settled: DownloadCompletion,
}
pub(crate) struct CancelEntryReceiveUseCase {
    transport: Arc<BlobTransport>,
    pub(super) receive_work: Arc<ReceiveWorkRegistry>,
    jobs: Mutex<HashMap<(EntryId, String), Job>>,
    pub(super) get_attempt: Arc<dyn GetEntryAttemptPort>,
    pub(super) request_cancel: Arc<dyn RequestReceiveCancellationPort>,
    pub(super) commit_inbound: Arc<dyn CommitInboundReceivePort>,
    pub(super) get_publish: Arc<dyn GetDirectoryPublishRecordPort>,
    pub(super) staging_cleanup: Arc<dyn CleanupDirectoryStagingPort>,
    pub(super) cancel_projection: Arc<dyn CancelDirectoryAttemptTransfersPort>,
    pub(super) transfer_cancellation: Arc<dyn AttemptTransferCancellation>,
    pub(super) clock: Arc<dyn ClockPort>,
    pub(super) host_event_bus: Arc<HostEventBus>,
}

impl CancelEntryReceiveUseCase {
    pub(crate) fn new(
        get_attempt: Arc<dyn GetEntryAttemptPort>,
        request_cancel: Arc<dyn RequestReceiveCancellationPort>,
        commit_inbound: Arc<dyn CommitInboundReceivePort>,
        get_publish: Arc<dyn GetDirectoryPublishRecordPort>,
        staging_cleanup: Arc<dyn CleanupDirectoryStagingPort>,
        cancel_projection: Arc<dyn CancelDirectoryAttemptTransfersPort>,
        transfer_cancellation: Arc<dyn AttemptTransferCancellation>,
        clock: Arc<dyn ClockPort>,
        host_event_bus: Arc<HostEventBus>,
        transport: Arc<BlobTransport>,
        receive_work: Arc<ReceiveWorkRegistry>,
    ) -> Arc<Self> {
        Arc::new(Self {
            transport,
            receive_work,
            jobs: Mutex::new(HashMap::new()),
            get_attempt,
            request_cancel,
            commit_inbound,
            get_publish,
            staging_cleanup,
            cancel_projection,
            transfer_cancellation,
            clock,
            host_event_bus,
        })
    }

    #[instrument(name = "usecase.cancel_entry_receive.execute", skip_all)]
    pub(crate) fn execute(
        self: &Arc<Self>,
        entry_id: &EntryId,
        expected_attempt_id: &str,
    ) -> impl std::future::Future<Output = Result<CancelEntryReceiveOutcome, CancelEntryReceiveError>>
           + Send
           + 'static {
        let deadline = Instant::now() + CANCEL_WAIT_TIMEOUT;
        let key = (entry_id.clone(), expected_attempt_id.to_owned());
        let job = {
            let mut jobs = self.jobs.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(existing) = jobs.get(&key) {
                Ok(existing.clone())
            } else {
                jobs.retain(|_, job| {
                    job.result.borrow().is_none() && job.settled.borrow().is_none()
                });
                if jobs.len() >= ACTIVE_DOWNLOADS + QUEUED_DOWNLOADS {
                    Err(CancelEntryReceiveError::Busy(BlobExchangeError::refused(
                        BlobExchangeFailure::Busy,
                    )))
                } else {
                    let (send, result) = watch::channel(None);
                    let owner = self.clone();
                    let (entry, attempt) = key.clone();
                    let running = self
                        .transport
                        .supervise(CancellationToken::new(), async move {
                            let outcome = owner.run(&entry, &attempt).await.map_err(Arc::new);
                            if let Err(source) = &outcome {
                                uc_warn!(
                                    error_kind = "entry_receive_cancellation",
                                    error_class = source.class(),
                                    io_error_kind = io_error_kind(source.as_ref()),
                                    "entry receive cancellation failed during settlement"
                                );
                            }
                            let settled = outcome.as_ref().map(|_| ()).map_err(|source| {
                                BlobExchangeError::new(
                                    BlobExchangeFailure::Settlement,
                                    source.clone(),
                                )
                            });
                            send.send_replace(Some(outcome));
                            settled
                        });
                    let job = Job {
                        result,
                        settled: running.completion(),
                    };
                    // 完整协调者已经归 transport 持有，外部等待没有撤销实际动作的权限。
                    drop(running);
                    jobs.insert(key, job.clone());
                    Ok(job)
                }
            }
        };
        async move {
            let job = job?;
            match timeout_at(deadline, wait(job)).await {
                Ok(result) => result,
                // discarded-source[timeout]: 五秒只结束调用等待，原协调者继续全部停止、清理和提交。
                Err(_) => Ok(CancelEntryReceiveOutcome::CancellationIncomplete),
            }
        }
    }
}
async fn wait(mut job: Job) -> Result<CancelEntryReceiveOutcome, CancelEntryReceiveError> {
    loop {
        if let Some(result) = job.result.borrow_and_update().clone() {
            return result.map_err(CancelEntryReceiveError::Shared);
        }
        // discarded-source[channel]: 结果通道关闭时改等待真正监督结果，以保留原 JoinError 或关闭来源。
        if job.result.changed().await.is_err() {
            loop {
                if let Some(result) = job.settled.borrow_and_update().clone() {
                    let error = match result {
                        Err(source) => BlobExchangeError::new(source.kind, source),
                        Ok(()) => BlobExchangeError::refused(BlobExchangeFailure::Settlement),
                    };
                    return Err(CancelEntryReceiveError::Supervision(error));
                }
                job.settled.changed().await.map_err(|source| {
                    CancelEntryReceiveError::Supervision(BlobExchangeError::new(
                        BlobExchangeFailure::Settlement,
                        source,
                    ))
                })?;
            }
        }
    }
}
