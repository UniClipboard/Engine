pub use super::output::BlobOutputTarget;
use super::output::PreparedBlobPath;
use super::BlobProgressSink;
use crate::transfer::receive::batch::ReceiveBatch;
use crate::transfer::receive::LocalSettlement;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use self::finalization::{transfer_error, FetchFinalization};
use self::preparation::FetchPreparationState;
mod cancellation;
mod finalization;
mod terminal_repair;
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::PoisonError;
pub(crate) use terminal_repair::{FetchTerminalFailure, TerminalSettlementPending};
mod preparation;
use tokio_util::sync::CancellationToken;

use uc_core::file_transfer::{FileTransferDirection, FileTransferFailureReason};
use uc_core::ids::EntryId;
use uc_core::ports::blob::{
    BlobDigest, BlobReferenceRepositoryPort, BlobTicket, BlobTransferPort, PlaintextHash,
};
use uc_core::ports::security::TransferCipherPort;
use uc_core::ports::{ContentHashPort, ReceiveItemRole};
use uc_observability_contract::{error_source::io_error_kind, log_fields::log_id, uc_warn};

use crate::facade::host_event::{HostEvent, HostEventBus, TransferHostEvent};
use crate::transfer::blob::transport::{
    BlobDownloadResources, BlobExchangeError, BlobExchangeFailure, BlobTransport,
    DownloadCompletion,
};
use crate::transfer::blob::{
    FetchBlobInput, FetchBlobPathInput, FetchBlobUseCase, PublishBlobInput, PublishBlobUseCase,
};
use crate::transfer::file::facade::{
    BeginReceiverTransfer, FileTransferFacade, ReceiverTransferRegistration,
};
use crate::transfer::file::session::ReceiverTransferHandle;

/// 共享的 host event 总线。
///
/// 多个装配阶段(bootstrap / Tauri setup / daemon start)通过 `bus.register`
/// 把自己关心的 transport (logging / Tauri / daemon WS) 挂到同一根 bus
/// 上;application 层各 use case 持有 `Arc<HostEventBus>` 引用,emit 时
/// fan-out 到所有已注册下游,装配顺序无关。
pub type SharedHostEventEmitter = Arc<HostEventBus>;

pub struct BlobTransferDeps {
    pub downloads: Arc<BlobDownloadResources>,
    pub hash: Arc<dyn ContentHashPort>,
    pub blob_transfer: Arc<dyn BlobTransferPort>,
    pub blob_reference: Arc<dyn BlobReferenceRepositoryPort>,
    pub transfer_cipher: Arc<dyn TransferCipherPort>,
    /// 可选 host event emitter。提供时,带 `transfer_context` 的 fetch_blob
    /// 会发出 progress 事件;不提供则 fetch_blob 退化为静默拉取。
    /// 状态变更(transferring / completed / failed)统一通过
    /// [`FileTransferFacade`] 走 lifecycle,不再由本 facade 直发。
    pub host_event_emitter: Option<SharedHostEventEmitter>,

    /// Optional process-wide file-transfer session owner. Tracked fetches use
    /// one session for registration, progress, terminal settlement, and cancel.
    pub file_transfer: Option<Arc<FileTransferFacade>>,
}

#[derive(Debug, Clone)]
pub struct PublishBlobCommand {
    pub plaintext: Bytes,
    pub entry_id: Option<EntryId>,
}

/// 用磁盘文件路径作为 blob 来源 publish。GH#487 P1: 让 outbound 的大文件
/// 走 iroh-blobs `add_path` 流式入库,避免 `tokio::fs::read` 把整文件读到
/// 内存,把 1GB 文件 publish 期间的 RSS 峰值从 ~2GB 降到 chunk 量级。
///
/// 与 [`PublishBlobCommand`] 在协议层等价(产出同样的 [`PublishBlobResult`]),
/// 但内存/IO 行为不同:`PublishBlobCommand` 适合已经在内存里的小 inline
/// payload(剪贴板文本扩展 rep / 小图);`PublishBlobPathCommand` 适合磁盘
/// 上的大文件 outbound。
#[derive(Debug, Clone)]
pub struct PublishBlobPathCommand {
    pub path: PathBuf,
    pub entry_id: Option<EntryId>,
}

#[derive(Debug, Clone)]
pub struct PublishBlobResult {
    pub ticket: BlobTicket,
    pub entry_id: EntryId,
    pub plaintext_hash: PlaintextHash,
    pub digest: BlobDigest,
    pub reused_existing: bool,
}

/// fetch_blob 期间向上报告进度时所需的传输上下文。
///
/// 由调用方提供:
/// - `transfer_id`: 接收端协议层这次传输的唯一关联 key。本工程的入站
///   pipeline 约定 `transfer_id == receiver_entry_id`(`ApplyInbound`
///   在流程入口预生成,贯穿占位卡片 → progress → `clipboard.new_content`),
///   所以 host event 里的 `transfer_id` 和 `entry_id` 字段最终发出的
///   是**同一个值**。前端 `useTransferProgress` 用它定位 UI,
///   `entryStatusById` 用它做 list row 关联。这两个字段在协议层职责
///   不同,但接收端值相等,是有意为之的对齐(避免前端做映射)。
/// - `peer_id` 是来源设备 ID,前端用它做"来自谁"的展示;
/// - `total_bytes` 来自 V3 envelope 的 advertised size,用于前端进度百分比与 ETA。
/// - `filename` 是 receiver-side projection 已经 seed 好的真实文件名;
///   The receiver session records it in the initial event and projection.
///   rep-bound blob / 没有显式文件名的场景填空字符串。
/// - 进度网络通知由完整接收负责人独立提交；下载回调只更新本地事实。
///
/// 不提供 transfer_context 时 fetch_blob 表现等同于改造前——只拉数据,不发事件。
#[derive(Debug, Clone)]
pub struct FetchTransferContext {
    pub item_role: ReceiveItemRole,
    pub(crate) batch: Option<Arc<ReceiveBatch>>,
    pub transfer_id: String,
    /// Clipboard entry that owns this transfer. Directory members use a
    /// distinct `transfer_id` while sharing this entry id for aggregation.
    pub entry_id: String,
    /// Directory receive attempt owning this member. Legacy and flat transfers
    /// leave it unset.
    pub attempt_id: Option<String>,
    pub peer_id: String,
    pub total_bytes: Option<u64>,
    pub filename: String,
    /// Multiple blob fetches may share one transfer session. The first fetch
    /// creates it, middle fetches reuse it, and the last fetch settles it.
    ///
    /// 单 blob 调用者(CLI `uniclip recv`、子 facade 内部转发)保留默认
    /// `Only`,行为与改造前完全一致。
    pub batch_position: BatchPosition,
    /// Track this fetch as its own receiver-side lifecycle row even when it
    /// belongs to a larger outbound progress batch.
    pub individual_lifecycle: bool,
}

/// 位置标志:在一个共享 `transfer_id` 的 fetch batch 里,本次 fetch 处在哪个位置。
///
/// Controls when a shared transfer session is created and settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BatchPosition {
    /// 这次 fetch 是 batch 里唯一一次(单 blob 调用者默认),seed+start 和
    /// complete+outbound terminal 都执行。
    #[default]
    Only,
    /// First fetch in a multi-fetch session.
    First,
    /// Middle fetch in a multi-fetch session.
    Middle,
    /// Last fetch in a multi-fetch session.
    Last,
}

impl BatchPosition {
    /// Whether this fetch creates the shared session.
    pub fn is_first(self) -> bool {
        matches!(self, Self::Only | Self::First)
    }

    /// Whether this fetch settles the shared session.
    pub fn is_last(self) -> bool {
        matches!(self, Self::Only | Self::Last)
    }
}

#[derive(Debug, Clone)]
pub struct FetchBlobCommand {
    pub cancellation: CancellationToken,
    pub ticket: BlobTicket,
    /// **发送端**侧的 entry_id —— 仅用于 iroh blob tag 与 fetch use case
    /// 内部记录,不会出现在 host event 里。前端关联用的是
    /// `transfer_context.transfer_id`(== receiver_entry_id)。
    pub entry_id: EntryId,
    /// Some 时 fetch_blob 会发出 status_changed + progress host events;
    /// None 时退化为静默拉取(用于不需要 UI 反馈的内部场景,例如 CLI 工具)。
    pub transfer_context: Option<FetchTransferContext>,
}

#[derive(Debug, Clone)]
pub struct FetchBlobResult {
    pub plaintext: Bytes,
    pub entry_id: EntryId,
    pub plaintext_hash: PlaintextHash,
    pub digest: BlobDigest,
}

/// Streaming counterpart of [`FetchBlobCommand`] — the blob lands on disk
/// at `target_path` instead of being returned as `Bytes`. GH#487 Phase 2.
///
/// Used by the inbound materializer for free-standing files so the
/// receive side stops materialising 800 MB+ payloads in memory only to
/// `tokio::fs::write` them out again.
#[derive(Debug)]
pub struct FetchBlobToPathCommand {
    pub cancellation: CancellationToken,
    pub ticket: BlobTicket,
    pub entry_id: EntryId,
    pub target_path: BlobOutputTarget,
    pub transfer_context: Option<FetchTransferContext>,
}

#[derive(Debug, Clone)]
pub struct FetchBlobToPathResult {
    pub entry_id: EntryId,
    pub plaintext_hash: PlaintextHash,
    pub digest: BlobDigest,
    /// Final file size on disk (from `tokio::fs::metadata` after the
    /// streaming export completed). Useful for callers that didn't get a
    /// `total_bytes` from the protocol layer and want to log the real size.
    pub bytes_written: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum BlobTransferError {
    #[error("publish blob failed")]
    Publish(#[source] anyhow::Error),
    #[error("fetch blob failed")]
    Fetch(#[source] anyhow::Error),
    /// fetch_blob / fetch_blob_to_path 在进行中被外部 cancel(用户点取消、
    /// timeout sweep、删除流程)。目标文件可能是 partial,调用方应当把
    /// `target_path` 视为不可用并交由 cleanup 删除。与 `Fetch` 不同:
    /// Fetch 表达"传输本身失败",Cancelled 表达"传输被主动撤回"。
    #[error("fetch blob cancelled")]
    Cancelled,
    #[error("blob cancellation remains incomplete")]
    CancellationIncomplete(#[source] tokio::time::error::Elapsed),
    #[error("record blob transfer failed")]
    Persistence(#[source] anyhow::Error),
}

/// Result of attempting to cancel an inbound transfer.
///
/// `cancel_inbound_transfer` is idempotent: re-invocations on the same
/// `transfer_id` are safe. This outcome lets callers distinguish between
/// "we actually tore down a live fetch (Cancelled event flowed via the
/// lifecycle)" and "no live fetch existed". The latter happens when the
/// transfer never reached `Started` (still pending), already terminated,
/// or was already cancelled by an earlier call. Callers that need to
/// move such rows to a terminal status must arrange a fallback themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundCancelOutcome {
    /// The fetch was live; it has been torn down and a `Cancelled` domain
    /// event has been appended when the fetch owns a tracked session.
    Cancelled,
    /// No live fetch was registered for this `transfer_id` — nothing was
    /// torn down and no event was emitted.
    NotInflight,
}

#[derive(Clone, Copy)]
pub(crate) enum TransferCancellationWait {
    Bounded,
    Complete,
}

/// 单项获取及唯一取消协调者的完成事实，取消不提前移除登记。
#[derive(Clone)]
struct InflightFetch {
    completion: DownloadCompletion,
    cancelling: Arc<Mutex<Option<DownloadCompletion>>>,
    token: CancellationToken,
    prepared: Arc<FetchPreparationState>,
    attempt_id: Option<String>,
    local: Option<LocalSettlement>,
}

#[derive(Clone)]
pub struct BlobTransferFacade {
    publish_uc: Arc<PublishBlobUseCase>,
    fetch_uc: Arc<FetchBlobUseCase>,
    /// 全部获取、通知和收尾任务由同一个传输负责人监督。
    pub(in super::super) transport: Arc<BlobTransport>,
    host_event_emitter: Option<SharedHostEventEmitter>,

    file_transfer: Option<Arc<FileTransferFacade>>,
    /// 以具体接收项登记取消；重复调用接入原来的真实完成通知。
    inflight_fetches: Arc<Mutex<HashMap<String, InflightFetch>>>,
}

impl BlobTransferFacade {
    #[doc(hidden)]
    pub fn new(deps: BlobTransferDeps) -> Self {
        let transport = deps.downloads.transport.clone();
        let publish_uc = Arc::new(PublishBlobUseCase::new(
            Arc::clone(&deps.hash),
            Arc::clone(&deps.blob_transfer),
            Arc::clone(&deps.blob_reference),
            Arc::clone(&deps.transfer_cipher),
        ));
        let fetch_uc = Arc::new(FetchBlobUseCase::new(
            deps.downloads,
            deps.hash,
            Arc::clone(&deps.blob_transfer),
            deps.blob_reference,
            deps.transfer_cipher,
        ));
        Self {
            publish_uc,
            fetch_uc,
            transport,
            host_event_emitter: deps.host_event_emitter,

            file_transfer: deps.file_transfer,
            inflight_fetches: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 旧扫描只恢复无负责人行；活跃获取及未完收尾使用本负责人的实际时钟。
    pub(in super::super) fn owns_inbound_work(&self, transfer_id: &str) -> bool {
        self.inflight_fetches
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(transfer_id)
            .is_some_and(InflightFetch::is_running)
    }
    async fn begin_lifecycle(
        &self,
        ctx: &FetchTransferContext,
        cached_path: String,
    ) -> Result<Option<Arc<ReceiverTransferHandle>>, BlobTransferError> {
        let Some(facade) = self.file_transfer.as_ref() else {
            return Ok(None);
        };
        facade
            .begin_receiver_transfer(BeginReceiverTransfer {
                item_role: Some(ctx.item_role),
                transfer_id: ctx.transfer_id.clone(),
                peer_id: ctx.peer_id.clone(),
                filename: ctx.filename.clone(),
                file_size: ctx.total_bytes,
                registration: ReceiverTransferRegistration::Entry {
                    entry_id: ctx.entry_id.clone(),
                    attempt_id: ctx.attempt_id.clone(),
                    cached_path,
                },
            })
            .await
            .map(Some)
            .map_err(|error| BlobTransferError::Persistence(anyhow::Error::from(error)))
    }

    async fn existing_lifecycle(
        &self,
        ctx: &FetchTransferContext,
    ) -> Result<Option<Arc<ReceiverTransferHandle>>, BlobTransferError> {
        let Some(facade) = self.file_transfer.as_ref() else {
            return Ok(None);
        };
        facade
            .active_session(&ctx.transfer_id)
            .await
            .map(Some)
            .ok_or_else(|| {
                BlobTransferError::Persistence(anyhow::anyhow!(
                    "active file transfer session is missing"
                ))
            })
    }

    pub async fn record_unfetched_failure(
        &self,
        ctx: FetchTransferContext,
        reason: FileTransferFailureReason,
    ) {
        match self.begin_lifecycle(&ctx, String::new()).await {
            Ok(Some(session)) => {
                if let Err(error) = session.fail(reason, None).await {
                    uc_warn!(
                        error_kind = "unfetched_failure_finish",
                        io_error_kind = io_error_kind(&error),
                        transfer_id = log_id(&ctx.transfer_id),
                        "could not finish unfetched transfer failure"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => {
                uc_warn!(
                    error_kind = "unfetched_failure_begin",
                    io_error_kind = io_error_kind(&error),
                    transfer_id = log_id(&ctx.transfer_id),
                    "could not begin unfetched transfer failure"
                );
            }
        }
    }

    pub async fn publish_blob(
        &self,
        command: PublishBlobCommand,
    ) -> Result<PublishBlobResult, BlobTransferError> {
        let outcome = self
            .publish_uc
            .execute(PublishBlobInput::Plaintext {
                plaintext: command.plaintext,
                entry_id: command.entry_id.unwrap_or_default(),
            })
            .await
            .map_err(|e| BlobTransferError::Publish(anyhow::Error::from(e)))?;
        Ok(PublishBlobResult {
            ticket: outcome.ticket,
            entry_id: outcome.entry_id,
            plaintext_hash: outcome.plaintext_hash,
            digest: outcome.digest,
            reused_existing: outcome.reused_existing,
        })
    }

    /// 流式 publish:从磁盘文件读取并入库,避免把整文件加载到内存。GH#487 P1。
    pub async fn publish_blob_path(
        &self,
        command: PublishBlobPathCommand,
    ) -> Result<PublishBlobResult, BlobTransferError> {
        let outcome = self
            .publish_uc
            .execute(PublishBlobInput::Path {
                path: command.path,
                entry_id: command.entry_id.unwrap_or_default(),
            })
            .await
            .map_err(|e| BlobTransferError::Publish(anyhow::Error::from(e)))?;
        Ok(PublishBlobResult {
            ticket: outcome.ticket,
            entry_id: outcome.entry_id,
            plaintext_hash: outcome.plaintext_hash,
            digest: outcome.digest,
            reused_existing: outcome.reused_existing,
        })
    }

    pub async fn fetch_blob(
        &self,
        command: FetchBlobCommand,
    ) -> Result<FetchBlobResult, BlobTransferError> {
        let settlement = command
            .transfer_context
            .as_ref()
            .and_then(|context| {
                context
                    .batch
                    .as_ref()
                    .map(|batch| batch.local_settlement(&context.transfer_id))
            })
            .unwrap_or_default();
        let iroh_tag_entry_id = command.entry_id.clone();

        let cancel_token = command.cancellation.child_token();
        let prepared = Arc::new(FetchPreparationState::default());
        let preparation = self.prepare_fetch(
            command.transfer_context.clone(),
            Box::pin(async { Ok(String::new()) }),
            cancel_token.clone(),
            prepared.clone(),
        );
        let input = FetchBlobInput {
            settlement: settlement.clone(),
            preparation,
            ticket: command.ticket,
            cancel: cancel_token.clone(),
            entry_id: iroh_tag_entry_id,
        };
        let finalization = FetchFinalization {
            context: command.transfer_context.clone(),
            prepared: prepared.clone(),
            cancel: cancel_token.clone(),
            settlement,
        };
        let operation = self.start_fetch(
            command.transfer_context.as_ref(),
            cancel_token,
            &prepared,
            || {
                self.fetch_uc
                    .execute(input, move |result, response| async move {
                        finalization
                            .finish(
                                result.map(|outcome| {
                                    let bytes = outcome.plaintext.len() as u64;
                                    (
                                        FetchBlobResult {
                                            plaintext: outcome.plaintext,
                                            entry_id: outcome.entry_id,
                                            plaintext_hash: outcome.plaintext_hash,
                                            digest: outcome.digest,
                                        },
                                        bytes,
                                    )
                                }),
                                response,
                            )
                            .await
                    })
            },
        )?;
        operation.await.map_err(transfer_error)
    }

    /// 流式 fetch:把 blob 从 iroh store 直接 export 到 `target_path`,
    /// 不经过 `Bytes`。GH#487 Phase 2:与 [`fetch_blob`](Self::fetch_blob) 在
    /// progress sink、status events、outbound 反向上报上行为完全一致 ——
    /// 只是不返回 plaintext。inbound materializer 用这个入口避免 800 MB+
    /// 文件先全量进内存再 `tokio::fs::write` 的双写盘问题。
    pub async fn fetch_blob_to_path(
        &self,
        command: FetchBlobToPathCommand,
    ) -> Result<FetchBlobToPathResult, BlobTransferError> {
        let settlement = command
            .transfer_context
            .as_ref()
            .and_then(|context| {
                context
                    .batch
                    .as_ref()
                    .map(|batch| batch.local_settlement(&context.transfer_id))
            })
            .unwrap_or_default();
        let iroh_tag_entry_id = command.entry_id.clone();

        let cancel_token = command.cancellation.child_token();
        let prepared = Arc::new(FetchPreparationState::default());
        let target_path = PreparedBlobPath::default();
        let output_path = target_path.clone();
        let hide_cached_path = command
            .transfer_context
            .as_ref()
            .is_some_and(|context| context.attempt_id.is_some());
        let preparation = self.prepare_fetch(
            command.transfer_context.clone(),
            Box::pin(async move {
                let path =
                    command.target_path.prepare().await.map_err(|source| {
                        BlobExchangeError::new(BlobExchangeFailure::Store, source)
                    })?;
                let cached_path = if hide_cached_path {
                    String::new()
                } else {
                    path.to_string_lossy().into_owned()
                };
                output_path.publish(path)?;
                Ok(cached_path)
            }),
            cancel_token.clone(),
            prepared.clone(),
        );
        let input = FetchBlobPathInput {
            settlement: settlement.clone(),
            preparation,
            ticket: command.ticket,
            cancel: cancel_token.clone(),
            entry_id: iroh_tag_entry_id,
            target_path,
        };
        let finalization = FetchFinalization {
            context: command.transfer_context.clone(),
            prepared: prepared.clone(),
            cancel: cancel_token.clone(),
            settlement,
        };
        let operation = self.start_fetch(
            command.transfer_context.as_ref(),
            cancel_token,
            &prepared,
            || {
                self.fetch_uc
                    .execute_to_path(input, move |result, response| async move {
                        finalization
                            .finish(
                                result.map(|outcome| {
                                    let bytes = outcome.bytes_written;
                                    (
                                        FetchBlobToPathResult {
                                            entry_id: outcome.entry_id,
                                            plaintext_hash: outcome.plaintext_hash,
                                            digest: outcome.digest,
                                            bytes_written: outcome.bytes_written,
                                        },
                                        bytes,
                                    )
                                }),
                                response,
                            )
                            .await
                    })
            },
        )?;
        operation.await.map_err(transfer_error)
    }

    async fn build_progress_sink(
        &self,
        ctx: Option<&FetchTransferContext>,
        session: Option<Arc<ReceiverTransferHandle>>,
    ) -> Option<Arc<dyn BlobProgressSink>> {
        let ctx = ctx?;
        let base_bytes = match session.as_ref() {
            Some(session) => session.bytes_transferred().await,
            None => 0,
        };
        if session.is_none() && self.host_event_emitter.is_none() && ctx.batch.is_none() {
            return None;
        }
        Some(Arc::new(FileTransferProgressSink {
            fallback_bus: self.host_event_emitter.clone(),
            session,
            transfer_id: ctx.transfer_id.clone(),
            entry_id: ctx.entry_id.clone(),
            attempt_id: ctx.attempt_id.clone(),
            peer_id: ctx.peer_id.clone(),
            base_bytes,
            fallback_total: ctx.total_bytes,
            batch: ctx.batch.clone(),
        }))
    }
}

fn cancellation_result(
    cancelled: usize,
    errors: Vec<BlobTransferError>,
) -> Result<usize, BlobTransferError> {
    let mut errors = errors.into_iter();
    let Some(first) = errors.next() else {
        return Ok(cancelled);
    };
    let additional = errors
        .map(|source| BlobExchangeError::new(BlobExchangeFailure::Settlement, source))
        .collect::<Vec<_>>();
    if additional.is_empty() {
        return Err(first);
    }
    let mut report = BlobExchangeError::new(BlobExchangeFailure::Settlement, first);
    report.additional = additional;
    Err(BlobTransferError::Fetch(report.into()))
}

struct FileTransferProgressSink {
    batch: Option<Arc<ReceiveBatch>>,
    fallback_bus: Option<SharedHostEventEmitter>,
    session: Option<Arc<ReceiverTransferHandle>>,
    transfer_id: String,
    entry_id: String,
    attempt_id: Option<String>,
    peer_id: String,
    base_bytes: u64,
    fallback_total: Option<u64>,
}

#[async_trait]
impl BlobProgressSink for FileTransferProgressSink {
    fn observe_round(&self, round: u32, bytes: u64, total: Option<u64>) {
        if let Some(batch) = &self.batch {
            batch.record_round(&self.transfer_id, round, bytes, total);
        }
    }
    fn observe_retrying(&self, round: u32) {
        if let Some(batch) = &self.batch {
            batch.record_retrying(&self.transfer_id, round);
        }
    }
    fn observe_obtained(&self, round: u32, verified_size: u64) {
        if let Some(batch) = &self.batch {
            batch.record_round(&self.transfer_id, round, verified_size, Some(verified_size));
        }
    }
    async fn report(&self, bytes_transferred: u64, total_bytes: Option<u64>) {
        let cumulative_bytes = self.base_bytes.saturating_add(bytes_transferred);
        let cumulative_total = total_bytes
            .or(self.fallback_total)
            .map(|total| self.base_bytes.saturating_add(total));

        if let Some(session) = self.session.as_ref() {
            if let Err(error) = session
                .report_progress(cumulative_bytes, cumulative_total)
                .await
            {
                uc_warn!(
                    transfer_id = log_id(&self.transfer_id),
                    error_kind = "progress_settle",
                    io_error_kind = io_error_kind(&error),
                    "blob fetch: progress settlement failed"
                );
            }
        } else if let Some(bus) = self.fallback_bus.as_ref() {
            bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::Progress {
                transfer_id: self.transfer_id.clone(),
                entry_id: Some(self.entry_id.clone()),
                attempt_id: self.attempt_id.clone(),
                peer_id: self.peer_id.clone(),
                direction: FileTransferDirection::Receiving,
                bytes_transferred: cumulative_bytes,
                total_bytes: cumulative_total,
            }));
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    mod budget;
    mod finalization;
    mod targets;

    use crate::transfer::blob::transport::{
        BlobExchangeError, BlobReadStorePort, BlobReceived, BlobSource, BlobSourcePort,
    };
    use std::path::Path;

    use super::*;
    use tokio::sync::Semaphore;
    use uc_core::clipboard::{ContentHash, HashAlgorithm};
    use uc_core::ports::blob::{BlobError, BlobReferenceError, TagReason};
    use uc_core::ports::security::{TransferCipherError, TransferCipherPort};

    #[test]
    fn cancelling_multiple_members_preserves_each_cleanup_source() {
        let error = cancellation_result(
            0,
            vec![
                BlobTransferError::Fetch(
                    std::io::Error::from(std::io::ErrorKind::PermissionDenied).into(),
                ),
                BlobTransferError::Fetch(
                    std::io::Error::from(std::io::ErrorKind::ReadOnlyFilesystem).into(),
                ),
            ],
        )
        .expect_err("cleanup failed");
        let BlobTransferError::Fetch(error) = error else {
            panic!("structured report required")
        };
        let report = error
            .downcast_ref::<BlobExchangeError>()
            .expect("exchange report");
        assert_eq!(
            report
                .source
                .chain()
                .find_map(|source| source.downcast_ref::<std::io::Error>())
                .expect("primary IO")
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            report.additional[0]
                .source
                .chain()
                .find_map(|source| source.downcast_ref::<std::io::Error>())
                .expect("additional IO")
                .kind(),
            std::io::ErrorKind::ReadOnlyFilesystem
        );
    }

    struct TestHash;

    impl ContentHashPort for TestHash {
        fn hash_bytes(&self, bytes: &[u8]) -> anyhow::Result<ContentHash> {
            Ok(ContentHash {
                alg: HashAlgorithm::Blake3V1,
                bytes: *blake3::hash(bytes).as_bytes(),
            })
        }
    }

    struct TestReferences;

    struct TestCipher;

    #[async_trait]
    impl TransferCipherPort for TestCipher {
        async fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            Ok(plaintext.to_vec())
        }

        async fn decrypt(&self, encrypted: &[u8]) -> Result<Vec<u8>, TransferCipherError> {
            Ok(encrypted.to_vec())
        }
    }

    #[async_trait]
    impl BlobReferenceRepositoryPort for TestReferences {
        async fn find_by_plaintext_hash(
            &self,
            _hash: &PlaintextHash,
        ) -> Result<Option<BlobDigest>, BlobReferenceError> {
            Ok(None)
        }

        async fn save(
            &self,
            _hash: PlaintextHash,
            _digest: BlobDigest,
        ) -> Result<(), BlobReferenceError> {
            Ok(())
        }

        async fn forget(&self, _hash: &PlaintextHash) -> Result<(), BlobReferenceError> {
            Ok(())
        }
    }

    struct CountedProtection(Arc<std::sync::atomic::AtomicUsize>);
    impl crate::transfer::blob::transport::BlobRetainedContent for CountedProtection {}
    impl Drop for CountedProtection {
        fn drop(&mut self) {
            self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    struct FileProbeSource;
    #[async_trait]
    impl BlobSourcePort for FileProbeSource {
        fn inspect(&self, ticket: &BlobTicket) -> Result<BlobSource, BlobExchangeError> {
            assert_eq!(ticket.as_bytes(), &[1]);
            Ok(BlobSource {
                source_key: [0; 32],
                digest: BlobDigest::from_bytes(*blake3::hash(b"retried").as_bytes()),
                address_hint: Vec::new(),
            })
        }
        async fn resolve_peer(&self, _: &[u8; 32]) -> Result<DeviceId, BlobExchangeError> {
            Ok(DeviceId::new("peer"))
        }
    }
    /// 导出失败后留下实际不可remove_file的目标，观察完整清理责任。
    pub(crate) struct FailedFileExport {
        pub(crate) output: std::sync::Mutex<Option<std::path::PathBuf>>,
        pub(crate) cancelled: bool,
        pub(crate) obstruct_cleanup: bool,
    }
    /// 实际下载调度与文件输出；本地替身持有可计数保护，不模拟网络验收。
    pub(crate) fn local_file_probe(
        failed_export: Option<Arc<FailedFileExport>>,
    ) -> (
        Arc<BlobTransferFacade>,
        Arc<BlobTransport>,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        use crate::transfer::blob::test_support::{FakeLocalStore, RejectUnexpectedNetwork};
        let transfer = Arc::new(BlockingBlobTransfer {
            failed_export,
            ..BlockingBlobTransfer::new()
        });
        transfer.release.add_permits(1024);
        let protected = transfer.protected.clone();
        let transport = BlobTransport::new(
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
            Arc::new(RejectUnexpectedNetwork),
        );
        let facade = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
            downloads: Arc::new(BlobDownloadResources {
                transport: transport.clone(),
                source: Arc::new(FileProbeSource),
                store: Arc::new(FakeLocalStore(transfer.clone())),
            }),
            hash: Arc::new(TestHash),
            blob_transfer: transfer,
            blob_reference: Arc::new(TestReferences),
            transfer_cipher: Arc::new(TestCipher),
            host_event_emitter: None,

            file_transfer: None,
        }));
        (facade, transport, protected)
    }

    struct BlockingBlobTransfer {
        failed_export: Option<Arc<FailedFileExport>>,
        started: Semaphore,
        release: Semaphore,
        fail_reads: std::sync::atomic::AtomicBool,
        protected: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl BlockingBlobTransfer {
        fn new() -> Self {
            Self {
                failed_export: None,
                started: Semaphore::new(0),
                release: Semaphore::new(0),
                fail_reads: std::sync::atomic::AtomicBool::new(false),
                protected: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl BlobReadStorePort for BlockingBlobTransfer {
        async fn protect(
            &self,
            digest: BlobDigest,
        ) -> Result<Option<BlobReceived>, BlobExchangeError> {
            self.protected
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(BlobReceived {
                digest,
                size: 0,
                protection: Box::new(CountedProtection(self.protected.clone())),
            }))
        }
        async fn read(&self, _: BlobDigest) -> Result<Bytes, BlobExchangeError> {
            self.started.add_permits(1);
            self.release
                .acquire()
                .await
                .expect("release slow local work")
                .forget();
            if self.fail_reads.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(BlobExchangeError::new(
                    BlobExchangeFailure::Store,
                    std::io::Error::from(std::io::ErrorKind::StorageFull),
                ));
            }
            Ok(Bytes::from_static(b"image"))
        }
        async fn export(&self, _: BlobDigest, target: &Path) -> Result<u64, BlobExchangeError> {
            self.started.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            if let Some(failure) = &self.failed_export {
                if failure.obstruct_cleanup {
                    tokio::fs::remove_file(target).await.unwrap();
                    tokio::fs::create_dir(target).await.unwrap();
                }
                *failure.output.lock().unwrap() = Some(target.to_owned());
                return Err(if failure.cancelled {
                    BlobExchangeError::refused(BlobExchangeFailure::Cancelled)
                } else {
                    BlobExchangeError::new(
                        BlobExchangeFailure::Store,
                        std::io::Error::from(std::io::ErrorKind::StorageFull),
                    )
                });
            }
            tokio::fs::write(target, b"retried")
                .await
                .map_err(|source| BlobExchangeError::new(BlobExchangeFailure::Store, source))?;
            Ok(7)
        }
    }

    #[async_trait]
    impl BlobTransferPort for BlockingBlobTransfer {
        async fn publish(
            &self,
            _ciphertext: Bytes,
            _reason: TagReason,
        ) -> Result<BlobDigest, BlobError> {
            Err(BlobError::Internal("unused".into()))
        }

        async fn publish_path(
            &self,
            _path: &std::path::Path,
            _reason: TagReason,
        ) -> Result<BlobDigest, BlobError> {
            Err(BlobError::Internal("unused".into()))
        }

        async fn issue_ticket(&self, _digest: &BlobDigest) -> Result<BlobTicket, BlobError> {
            Err(BlobError::Internal("unused".into()))
        }

        async fn has(&self, _digest: &BlobDigest) -> Result<bool, BlobError> {
            Ok(false)
        }

        async fn tag(&self, _digest: &BlobDigest, _reason: TagReason) -> Result<(), BlobError> {
            Ok(())
        }

        async fn untag(&self, _reason: TagReason) -> Result<(), BlobError> {
            Ok(())
        }

        fn digest_of(&self, ticket: &BlobTicket) -> Result<BlobDigest, BlobError> {
            let bytes: [u8; 32] = ticket
                .as_bytes()
                .try_into()
                .map_err(BlobError::invalid_ticket_from)?;
            Ok(BlobDigest::from_bytes(bytes))
        }
    }

    use uc_core::file_transfer::FileTransferCancellationReason;
    use uc_core::ids::DeviceId;

    #[tokio::test(start_paused = true)]
    async fn attempt_cancel_settles_slow_local_work_even_when_the_caller_is_abandoned() {
        for abandon_waiter in [false, true] {
            let transfer = Arc::new(BlockingBlobTransfer::new());
            let facade = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
                downloads: {
                    use crate::transfer::blob::test_support::{
                        FakeLocalStore, RejectUnexpectedNetwork,
                    };
                    use crate::transfer::blob::transport::{BlobDownloadResources, BlobTransport};
                    let local = Arc::new(FakeLocalStore(transfer.clone()));
                    Arc::new(BlobDownloadResources {
                        transport: BlobTransport::new(
                            Arc::new(RejectUnexpectedNetwork),
                            Arc::new(RejectUnexpectedNetwork),
                            Arc::new(RejectUnexpectedNetwork),
                            Arc::new(RejectUnexpectedNetwork),
                        ),
                        source: local.clone(),
                        store: local,
                    })
                },
                hash: Arc::new(TestHash),
                blob_transfer: transfer.clone(),
                blob_reference: Arc::new(TestReferences),
                transfer_cipher: Arc::new(TestCipher),
                host_event_emitter: None,

                file_transfer: None,
            }));
            let fetch = tokio::spawn({
                let facade = Arc::clone(&facade);
                async move {
                    facade
                        .fetch_blob(FetchBlobCommand {
                            cancellation: CancellationToken::new(),
                            ticket: BlobTicket::from_bytes(vec![1; 32]),
                            entry_id: EntryId::from("sender-image"),
                            transfer_context: Some(FetchTransferContext {
                                item_role: ReceiveItemRole::Representation,
                                batch: None,
                                transfer_id: "entry:attempt:a1:representation:0".to_owned(),
                                entry_id: "entry".to_owned(),
                                attempt_id: Some("a1".to_owned()),
                                peer_id: "peer".to_owned(),
                                total_bytes: Some(5),
                                filename: String::new(),
                                batch_position: BatchPosition::Only,
                                individual_lifecycle: true,
                            }),
                        })
                        .await
                }
            });
            let permit = transfer.started.acquire().await.expect("fetch started");
            permit.forget();

            if abandon_waiter {
                fetch.abort();
            }
            assert!(facade.owns_inbound_work("entry:attempt:a1:representation:0"));
            let incomplete = facade
                .cancel_inbound_attempt(
                    "a1",
                    FileTransferCancellationReason::LocalUser,
                    TransferCancellationWait::Bounded,
                )
                .await;
            assert!(matches!(
                incomplete,
                Err(BlobTransferError::CancellationIncomplete(_))
            ));
            transfer.release.add_permits(1);
            let cancelled = facade
                .cancel_inbound_attempt(
                    "a1",
                    FileTransferCancellationReason::LocalUser,
                    TransferCancellationWait::Bounded,
                )
                .await
                .expect("cancel attempt");
            let fetch_result = fetch.await;
            assert_eq!(cancelled, 1);
            if abandon_waiter {
                assert!(fetch_result.expect_err("waiter aborted").is_cancelled());
            } else {
                assert!(matches!(
                    fetch_result.expect("fetch task"),
                    Err(BlobTransferError::Cancelled)
                ));
            }
            facade
                .transport
                .shutdown(super::super::transport::BlobShutdownWait::Complete)
                .await
                .expect("full shutdown");
        }
    }
}
