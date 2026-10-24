//! 原下载任务持有原资源修复同一终态；首次失败可交付调用方，实际完成另有屏障。
use crate::transfer::blob::transport::{BlobExchangeError, BlobExchangeFailure, DownloadResponse};
use crate::transfer::file::session::ReceiverTransferHandle;
use crate::transfer::file::FileTransferApplicationError;
use crate::transfer::policy::{LOCAL_SETTLEMENT_RETRY_INITIAL, LOCAL_SETTLEMENT_RETRY_MAX};
use std::sync::Arc;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{FileTransferCancellationReason, FileTransferFailureReason};
use uc_core::ports::TrackedFileTransferStatus;
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

#[derive(Clone, Copy)]
pub(super) enum TerminalIntent {
    Completed,
    Failed(FileTransferFailureReason),
    Cancelled(FileTransferCancellationReason),
}
impl TerminalIntent {
    fn status(self) -> TrackedFileTransferStatus {
        match self {
            Self::Completed => TrackedFileTransferStatus::Completed,
            Self::Failed(_) => TrackedFileTransferStatus::Failed,
            Self::Cancelled(_) => TrackedFileTransferStatus::Cancelled,
        }
    }
    async fn commit(
        self,
        session: &ReceiverTransferHandle,
    ) -> Result<(), FileTransferApplicationError> {
        match self {
            Self::Completed => session.complete().await?,
            Self::Failed(reason) => session.fail(reason, None).await?,
            Self::Cancelled(reason) => session.cancel(reason).await?,
        };
        Ok(())
    }
}

/// 只携带原结果和持久交接错误；不在错误中持有下载许可、保护或输出。
#[derive(Debug, Clone, thiserror::Error)]
#[error("blob terminal settlement failed")]
pub(crate) struct FetchTerminalFailure {
    pub(crate) status: TrackedFileTransferStatus,
    #[source]
    pub(crate) source: Arc<BlobExchangeError>,
    settled: CancellationToken,
}
impl FetchTerminalFailure {
    pub(crate) fn is_settled(&self) -> bool {
        self.settled.is_cancelled()
    }
    pub(crate) async fn wait_settled(&self) {
        self.settled.cancelled().await;
    }
}

#[derive(Debug, thiserror::Error)]
#[error("blob terminal settlement remains pending")]
pub(crate) struct TerminalSettlementPending;

pub(super) fn terminal_failure(
    status: TrackedFileTransferStatus,
    source: Arc<BlobExchangeError>,
    settled: CancellationToken,
) -> BlobExchangeError {
    BlobExchangeError::new(
        BlobExchangeFailure::Settlement,
        FetchTerminalFailure {
            status,
            source,
            settled,
        },
    )
}

pub(super) async fn finish_terminal<T>(
    session: &ReceiverTransferHandle,
    intent: TerminalIntent,
    primary: Option<Arc<BlobExchangeError>>,
    response: &DownloadResponse<T>,
) -> Result<(), BlobExchangeError> {
    let Err(source) = intent.commit(session).await else {
        return Ok(());
    };
    let settlement = BlobExchangeError::new(BlobExchangeFailure::Settlement, source);
    let source = match primary {
        Some(primary) => BlobExchangeError::new(primary.kind, primary).with_settlement(settlement),
        None => settlement,
    };
    let settled = CancellationToken::new();
    let _finished = settled.clone().drop_guard();
    let failure = Arc::new(terminal_failure(intent.status(), Arc::new(source), settled));
    response.send(Err(failure.clone()));
    let mut delay = LOCAL_SETTLEMENT_RETRY_INITIAL;
    loop {
        // 首次失败已经返回；停止／五秒等待只结束外部等待，不撤销本地真实交接。
        sleep(delay).await;
        match intent.commit(session).await {
            Ok(()) => {
                return Err(BlobExchangeError::new(
                    BlobExchangeFailure::Settlement,
                    failure,
                ))
            }
            Err(source) => {
                // discarded-source[business-outcome]: 保留首次完整失败链，后续失败不形成无界错误历史。
                uc_warn!(
                    error_kind = "blob_terminal_settlement_repair",
                    io_error_kind = io_error_kind(&source),
                    "blob terminal settlement repair remains pending"
                );
                delay = (delay * 2).min(LOCAL_SETTLEMENT_RETRY_MAX);
            }
        }
    }
}
