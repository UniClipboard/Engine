//! Slice 1 composition root for [`SpaceFacade`].
//!
//! Assembles the pairing stack (rendezvous client + iroh session adapter +
//! identity store + proof verifier) plus the pre-existing persistence /
//! identity ports from [`WiredDependencies`] into a single
//! [`SyncEngineAssembly`] that external callers (Tauri commands, CLI, daemon)
//! can drive through `Arc<SpaceFacade>`.
//!
//! Everything iroh-specific stays inside
//! [`uc_infra::network::iroh::IrohNode`] so this module depends only on
//! core ports + the `IrohNode` handle. When Slice 2 adds a clipboard-sync
//! transport, the extension point is `IrohNode::install_clipboard` rather
//! than a second stack.
//!
//! Shutdown is a two-step coordinated teardown: first drive the facade's
//! own shutdown (aborts the sponsor-side inbound orchestrator task + best-
//! effort `stop_network`), then shut the iroh router down so live
//! connections see a clean `CONNECTION_CLOSE` rather than waiting for peer
//! timeouts.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, instrument, warn};

use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::debug;

/// 反向 progress 翻译器对前端 emit 的硬上限(<=5/sec per transfer)。
///
/// 防御性节流——即便 peer 端(可能是旧版本、可能跑没修过的代码)以
/// 100+/sec 速率从反向 ALPN 通道发 progress 帧过来,本机译者也只把
/// 它转为最多 5/sec 的 host event 推给前端,避免 WebKit native 堆被
/// 高频 WS 帧冲爆(详见 findings.md 2026-05-23 Phase 4 vmmap 取证)。
///
/// 与 `uc-infra::network::iroh::blobs::PROGRESS_REPORT_INTERVAL` 是两条
/// 独立的防线:一个保护"我作为接收方时不要给对端发太快",一个保护
/// "我作为发送方时不要把对端发来的高频中转给前端"。两条都设 200ms。
///
/// **终态帧(Completed/Failed/Cancelled)永远绕过节流**,确保前端立刻看到
/// 最终状态,不会因为正好落在 cooldown 窗口里被丢掉。
const TRANSLATOR_PROGRESS_MIN_INTERVAL: Duration = Duration::from_millis(200);

use uc_application::facade::clipboard_capture::CaptureClipboardUseCase;
use uc_application::facade::{
    build_active_clipboard_pull_serve_port, ActiveClipboardDeps, ActiveClipboardFacade,
    ActiveClipboardLifecycle, ActiveClipboardPullServeFacadeDeps, AutomaticLegacyUpgradeDeps,
    BlobTransferDeps, BlobTransferFacade, ClipboardLiveIndexDeps, ClipboardLiveIndexPort,
    ClipboardLiveIndexer, ClipboardSnapshotDeps, ClipboardSyncDeps, ClipboardSyncFacade, HostEvent,
    HostEventBus, InboundClipboardApplyPort, MemberRosterDeps, MemberRosterFacade,
    MembershipConnectivityDeps, MembershipConvergenceDeps, SpaceAdmissionDeps,
    SpaceApplicationRuntime, SpaceConvergenceAssembly, SpaceConvergenceDeps, SpaceFacade,
    SpaceFacadeDeps, SpaceSessionDeps, SpaceTransitionDeps, TransferHostEvent,
    WorkspaceConvergenceDeps,
};
use uc_application::facade::{
    ApplyInboundClipboardUseCase, FileCacheBlobMaterializer, InboundApplyCommonDeps,
    InboundCapture as ApplyInboundCapture, InboundReceiveAttemptDeps, StoreOnlyPullDeps,
};
use uc_core::file_transfer::{
    FileTransferCancellationReason, FileTransferDirection, OutboundProgressStatus,
};
use uc_core::membership::{
    CurrentWorkspacePeerScopeError, CurrentWorkspacePeerScopePort, CurrentWorkspacePeerSnapshot,
    LegacyUpgradeDispatchPort,
};
use uc_core::ports::blob::BlobTransferPort;
use uc_core::ports::space::ProofPort;
use uc_core::ports::{
    ActiveClipboardDispatchPort, ActiveClipboardReceiverPort, ClipboardDispatchPort,
    ClipboardReceiverPort, ConnectionChannelPort, LocalIdentityPort, PresencePort,
};
use uc_infra::network::iroh::transfer_progress_adapter::InboundProgressEvent;
use uc_infra::network::iroh::{
    ActiveClipboardHandlers, ActiveClipboardPullHandlers, BlobHandlers, ClipboardHandlers,
    GroupUpdateHandlers, IrohIdentityStore, IrohNode, IrohNodeBuilder, IrohNodeError,
    TransferProgressHandlers,
};
use uc_infra::security::HmacProofAdapter;
// Re-exported so external callers can parametrise the assembly without
// having to `use uc_infra` themselves.
use crate::assembly::deps::{SharedRuntimeDeps, SyncEngineDeps};
use uc_application::deps::AppDeps;
use uc_infra::fs::{
    FsAtomicPublisher, FsDirectoryStagingCleaner, FsHiddenPathMarker, FsInboundFileTarget,
};
pub(crate) use uc_infra::network::iroh::IrohNodeConfig;
use uc_infra::security::DefaultMembershipSecurityUpdateAdapter;
use uc_infra::security::Sha256IdentityFingerprintFactory;

struct DeferredAdmissionOutboxDelivery;

#[derive(Default)]
struct DeferredCurrentWorkspacePeerScope {
    delegate: tokio::sync::RwLock<Option<Arc<dyn CurrentWorkspacePeerScopePort>>>,
}

impl DeferredCurrentWorkspacePeerScope {
    async fn install(&self, delegate: Arc<dyn CurrentWorkspacePeerScopePort>) {
        *self.delegate.write().await = Some(delegate);
    }
}

#[async_trait::async_trait]
impl CurrentWorkspacePeerScopePort for DeferredCurrentWorkspacePeerScope {
    async fn snapshot(
        &self,
    ) -> Result<CurrentWorkspacePeerSnapshot, CurrentWorkspacePeerScopeError> {
        let delegate = self.delegate.read().await.clone();
        match delegate {
            Some(delegate) => delegate.snapshot().await,
            None => Err(CurrentWorkspacePeerScopeError::Unavailable),
        }
    }
}

#[async_trait::async_trait]
impl uc_core::membership::AdmissionOutboxDeliveryPort for DeferredAdmissionOutboxDelivery {
    async fn deliver(
        &self,
        _attempt_id: uc_core::membership::AdmissionAttemptId,
        _message: &uc_core::membership::AdmissionOutboxMessageV1,
    ) -> Result<
        uc_core::membership::AdmissionOutboxDeliveryResultV1,
        uc_core::membership::AdmissionOutboxDeliveryError,
    > {
        Ok(uc_core::membership::AdmissionOutboxDeliveryResultV1::Deferred)
    }
}

#[cfg(not(feature = "lan-compat"))]
struct UnavailableMobileDeviceLookup;

#[cfg(not(feature = "lan-compat"))]
#[async_trait::async_trait]
impl uc_core::ports::FindMobileDeviceByIdPort for UnavailableMobileDeviceLookup {
    async fn find_by_device_id(
        &self,
        _device_id: &uc_core::mobile_sync::MobileDeviceId,
    ) -> Result<Option<uc_core::mobile_sync::MobileDevice>, uc_core::mobile_sync::MobileDeviceError>
    {
        Ok(None)
    }
}

/// Output of [`build_sync_engine_assembly`]. External callers keep the
/// whole assembly alive for the process lifetime; they only dispatch
/// user-facing commands through [`Self::facade`] / [`Self::roster`] and
/// run [`Self::shutdown`] once on exit.
pub struct SyncEngineAssembly {
    pub facade: Arc<SpaceFacade>,
    /// Slice 2 Phase 1 · T9:roster 查询门面(`list_with_presence` +
    /// `subscribe_presence_events`)。CLI `members` 命令从这里拿状态,
    /// tauri `get_roster` 将来也走同一条。共享同一个 `peer_addr_repo` /
    /// `presence` 实例,所以 F1 hook 填好的缓存这里能直接读到。
    pub roster: Arc<MemberRosterFacade>,
    /// Slice 2 Phase 2 · T10:剪切板同步门面。CLI `send` 通过这里走。
    /// 与 `roster` 同样共享 `peer_addr_repo` / `presence`,所以 F1 hook
    /// 喂好的 presence 缓存,`dispatch_entry` 能直接读到。
    pub clipboard_sync: Arc<ClipboardSyncFacade>,
    /// Shared peer reachability source for clipboard recovery and roster
    /// consumers. All subscribers observe the same online transitions.
    pub(crate) presence: Arc<dyn PresencePort>,
    /// Slice 3 Phase 2:大 payload 发布 / 拉取门面。CLI 与后续 daemon/UI
    /// 都从这里走完整的 hash 去重、加解密和 blob 传输编排。
    pub blob: Arc<BlobTransferFacade>,
    pub(crate) outbound_progress_reporter:
        Arc<dyn uc_core::file_transfer::OutboundProgressReporterPort>,
    /// Slice 3 Phase 1:大 payload 的 iroh-blobs 传输能力。Phase 2 的
    /// blob use case 会从这里接入。
    pub blob_transfer: Arc<dyn BlobTransferPort>,
    /// Slice 3 Phase 1:明文 hash → 密文 digest 去重缓存。与
    /// `blob_transfer` 分开装配,保持传输和 sqlite 缓存职责独立。
    /// Slice 4 Phase 1:presence port 直出。`facade` / `roster` /
    /// `clipboard_sync` 内部都已经持有同一份 Arc;daemon `PresenceMonitor`
    /// 也需要直接订阅事件流,所以这里再 clone 一份对外暴露,避免门面层
    /// 多包一道 subscribe 转发。
    /// Inbound active-clipboard state stream. The 0xC3 accept handler is
    /// installed on the shared iroh node during assembly so the ALPN is
    /// reachable; this port exposes the broadcast of inbound observations for
    /// a downstream consumer to drive register convergence. Held here as the
    /// single subscription seam, mirroring how `clipboard_sync` owns the bulk
    /// inbound stream.
    /// Active-clipboard register convergence facade (issue #1017). Background
    /// task ownership stays behind `active_clipboard_lifecycle`; callers use
    /// this facade only for convergence actions and queries.
    pub active_clipboard: Arc<ActiveClipboardFacade>,
    /// The shared iroh node. Held privately so callers can't bind a second
    /// node or install additional handlers after `spawn` — that would
    /// fragment peer identity (§"共用网络栈" decision, Slice 1 planning).
    iroh_node: IrohNode,
    /// Bulk clipboard receiver installed on the shared node. The complete
    /// application runtime subscribes after all apply and event dependencies
    /// are ready; network assembly never starts a partial inbound flow.
    clipboard_receiver: Arc<dyn ClipboardReceiverPort>,
    /// Owns the complete Active Clipboard worker topology: inbound
    /// convergence, peer-online resync, restore broadcast, and history
    /// resurface. This is the assembly's sole lifecycle seam for that module.
    active_clipboard_lifecycle: ActiveClipboardLifecycle,
    /// 反向"传输进度"翻译 worker 的 join handle。订阅
    /// `IrohTransferProgressAdapter` 的 inbound 流,将每帧 progress 翻译
    /// 为 `HostEvent::Transfer { direction: Sending, ... }` 并发到 emitter。
    /// 与 sync assembly 同生命周期。
    outbound_progress_translator: OutboundProgressRuntime,
    convergence_assembly: Arc<SpaceConvergenceAssembly>,
    space_application_runtime: SpaceApplicationRuntime,
}

impl SyncEngineAssembly {
    pub(crate) fn current_peer_scope(
        &self,
    ) -> Arc<dyn uc_core::membership::CurrentWorkspacePeerScopePort> {
        self.convergence_assembly.current_peer_scope()
    }

    pub(crate) fn subscribe_network_recovery_observations(
        &self,
    ) -> tokio::sync::broadcast::Receiver<uc_infra::network::iroh::NetworkRecoveryObservation> {
        self.iroh_node.subscribe_network_recovery_observations()
    }

    #[cfg(test)]
    pub(crate) async fn membership_attestation_is_reachable_for_test(&self) -> bool {
        self.iroh_node
            .accepts_protocol_for_test(
                uc_infra::network::iroh::membership_attestation_adapter::MEMBERSHIP_ATTESTATION_ALPN,
            )
            .await
    }

    #[cfg(test)]
    pub(crate) async fn membership_history_exchange_is_reachable_for_test(&self) -> bool {
        self.iroh_node
            .accepts_protocol_for_test(uc_infra::network::iroh::MEMBERSHIP_HISTORY_EXCHANGE_ALPN)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn admission_completion_recovery_is_reachable_for_test(&self) -> bool {
        self.iroh_node
            .accepts_protocol_for_test(uc_infra::network::iroh::ADMISSION_COMPLETION_RECOVERY_ALPN)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn legacy_upgrade_is_reachable_for_test(&self) -> bool {
        self.iroh_node
            .accepts_protocol_for_test(uc_infra::network::iroh::LEGACY_UPGRADE_ALPN)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn deprecated_removal_protocols_are_reachable_for_test(&self) -> bool {
        let (exchange, late, notice) = tokio::join!(
            self.iroh_node
                .accepts_protocol_for_test(b"uniclipboard/removal-exchange/1"),
            self.iroh_node
                .accepts_protocol_for_test(b"uniclipboard/removal-late/1"),
            self.iroh_node
                .accepts_protocol_for_test(b"uniclipboard/removal-notice/1"),
        );
        exchange || late || notice
    }

    /// Attach the externally-created restore source to the Active Clipboard
    /// lifecycle. The lifecycle itself enforces its single-attachment rule.
    pub fn attach_restore_broadcast(
        &self,
        rx: tokio::sync::mpsc::UnboundedReceiver<
            uc_application::facade::clipboard_write::RestoreBroadcastRequest,
        >,
    ) {
        if let Err(error) = self.active_clipboard_lifecycle.attach_restore_broadcast(rx) {
            warn!(error = %error, "active clipboard restore source attachment failed");
        }
    }

    pub(crate) fn space_application_handle(
        &self,
    ) -> uc_application::facade::SpaceApplicationHandle {
        self.space_application_runtime.handle()
    }

    pub(crate) fn clipboard_receiver(&self) -> Arc<dyn ClipboardReceiverPort> {
        Arc::clone(&self.clipboard_receiver)
    }

    pub(crate) fn convergence_content_gate(
        &self,
    ) -> Arc<dyn uc_core::membership::ContentExchangeGatePort> {
        self.convergence_assembly.removal_gate()
    }

    pub(crate) fn space_transition_recovery(
        &self,
    ) -> Arc<dyn uc_application::facade::SpaceTransitionRecoveryPort> {
        self.convergence_assembly.space_transition_recovery()
    }

    pub(crate) fn workspace_convergence(
        &self,
    ) -> Arc<uc_application::facade::WorkspaceConvergence> {
        self.convergence_assembly.workspace_convergence()
    }

    /// Coordinated teardown. Order matters:
    ///
    /// 1. [`SpaceFacade::on_shutdown`] aborts the sponsor-side inbound
    ///    orchestrator task so the `pairing_events` receiver is dropped
    ///    before the adapter is torn down.
    /// 2. [`IrohNode::shutdown`] shuts the iroh router, which fires
    ///    `ProtocolHandler::shutdown` on the pairing handler and emits
    ///    `CONNECTION_CLOSE` to any live peer.
    #[instrument(skip_all)]
    pub async fn shutdown(self, transfer_reason: FileTransferCancellationReason) {
        self.active_clipboard_lifecycle.shutdown().await;
        self.outbound_progress_translator
            .shutdown(transfer_reason)
            .await;
        self.space_application_runtime.shutdown().await;
        self.iroh_node.shutdown().await;
    }
}

/// 把接收端推回的进度帧翻译成 `HostEvent::Transfer` 发给 emitter。
///
/// 每帧:
/// * 先发一条 `Progress { direction: Sending }`,前端用它更新 sender 端
///   transfer 进度条 + 文案。
/// * 终态(`Completed` / `Failed`)再补一条 `StatusChanged`,前端把
///   `entryStatusById[transfer_id]` 切到对应状态,UI 退出 transferring。
///
/// transfer_id 字段直接复用帧里的 sender 端 entry_id —— sender 本地
/// entry_id == transfer_id 是发送侧的协议约定(同接收侧约定对称)。
struct OutboundProgressRuntime {
    commands: mpsc::UnboundedSender<OutboundProgressCommand>,
    task: JoinHandle<()>,
}

enum OutboundProgressCommand {
    Shutdown {
        reason: FileTransferCancellationReason,
        done: oneshot::Sender<()>,
    },
}

struct ActiveOutboundProgress {
    peer_id: String,
    bytes_transferred: u64,
    total_bytes: Option<u64>,
}

fn forward_outbound_progress(
    bus: &HostEventBus,
    last_progress_emit: &mut HashMap<String, Instant>,
    active: &mut HashMap<String, ActiveOutboundProgress>,
    event: InboundProgressEvent,
) {
    let terminal = match &event.status {
        OutboundProgressStatus::InProgress => None,
        OutboundProgressStatus::Completed => Some(("completed", None)),
        OutboundProgressStatus::Failed => {
            Some(("failed", Some("receiver fetch failed".to_string())))
        }
        OutboundProgressStatus::Cancelled { reason } => {
            Some(("cancelled", Some(reason.as_str().to_string())))
        }
    };

    // Terminal frames bypass throttling so the host receives the final bytes and state.
    let should_emit_progress = if terminal.is_some() {
        true
    } else {
        let now = Instant::now();
        match last_progress_emit.get(&event.transfer_id) {
            Some(previous) if now.duration_since(*previous) < TRANSLATOR_PROGRESS_MIN_INTERVAL => {
                false
            }
            _ => {
                last_progress_emit.insert(event.transfer_id.clone(), now);
                true
            }
        }
    };

    if should_emit_progress {
        bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::Progress {
            transfer_id: event.transfer_id.clone(),
            entry_id: Some(event.transfer_id.clone()),
            attempt_id: None,
            peer_id: event.from_device.as_str().to_string(),
            direction: FileTransferDirection::Sending,
            bytes_transferred: event.bytes_transferred,
            total_bytes: event.total_bytes,
        }));
    }

    if let Some((status, reason)) = terminal {
        // Terminal frames remove active tracking before shutdown can cancel it again.
        last_progress_emit.remove(&event.transfer_id);
        active.remove(&event.transfer_id);
        bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::StatusChanged {
            transfer_id: event.transfer_id.clone(),
            entry_id: event.transfer_id,
            attempt_id: None,
            status: status.to_string(),
            reason,
        }));
    } else {
        active.insert(
            event.transfer_id,
            ActiveOutboundProgress {
                peer_id: event.from_device.as_str().to_owned(),
                bytes_transferred: event.bytes_transferred,
                total_bytes: event.total_bytes,
            },
        );
    }
}

impl OutboundProgressRuntime {
    fn spawn(mut rx: broadcast::Receiver<InboundProgressEvent>, bus: Arc<HostEventBus>) -> Self {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            // Track each transfer's last host progress event for the 5/sec limit.
            // Terminal frames remove their entries so long-running sessions do not grow unbounded.
            let mut last_progress_emit: HashMap<String, Instant> = HashMap::new();
            let mut active = HashMap::<String, ActiveOutboundProgress>::new();
            loop {
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(OutboundProgressCommand::Shutdown { reason, done }) => {
                            while let Ok(event) = rx.try_recv() {
                                forward_outbound_progress(&bus, &mut last_progress_emit, &mut active, event);
                            }
                            for (transfer_id, progress) in active.drain() {
                                bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::Progress {
                                    entry_id: Some(transfer_id.clone()),
                                    transfer_id: transfer_id.clone(),
                                    attempt_id: None,
                                    peer_id: progress.peer_id,
                                    direction: FileTransferDirection::Sending,
                                    bytes_transferred: progress.bytes_transferred,
                                    total_bytes: progress.total_bytes,
                                }));
                                bus.emit_or_warn(HostEvent::Transfer(TransferHostEvent::StatusChanged {
                                    entry_id: transfer_id.clone(),
                                    transfer_id,
                                    attempt_id: None,
                                    status: "cancelled".to_owned(),
                                    reason: Some(reason.as_str().to_owned()),
                                }));
                            }
                            let _ = done.send(());
                            return;
                        }
                        None => return,
                    },
                    received = rx.recv() => match received {
                    Ok(event) => forward_outbound_progress(&bus, &mut last_progress_emit, &mut active, event),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        debug!(
                            skipped = n,
                            "outbound progress translator: lagged; some frames skipped"
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }}
            }
        });
        Self { commands, task }
    }

    async fn shutdown(self, reason: FileTransferCancellationReason) {
        let (done, received) = oneshot::channel();
        if self
            .commands
            .send(OutboundProgressCommand::Shutdown { reason, done })
            .is_ok()
        {
            let _ = received.await;
        }
        let _ = self.task.await;
    }
}

#[cfg(test)]
mod outbound_progress_tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use uc_application::facade::{EmitError, HostEventEmitterPort};
    use uc_core::ids::DeviceId;

    #[derive(Default)]
    struct Recorder(Mutex<Vec<HostEvent>>);

    impl HostEventEmitterPort for Recorder {
        fn emit(&self, event: HostEvent) -> Result<(), EmitError> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn network_recovery_finishes_each_active_outbound_transfer_once() {
        let (events, _) = broadcast::channel(4);
        let bus = Arc::new(HostEventBus::new());
        let recorder = Arc::new(Recorder::default());
        bus.register(
            "test",
            Arc::clone(&recorder) as Arc<dyn HostEventEmitterPort>,
        );
        let runtime = OutboundProgressRuntime::spawn(events.subscribe(), bus);

        events
            .send(InboundProgressEvent {
                from_device: DeviceId::new("peer-a"),
                transfer_id: "transfer-a".to_owned(),
                bytes_transferred: 12,
                total_bytes: Some(20),
                status: OutboundProgressStatus::InProgress,
            })
            .unwrap_or_else(|error| panic!("send progress: {error}"));
        events
            .send(InboundProgressEvent {
                from_device: DeviceId::new("peer-a"),
                transfer_id: "transfer-a".to_owned(),
                bytes_transferred: 12,
                total_bytes: Some(20),
                status: OutboundProgressStatus::InProgress,
            })
            .unwrap_or_else(|error| panic!("send progress: {error}"));
        tokio::task::yield_now().await;

        runtime
            .shutdown(FileTransferCancellationReason::ConnectivityRecovery)
            .await;

        let events = recorder
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let terminals = events.iter().filter(|event| matches!(event,
            HostEvent::Transfer(TransferHostEvent::StatusChanged { transfer_id, status, reason, .. })
            if transfer_id == "transfer-a" && status == "cancelled" && reason.as_deref() == Some("connectivity_recovery")
        )).count();
        assert_eq!(terminals, 1);
    }

    #[tokio::test]
    async fn network_recovery_does_not_repeat_an_existing_outbound_terminal() {
        let (events, _) = broadcast::channel(4);
        let bus = Arc::new(HostEventBus::new());
        let recorder = Arc::new(Recorder::default());
        bus.register(
            "test",
            Arc::clone(&recorder) as Arc<dyn HostEventEmitterPort>,
        );
        let runtime = OutboundProgressRuntime::spawn(events.subscribe(), bus);

        events
            .send(InboundProgressEvent {
                from_device: DeviceId::new("peer-a"),
                transfer_id: "transfer-a".to_owned(),
                bytes_transferred: 20,
                total_bytes: Some(20),
                status: OutboundProgressStatus::Completed,
            })
            .unwrap_or_else(|error| panic!("send terminal: {error}"));
        runtime
            .shutdown(FileTransferCancellationReason::ConnectivityRecovery)
            .await;

        let events = recorder
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let terminals = events.iter().filter(|event| matches!(event,
            HostEvent::Transfer(TransferHostEvent::StatusChanged { transfer_id, .. }) if transfer_id == "transfer-a"
        )).count();
        assert_eq!(terminals, 1);
    }
}

/// Failures during Slice 1 assembly. Bootstrap callers surface these as
/// fatal startup errors — there is no useful retry here.
#[derive(Debug, thiserror::Error)]
pub enum SyncEngineAssemblyError {
    #[error(transparent)]
    IrohNode(#[from] IrohNodeError),
    #[error(transparent)]
    AppVersionState(#[from] uc_core::ports::AppVersionStateError),
}

/// Assemble the Slice 1 `SpaceFacade` from an already-wired dependency
/// graph. Blocking responsibility: binds an iroh `Endpoint` and spawns its
/// router, so must be called inside a tokio runtime.
///
/// The resulting facade owns the entire Slice 1 lifecycle surface (A1 / A2
/// / B1 / B2 / F2). Call sites that also want to drive legacy setup should
/// keep holding their pre-existing `SetupFacade` alongside; the two
/// coexist until Slice 5 retires libp2p.
#[instrument(skip_all)]
pub async fn build_sync_engine_assembly(
    deps: &AppDeps,
    space_setup: &SyncEngineDeps,
    shared: &SharedRuntimeDeps,
    current_app_version: &str,
    #[cfg(feature = "lan-compat")] mobile_sync_ports: uc_mobile_lan::MobileSyncPorts,
    iroh_config: IrohNodeConfig,
) -> Result<SyncEngineAssembly, SyncEngineAssemblyError> {
    let previous_app_version = deps.app_version_state.read().await?;
    let initial_state_origin =
        uc_application::facade::WorkspaceConvergenceStateOrigin::from_version_transition(
            previous_app_version.as_deref(),
            current_app_version,
        );
    // IdentityFingerprintFactory is stateless — the one in SecurityPorts is
    // the same `Sha256IdentityFingerprintFactory` ZST, but we construct a
    // fresh one here rather than down-casting through `dyn` because
    // `IrohIdentityStore::new` takes the concrete factory trait object and
    // we'd have to re-wrap anyway.
    //
    let identity_store = Arc::new(IrohIdentityStore::new(
        Arc::clone(&space_setup.iroh_identity_storage),
        Arc::new(Sha256IdentityFingerprintFactory),
    ));

    // Bind the shared iroh node + install the pairing transport. The
    // returned PairingHandlers carry the trait objects SpaceFacadeDeps
    // wants; the iroh Router stays inside `IrohNode` so iroh types don't
    // leak out of this module.
    let mut builder = IrohNodeBuilder::bind(&identity_store, iroh_config).await?;
    let handlers = builder.install_pairing(
        Arc::clone(&deps.device.device_identity),
        Arc::clone(&deps.settings),
    );
    let membership_attestation = builder.build_membership_attestation_adapter(
        Arc::clone(&space_setup.membership_session),
        Arc::clone(&deps.device.device_identity),
        Arc::clone(&deps.settings),
        Arc::clone(&space_setup.current_member_signatures),
        Arc::clone(&deps.security.fingerprint),
    );
    let removal_identity = builder.build_membership_identity_adapter(
        Arc::clone(&space_setup.membership_session),
        Arc::clone(&deps.device.device_identity),
        Arc::clone(&deps.settings),
        Arc::clone(&deps.security.fingerprint),
    );
    let membership_history_exchange_adapter =
        builder.build_membership_history_exchange_adapter(Arc::clone(&space_setup.peer_addr_repo));
    let admission_completion_recovery_adapter = builder
        .build_admission_completion_recovery_adapter(Arc::clone(&space_setup.peer_addr_repo));
    let membership_transport = builder.build_membership_gossip_transport(
        Arc::clone(&space_setup.membership_session),
        Arc::clone(&deps.device.device_identity),
        Arc::clone(&deps.settings),
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&deps.security.fingerprint),
    );
    let group_update_recovery_scope = Arc::new(DeferredCurrentWorkspacePeerScope::default());
    let GroupUpdateHandlers {
        dispatch: group_update_dispatch,
    } = builder.install_group_updates(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&group_update_recovery_scope) as Arc<dyn CurrentWorkspacePeerScopePort>,
        Arc::clone(&deps.security.fingerprint),
        Arc::clone(&deps.security.space_access_ports.group_revocation),
    )?;
    let legacy_upgrade_adapter = builder.build_legacy_upgrade_adapter(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&deps.security.fingerprint),
    );
    let legacy_upgrade_dispatch: Arc<dyn LegacyUpgradeDispatchPort> =
        legacy_upgrade_adapter.clone();
    // Presence is installed before the convergence owner is assembled so the
    // owner can expose reachability as an independent product fact.
    let presence: Arc<dyn PresencePort> = builder.install_presence(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&deps.security.fingerprint),
        Arc::clone(&deps.system.clock),
    );
    let convergence_assembly = SpaceConvergenceAssembly::new(SpaceConvergenceDeps {
        workspace: WorkspaceConvergenceDeps {
            initial_state_origin,
            repository: Arc::clone(&space_setup.workspace_convergence_repository),
            admission_attempts: Arc::clone(&space_setup.admission_attempt_repository),
            historical_membership_signatures: Arc::new(
                uc_infra::security::OpenMlsHistoricalSignatureVerifier,
            ),
            admission_security_transition: Arc::new(
                uc_infra::security::AdmissionSecurityTransitionAdapter,
            ),
            prepare_sponsor_admission_security: Arc::clone(
                &deps
                    .security
                    .space_access_ports
                    .prepare_sponsor_admission_security,
            ),
            activate_sponsor_admission_security: Arc::clone(
                &deps
                    .security
                    .space_access_ports
                    .activate_sponsor_admission_security,
            ),
            activate_completion_helper_admission_security: Arc::clone(
                &deps
                    .security
                    .space_access_ports
                    .activate_completion_helper_admission_security,
            ),
            admission_space_transition: Arc::clone(&space_setup.admission_space_transition),
            admission_outbox_delivery: Arc::new(DeferredAdmissionOutboxDelivery),
            admission_completion_recovery: admission_completion_recovery_adapter.clone(),
            legacy_migration_recovery: Arc::clone(&space_setup.legacy_migration_recovery),
            member_signatures: Arc::clone(&space_setup.current_member_signatures),
            member_repo: Arc::clone(&deps.device.member_repo),
            membership_identity: removal_identity,
            announcement_material: membership_transport.clone(),
            security_updates: Arc::new(DefaultMembershipSecurityUpdateAdapter::new(
                Arc::clone(&space_setup.membership_session),
                Arc::clone(&space_setup.current_member_signatures),
                Arc::clone(&deps.security.space_access_ports.group_revocation),
            )),
            clock: Arc::clone(&deps.system.clock),
            device_identity: Arc::clone(&deps.device.device_identity),
            membership_history_exchange: membership_history_exchange_adapter.clone(),
            legacy_peer_probe: legacy_upgrade_adapter.clone(),
            trusted_peer_repo: Arc::clone(&shared.trusted_peer_repo),
            peer_addr_repo: Arc::clone(&space_setup.peer_addr_repo),
            presence: Arc::clone(&presence),
            space_protection: Arc::clone(&deps.security.space_access_ports.space_protection),
            group_bootstrap: Arc::clone(&deps.security.space_access_ports.group_bootstrap),
            own_device: deps.device.device_identity.current_device_id(),
        },
        membership: MembershipConvergenceDeps {
            candidate_repo: Arc::clone(&space_setup.membership_candidate_repo),
            announcement_repo: Arc::clone(&space_setup.membership_announcement_repo),
            outbox_repo: Arc::clone(&space_setup.membership_outbox_repo),
            security_updates: Arc::new(DefaultMembershipSecurityUpdateAdapter::new(
                Arc::clone(&space_setup.membership_session),
                Arc::clone(&space_setup.current_member_signatures),
                Arc::clone(&deps.security.space_access_ports.group_revocation),
            )),
            applied_security_updates: Arc::clone(
                &space_setup.membership_applied_security_update_repo,
            ),
            transport: membership_transport.clone(),
            clock: Arc::clone(&deps.system.clock),
            device_identity: Arc::clone(&deps.device.device_identity),
            announcement_material: membership_transport.clone(),
            member_signatures: Arc::clone(&space_setup.current_member_signatures),
            fingerprint_factory: Arc::clone(&deps.security.fingerprint),
            attestation: membership_attestation.clone(),
            verified_peer_promotion: Arc::clone(&space_setup.verified_peer_promotion),
            member_repo: Arc::clone(&deps.device.member_repo),
            trusted_peer_repo: Arc::clone(&shared.trusted_peer_repo),
            peer_address_repo: Arc::clone(&space_setup.peer_addr_repo),
            hash: Arc::clone(&deps.system.hash),
        },
        group_revocation: Arc::clone(&deps.security.space_access_ports.group_revocation),
        group_update_dispatch: Arc::clone(&group_update_dispatch),
        legacy_upgrade: AutomaticLegacyUpgradeDeps {
            member_repo: Arc::clone(&deps.device.member_repo),
            device_identity: Arc::clone(&deps.device.device_identity),
            protection: Arc::clone(&space_setup.legacy_protection),
            dispatch: Arc::clone(&legacy_upgrade_dispatch),
            presence: Arc::clone(&presence),
        },
    });
    group_update_recovery_scope
        .install(convergence_assembly.current_peer_scope())
        .await;
    builder.install_membership_handler(
        &membership_attestation,
        convergence_assembly.membership_attestation_endpoint(),
        &membership_transport,
        convergence_assembly.membership_gossip_endpoint(),
    )?;
    builder.install_membership_history_exchange(
        &membership_history_exchange_adapter,
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&deps.security.fingerprint),
        convergence_assembly.membership_history_exchange(),
    )?;
    builder.install_admission_completion_recovery(
        &admission_completion_recovery_adapter,
        convergence_assembly.admission_completion_recovery(),
    )?;
    builder.install_legacy_upgrade_handler(
        legacy_upgrade_adapter.as_ref(),
        convergence_assembly.legacy_upgrade_endpoint(),
    )?;
    // Phase 96 INDIC-01:连接通道单一真相源。复用同一 endpoint +
    // peer_addr_repo,纯读 adapter 不装 ALPN handler。
    let connection_channel: Arc<dyn ConnectionChannelPort> =
        builder.install_connection_channel(Arc::clone(&space_setup.peer_addr_repo));
    // Slice 2 Phase 2 · T10:同一节点装第三个 ALPN(剪切板同步)。dispatch
    // 复用 endpoint + peer_addr_repo,与 presence 共享 NAT/relay 映射;
    // receiver handler 通过 `member_repo` 把 `Connection::remote_id()` 反查
    // 成 DeviceId 再喂给应用层 broadcast。同样必须在 `spawn` 前装。
    let ClipboardHandlers {
        dispatch: clipboard_dispatch,
        receiver: clipboard_receiver,
    } = builder.install_clipboard(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&deps.security.fingerprint),
        Arc::clone(&presence),
    );
    let clipboard_dispatch: Arc<dyn ClipboardDispatchPort> = clipboard_dispatch;
    let clipboard_receiver: Arc<dyn ClipboardReceiverPort> = clipboard_receiver;
    // Install the active-clipboard state ALPN (0xC3) as an independent
    // sibling on the same node. A lone `.accept()` deeper in the node would
    // not be reachable from here — the handler has to be installed on this
    // builder before `spawn()`, so the seam is threaded through here. Produces
    // both the inbound receiver (broadcast of peer observations) and the
    // outbound dispatch port (re-broadcast of converged state), sharing the
    // endpoint + peer_addr_repo like install_clipboard.
    let ActiveClipboardHandlers {
        dispatch: active_clipboard_dispatch,
        receiver: active_clipboard_receiver,
    } = builder.install_active_clipboard(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&deps.security.fingerprint),
    );
    let active_clipboard_dispatch: Arc<dyn ActiveClipboardDispatchPort> = active_clipboard_dispatch;
    let active_clipboard_receiver: Arc<dyn ActiveClipboardReceiverPort> = active_clipboard_receiver;
    // 反向"传输进度"通道(receiver → sender):同一节点装第四个 ALPN。
    // 装在 install_blobs 之前是为了让 `IrohTransferProgressAdapter` 的
    // reporter 能在 BlobTransferDeps 构造时一起接入 facade。inbound_events
    // 由下面的 translator worker 消费,翻译为 host event。
    let TransferProgressHandlers {
        reporter: outbound_progress_reporter,
        inbound_events: outbound_progress_events,
    } = builder.install_transfer_progress(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&deps.security.fingerprint),
    );

    // Slice 3 Phase 1:同一节点装第五个 ALPN(iroh-blobs)。BlobReference
    // 是 sqlite 仓储,不跟 router 绑定;这里只拿传输 port。
    let BlobHandlers { blob_transfer } = builder
        .install_blobs(space_setup.iroh_blob_store_dir.clone())
        .await?;

    // Build the blob transfer facade now (before `spawn()`) so the
    // active-clipboard pull serve port can reuse it: the serve side publishes
    // large/image reps + free-standing files through it, re-issuing tickets
    // pinned to this device (D3). All of its deps are already available.
    let blob = Arc::new(BlobTransferFacade::new(BlobTransferDeps {
        hash: Arc::clone(&deps.system.hash),
        blob_transfer: Arc::clone(&blob_transfer),
        blob_reference: Arc::clone(&space_setup.blob_reference_repo),
        transfer_cipher: Arc::clone(&deps.security.transfer_cipher),
        // 共享同一根 host_event_bus —— daemon bootstrap 注册自己的 WS
        // emitter 之后, fetch_blob 自动开始向前端 fan-out progress 事件;
        // CLI 装配走同一 bus 但只挂着 logging emitter, 事件被静默打 log,
        // 不影响行为。状态切换(transferring / completed / failed)走
        // file_transfer lifecycle, 由 `FileTransferHostEventPublisher`
        // 统一发出。
        host_event_emitter: Some(Arc::clone(&shared.host_event_bus)),
        // 反向进度上报端口:接收端 fetch 进度通过新 ALPN 推回 sender。
        outbound_progress_reporter: Some(Arc::clone(&outbound_progress_reporter)),
        // file_transfer lifecycle facade —— iroh 路径每次 fetch 通过它落
        // `Started` / `Completed` / `Failed` 事件,让 file_transfer 表的
        // 状态投影与 sweep / reconcile workers 真正发挥作用。
        file_transfer: Some(Arc::clone(&shared.file_transfer_facade)),
    }));

    // Install the active-clipboard pull ALPN (0xC2, issue #1017 PR8) as a
    // further independent sibling, before `spawn()`. The serve port reuses the
    // resend crypto chain (reconstruct → publish blobs re-signing self-pinned
    // tickets, D3 → encode V3 → encrypt, D4); the returned client port drives
    // the inbound seam's on-demand pull.
    let active_clipboard_pull_serve =
        build_active_clipboard_pull_serve_port(ActiveClipboardPullServeFacadeDeps {
            entry_lookup: Arc::clone(&deps.clipboard.entry_ports.find_by_snapshot_hash),
            settings: Arc::clone(&deps.settings),
            transfer_cipher: Arc::clone(&deps.security.transfer_cipher),
            blob_publisher: Arc::clone(&blob),
            entry_file_set_repo: Arc::clone(&deps.storage.entry_file_set_repo),
            snapshot: ClipboardSnapshotDeps {
                entry_repo: Arc::clone(&deps.clipboard.entry_ports.get),
                selection_repo: Arc::clone(&deps.clipboard.selection_repo),
                representation_repo: Arc::clone(&deps.clipboard.representation_ports.get),
                rep_processing_repo: Arc::clone(
                    &deps.clipboard.representation_ports.update_processing_result,
                ),
                payload_resolver: Arc::clone(&deps.clipboard.payload_resolver),
                blob_store: Arc::clone(&deps.storage.blob_store),
            },
        });
    let ActiveClipboardPullHandlers {
        client: active_clipboard_pull_client,
    } = builder.install_active_clipboard_pull(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&deps.device.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&deps.security.fingerprint),
        active_clipboard_pull_serve,
        convergence_assembly.removal_gate(),
    );

    let iroh_node = builder.spawn();

    // Translator worker:从 sender 端的反向通道收 InboundProgressEvent,
    // 翻译为 application 层 HostEvent(Sending 方向)发到 host_event_bus。
    // 每次 progress → `TransferHostEvent::Progress`;终态 → 额外一帧
    // `StatusChanged`。shutdown 会显式停止并等待该任务。
    let outbound_progress_translator = OutboundProgressRuntime::spawn(
        outbound_progress_events,
        Arc::clone(&shared.host_event_bus),
    );

    // Pairing verification receives the one-shot invitation credential
    // explicitly; it never falls back to a Space content key.
    let proof_port: Arc<dyn ProofPort> = Arc::new(HmacProofAdapter::new());

    let local_identity: Arc<dyn LocalIdentityPort> = identity_store;
    let convergence_assembly = Arc::new(convergence_assembly);

    let facade = Arc::new(SpaceFacade::new(SpaceFacadeDeps {
        session: SpaceSessionDeps {
            space_access: deps.security.space_access_ports.clone(),
            setup_status: Arc::clone(&deps.setup_status),
            mobile_consumable_backfill: Arc::clone(&deps.clipboard.mobile_consumable_backfill),
        },
        admission: SpaceAdmissionDeps {
            local_identity: Arc::clone(&local_identity),
            device_identity: Arc::clone(&deps.device.device_identity),
            member_repo: Arc::clone(&deps.device.member_repo),
            settings: Arc::clone(&deps.settings),
            clock: Arc::clone(&deps.system.clock),
            pairing_invitation: handlers.invitation,
            pairing_invitation_addresses: handlers.invitation_addresses,
            pairing_invitation_by_address: handlers.invitation_by_address,
            pairing_session: handlers.session,
            pairing_events: handlers.events,
            proof_port,
            trusted_peer_repo: Arc::clone(&shared.trusted_peer_repo),
            peer_addr_repo: Arc::clone(&space_setup.peer_addr_repo),
            presence: Arc::clone(&presence),
            analytics: Arc::clone(&space_setup.analytics_facade),
            convergence: Arc::clone(&convergence_assembly),
        },
        transition: SpaceTransitionDeps {
            relationship_reset: Arc::clone(&space_setup.relationship_reset),
            space_security_reset: Arc::clone(&space_setup.space_security_reset),
        },
    }));

    // Slice 2 Phase 1 · T9:roster 门面和 space_setup facade 共享同一组
    // 实例(`member_repo` / `local_identity` / `presence`),这样 F1 hook
    // 通过 `presence.ensure_reachable_all` 填好的缓存,`list_with_presence`
    // 能直接读到。Facade 本身是纯 thin wrapper,构造非常便宜。
    let roster = Arc::new(
        MemberRosterFacade::new(MemberRosterDeps {
            member_repo: Arc::clone(&deps.device.member_repo),
            local_identity: Arc::clone(&local_identity),
            presence: Arc::clone(&presence),
            connection_channel: Some(Arc::clone(&connection_channel)),
        })
        .with_group_bootstrap(Arc::clone(
            &deps.security.space_access_ports.group_bootstrap,
        ))
        .with_space_protection(Arc::clone(
            &deps.security.space_access_ports.space_protection,
        ))
        .with_convergence(Arc::clone(&convergence_assembly)),
    );
    if let Err(error) = roster.resume_legacy_bootstraps().await {
        warn!(error = %error, "legacy bootstrap recovery could not resume during startup");
    }
    let space_application_runtime = SpaceApplicationRuntime::start(
        Arc::clone(&convergence_assembly),
        MembershipConnectivityDeps {
            peer_addresses: Arc::clone(&space_setup.peer_addr_repo),
            presence: Arc::clone(&presence),
            local_device_id: deps.device.device_identity.current_device_id(),
            peer_scope: convergence_assembly.current_peer_scope(),
        },
        presence.subscribe(),
        Arc::clone(&facade),
    );
    // Slice 2 Phase 2 · T10:剪切板同步门面。`dispatch_entry` 共享同一份
    // `peer_addr_repo` / `presence` 让 F1 hook 喂的 presence 缓存直接生
    // 效;`transfer_cipher` 与已有 file_transfer 路径同享 V3 chunked
    // AEAD adapter。ingest 后台 loop 立刻起一次,与 receiver handler 同
    // 生命周期(随 `iroh_node.shutdown()` 自然退出 `RecvError::Closed`,
    // `SyncEngineAssembly::shutdown` 显式 `abort()` 加速过程)。
    #[cfg(feature = "lan-compat")]
    let mobile_device_repo = Arc::clone(&mobile_sync_ports.devices.find_by_id);
    #[cfg(not(feature = "lan-compat"))]
    let mobile_device_repo: Arc<dyn uc_core::ports::FindMobileDeviceByIdPort> =
        Arc::new(UnavailableMobileDeviceLookup);

    let clipboard_sync = Arc::new(
        ClipboardSyncFacade::new(ClipboardSyncDeps {
            peer_addr_repo: Arc::clone(&space_setup.peer_addr_repo),
            member_repo: Arc::clone(&deps.device.member_repo),
            removal_gate: convergence_assembly.removal_gate(),
            peer_scope: convergence_assembly.current_peer_scope(),
            presence: Arc::clone(&presence),
            transfer_cipher: Arc::clone(&deps.security.transfer_cipher),
            clipboard_dispatch,
            device_identity: Arc::clone(&deps.device.device_identity),
            local_identity,
            settings: Arc::clone(&deps.settings),
            clock: Arc::clone(&deps.system.clock),
            analytics: Arc::clone(&deps.analytics),
            first_sync_state: Arc::clone(&deps.first_sync_state),
            entry_delivery_repo: Arc::clone(&shared.entry_delivery_repo),
            entry_repo: Arc::clone(&deps.clipboard.entry_ports.get),
            event_repo: Arc::clone(&shared.clipboard_event_reader_repo),
            trusted_peer_repo: Arc::clone(&shared.trusted_peer_repo),
            mobile_device_repo,
            // Issue #747 Phase 5：与 blob_transfer / apply_inbound 共享同一根
            // host_event_bus。GUI 装配链路在 Tauri setup callback 中
            // `bus.register("tauri", TauriHostEventEmitter)`,daemon 启动时
            // `bus.register("daemon_ws", DaemonApiEventEmitter)`。dispatch_uc
            // fan-out 完成、delivery 落盘后追发 `HostEvent::Delivery::
            // StatusChanged`,bus 把事件 fan-out 给所有已注册下游;CLI 装配
            // 走同一 bus,只挂着默认 logging emitter,emit 无副作用。
            host_event_bus: Arc::clone(&shared.host_event_bus),
        })
        .with_entry_receive_cancellation(
            Arc::clone(&deps.storage.directory_receive.get_attempt),
            Arc::clone(&deps.storage.directory_receive.request_cancel),
            Arc::clone(&deps.storage.directory_receive.entry_progress),
            Arc::clone(&deps.storage.directory_receive.list_attempts),
            Arc::clone(&deps.storage.directory_receive.commit_inbound),
            Arc::clone(&deps.storage.directory_receive.get_publish),
            FsDirectoryStagingCleaner::new(),
            Arc::clone(&deps.storage.file_transfer.cancel_attempt),
            Arc::clone(&blob),
            Arc::clone(&deps.system.clock),
        ),
    );
    // Store-only inbound apply path for pulled content (issue #1017 PR8). It
    // reuses the same inbound pipeline the bulk 0xC1 path uses (decode V3 →
    // materialize blobs → capture) through the named store-only mode. That mode
    // has no system-clipboard writer or active-register capability: the
    // active-clipboard convergence tail owns both actions and couples them to
    // OS-write success.
    let pull_store_capture = Arc::new(
        CaptureClipboardUseCase::new(
            Arc::clone(&deps.clipboard.entry_ports.save),
            Arc::clone(&deps.clipboard.entry_ports.touch),
            Arc::clone(&deps.clipboard.entry_ports.find_by_snapshot_hash),
            Arc::clone(&deps.clipboard.clipboard_event_repo),
            Arc::clone(&deps.clipboard.representation_policy),
            Arc::clone(&deps.clipboard.representation_normalizer),
            Arc::clone(&deps.device.device_identity),
            Arc::clone(&deps.clipboard.representation_cache),
            Arc::clone(&deps.clipboard.spool_queue),
            Arc::clone(&deps.storage.blob_content_ingest),
            Arc::clone(&deps.storage.entry_file_set_repo),
            Arc::clone(&deps.settings),
            Arc::clone(&deps.clipboard.entry_ports.replace_content),
            Arc::clone(&deps.analytics),
        )
        .with_inbound_receive_commit(Arc::clone(&deps.storage.directory_receive.commit_inbound))
        .with_entry_identity_coordinator(Arc::clone(&deps.clipboard.entry_identity_coordinator)),
    );
    let pull_store_materializer = Arc::new(
        FileCacheBlobMaterializer::new(
            blob.clone() as Arc<dyn uc_application::facade::InboundBlobFetcher>,
            shared.file_cache_dir.clone(),
            FsAtomicPublisher::new(),
        )
        .with_directory_receive_attempt_ports(
            Arc::clone(&deps.storage.directory_receive.get_attempt),
            Arc::clone(&deps.storage.directory_receive.claim_commit),
            Arc::clone(&deps.storage.directory_receive.record_publish),
            Arc::clone(&deps.system.clock),
        )
        .with_receive_artifact_log(Arc::clone(&deps.storage.directory_receive.record_artifacts))
        .with_target_reserver(FsInboundFileTarget::new(Arc::clone(&deps.settings)))
        .with_save_dir_resolver(FsInboundFileTarget::new(Arc::clone(&deps.settings)))
        .with_hidden_marker(FsHiddenPathMarker::new()),
    );
    // Index pull-store entries for search too (same rationale as the main
    // inbound path): content materialized via the 0xC2 pull should be findable.
    let pull_store_indexer: Arc<dyn ClipboardLiveIndexPort> =
        Arc::new(ClipboardLiveIndexer::new(ClipboardLiveIndexDeps {
            clipboard_entry_repo: Arc::clone(&deps.clipboard.entry_ports.get),
            representation_policy: Arc::clone(&deps.clipboard.representation_policy),
            search_key_derivation: Arc::clone(&deps.search.search_key_derivation),
            search_pipeline: Arc::clone(&deps.search.search_pipeline),
            search_index: Arc::clone(&deps.search.search_index),
            event_repo: Arc::clone(&shared.clipboard_event_reader_repo),
            entry_file_set_repo: Arc::clone(&deps.storage.entry_file_set_repo),
        }));
    let pull_store_apply: Arc<dyn InboundClipboardApplyPort> = Arc::new(
        ApplyInboundClipboardUseCase::store_only_pull(StoreOnlyPullDeps {
            common: InboundApplyCommonDeps {
                entry_repo: Arc::clone(&deps.clipboard.entry_ports.find_by_snapshot_hash),
                capture: pull_store_capture as Arc<dyn ApplyInboundCapture>,
                blob_materializer: pull_store_materializer,
                receive_attempts: InboundReceiveAttemptDeps {
                    get: Arc::clone(&deps.storage.directory_receive.get_attempt),
                    begin: Arc::clone(&deps.storage.directory_receive.begin_receive),
                    claim_commit: Arc::clone(&deps.storage.directory_receive.claim_commit),
                    request_cancel: Arc::clone(&deps.storage.directory_receive.request_cancel),
                    begin_failure: Arc::clone(&deps.storage.directory_receive.begin_failure),
                    commit: Arc::clone(&deps.storage.directory_receive.commit_inbound),
                    clock: Arc::clone(&deps.system.clock),
                },
                receive_artifact_cleanup: Arc::new(uc_infra::fs::FsReceiveArtifactCleaner),
                receive_readiness: Arc::clone(&shared.receive_readiness),
                host_event_emitter: Arc::clone(&shared.host_event_bus),
                search_live_index: pull_store_indexer,
                availability: Arc::clone(&deps.clipboard.entry_ports.availability),
                entry_identity_coordinator: Arc::clone(&deps.clipboard.entry_identity_coordinator),
            },
        }),
    );

    // Active-clipboard register convergence (issue #1017). The module owns
    // its inbound convergence, peer-online resync, restore broadcast, and
    // history-resurface worker topology behind one lifecycle seam. Assembly
    // only constructs the facade and retains that lifecycle for shutdown.
    let active_clipboard = Arc::new(ActiveClipboardFacade::new(ActiveClipboardDeps {
        receiver: Arc::clone(&active_clipboard_receiver),
        dispatch: active_clipboard_dispatch,
        is_unlocked: Arc::clone(&deps.security.space_access_ports.is_unlocked),
        load_register: Arc::clone(&deps.clipboard.active_register_load),
        advance_register: Arc::clone(&deps.clipboard.active_register),
        mobile_consumability: deps.clipboard.mobile_consumability.clone(),
        member_repo: Arc::clone(&deps.device.member_repo),
        content_gate: convergence_assembly.removal_gate(),
        peer_addr_repo: Arc::clone(&space_setup.peer_addr_repo),
        peer_scope: convergence_assembly.current_peer_scope(),
        presence: Arc::clone(&presence),
        entry_lookup: Arc::clone(&deps.clipboard.entry_ports.find_by_snapshot_hash),
        availability: Some(Arc::clone(&deps.clipboard.entry_ports.availability)),
        coordinator: Arc::clone(&shared.clipboard_write_coordinator),
        clock: Arc::clone(&deps.system.clock),
        device_identity: Arc::clone(&deps.device.device_identity),
        settings: Arc::clone(&deps.settings),
        snapshot: ClipboardSnapshotDeps {
            entry_repo: Arc::clone(&deps.clipboard.entry_ports.get),
            selection_repo: Arc::clone(&deps.clipboard.selection_repo),
            representation_repo: Arc::clone(&deps.clipboard.representation_ports.get),
            rep_processing_repo: Arc::clone(
                &deps.clipboard.representation_ports.update_processing_result,
            ),
            payload_resolver: Arc::clone(&deps.clipboard.payload_resolver),
            blob_store: Arc::clone(&deps.storage.blob_store),
        },
        // On-demand pull subsystem (PR8): when the observed content is not held
        // locally, pull it from the reporting peer (10s deadline), decrypt +
        // store it via the store-only apply path, then converge.
        transfer_cipher: Arc::clone(&deps.security.transfer_cipher),
        pull_client: Some(active_clipboard_pull_client),
        pull_apply: Some(pull_store_apply),
        touch_entry: Arc::clone(&deps.clipboard.entry_ports.touch),
        host_event_emitter: Arc::clone(&shared.host_event_bus),
        resurface_clock: Arc::clone(&deps.system.clock),
    }));
    let active_clipboard_lifecycle = active_clipboard.start_background_workers();

    info!("Slice 2/3 SpaceFacade + MemberRosterFacade + ClipboardSyncFacade + BlobTransferFacade assembled");
    Ok(SyncEngineAssembly {
        facade,
        roster,
        clipboard_sync,
        presence,
        blob,
        outbound_progress_reporter,
        blob_transfer,
        active_clipboard,
        iroh_node,
        clipboard_receiver,
        active_clipboard_lifecycle,
        outbound_progress_translator,
        convergence_assembly,
        space_application_runtime,
    })
}
