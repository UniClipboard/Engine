use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

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
use uc_core::ports::ContentHashPort;

use crate::facade::host_event::{HostEvent, HostEventBus, TransferHostEvent};
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
#[derive(Debug, Clone)]
pub struct FetchBlobToPathCommand {
    pub ticket: BlobTicket,
    pub entry_id: EntryId,
    pub target_path: PathBuf,
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
    #[error("publish blob failed: {0}")]
    Publish(String),
    #[error("fetch blob failed: {0}")]
    Fetch(String),
    /// fetch_blob / fetch_blob_to_path 在进行中被外部 cancel(用户点取消、
    /// timeout sweep、删除流程)。目标文件可能是 partial,调用方应当把
    /// `target_path` 视为不可用并交由 cleanup 删除。与 `Fetch` 不同:
    /// Fetch 表达"传输本身失败",Cancelled 表达"传输被主动撤回"。
    #[error("fetch blob cancelled")]
    Cancelled,
    #[error("record blob transfer failed: {0}")]
    Persistence(String),
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

/// 一次进行中 fetch 的取消句柄。
struct InflightFetch {
    /// trigger 后:facade 内的 select! 唤醒 fetch 路径,提前 break。
    token: CancellationToken,
    /// 让 cancel 路径能调 `BlobTransferPort::shutdown_inflight_fetch`
    /// 把 iroh-blobs Downloader 内部 actor task 用的那条 QUIC connection
    /// 也撕掉(只 break caller 没用 —— actor 还会继续下载完整 blob)。
    ticket: BlobTicket,
    /// The same session used by fetch progress and terminal settlement.
    session: Option<Arc<ReceiverTransferHandle>>,
    /// Directory receive attempt that owns this member fetch.
    attempt_id: Option<String>,
    /// 反向上报通道。`cancel_inbound_transfer` 在撕 QUIC connection
    /// 之前先用它给 sender 发一帧 `Cancelled` 状态,让 sender UI 也能
    /// 看到中性"已取消"展示(而不是 fetch error 路径反向推的 Failed)。
    /// 反向通道用独立 ALPN,撕 fetch connection 不会影响这条帧。
    outbound: Option<OutboundReportContext>,
}

pub struct BlobTransferFacade {
    publish_uc: Arc<PublishBlobUseCase>,
    fetch_uc: Arc<FetchBlobUseCase>,
    /// `BlobTransferPort` 句柄留一份给取消路径调 `shutdown_inflight_fetch`;
    /// use case 内部也持有同一个 `Arc<dyn BlobTransferPort>`。
    blob_transfer: Arc<dyn BlobTransferPort>,
    host_event_emitter: Option<SharedHostEventEmitter>,
    outbound_progress_reporter: Option<Arc<dyn OutboundProgressReporterPort>>,
    file_transfer: Option<Arc<FileTransferFacade>>,
    /// 进行中 fetch 的取消句柄登记表。key=`transfer_id`。
    ///
    /// fetch_blob / fetch_blob_to_path 在带 `transfer_context` 时
    /// 入口注册、出口移除。`cancel_inbound_transfer` 通过 transfer_id
    /// 查表,trigger token + 通过 ticket 撕 QUIC connection + 落
    /// `Cancelled` domain event。
    inflight_fetches: Arc<Mutex<HashMap<String, InflightFetch>>>,
}

impl BlobTransferFacade {
    pub(crate) fn new(deps: BlobTransferDeps) -> Self {
        let publish_uc = Arc::new(PublishBlobUseCase::new(
            Arc::clone(&deps.hash),
            Arc::clone(&deps.blob_transfer),
            Arc::clone(&deps.blob_reference),
            Arc::clone(&deps.transfer_cipher),
        ));
        let fetch_uc = Arc::new(FetchBlobUseCase::new(
            deps.hash,
            Arc::clone(&deps.blob_transfer),
            deps.blob_reference,
            deps.transfer_cipher,
        ));
        Self {
            publish_uc,
            fetch_uc,
            blob_transfer: deps.blob_transfer,
            host_event_emitter: deps.host_event_emitter,
            outbound_progress_reporter: deps.outbound_progress_reporter,
            file_transfer: deps.file_transfer,
            inflight_fetches: Arc::new(Mutex::new(HashMap::new())),
        }
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
            .map_err(|error| BlobTransferError::Persistence(error.to_string()))
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
                BlobTransferError::Persistence("active file transfer session is missing".into())
            })
    }

    pub async fn record_unfetched_failure(&self, ctx: FetchTransferContext, detail: String) {
        match self.begin_lifecycle(&ctx, String::new()).await {
            Ok(Some(session)) => {
                if let Err(error) = session
                    .fail(FileTransferFailureReason::Unknown, Some(detail))
                    .await
                {
                    warn!(error = %error, transfer_id = %ctx.transfer_id, "could not finish unfetched transfer failure");
                }
            }
            Ok(None) => {}
            Err(error) => {
                warn!(error = %error, transfer_id = %ctx.transfer_id, "could not begin unfetched transfer failure");
            }
        }
    }

    /// 取消一次进行中的 inbound fetch。
    ///
    /// 四件事按顺序发生:
    /// 1. **先**通过反向 progress 通道给 sender 发一帧
    ///    `OutboundProgressStatus::Cancelled { reason }` —— 让 sender UI
    ///    收到 cancel 终态(而不是后续 fetch error 路径反向推的 Failed)。
    ///    反向通道用独立 ALPN,与待撕的 fetch QUIC connection 物理隔离,
    ///    所以这一步必须在 step 3 之前完成,**且 await 直到写帧返回**,
    ///    避免 reporter 内部还没真把帧写入 socket 就被后续 fetch error
    ///    覆盖。
    /// 2. trigger registry 里这个 `transfer_id` 对应的 cancellation token —
    ///    fetch 路径里的 `tokio::select!` 会立刻唤醒,fetch_uc 那条分支被
    ///    drop,本端 receiver 不再接收 bytes;
    /// 3. 通过 `BlobTransferPort::shutdown_inflight_fetch(&ticket)` 撕掉
    ///    iroh-blobs Downloader actor 内部用的 QUIC connection,让 actor
    ///    task 也真的退出(否则它会继续把整个 blob 下完);
    /// 4. Settle the same file-transfer session as cancelled.
    ///
    /// 幂等:同一个 `transfer_id` 不在 registry(没有进行中的 fetch,或者
    /// 已经被取消过)时返回 `Ok(())`,不重复发事件。
    ///
    /// `reason` 透传给 `Cancelled` 事件与反向 cancel 帧 ——
    /// - 用户点取消按钮:`LocalUser`
    /// - timeout sweep 触发:`Timeout`
    /// - 删除流程联动:`Replaced` 或 `LocalUser`
    pub async fn cancel_inbound_transfer(
        &self,
        transfer_id: &str,
        reason: FileTransferCancellationReason,
    ) -> Result<InboundCancelOutcome, BlobTransferError> {
        // 一次性取出 entry,避免锁跨 await。
        let entry = self
            .inflight_fetches
            .lock()
            .map_err(|_| BlobTransferError::Fetch("in-flight fetch registry is poisoned".into()))?
            .remove(transfer_id);
        let Some(entry) = entry else {
            info!(
                transfer_id,
                "cancel_inbound_transfer: no in-flight fetch, no-op"
            );
            return Ok(InboundCancelOutcome::NotInflight);
        };

        // Step 1: 先告诉 sender 我们要取消了。一定要在撕 connection 之前
        // 完成 ——cancel 帧走的是独立 ALPN 通道,与 fetch QUIC connection
        // 不共用,撕 fetch 不会影响这一帧;但 reporter 内部要 dial / open_uni
        // / write,需要 await 完成。
        //
        // **视角翻转**:`reason` 是 receiver 视角(`LocalUser` 是 receiver
        // 设备上的用户、`RemotePeer` 是 sender 那边触发的)。沿反向通道发
        // 给 sender 时,把 device-relative 的两个变体对调一下,这样 sender
        // 端 UI 拿到的就是 sender 视角的 reason —— 可以直接用同一张 i18n
        // 表渲染("你/对方"的归属正确)。`Timeout` / `Replaced` / `Unknown`
        // 是设备无关的共同语义,原样透传。
        if let Some(outbound) = entry.outbound.as_ref() {
            let sender_view_reason = flip_cancel_reason_perspective(reason);
            outbound
                .reporter
                .report(
                    &outbound.target,
                    &outbound.transfer_id,
                    0,
                    None,
                    OutboundProgressStatus::Cancelled {
                        reason: sender_view_reason,
                    },
                )
                .await;
        }

        // Step 2: trigger select! 让 fetch 路径退出。
        entry.token.cancel();

        // Step 3: 撕 QUIC connection 让 iroh-blobs actor task 也退出。
        // best-effort: shutdown 失败说明 connection 已经关了,语义上等价
        // 于"已经撤回",继续走 step 4。
        if let Err(err) = self
            .blob_transfer
            .shutdown_inflight_fetch(&entry.ticket)
            .await
        {
            warn!(
                transfer_id,
                error = %err,
                "cancel_inbound_transfer: shutdown_inflight_fetch failed (treated as already gone)"
            );
        }

        // Step 4: 落 Cancelled 事件。peer_id 在 fetch 入口注册时一起存进
        // registry,这里取出来直接发,避免编一个污染事件流。
        if let Some(session) = entry.session {
            if let Err(error) = session.cancel(reason).await {
                warn!(transfer_id, error = %error, "cancel_inbound_transfer: session cancel failed");
            }
        }

        Ok(InboundCancelOutcome::Cancelled)
    }

    pub(crate) async fn cancel_inbound_attempt(
        &self,
        attempt_id: &str,
        reason: FileTransferCancellationReason,
    ) -> Result<usize, BlobTransferError> {
        let transfer_ids = self
            .inflight_fetches
            .lock()
            .map_err(|_| BlobTransferError::Fetch("in-flight fetch registry is poisoned".into()))?
            .iter()
            .filter(|(_, fetch)| fetch.attempt_id.as_deref() == Some(attempt_id))
            .map(|(transfer_id, _)| transfer_id.clone())
            .collect::<Vec<_>>();

        let mut cancelled = 0usize;
        let mut first_error = None;
        for transfer_id in transfer_ids {
            match self.cancel_inbound_transfer(&transfer_id, reason).await {
                Ok(InboundCancelOutcome::Cancelled) => cancelled += 1,
                Ok(InboundCancelOutcome::NotInflight) => {}
                Err(error) => {
                    warn!(%transfer_id, %error, "failed to cancel directory member transfer");
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(cancelled),
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
            .map_err(|e| BlobTransferError::Publish(e.to_string()))?;
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
            .map_err(|e| BlobTransferError::Publish(e.to_string()))?;
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
        let iroh_tag_entry_id = command.entry_id.clone();
        let outbound_ctx = self.build_outbound_context(command.transfer_context.as_ref());
        let lifecycle_session = match command.transfer_context.as_ref() {
            Some(ctx) if ctx.individual_lifecycle || ctx.batch_position.is_first() => {
                self.begin_lifecycle(ctx, String::new()).await?
            }
            Some(ctx) => self.existing_lifecycle(ctx).await?,
            None => None,
        };
        let progress_sink = self
            .build_progress_sink(
                command.transfer_context.as_ref(),
                lifecycle_session.clone(),
                outbound_ctx.clone(),
            )
            .await;
        if let (Some(ctx), Some(sink)) = (command.transfer_context.as_ref(), progress_sink.as_ref())
        {
            if ctx.individual_lifecycle || ctx.batch_position.is_first() {
                sink.report(0, ctx.total_bytes).await;
            }
        }

        let cancel_token = CancellationToken::new();
        if let Some(ctx) = command.transfer_context.as_ref() {
            self.inflight_fetches
                .lock()
                .map_err(|_| {
                    BlobTransferError::Fetch("in-flight fetch registry is poisoned".into())
                })?
                .insert(
                    ctx.transfer_id.clone(),
                    InflightFetch {
                        token: cancel_token.clone(),
                        ticket: command.ticket.clone(),
                        session: lifecycle_session.clone(),
                        attempt_id: ctx.attempt_id.clone(),
                        outbound: outbound_ctx.clone(),
                    },
                );
        }

        let result = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => Err(BlobTransferError::Cancelled),
            result = self.fetch_uc.execute(FetchBlobInput {
                ticket: command.ticket,
                entry_id: iroh_tag_entry_id,
                progress: progress_sink.clone(),
            }) => result.map_err(|error| BlobTransferError::Fetch(error.to_string())),
        };

        if let Some(ctx) = command.transfer_context.as_ref() {
            self.inflight_fetches
                .lock()
                .map_err(|_| {
                    BlobTransferError::Fetch("in-flight fetch registry is poisoned".into())
                })?
                .remove(&ctx.transfer_id);
        }

        match result {
            Ok(outcome) => {
                if let Some(ctx) = command.transfer_context.as_ref() {
                    let final_size = outcome.plaintext.len() as u64;
                    let total = ctx.total_bytes.or(Some(final_size));
                    // 最终一帧 100% Progress 每次都推(进度条 UI 体验),
                    // 但 complete lifecycle + outbound terminal Completed
                    // 只在 batch 收尾时发,避免提前告诉 sender"完成了"。
                    if let Some(sink) = progress_sink.as_ref() {
                        sink.report(final_size, total).await;
                    }
                    if ctx.individual_lifecycle {
                        if let Some(session) = lifecycle_session.as_ref() {
                            if let Err(error) = session.complete().await {
                                warn!(transfer_id = %ctx.transfer_id, error = %error, "blob fetch: session completion failed");
                            }
                        }
                    }
                    if ctx.batch_position.is_last() {
                        if !ctx.individual_lifecycle {
                            if let Some(session) = lifecycle_session.as_ref() {
                                if let Err(error) = session.complete().await {
                                    warn!(transfer_id = %ctx.transfer_id, error = %error, "blob fetch: batch session completion failed");
                                }
                            }
                        }
                        self.report_outbound_terminal(
                            ctx,
                            final_size,
                            total,
                            OutboundProgressStatus::Completed,
                        )
                        .await;
                    }
                }
                Ok(FetchBlobResult {
                    plaintext: outcome.plaintext,
                    entry_id: outcome.entry_id,
                    plaintext_hash: outcome.plaintext_hash,
                    digest: outcome.digest,
                })
            }
            Err(BlobTransferError::Cancelled) => Err(BlobTransferError::Cancelled),
            Err(BlobTransferError::Fetch(msg)) => {
                if let Some(ctx) = command.transfer_context.as_ref() {
                    if let Some(session) = lifecycle_session.as_ref() {
                        if let Err(error) = session
                            .fail(FileTransferFailureReason::Unknown, Some(msg.clone()))
                            .await
                        {
                            warn!(transfer_id = %ctx.transfer_id, error = %error, "blob fetch: session failure settlement failed");
                        }
                    }
                    self.report_outbound_terminal(
                        ctx,
                        0,
                        ctx.total_bytes,
                        OutboundProgressStatus::Failed,
                    )
                    .await;
                }
                Err(BlobTransferError::Fetch(msg))
            }
            Err(other) => Err(other),
        }
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
        let iroh_tag_entry_id = command.entry_id.clone();
        let outbound_ctx = self.build_outbound_context(command.transfer_context.as_ref());
        let lifecycle_session = match command.transfer_context.as_ref() {
            Some(ctx) if ctx.individual_lifecycle || ctx.batch_position.is_first() => {
                let cached_path = if ctx.attempt_id.is_some() {
                    String::new()
                } else {
                    command.target_path.to_string_lossy().into_owned()
                };
                self.begin_lifecycle(ctx, cached_path).await?
            }
            Some(ctx) => self.existing_lifecycle(ctx).await?,
            None => None,
        };
        let progress_sink = self
            .build_progress_sink(
                command.transfer_context.as_ref(),
                lifecycle_session.clone(),
                outbound_ctx.clone(),
            )
            .await;
        if let (Some(ctx), Some(sink)) = (command.transfer_context.as_ref(), progress_sink.as_ref())
        {
            if ctx.individual_lifecycle || ctx.batch_position.is_first() {
                sink.report(0, ctx.total_bytes).await;
            }
        }

        // 注册取消句柄:带 transfer_context 时,在 inflight registry 留下
        // (token, ticket, peer_id, outbound),让 `cancel_inbound_transfer`
        // 能查到并 trigger token + 反向推 cancel 帧 + 撕 QUIC connection。
        // 无 transfer_context 时(纯静默拉取,例如 CLI 工具),token 仍然
        // 创建但不进 registry —— select! 这一条分支就永远不会被唤醒,
        // 等价于原行为。
        let cancel_token = CancellationToken::new();
        if let Some(ctx) = command.transfer_context.as_ref() {
            self.inflight_fetches
                .lock()
                .map_err(|_| {
                    BlobTransferError::Fetch("in-flight fetch registry is poisoned".into())
                })?
                .insert(
                    ctx.transfer_id.clone(),
                    InflightFetch {
                        token: cancel_token.clone(),
                        ticket: command.ticket.clone(),
                        session: lifecycle_session.clone(),
                        attempt_id: ctx.attempt_id.clone(),
                        outbound: outbound_ctx.clone(),
                    },
                );
        }

        // select! 把 fetch_uc 包到一个取消感知的 future 里。cancel arm 中
        // 不重发 Cancelled lifecycle event —— `cancel_inbound_transfer` 已
        // 经在调用端把 event 落了,这里只负责让 fetch 路径退出。
        let result = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                Err(BlobTransferError::Cancelled)
            }
            res = self.fetch_uc.execute_to_path(FetchBlobPathInput {
                ticket: command.ticket,
                entry_id: iroh_tag_entry_id,
                target_path: command.target_path,
                progress: progress_sink.clone(),
            }) => {
                res.map_err(|e| BlobTransferError::Fetch(e.to_string()))
            }
        };

        // fetch 出口必移除 registry,否则 cancel_inbound_transfer 会找到
        // 一个已经结束的 entry,无害但会污染 metric。
        if let Some(ctx) = command.transfer_context.as_ref() {
            self.inflight_fetches
                .lock()
                .map_err(|_| {
                    BlobTransferError::Fetch("in-flight fetch registry is poisoned".into())
                })?
                .remove(&ctx.transfer_id);
        }

        match result {
            Ok(outcome) => {
                if let Some(ctx) = command.transfer_context.as_ref() {
                    let final_size = outcome.bytes_written;
                    let total = ctx.total_bytes.or(Some(final_size));
                    if let Some(sink) = progress_sink.as_ref() {
                        sink.report(final_size, total).await;
                    }
                    if ctx.individual_lifecycle {
                        if let Some(session) = lifecycle_session.as_ref() {
                            if let Err(error) = session.complete().await {
                                warn!(transfer_id = %ctx.transfer_id, error = %error, "blob fetch: session completion failed");
                            }
                        }
                    }
                    if ctx.batch_position.is_last() {
                        if !ctx.individual_lifecycle {
                            if let Some(session) = lifecycle_session.as_ref() {
                                if let Err(error) = session.complete().await {
                                    warn!(transfer_id = %ctx.transfer_id, error = %error, "blob fetch: batch session completion failed");
                                }
                            }
                        }
                        self.report_outbound_terminal(
                            ctx,
                            final_size,
                            total,
                            OutboundProgressStatus::Completed,
                        )
                        .await;
                    }
                }
                Ok(FetchBlobToPathResult {
                    entry_id: outcome.entry_id,
                    plaintext_hash: outcome.plaintext_hash,
                    digest: outcome.digest,
                    bytes_written: outcome.bytes_written,
                })
            }
            Err(BlobTransferError::Cancelled) => {
                // 取消路径不发任何 lifecycle / outbound 终态 ——
                // `cancel_inbound_transfer` 已经在撕 connection 之前把
                // `OutboundProgressStatus::Cancelled` 帧推给 sender,并
                // 落了 `Cancelled` domain event。这里再发 outbound Failed
                // 会让 sender 端 UI 从已经显示的"已取消"切回"失败"(race)。
                Err(BlobTransferError::Cancelled)
            }
            Err(e) => {
                let msg = e.to_string();
                if let Some(ctx) = command.transfer_context.as_ref() {
                    if let Some(session) = lifecycle_session.as_ref() {
                        if let Err(error) = session
                            .fail(FileTransferFailureReason::Unknown, Some(msg.clone()))
                            .await
                        {
                            warn!(transfer_id = %ctx.transfer_id, error = %error, "blob fetch: session failure settlement failed");
                        }
                    }
                    self.report_outbound_terminal(
                        ctx,
                        0,
                        ctx.total_bytes,
                        OutboundProgressStatus::Failed,
                    )
                    .await;
                }
                Err(BlobTransferError::Fetch(msg))
            }
        }
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
        let (Some(reporter), Some(tid), Some(target)) = (
            self.outbound_progress_reporter.as_ref(),
            ctx.outbound_transfer_id.as_ref(),
            ctx.outbound_target.as_ref(),
        ) else {
            return;
        };
        reporter
            .report(target, tid, bytes_transferred, total_bytes, status)
            .await;
    }
}

/// 反向上报上下文。同时配齐三个字段才会触发 outbound report。
#[derive(Clone)]
struct OutboundReportContext {
    reporter: Arc<dyn OutboundProgressReporterPort>,
    transfer_id: String,
    target: DeviceId,
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
                warn!(
                    transfer_id = %self.transfer_id,
                    error = %error,
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
            ob.reporter
                .report(
                    &ob.target,
                    &ob.transfer_id,
                    cumulative_bytes,
                    cumulative_total,
                    OutboundProgressStatus::InProgress,
                )
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Semaphore;
    use uc_core::clipboard::{ContentHash, HashAlgorithm};
    use uc_core::ports::blob::{BlobError, BlobReferenceError, TagReason};
    use uc_core::ports::security::{TransferCipherError, TransferCipherPort};

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

    struct BlockingBlobTransfer {
        started: Semaphore,
        release: Semaphore,
    }

    impl BlockingBlobTransfer {
        fn new() -> Self {
            Self {
                started: Semaphore::new(0),
                release: Semaphore::new(0),
            }
        }
    }

    #[async_trait]
    impl BlobTransferPort for BlockingBlobTransfer {
        async fn publish(
            &self,
            _ciphertext: Bytes,
            _reason: TagReason,
        ) -> Result<BlobDigest, BlobError> {
            Err(BlobError::Internal("unused".to_owned()))
        }

        async fn publish_path(
            &self,
            _path: &std::path::Path,
            _reason: TagReason,
        ) -> Result<BlobDigest, BlobError> {
            Err(BlobError::Internal("unused".to_owned()))
        }

        async fn issue_ticket(&self, _digest: &BlobDigest) -> Result<BlobTicket, BlobError> {
            Err(BlobError::Internal("unused".to_owned()))
        }

        async fn fetch(
            &self,
            _ticket: &BlobTicket,
            _progress: Option<&dyn BlobProgressSink>,
        ) -> Result<Bytes, BlobError> {
            self.started.add_permits(1);
            let permit = self
                .release
                .acquire()
                .await
                .map_err(|error| BlobError::Internal(error.to_string()))?;
            permit.forget();
            Ok(Bytes::from_static(b"image"))
        }

        async fn fetch_to_path(
            &self,
            _ticket: &BlobTicket,
            _target_path: &std::path::Path,
            _progress: Option<&dyn BlobProgressSink>,
        ) -> Result<BlobDigest, BlobError> {
            Err(BlobError::Internal("unused".to_owned()))
        }

        async fn shutdown_inflight_fetch(&self, _ticket: &BlobTicket) -> Result<(), BlobError> {
            self.release.add_permits(1);
            Ok(())
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
                .map_err(|_| BlobError::InvalidTicket)?;
            Ok(BlobDigest::from_bytes(bytes))
        }
    }

    #[tokio::test]
    async fn attempt_cancel_interrupts_representation_fetch() {
        let transfer = Arc::new(BlockingBlobTransfer::new());
        let facade = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
            hash: Arc::new(TestHash),
            blob_transfer: transfer.clone(),
            blob_reference: Arc::new(TestReferences),
            transfer_cipher: Arc::new(TestCipher),
            host_event_emitter: None,
            outbound_progress_reporter: None,
            file_transfer: None,
        }));
        let fetch = tokio::spawn({
            let facade = Arc::clone(&facade);
            async move {
                facade
                    .fetch_blob(FetchBlobCommand {
                        ticket: BlobTicket::from_bytes(vec![1; 32]),
                        entry_id: EntryId::from("sender-image"),
                        transfer_context: Some(FetchTransferContext {
                            transfer_id: "entry:attempt:a1:representation:0".to_owned(),
                            entry_id: "entry".to_owned(),
                            attempt_id: Some("a1".to_owned()),
                            peer_id: "peer".to_owned(),
                            total_bytes: Some(5),
                            filename: String::new(),
                            outbound_transfer_id: None,
                            outbound_target: None,
                            batch_position: BatchPosition::Only,
                            individual_lifecycle: true,
                        }),
                    })
                    .await
            }
        });
        let permit = transfer.started.acquire().await.expect("fetch started");
        permit.forget();

        let cancelled = facade
            .cancel_inbound_attempt("a1", FileTransferCancellationReason::LocalUser)
            .await
            .expect("cancel attempt");
        transfer.release.add_permits(1);
        let fetch_result = fetch.await.expect("fetch task");

        assert_eq!(cancelled, 1);
        assert!(matches!(fetch_result, Err(BlobTransferError::Cancelled)));
    }
}
