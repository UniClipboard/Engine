pub use super::output::BlobOutputTarget;
use super::output::PreparedBlobPath;
use crate::transfer::receive::batch::ReceiveBatch;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use self::finalization::{transfer_error, FetchFinalization};
use self::preparation::FetchPreparationState;
mod finalization;
use crate::transfer::policy::CANCEL_WAIT_TIMEOUT;
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::PoisonError;
mod preparation;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;

use uc_core::file_transfer::{
    FileTransferCancellationReason, FileTransferDirection, FileTransferFailureReason,
    OutboundProgressReporterPort, OutboundProgressStatus,
};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::blob::{
    BlobDigest, BlobProgressSink, BlobReferenceRepositoryPort, BlobTicket, BlobTransferPort,
    PlaintextHash,
};
use uc_core::ports::security::TransferCipherPort;
use uc_core::ports::{ContentHashPort, ReceiveItemRole};
use uc_observability_contract::{error_source::io_error_kind, log_fields::log_id, uc_warn};

use super::notifications::OutboundNotifications;
use crate::facade::host_event::{HostEvent, HostEventBus, TransferHostEvent};
use crate::transfer::blob::transport::{
    BlobDownloadResources, BlobExchangeError, BlobExchangeFailure, BlobPreparationStop,
    BlobTransport, DownloadCompletion,
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

pub(crate) struct BlobTransferDeps {
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
    /// 可选反向进度上报端口。提供时,fetch_blob 会在每次本地进度回调上额外
    /// 推一帧给数据来源端(sender),让 sender UI 能实时展示对端接收字节进度。
    /// 不提供则 fetch_blob 退化为只发本地 host event。
    pub outbound_progress_reporter: Option<Arc<dyn OutboundProgressReporterPort>>,
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
/// - `outbound_transfer_id` / `outbound_target`:可选的反向上报上下文。
///   设置时,sink 在每次进度回调上会通过 `OutboundProgressReporterPort`
///   把 (bytes, total, status) 推回数据来源端(sender),让 sender UI
///   实时显示对端接收字节进度。两个字段必须成对设置:
///   * `outbound_transfer_id` 是 sender 端的 entry_id(来自 V3 envelope
///     `blob_refs[i].entry_id`),sender 端用它索引本地 entry。
///   * `outbound_target` 是 sender 的 DeviceId,reporter 用它定位反向
///     连接目标(同 `peer_id` 的语义,但强类型化以避免重复字符串解析)。
///   只设置一个会被忽略 —— sink 拒绝出向上报。
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
    pub outbound_transfer_id: Option<String>,
    pub outbound_target: Option<DeviceId>,
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
}

#[derive(Clone)]
pub struct BlobTransferFacade {
    publish_uc: Arc<PublishBlobUseCase>,
    fetch_uc: Arc<FetchBlobUseCase>,
    /// 全部获取、通知和收尾任务由同一个传输负责人监督。
    pub(in super::super) transport: Arc<BlobTransport>,
    host_event_emitter: Option<SharedHostEventEmitter>,
    outbound_progress_reporter: Option<Arc<OutboundNotifications>>,
    file_transfer: Option<Arc<FileTransferFacade>>,
    /// 以具体接收项登记取消；重复调用接入原来的真实完成通知。
    inflight_fetches: Arc<Mutex<HashMap<String, InflightFetch>>>,
}

impl BlobTransferFacade {
    pub(crate) fn new(deps: BlobTransferDeps) -> Self {
        let outbound_progress_reporter = deps
            .outbound_progress_reporter
            .map(|reporter| OutboundNotifications::new(reporter, &deps.downloads.transport));
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
            outbound_progress_reporter,
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
        {
            return Ok(InboundCancelOutcome::NotInflight);
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
                    let job = self
                        .transport
                        .supervise(CancellationToken::new(), async move {
                            match wait_download_completion(&mut pending).await {
                                Ok(()) => {
                                    return Err(BlobExchangeError::refused(
                                        BlobExchangeFailure::AlreadyCompleted,
                                    ))
                                }
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
        let outbound_ctx = self.build_outbound_context(command.transfer_context.as_ref());
        let cancel_token = command.cancellation.child_token();
        let prepared = Arc::new(FetchPreparationState::default());
        let preparation = self.prepare_fetch(
            command.transfer_context.clone(),
            Box::pin(async { Ok(String::new()) }),
            outbound_ctx.clone(),
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
            owner: self.clone(),
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
                self.fetch_uc.execute(input, move |result| async move {
                    finalization
                        .finish(result.map(|outcome| {
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
                        }))
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
        let outbound_ctx = self.build_outbound_context(command.transfer_context.as_ref());
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
            outbound_ctx.clone(),
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
            owner: self.clone(),
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
                    .execute_to_path(input, move |result| async move {
                        finalization
                            .finish(result.map(|outcome| {
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
                            }))
                            .await
                    })
            },
        )?;
        operation.await.map_err(transfer_error)
    }

    /// 从 `FetchTransferContext` 构造 outbound report context。
    ///
    /// 同时配齐 reporter / outbound_transfer_id / outbound_target 才会返回
    /// `Some` —— 否则反向上报功能未启用,sink 与 cancel 路径都跳过 outbound
    /// 分支。让 sink 与 `InflightFetch` 共用同一份配置,避免两处构造逻辑
    /// 分叉。
    fn build_outbound_context(
        &self,
        ctx: Option<&FetchTransferContext>,
    ) -> Option<OutboundReportContext> {
        let ctx = ctx?;
        let reporter = self.outbound_progress_reporter.clone()?;
        let transfer_id = ctx.outbound_transfer_id.clone()?;
        let target = ctx.outbound_target?;
        Some(OutboundReportContext {
            item: ctx.transfer_id.clone(),
            batch: ctx.batch.clone(),
            reporter,
            transfer_id,
            target,
        })
    }

    async fn build_progress_sink(
        &self,
        ctx: Option<&FetchTransferContext>,
        session: Option<Arc<ReceiverTransferHandle>>,
        outbound: Option<OutboundReportContext>,
    ) -> Option<Arc<dyn BlobProgressSink>> {
        let ctx = ctx?;
        let base_bytes = match session.as_ref() {
            Some(session) => session.bytes_transferred().await,
            None => 0,
        };
        if session.is_none() && self.host_event_emitter.is_none() && outbound.is_none() {
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
            outbound,
        }))
    }

    /// fetch 收尾时把最终状态(Completed/Failed)推回 sender。中间进度
    /// 由 sink 在 adapter 节流回调里推送,这里只补"最后一帧"。
    async fn report_outbound_terminal(
        &self,
        ctx: &FetchTransferContext,
        bytes_transferred: u64,
        total_bytes: Option<u64>,
        status: OutboundProgressStatus,
    ) {
        if let Some(outbound) = self.build_outbound_context(Some(ctx)) {
            outbound.report(bytes_transferred, total_bytes, status);
        }
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

/// 反向上报上下文。同时配齐三个字段才会触发 outbound report。
#[derive(Clone)]
struct OutboundReportContext {
    item: String,
    batch: Option<Arc<ReceiveBatch>>,
    reporter: Arc<OutboundNotifications>,
    transfer_id: String,
    target: DeviceId,
}

impl OutboundReportContext {
    fn report(&self, bytes: u64, total: Option<u64>, status: OutboundProgressStatus) {
        if let Some(batch) = &self.batch {
            batch.record(
                self.reporter.clone(),
                self.target,
                &self.transfer_id,
                &self.item,
                bytes,
                status,
            );
        } else {
            self.reporter
                .report(&self.target, &self.transfer_id, bytes, total, status);
        }
    }
}

/// 把 receiver 视角的 cancel reason 翻转成 sender 视角的 reason。
///
/// `LocalUser` 与 `RemotePeer` 是 device-relative 的,沿反向通道发给对端
/// 设备时必须对调,否则 sender 端 UI 会把"对方取消"误显示为"你取消"。
/// `Timeout` / `Replaced` / `Unknown` 与设备无关,原样透传。
fn flip_cancel_reason_perspective(
    reason: FileTransferCancellationReason,
) -> FileTransferCancellationReason {
    match reason {
        FileTransferCancellationReason::LocalUser => FileTransferCancellationReason::RemotePeer,
        FileTransferCancellationReason::RemotePeer => FileTransferCancellationReason::LocalUser,
        other => other,
    }
}

struct FileTransferProgressSink {
    fallback_bus: Option<SharedHostEventEmitter>,
    session: Option<Arc<ReceiverTransferHandle>>,
    transfer_id: String,
    entry_id: String,
    attempt_id: Option<String>,
    peer_id: String,
    base_bytes: u64,
    fallback_total: Option<u64>,
    outbound: Option<OutboundReportContext>,
}

#[async_trait]
impl BlobProgressSink for FileTransferProgressSink {
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

        if let Some(ob) = self.outbound.as_ref() {
            ob.report(
                cumulative_bytes,
                cumulative_total,
                OutboundProgressStatus::InProgress,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    mod budget;
    mod finalization;
    mod targets;

    use crate::transfer::blob::transport::{BlobExchangeError, BlobReadStorePort, BlobReceived};
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
    struct BlockingBlobTransfer {
        started: Semaphore,
        release: Semaphore,
        fail_reads: std::sync::atomic::AtomicBool,
        protected: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl BlockingBlobTransfer {
        fn new() -> Self {
            Self {
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
        async fn export(&self, _: BlobDigest, _: &Path) -> Result<u64, BlobExchangeError> {
            panic!("unused local export")
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

    struct BlockedReporter {
        started: Semaphore,
    }
    #[async_trait]
    impl OutboundProgressReporterPort for BlockedReporter {
        async fn report(
            &self,
            _: &DeviceId,
            _: &str,
            _: u64,
            _: Option<u64>,
            _: OutboundProgressStatus,
        ) {
            self.started.add_permits(1);
            std::future::pending::<()>().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn attempt_cancel_settles_slow_local_work_with_a_permanently_blocked_reporter() {
        for abandon_waiter in [false, true] {
            let reporter = Arc::new(BlockedReporter {
                started: Semaphore::new(0),
            });
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
                outbound_progress_reporter: Some(reporter.clone()),
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
                                outbound_transfer_id: Some("sender-image".to_owned()),
                                outbound_target: Some(DeviceId::new("peer")),
                                batch_position: BatchPosition::Only,
                                individual_lifecycle: true,
                            }),
                        })
                        .await
                }
            });
            let permit = transfer.started.acquire().await.expect("fetch started");
            permit.forget();

            reporter
                .started
                .acquire()
                .await
                .expect("reporter entered")
                .forget();
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
