//! Shared application-facade assembly owned by the cross-platform engine.
//!
//! Host entry points prepare platform capabilities and pass the resulting
//! application dependencies into the builders in this module.

use std::sync::Arc;

use async_trait::async_trait;
use uc_application::facade::settings::{
    RelayAccessToken, RelayDiagnosticPort, RelayProbeError, RelayProbeReport,
};
#[cfg(feature = "lan-compat")]
use uc_application::facade::{
    ActiveClipboardFacade, FileTransferFacade, InboundClipboardApplyPort,
};
#[cfg(feature = "lan-compat")]
use uc_application::facade::{AppPaths, ClipboardOutboundFacade};
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

pub(crate) fn build_relay_diagnostic() -> Option<Arc<dyn RelayDiagnosticPort>> {
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
    relay_diagnostic
}

/// `ClipboardRestoreFacade` 的可选装配输入。
///
/// GUI 和 daemon 需要 restore 能力；部分 CLI 查询入口不需要，因此通过
/// 显式选项传入，避免各入口各自复制 facade 拼装代码。
/// 构造 [`MobileSyncFacade`] —— 抽出来供 daemon-lifecycle 装配复用。
///
/// `apply_inbound` 由 engine 运行期组装并传入。`endpoint_info`
/// 由 [`uc_application::facade::ApplicationAssembly`] 持有的依赖携带
/// (单例,daemon LAN listener 与 facade 共享同一份
/// Arc),无需 caller 透传。`file_transfer` 进程级 facade:daemon 装配
/// 必传,SyncDoc apply 后 link + complete 让 mobile_lan transfer 在
/// file_transfer 表里闭环。
#[cfg(feature = "lan-compat")]
pub fn build_mobile_sync_facade(
    application: &crate::assembly::deps::MobileSyncApplicationDeps,
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
        clock: Arc::clone(&application.clock),
        // v3 SyncClipboard 兼容: 单一 minter 一次性出 (username, password,
        // password_hash, device_id), Argon2id 作为口令 hash;无状态 ZST,
        // 装配处直接 new 即可。
        credentials_minter: Arc::new(OsRngCredentialsMinter),
        password_hasher: Arc::new(Argon2idPasswordHasher),
        devices: mobile_ports.devices.clone(),
        endpoint_info: mobile_ports.endpoint_info.clone(),
        lan_interface_probe: Arc::new(NetworkInterfaceLanProbe::new()),
        settings: Arc::clone(&application.settings),
        apply_inbound,
        incoming_buffer: Arc::new(IncomingMobileBuffer::new()),
        file_staging: FilesystemMobileFileStaging::new_with_target_reserver(
            storage_paths.file_cache_dir.clone(),
            FsInboundFileTarget::new(Arc::clone(&application.settings)),
        ),
        snapshot_ports: MobileSyncSnapshotPorts {
            mobile_consumable_load: Arc::clone(&application.mobile_consumable_load),
            entry_repo: Arc::clone(&application.entry_repo),
            selection_repo: Arc::clone(&application.selection_repo),
            representation_repo: Arc::clone(&application.representation_repo),
            payload_resolver: Arc::clone(&application.payload_resolver),
            blob_reader: Arc::clone(&application.blob_reader),
        },
        file_transfer,
        clipboard_outbound,
        lan_lifecycle,
        // schema doc §7.6 / §12.2 P1：mobile_sync 域共用 process-wide analytics
        // sink。bootstrap 已把 GatedAnalyticsSink 包好，runtime 切换 noop / 真
        // 实 sink 是 sink 自身职责，不在此装配。
        analytics: Arc::clone(&application.analytics),
        active_clipboard,
        find_entry_by_snapshot_hash: Arc::clone(&application.find_entry_by_snapshot_hash),
        check_entry_availability: Arc::clone(&application.check_entry_availability),
    }))
}
