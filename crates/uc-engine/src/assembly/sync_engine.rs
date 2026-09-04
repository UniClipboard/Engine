//! Engine 网络组合根。
//!
//! 本模块选择并安装 Iroh、Infra 与观测 adapter，再通过一次性
//! [`ApplicationNetworkBinding`] 取得 Router 必需的窄 endpoint。Space、
//! Clipboard、Blob 与文件传输对象图及其关闭顺序均由 Application 持有；
//! [`SyncEngineAssembly`] 只拥有共享 Iroh node 和进度翻译 worker。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{info, instrument};

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

use uc_application::deps::{
    ApplicationClipboardAdapters, ApplicationNetworkAdapters, ApplicationNetworkBinding,
    ApplicationSpaceAdapters, CurrentSpaceMemberScopePort, SpaceAdmissionAdapters,
    SpaceMembershipAdapters, SpaceRuntimeAdapters,
};
use uc_application::facade::ApplicationAssembly;
use uc_application::facade::{HostEvent, HostEventBus, TransferHostEvent};
use uc_core::file_transfer::{
    FileTransferCancellationReason, FileTransferDirection, OutboundProgressStatus,
};
use uc_core::membership::ContentExchangeGatePort;
use uc_core::ports::{
    ActiveClipboardDispatchPort, ActiveClipboardReceiverPort, ClipboardDispatchPort,
    ClipboardReceiverPort, ConnectionChannelPort, LocalIdentityPort, PeerReachabilityPort,
};
use uc_infra::network::iroh::transfer_progress_adapter::InboundProgressEvent;
use uc_infra::network::iroh::{
    encode_space_admission_route, ActiveClipboardHandlers, ActiveClipboardPullHandlers,
    BlobHandlers, ClipboardHandlers, GroupUpdateHandlers, IrohIdentityStore, IrohNode,
    IrohNodeBuilder, IrohNodeError, TransferProgressHandlers,
};
// Re-exported so external callers can parametrise the assembly without
// having to `use uc_infra` themselves.
use crate::assembly::deps::SyncEngineDeps;
use uc_infra::fs::{
    FsAtomicPublisher, FsDirectoryStagingCleaner, FsHiddenPathMarker, FsInboundFileTarget,
};
pub(crate) use uc_infra::network::iroh::IrohNodeConfig;
use uc_infra::security::Sha256IdentityFingerprintFactory;
use uc_infra::space::{
    DefaultJoinerActivationExecutor, DefaultJoinerActivationPreparation,
    DefaultJoinerAppliedPreparation, DefaultJoinerCancellationPreparation,
    DefaultJoinerCandidatePreparation, DefaultJoinerInvitationPreparation,
    DefaultJoinerStartMaterial, DefaultMembershipBranchTransitionPreparation,
    DefaultMembershipSecurityUpdateAdapter, DefaultSponsorAdmissionActivation,
    DefaultSponsorCandidatePreparation, DefaultSponsorCommitPreparation,
    DefaultSponsorCompletePreparation, DefaultSponsorSettledPreparation,
    DeviceTrustObservationsAdapter, GatedMembershipHistoryExchange, GatedSpaceAdmissionTransport,
    MembershipActivationAdapter, MembershipMemberFactsAdapter, MembershipNetworkGate,
    MembershipProjectionCleanupAdapter, OpenMlsHistoricalSignatureVerifier,
};

struct CurrentMemberContentGate {
    scope: Arc<dyn CurrentSpaceMemberScopePort>,
}

impl CurrentMemberContentGate {
    fn new(scope: Arc<dyn CurrentSpaceMemberScopePort>) -> Self {
        Self { scope }
    }
}

#[async_trait::async_trait]
impl ContentExchangeGatePort for CurrentMemberContentGate {
    async fn is_locally_removed(&self, device_id: &uc_core::ids::DeviceId) -> bool {
        let Ok(scope) = self.scope.snapshot().await else {
            return true;
        };
        !scope.local_member_active || !scope.usable_peer_device_ids.contains(device_id)
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

/// Engine 持有的网络生命周期 owner。
pub struct SyncEngineAssembly {
    /// The shared iroh node. Held privately so callers can't bind a second
    /// node or install additional handlers after `spawn` — that would
    /// fragment peer identity (§"共用网络栈" decision, Slice 1 planning).
    iroh_node: IrohNode,
    /// 反向"传输进度"翻译 worker 的 join handle。订阅
    /// `IrohTransferProgressAdapter` 的 inbound 流,将每帧 progress 翻译
    /// 为 `HostEvent::Transfer { direction: Sending, ... }` 并发到 emitter。
    /// 与 sync assembly 同生命周期。
    outbound_progress_translator: OutboundProgressRuntime,
}

/// Engine 完成网络装配后一次性交给 Application 的被动 adapter 集合。
///
/// 该集合按值移交；`SyncEngineAssembly` 不保留 Clipboard 领域句柄。
pub(crate) struct SyncApplicationAdapters {
    pub binding: ApplicationNetworkBinding,
    pub active_pull_client: Arc<dyn uc_core::ports::ActiveClipboardPullClientPort>,
}

pub(crate) struct SyncEngineAssemblyOutput {
    pub network: SyncEngineAssembly,
    pub application: SyncApplicationAdapters,
}

impl SyncEngineAssembly {
    pub(crate) fn subscribe_network_recovery_observations(
        &self,
    ) -> tokio::sync::broadcast::Receiver<uc_infra::network::iroh::NetworkRecoveryObservation> {
        self.iroh_node.subscribe_network_recovery_observations()
    }

    #[cfg(test)]
    pub(crate) async fn membership_history_exchange_is_reachable_for_test(&self) -> bool {
        self.iroh_node
            .accepts_protocol_for_test(uc_infra::network::iroh::MEMBERSHIP_HISTORY_EXCHANGE_ALPN)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn membership_branch_recovery_is_reachable_for_test(&self) -> bool {
        self.iroh_node
            .accepts_protocol_for_test(uc_infra::network::iroh::MEMBERSHIP_BRANCH_RECOVERY_ALPN)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn space_admission_is_reachable_for_test(&self) -> bool {
        self.iroh_node
            .accepts_protocol_for_test(uc_infra::network::iroh::SPACE_ADMISSION_ALPN)
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

    /// 先停止 Engine 自有的进度翻译，再关闭共享 Iroh Router；Application
    /// 领域运行期由独立的 `ApplicationRuntime` 负责停止。
    #[instrument(skip_all)]
    pub async fn shutdown(self, transfer_reason: FileTransferCancellationReason) {
        self.outbound_progress_translator
            .shutdown(transfer_reason)
            .await;
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

/// 网络与 Application endpoint 装配失败；启动调用方将其作为致命错误处理。
#[derive(Debug, thiserror::Error)]
pub enum SyncEngineAssemblyError {
    #[error(transparent)]
    IrohNode(#[from] IrohNodeError),
    #[error(transparent)]
    ApplicationUpgrade(#[from] uc_application::facade::ApplicationUpgradeError),
    #[error("failed to assemble the Space application")]
    ApplicationAssembly {
        #[source]
        source: anyhow::Error,
    },
}

/// 从已完成的 Application factory 与 Engine adapter 构造共享 Iroh 网络。
/// 该函数绑定 endpoint 并启动 Router，必须在 Tokio runtime 内调用。
#[instrument(skip_all)]
pub async fn build_sync_engine_assembly(
    application: &ApplicationAssembly,
    space_setup: &SyncEngineDeps,
    current_app_version: &str,
    #[cfg(feature = "lan-compat")] mobile_sync_ports: uc_mobile_lan::MobileSyncPorts,
    iroh_config: IrohNodeConfig,
) -> Result<SyncEngineAssemblyOutput, SyncEngineAssemblyError> {
    application
        .ensure_current_version(current_app_version)
        .await?;
    // IdentityFingerprintFactory 无状态；这里直接构造具体实现，避免从
    // trait object 下转型后再包装。
    let identity_store = Arc::new(IrohIdentityStore::new(
        Arc::clone(&space_setup.iroh_identity_storage),
        Arc::new(Sha256IdentityFingerprintFactory),
    ));

    // 绑定共享 Iroh node 并安装邀请发现。Iroh Router 始终封装在
    // `IrohNode` 内，不向 Application 泄漏具体 Iroh 类型。
    let mut builder = IrohNodeBuilder::bind(&identity_store, iroh_config).await?;
    let handlers = builder.install_pairing_invitation(
        Arc::clone(&space_setup.device_identity),
        Arc::clone(&space_setup.settings),
    );
    let removal_identity = builder.build_membership_identity_adapter(
        Arc::clone(&space_setup.membership_session),
        Arc::clone(&space_setup.device_identity),
        Arc::clone(&space_setup.settings),
        Arc::clone(&space_setup.fingerprint),
    );
    let membership_history_exchange_adapter =
        builder.build_membership_history_exchange_adapter(Arc::clone(&space_setup.peer_addr_repo));
    let membership_branch_recovery_channel =
        builder.build_membership_branch_recovery_channel(Arc::clone(&space_setup.peer_addr_repo));
    let membership_transport = builder.build_membership_gossip_transport(
        Arc::clone(&space_setup.membership_session),
        Arc::clone(&space_setup.device_identity),
        Arc::clone(&space_setup.settings),
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&space_setup.fingerprint),
    );
    // Presence is installed before the convergence owner is assembled so the
    // owner can expose reachability as an independent product fact.
    let peer_reachability: Arc<dyn PeerReachabilityPort> = builder.install_presence(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&space_setup.fingerprint),
        Arc::clone(&space_setup.clock),
    );
    // Phase 96 INDIC-01:连接通道单一真相源。复用同一 endpoint +
    // peer_addr_repo,纯读 adapter 不装 ALPN handler。
    let connection_channel: Arc<dyn ConnectionChannelPort> =
        builder.install_connection_channel(Arc::clone(&space_setup.peer_addr_repo));
    let GroupUpdateHandlers {
        dispatch: group_update_dispatch,
    } = builder.install_group_updates(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.space_access.group_revocation),
    )?;
    // Slice 2 Phase 2 · T10:同一节点装第三个 ALPN(剪切板同步)。dispatch
    // 复用 endpoint + peer_addr_repo,与 presence 共享 NAT/relay 映射;
    // receiver handler 通过 `member_repo` 把 `Connection::remote_id()` 反查
    // 成 DeviceId 再喂给应用层 broadcast。同样必须在 `spawn` 前装。
    let ClipboardHandlers {
        dispatch: clipboard_dispatch,
        receiver: clipboard_receiver,
    } = builder.install_clipboard(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&space_setup.fingerprint),
        Arc::clone(&peer_reachability),
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
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&space_setup.fingerprint),
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
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&space_setup.fingerprint),
    );

    // Slice 3 Phase 1:同一节点装第五个 ALPN(iroh-blobs)。BlobReference
    // 是 sqlite 仓储,不跟 router 绑定;这里只拿传输 port。
    let BlobHandlers { blob_transfer } = builder
        .install_blobs(space_setup.iroh_blob_store_dir.clone())
        .await?;

    // Application 先构造认证 endpoint；Space 持续维护要等 Router 就绪后
    // 才由 ApplicationRuntime 启动。
    let endpoint_addr = builder.local_endpoint_addr();
    let endpoint_addr_blob = builder.local_endpoint_addr_blob()?;
    let continuation_route =
        encode_space_admission_route(&endpoint_addr, None).map_err(|source| {
            SyncEngineAssemblyError::ApplicationAssembly {
                source: anyhow::Error::new(source).context("failed to encode the admission route"),
            }
        })?;
    let identity_fingerprint = space_setup
        .fingerprint
        .from_public_key(endpoint_addr.id.as_bytes())
        .map_err(|source| SyncEngineAssemblyError::ApplicationAssembly {
            source: source.context("failed to derive the endpoint identity fingerprint"),
        })?;
    let historical_signatures = Arc::new(OpenMlsHistoricalSignatureVerifier);
    let membership_network_gate = MembershipNetworkGate::active();
    let admission_transport: Arc<dyn uc_application::deps::SpaceAdmissionTransportPort> =
        Arc::new(GatedSpaceAdmissionTransport::new(
            Arc::clone(&membership_network_gate),
            builder.space_admission_transport(),
        ));
    let membership_history_transport = Arc::new(GatedMembershipHistoryExchange::new(
        Arc::clone(&membership_network_gate),
        Arc::clone(&membership_history_exchange_adapter),
    ));
    let membership_security = Arc::new(DefaultMembershipSecurityUpdateAdapter::new(
        Arc::clone(&space_setup.membership_session),
        Arc::clone(&space_setup.current_member_signatures),
        Arc::clone(&space_setup.space_access.group_revocation),
        Arc::clone(&space_setup.clock),
    ));
    let local_device_id = space_setup.device_identity.current_device_id();
    let local_identity: Arc<dyn LocalIdentityPort> = identity_store;
    let build_admission =
        |membership_committer: Arc<dyn uc_application::deps::CommitMembershipLedgerPort>| {
            crate::assembly::observability::observe_admission(SpaceAdmissionAdapters {
                re_pairing_state_store: Arc::clone(&space_setup.re_pairing_state_store),
                prepare_joiner_invitation: Arc::new(DefaultJoinerInvitationPreparation),
                resolve_joiner_invitation: handlers.joiner_invitation_resolver,
                joiner_start_material: Arc::new(DefaultJoinerStartMaterial::new(
                    local_device_id.clone(),
                    Arc::clone(&space_setup.settings),
                    identity_fingerprint,
                    endpoint_addr.id.as_bytes().to_vec(),
                    endpoint_addr_blob,
                )),
                joiner_start_state: space_setup.admission_state.clone()
                    as Arc<dyn uc_application::deps::JoinerStartStatePort>,
                current_join_admission_state: space_setup.admission_state.clone()
                    as Arc<dyn uc_application::deps::CurrentJoinAdmissionStatePort>,
                prepare_joiner_cancellation: Arc::new(DefaultJoinerCancellationPreparation),
                pending_admission_recovery_state: space_setup.admission_state.clone()
                    as Arc<dyn uc_application::deps::PendingAdmissionRecoveryStatePort>,
                space_admission_transport: admission_transport,
                sponsor_admission_state: space_setup.admission_state.clone()
                    as Arc<dyn uc_application::deps::SponsorAdmissionStatePort>,
                prepare_sponsor_candidate: Arc::new(DefaultSponsorCandidatePreparation::new(
                    local_device_id.clone(),
                    continuation_route,
                    Arc::clone(&space_setup.current_member_signatures),
                    historical_signatures.clone(),
                    Arc::clone(&space_setup.space_access.prepare_sponsor_admission_security),
                )),
                prepare_sponsor_commit: Arc::new(DefaultSponsorCommitPreparation::new(
                    historical_signatures.clone(),
                )),
                prepare_sponsor_complete: Arc::new(DefaultSponsorCompletePreparation::new(
                    local_device_id,
                    Arc::clone(&space_setup.current_member_signatures),
                    historical_signatures.clone(),
                )),
                activate_sponsor_admission: Arc::new(DefaultSponsorAdmissionActivation::new(
                    Arc::clone(&space_setup.space_access.activate_sponsor_admission_security),
                    space_setup.membership_ledger.clone()
                        as Arc<dyn uc_application::deps::LoadMembershipLedgerPort>,
                    Arc::clone(&membership_committer),
                    historical_signatures.clone(),
                    Arc::new(MembershipMemberFactsAdapter::new(
                        Arc::clone(&space_setup.member_repo),
                        Arc::clone(&space_setup.trusted_peer_repo),
                        Arc::clone(&space_setup.peer_addr_repo),
                        Arc::clone(&space_setup.device_identity),
                        Arc::clone(&space_setup.clock),
                    )),
                )),
                prepare_sponsor_settled: Arc::new(DefaultSponsorSettledPreparation),
                prepare_joiner_candidate: Arc::new(DefaultJoinerCandidatePreparation::new(
                    historical_signatures.clone(),
                    Arc::clone(&space_setup.space_access.prepare_admission_target_access),
                )),
                prepare_joiner_applied: Arc::new(DefaultJoinerAppliedPreparation::new(
                    historical_signatures.clone(),
                )),
                prepare_joiner_activation: Arc::new(DefaultJoinerActivationPreparation::new(
                    historical_signatures.clone(),
                    Arc::clone(&space_setup.admission_space_transition),
                )),
                joiner_activation_state: space_setup.admission_state.clone()
                    as Arc<dyn uc_application::deps::JoinerActivationStatePort>,
                execute_joiner_activation: Arc::new(DefaultJoinerActivationExecutor::new(
                    Arc::clone(&space_setup.admission_space_transition),
                    historical_signatures.clone(),
                )),
                current_join_status: space_setup.admission_state.clone()
                    as Arc<dyn uc_application::deps::LoadCurrentJoinStatusPort>,
            })
        };
    let membership = crate::assembly::observability::observe_membership(
        SpaceMembershipAdapters {
            load_membership_ledger: space_setup.membership_ledger.clone()
                as Arc<dyn uc_application::deps::LoadMembershipLedgerPort>,
            commit_membership_ledger: space_setup.membership_ledger.clone()
                as Arc<dyn uc_application::deps::CommitMembershipLedgerPort>,
            historical_membership_signatures: historical_signatures.clone(),
            current_member_signatures: Arc::clone(&space_setup.current_member_signatures),
            membership_identity: removal_identity,
            membership_announcement: membership_transport,
            device_trust_observations: Arc::new(DeviceTrustObservationsAdapter::new(
                Arc::clone(&space_setup.member_repo),
                Arc::clone(&peer_reachability),
            )),
            membership_history_transport: membership_history_transport.clone(),
            membership_branch_recovery_channel,
            membership_branch_recovery_recipient: Arc::clone(
                &space_setup
                    .space_access
                    .prepare_membership_branch_recovery_recipient,
            ),
            membership_branch_transition: Arc::new(
                DefaultMembershipBranchTransitionPreparation::new(Arc::clone(
                    &space_setup.active_generation_manifest_store,
                )),
            ),
            membership_branch_transition_executor: Arc::clone(
                &space_setup.membership_branch_transition_executor,
            ),
            membership_branch_recovery_material: Arc::clone(
                &space_setup
                    .space_access
                    .prepare_membership_branch_recovery_material,
            ),
            apply_membership_member_facts: Arc::new(MembershipMemberFactsAdapter::new(
                Arc::clone(&space_setup.member_repo),
                Arc::clone(&space_setup.trusted_peer_repo),
                Arc::clone(&space_setup.peer_addr_repo),
                Arc::clone(&space_setup.device_identity),
                Arc::clone(&space_setup.clock),
            )),
            apply_membership_security: membership_security,
            activate_membership_effect: Arc::new(MembershipActivationAdapter::new(Arc::clone(
                &peer_reachability,
            ))),
            restricted_membership_delivery: membership_history_transport,
            group_update_store: Arc::clone(&space_setup.space_access.group_revocation),
            group_update_dispatch,
            cleanup_legacy_membership_data: Arc::new(MembershipProjectionCleanupAdapter::new(
                space_setup.membership_ledger.clone()
                    as Arc<dyn uc_application::deps::LoadMembershipLedgerPort>,
                Arc::clone(&space_setup.member_repo),
                Arc::clone(&space_setup.peer_addr_repo),
            )),
            membership_network_activity: membership_network_gate,
        },
        application.host_event_bus(),
    );
    let admission = build_admission(Arc::clone(&membership.commit_membership_ledger));
    let space_runtime = SpaceRuntimeAdapters {
        admission,
        membership,
    };
    #[cfg(feature = "lan-compat")]
    let mobile_device_repo = Arc::clone(&mobile_sync_ports.devices.find_by_id);
    #[cfg(not(feature = "lan-compat"))]
    let mobile_device_repo: Arc<dyn uc_core::ports::FindMobileDeviceByIdPort> =
        Arc::new(UnavailableMobileDeviceLookup);
    let application_network = application.assemble_network(ApplicationNetworkAdapters {
        blob_transfer: Arc::clone(&blob_transfer),
        blob_reference: Arc::clone(&space_setup.blob_reference_repo),
        outbound_progress_reporter: Arc::clone(&outbound_progress_reporter),
        space: ApplicationSpaceAdapters {
            current_engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            admission_credentials: space_setup.admission_credentials.clone()
                as Arc<dyn uc_application::deps::PrepareSpaceAdmissionCredentialsPort>,
            local_identity: Arc::clone(&local_identity),
            pairing_invitation: handlers.invitation,
            pairing_invitation_addresses: handlers.invitation_addresses,
            pairing_invitation_by_address: handlers.invitation_by_address,
            presence: Arc::clone(&peer_reachability),
            analytics: Arc::clone(&space_setup.analytics_facade),
            connection_channel: Some(Arc::clone(&connection_channel)),
            device_management_reset_data: Arc::clone(&space_setup.device_management_reset_data),
            relationship_reset: Arc::clone(&space_setup.relationship_reset),
            space_security_reset: Arc::clone(&space_setup.space_security_reset),
            runtime: space_runtime,
            peer_reachability_changed_events: peer_reachability.subscribe(),
        },
        clipboard: ApplicationClipboardAdapters {
            peer_addresses: Arc::clone(&space_setup.peer_addr_repo),
            peer_reachability: Arc::clone(&peer_reachability),
            clipboard_dispatch,
            clipboard_receiver,
            local_identity,
            mobile_device_repo,
            active_receiver: active_clipboard_receiver,
            active_dispatch: active_clipboard_dispatch,
            active_pull_publisher: FsAtomicPublisher::new(),
            active_pull_target_reserver: FsInboundFileTarget::new(Arc::clone(
                &space_setup.settings,
            )),
            active_pull_hidden_marker: FsHiddenPathMarker::new(),
            staging_cleanup: FsDirectoryStagingCleaner::new(),
        },
    });
    builder.install_space_admission(
        application_network.space_admission_endpoint(),
        space_setup.admission_credentials.clone()
            as Arc<dyn uc_infra::network::iroh::SpaceAdmissionChannelCredentialPort>,
    )?;
    builder.install_membership_history_exchange(
        &membership_history_exchange_adapter,
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.fingerprint),
        application_network.membership_history_endpoint(),
    )?;
    builder.install_membership_branch_recovery(
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.fingerprint),
        application_network.membership_branch_recovery_endpoint(),
    )?;
    let content_gate: Arc<dyn ContentExchangeGatePort> = Arc::new(CurrentMemberContentGate::new(
        application_network.current_member_scope(),
    ));

    // Install the active-clipboard pull ALPN (0xC2, issue #1017 PR8) as a
    // further independent sibling, before `spawn()`. The serve port reuses the
    // resend crypto chain (reconstruct → publish blobs re-signing self-pinned
    // tickets, D3 → encode V3 → encrypt, D4); the returned client port drives
    // the inbound seam's on-demand pull.
    let ActiveClipboardPullHandlers {
        client: active_clipboard_pull_client,
    } = builder.install_active_clipboard_pull(
        Arc::clone(&space_setup.peer_addr_repo),
        Arc::clone(&space_setup.member_repo),
        Arc::clone(&space_setup.peer_admission),
        Arc::clone(&space_setup.fingerprint),
        application_network.active_clipboard_pull_serve(),
        content_gate,
    );

    let iroh_node = builder.spawn();

    // Translator worker:从 sender 端的反向通道收 InboundProgressEvent,
    // 翻译为 application 层 HostEvent(Sending 方向)发到 host_event_bus。
    // 每次 progress → `TransferHostEvent::Progress`;终态 → 额外一帧
    // `StatusChanged`。shutdown 会显式停止并等待该任务。
    let outbound_progress_translator = OutboundProgressRuntime::spawn(
        outbound_progress_events,
        Arc::clone(&application.host_event_bus()),
    );

    info!("Iroh adapters registered against the Application network binding");
    Ok(SyncEngineAssemblyOutput {
        network: SyncEngineAssembly {
            iroh_node,
            outbound_progress_translator,
        },
        application: SyncApplicationAdapters {
            binding: application_network,
            active_pull_client: active_clipboard_pull_client,
        },
    })
}
