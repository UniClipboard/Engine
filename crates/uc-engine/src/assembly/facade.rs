//! Shared application-facade assembly owned by the cross-platform engine.
//!
//! Host entry points prepare platform capabilities and pass the resulting
//! application dependencies into the builders in this module.

use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::AppDeps;
use uc_application::facade::settings::{
    RelayAccessToken, RelayDiagnosticPort, RelayProbeError, RelayProbeReport,
};
use uc_application::facade::space_setup::SpaceFacade;
#[cfg(feature = "lan-compat")]
use uc_application::facade::{
    ActiveClipboardFacade, FileTransferFacade, InboundClipboardApplyPort,
};
use uc_application::facade::{
    AppFacade, AppFacadeParts, AppPaths, BlobTransferFacade, ClipboardAssembly,
    ClipboardOutboundFacade, ClipboardSyncFacade, ProbeProfileKeyAccessUseCase,
    QueryLocalDeviceUseCase, SearchFacade, SettingsAssembly,
};
use uc_core::clipboard::ClipboardIntegrationMode;
#[cfg(feature = "lan-compat")]
use uc_infra::fs::FsInboundFileTarget;
#[cfg(feature = "lan-compat")]
use uc_infra::mobile_sync::{
    Argon2idPasswordHasher, FilesystemMobileFileStaging, NetworkInterfaceLanProbe,
    OsRngCredentialsMinter,
};
use uc_infra::network::iroh::{IrohRelayProbeAdapter, IrohRelayProbeError, IrohRelayProbeReport};
#[cfg(feature = "lan-compat")]
use uc_mobile_lan::{
    IncomingMobileBuffer, MobileSyncFacade, MobileSyncFacadeDeps, MobileSyncSnapshotPorts,
};

// ---------------------------------------------------------------------------
// IrohRelayDiagnosticAdapter
// ---------------------------------------------------------------------------

/// Adapts the infrastructure relay probe to the application diagnostic port.
///
/// The engine owns this adapter because it is the shared composition boundary
/// that can see both contracts without reversing either dependency direction.
struct IrohRelayDiagnosticAdapter {
    inner: Arc<IrohRelayProbeAdapter>,
}

#[async_trait]
impl RelayDiagnosticPort for IrohRelayDiagnosticAdapter {
    async fn probe(
        &self,
        url: &str,
        access_token: Option<&RelayAccessToken>,
    ) -> Result<RelayProbeReport, RelayProbeError> {
        self.inner
            .probe_with_access_token(url, access_token.map(RelayAccessToken::expose_secret))
            .await
            .map(map_relay_probe_report)
            .map_err(map_relay_probe_error)
    }
}

fn map_relay_probe_report(report: IrohRelayProbeReport) -> RelayProbeReport {
    RelayProbeReport {
        latency_ms: report.latency_ms,
    }
}

fn map_relay_probe_error(err: IrohRelayProbeError) -> RelayProbeError {
    match err {
        IrohRelayProbeError::InvalidUrl(msg) => RelayProbeError::InvalidUrl(msg),
        IrohRelayProbeError::Dns(msg) => RelayProbeError::Dns(msg),
        IrohRelayProbeError::Tls(msg) => RelayProbeError::Tls(msg),
        IrohRelayProbeError::Handshake(msg) => RelayProbeError::Handshake(msg),
        IrohRelayProbeError::Timeout => RelayProbeError::Timeout,
        IrohRelayProbeError::Other(msg) => RelayProbeError::Other(msg),
    }
}

pub(crate) fn build_settings_assembly(deps: &AppDeps, paths: &AppPaths) -> SettingsAssembly {
    let relay_diagnostic = match IrohRelayProbeAdapter::new() {
        Ok(probe) => Some(Arc::new(IrohRelayDiagnosticAdapter {
            inner: Arc::new(probe),
        }) as Arc<dyn RelayDiagnosticPort>),
        Err(error) => {
            tracing::warn!(
                target: "bootstrap.network",
                error = %error,
                "relay probe adapter unavailable; settings.probe_relay_url will reject"
            );
            None
        }
    };
    SettingsAssembly::build(deps, paths, relay_diagnostic)
}

/// `ClipboardRestoreFacade` 的可选装配输入。
///
/// GUI 和 daemon 需要 restore 能力；部分 CLI 查询入口不需要，因此通过
/// 显式选项传入，避免各入口各自复制 facade 拼装代码。
/// 构造 [`MobileSyncFacade`] —— 抽出来供 daemon-lifecycle 装配复用。
///
/// `apply_inbound` 由 engine 运行期组装并传入。`endpoint_info`
/// 由 [`AppDeps`] 携带 (单例,daemon LAN listener 与 facade 共享同一份
/// Arc),无需 caller 透传。`file_transfer` 进程级 facade:daemon 装配
/// 必传,SyncDoc apply 后 link + complete 让 mobile_lan transfer 在
/// file_transfer 表里闭环。
#[cfg(feature = "lan-compat")]
pub fn build_mobile_sync_facade(
    deps: &AppDeps,
    storage_paths: &AppPaths,
    mobile_ports: uc_mobile_lan::MobileSyncPorts,
    apply_inbound: Arc<dyn InboundClipboardApplyPort>,
    file_transfer: Option<Arc<FileTransferFacade>>,
    // GUI daemon 装配传 `Some(controller)` —— update_settings 写盘后即时
    // start/stop/rebind listener。CLI fallback 传 `None`,settings 只写盘,
    // 等下次 daemon 进程启动一次性读取(与本字段引入前完全一致的行为)。
    lan_lifecycle: Option<Arc<dyn uc_core::ports::MobileLanLifecyclePort>>,
    // 同进程内已构造好的 `ClipboardOutboundFacade`(daemon 启动时装配)。
    // 装入时,移动端 PUT 落地本机后会异步把同一份 snapshot 走"本机捕获
    // → 出站"完整管线 fan-out 给 Space 内其他已配对设备 ——
    //
    // - 文本 / 小图 inline 进 V3 envelope;
    // - 大图自动剥成 iroh-blobs ref;
    // - **文件**:`publish_blob_path` 流式发布到 iroh-blobs, 构造 free-file
    //   V3BlobRef, 接收端拉回并改写 file-list rep 成本机 URI ——
    //   "手机文件 → 其他桌面"的真正传输靠这条路径成立。
    //
    // CLI fallback / 不接 P2P 出站的入口传 `None`, mobile 上传仅落地本机,
    // 不传播。
    clipboard_outbound: Option<Arc<ClipboardOutboundFacade>>,
    // Mobile-activation announce (issue #1017 PR7): the active-clipboard facade
    // (advance register + send-gated 0xC3 fan-out). daemon 装配传 `Some(...)`;
    // CLI fallback / 不接 active-clipboard 的入口传 `None`,移动端上传仅落地
    // 本机, 不向对端收敛。OS 剪贴板由入站管线负责写, 不经过这里。
    active_clipboard: Option<Arc<ActiveClipboardFacade>>,
) -> Arc<MobileSyncFacade> {
    Arc::new(MobileSyncFacade::new(MobileSyncFacadeDeps {
        clock: deps.system.clock.clone(),
        // v3 SyncClipboard 兼容: 单一 minter 一次性出 (username, password,
        // password_hash, device_id), Argon2id 作为口令 hash;无状态 ZST,
        // 装配处直接 new 即可。
        credentials_minter: Arc::new(OsRngCredentialsMinter),
        password_hasher: Arc::new(Argon2idPasswordHasher),
        devices: mobile_ports.devices.clone(),
        endpoint_info: mobile_ports.endpoint_info.clone(),
        lan_interface_probe: Arc::new(NetworkInterfaceLanProbe::new()),
        settings: deps.settings.clone(),
        apply_inbound,
        incoming_buffer: Arc::new(IncomingMobileBuffer::new()),
        file_staging: FilesystemMobileFileStaging::new_with_target_reserver(
            storage_paths.file_cache_dir.clone(),
            FsInboundFileTarget::new(deps.settings.clone()),
        ),
        snapshot_ports: MobileSyncSnapshotPorts {
            mobile_consumable_load: deps.clipboard.mobile_consumable_load.clone(),
            entry_repo: deps.clipboard.entry_ports.get.clone(),
            selection_repo: deps.clipboard.selection_repo.clone(),
            representation_repo: deps.clipboard.representation_ports.get.clone(),
            payload_resolver: deps.clipboard.payload_resolver.clone(),
            blob_reader: deps.storage.blob_store.clone(),
        },
        file_transfer,
        clipboard_outbound,
        lan_lifecycle,
        // schema doc §7.6 / §12.2 P1：mobile_sync 域共用 process-wide analytics
        // sink。bootstrap 已把 GatedAnalyticsSink 包好，runtime 切换 noop / 真
        // 实 sink 是 sink 自身职责，不在此装配。
        analytics: deps.analytics.clone(),
        active_clipboard,
        find_entry_by_snapshot_hash: deps.clipboard.entry_ports.find_by_snapshot_hash.clone(),
        check_entry_availability: deps.clipboard.entry_ports.availability.clone(),
    }))
}

/// 生产运行期构造完整 [`AppFacade`] 所需的全部能力。
pub struct RuntimeAppFacadeAssembly {
    pub space: Arc<SpaceFacade>,
    pub clipboard_sync: Arc<ClipboardSyncFacade>,
    pub blob_transfer: Arc<BlobTransferFacade>,
    pub file_transfer: Arc<uc_application::facade::FileTransferFacade>,
    pub clipboard_outbound: Arc<ClipboardOutboundFacade>,
    /// 底层 `BlobTransferPort`(`IrohBlobTransferAdapter`)直连引用,供
    /// `ClipboardHistoryFacade` 在 `delete_entry` / `clear_history` 时
    /// 调 `untag` 释放对应 entry 对 iroh-blobs 的引用。与 `blob_transfer`
    /// 字段(承载发布/拉取 use case 的 facade)分开装配:facade 用于
    /// "发布、拉取 blob"业务动作,这个 port 用于"释放 blob 引用"基础
    /// 设施动作,两者共享同一个底层 adapter 实例。
    pub blob_transfer_port: Arc<dyn uc_core::ports::blob::BlobTransferPort>,
    pub clipboard: Arc<ClipboardAssembly>,
    pub restore_broadcast: Option<uc_application::facade::clipboard_write::RestoreBroadcastTrigger>,
    pub search: Arc<SearchFacade>,
    pub settings: SettingsAssembly,
    pub network_recovery: Arc<uc_application::facade::NetworkRecoveryFacade>,
}

/// 从已注入的 application deps 构造统一业务入口。
///
/// 这是 GUI、daemon、CLI 共享的 application facade 装配点。调用方仍然
/// 决定运行模式、事件源、HTTP/WS/Tauri 接入和后台任务；本函数只负责把
/// ports 组合成 `AppFacade`。
pub fn build_app_facade_from_deps(
    deps: &AppDeps,
    _storage_paths: &AppPaths,
    runtime: RuntimeAppFacadeAssembly,
) -> Arc<AppFacade> {
    let settings = runtime.settings.into_parts();
    let clipboard_restore = runtime
        .clipboard
        .restore(ClipboardIntegrationMode::Full, runtime.restore_broadcast);

    Arc::new(AppFacade::new(AppFacadeParts {
        space: runtime.space,
        probe_profile_key_access: Arc::new(ProbeProfileKeyAccessUseCase::new(
            deps.security.profile_key_access_probe.clone(),
        )),
        resource: runtime.clipboard.resource(),
        clipboard_history: runtime.clipboard.history(Some(runtime.blob_transfer_port)),
        clipboard_capture: runtime.clipboard.capture(),
        clipboard_sync: runtime.clipboard_sync,
        blob_transfer: runtime.blob_transfer,
        file_transfer: runtime.file_transfer,
        clipboard_outbound: runtime.clipboard_outbound,
        clipboard_restore,
        search: runtime.search,
        settings: settings.settings,
        diagnostics: settings.diagnostics,
        query_local_device: Arc::new(QueryLocalDeviceUseCase::new(
            deps.device.device_identity.clone(),
            deps.settings.clone(),
        )),
        storage: settings.storage,
        config_migration: settings.config_migration,
        upgrade: settings.upgrade,
        network_recovery: runtime.network_recovery,
    }))
}
