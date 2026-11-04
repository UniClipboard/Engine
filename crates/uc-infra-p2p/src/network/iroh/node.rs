//! Process-wide iroh node shared by every Slice 1+ transport.
//!
//! A single [`iroh::Endpoint`] per process owns the Ed25519 identity, the
//! UDP socket, and the NAT-traversal / relay state. Every business
//! transport (Space admission, clipboard sync, blob transfer) registers its own
//! ALPN on the same [`iroh::protocol::Router`] instead of binding a new
//! endpoint — see `docs/design-docs/layers/infrastructure.md` §4.2 (technical detail stays
//! contained) and the Slice 1 decision log on shared endpoint ownership.
//!
//! The builder pattern is deliberate: each `install_*` method is where a
//! new transport slices in. Invitation discovery does not register an ALPN; Slice 2
//! Phase 1 adds [`install_peer_reachability`]; Slice 2 Phase 2 adds
//! [`install_clipboard`]; Slice 3 will add `install_blobs` on the same
//! builder.
//!
//! [`install_peer_reachability`]: IrohSessionBuilder::install_peer_reachability
//! [`install_clipboard`]: IrohSessionBuilder::install_clipboard
#[cfg(test)]
use uuid::Uuid;

use super::super::mdns_interfaces::MdnsInterfaceSnapshot;
use super::blob_exchange::without_request_span;
use super::discovery::endpoint_id;
use futures_util::{stream, stream::BoxStream, StreamExt};
use iroh::endpoint::BindOpts;
use iroh::Watcher as _;
use std::collections::BTreeMap;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::str::FromStr;
#[cfg(not(any(test, feature = "test-util")))]
use std::sync::Mutex;
use std::sync::OnceLock;
use std::{path::PathBuf, sync::Arc, time::Duration};
use uc_application::deps::ActivePullPreparationPort;
use uc_application::deps::ClipboardReceiverPort;
use uc_application::deps::GroupUpdateReceiverPort;
use uc_application::deps::PeerIdentityDirectoryPort;
use uc_application::deps::{
    ActivePullClient, ActivePullReceiver, ActiveStateReceiver, ActiveStateSender,
    CurrentSpaceMemberScopePort,
};
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_vocab, uc_debug, uc_info, uc_warn,
};
use uc_sync_protocol::active_clipboard_pull::ACTIVE_CLIPBOARD_PULL_ALPN;
use uc_sync_protocol::active_clipboard_state::ACTIVE_CLIPBOARD_ALPN;

use super::admitted_connections::AdmittedConnections;
use super::discovery::{
    ConfiguredPeerAddresses, DirectoryAddressLookupBuilder, DiscoveryRuntime, LifetimeMdns,
};
use super::protocol_router::ProtocolRouterBuilder;
use super::session_generation::{
    SessionProtocolGenerationHandle, SessionProtocolHandlers, SessionProtocolHandlersBuilder,
    SessionProtocolRegistry, SessionProtocolRegistryError,
};
use iroh::address_lookup::AddrFilter;
use iroh::endpoint::{presets, QuicTransportConfig, VarInt};
use iroh::protocol::Router;
use iroh::{Endpoint, RelayConfig, RelayMode, RelayUrl, TransportAddr};
use iroh_mdns_address_lookup::{MdnsAddressLookup, MulticastInterfacesV4, MulticastInterfacesV6};
use noq_proto::congestion::{Bbr3Config, CubicConfig};
use tracing::instrument::WithSubscriber;
use tracing::{dispatcher, instrument, Dispatch};
use uc_application::deps::{
    BranchRecoveryPeerPort, BranchRecoveryReceiver, ConnectionHint, DiscoveryError,
    DiscoveryManager, PeerReachabilityEnvironmentPort, PeerReachabilityTransport,
};
use uc_core::settings::model::CongestionController;
use uc_core::settings::relay_routing::RelayRouting;
use zeroize::{Zeroize, ZeroizeOnDrop};

use uc_application::deps::{
    HandleAuthenticatedSpaceAdmissionMessagePort, ResolveJoinerInvitationPort,
    SpaceAdmissionTransportPort,
};

use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::blob::BlobTransferPort;
use uc_core::ports::pairing_invitation::{
    PairingInvitationAddressQueryPort, PairingInvitationByAddressPort, PairingInvitationPort,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{
    ActiveClipboardDispatchPort, ActiveClipboardPullClientPort, ActiveClipboardReceiverPort,
    ClipboardDispatchPort, ClockPort, ConnectionChannelPort, DeviceIdentityPort,
    LocalIdentityError, PeerAddressRepositoryPort, PeerReachabilityPort, SettingsPort,
};

use crate::pairing::PairingInvitationResolverAdapter;
use crate::rendezvous::{RendezvousClient, RendezvousPairingInvitationAdapter};
use uc_infra_security::InMemorySession;

use super::active_clipboard::{
    ActiveClipboardWire, IrohActivePullHandler, IrohActiveStateHandler, SessionActivePullLocalWork,
};
use super::addr_filter::apply_addr_filter;
use super::admission_transport_material::IrohAdmissionTransportMaterial;
use super::blob_exchange::{IrohBlobExchangeAdapter, IrohBlobHandler};
use super::blobs::{IrohBlobTransferAdapter, BLOBS_ALPN};
use super::connectivity::{IrohPeerReachabilityHandler, PresenceEnvironment};
use uc_application::deps::{
    BlobExchangePort, BlobReadStorePort, BlobStoreLifecyclePort, BlobTransport,
};
use uc_sync_protocol::peer_reachability::{LEGACY_PEER_REACHABILITY_ALPN, PEER_REACHABILITY_ALPN};
#[cfg(test)]
const LEGACY_CLIPBOARD_ALPN: &[u8] = b"uniclipboard/clipboard/0";
use super::connection_channel_adapter::IrohConnectionChannelAdapter;
use super::connectivity::IrohTransferProgressHandler;
use super::connectivity::{
    BranchRecoveryAdmission, ConnectivityProfile, IrohBranchRecoveryHandler, IrohClipboardHandler,
    IrohConnectionAdmission, IrohConnectivityBackend,
};
use super::group_update_adapter::{
    IrohGroupUpdateHandler, GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN,
};
use super::identity_store::IrohIdentityStore;
use super::inbound_peer::{PeerIdentityError, PeerIdentityResolver};
use super::membership_history_exchange_adapter::{
    HistoryConnectionAdmission, IrohMembershipHistoryExchangeHandler,
    IrohVerifiedPeerAddressRefresh, MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
    MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
};
use super::membership_identity_adapter::IrohMembershipIdentityAdapter;
use super::net_recovery::DemandRecoveryCoordinator;
use super::net_recovery::NetworkRecoveryObservationSource;
use super::network_partition::IrohNetworkPartitionGate;
use super::space_admission::{
    IrohSpaceAdmissionHandler, IrohSpaceAdmissionTransport, SpaceAdmissionChannelCredentialPort,
    SPACE_ADMISSION_ALPN,
};
use crate::network::clipboard::{ClipboardDispatchAdapter, ClipboardReceiverAdapter};
#[cfg(test)]
use uc_application::deps::ConnectivityPolicy;
use uc_application::deps::{
    ConnectivityBackend, ConnectivityManager, HistoryConnectionRetentionPort, HistoryReceiverPort,
    SessionConnectionAdmissionPort, TransferProgressTransport,
};
use uc_sync_protocol::clipboard::{
    ATTRIBUTED_CLIPBOARD_ALPN, CLIPBOARD_ALPN, MULTIPLEXED_CLIPBOARD_ALPN,
};
use uc_sync_protocol::membership_branch_recovery::MEMBERSHIP_BRANCH_RECOVERY_ALPN;
use uc_sync_protocol::transfer_progress::v2::TRANSFER_PROGRESS_ALPN;
use uc_sync_protocol::transfer_progress::LEGACY_TRANSFER_PROGRESS_ALPN;

#[cfg(test)]
mod discovery_tests;
mod shutdown;
pub use shutdown::IrohNodeShutdownError;

/// 邀请发布与解析端口，由 [`IrohSessionBuilder::install_pairing_invitation`] 构造。
///
/// resolver 只解析短码或完整邀请；`invitation`、`invitation_addresses` 和
/// `invitation_by_address` 是同一个 rendezvous HTTP adapter 的三个端口视图。
/// 它们共享 endpoint，但不会创建 session 或注册 Router handler。
pub struct PairingInvitationHandlers {
    pub joiner_invitation_resolver: Arc<dyn ResolveJoinerInvitationPort>,
    pub invitation: Arc<dyn PairingInvitationPort>,
    pub invitation_addresses: Arc<dyn PairingInvitationAddressQueryPort>,
    /// Dev-only: issue an invitation pinned to a single local IP. Not
    /// part of the standard sponsor lifecycle — wired into the CLI's
    /// hidden `dev pairing` subcommand for multi-NIC diagnosis.
    pub invitation_by_address: Arc<dyn PairingInvitationByAddressPort>,
}

/// 组装普通剪贴板连接负责人所需的完整能力；不暴露准入步骤。
pub struct ConnectivityDeps {
    pub backend: Arc<dyn ConnectivityBackend>,
    pub admission: Arc<dyn SessionConnectionAdmissionPort>,
}

pub struct HistoryConnectivityDeps {
    pub backend: Arc<dyn ConnectivityBackend>,
    pub admission: Arc<HistoryConnectionAdmission>,
}

/// 普通剪贴板双向能力；同一 endpoint 同时服务 /2 多流和 /1 单次交换。
pub struct ClipboardHandlers {
    pub dispatch: Arc<dyn ClipboardDispatchPort>,
    pub receiver: Arc<dyn ClipboardReceiverPort>,
}

/// The active-clipboard state ports produced by
/// [`IrohSessionBuilder::install_active_clipboard`].
///
/// `receiver` exposes the broadcast of inbound active-clipboard state
/// observations; `dispatch` sends one such observation to a single peer.
/// Both share the endpoint — the receiver handler is registered under
/// [`ACTIVE_CLIPBOARD_ALPN`] on the same `RouterBuilder` and `dispatch` dials
/// out on the same ALPN.
pub struct ActiveClipboardHandlers {
    pub dispatch: Arc<dyn ActiveClipboardDispatchPort>,
    pub receiver: Arc<dyn ActiveClipboardReceiverPort>,
}

/// The active-clipboard pull port produced by
/// [`IrohSessionBuilder::install_active_clipboard_pull`].
///
/// `client` requests the on-demand transfer envelope for content this device
/// observed but does not hold; the serve handler (the holder side) is
/// registered under [`ACTIVE_CLIPBOARD_PULL_ALPN`] on the same `RouterBuilder`
/// and delegates content production to the application-layer serve port passed
/// into the install method. Only the client port is returned — the serve side
/// has no outbound surface.
pub struct ActiveClipboardPullHandlers {
    pub client: Arc<dyn ActiveClipboardPullClientPort>,
}

/// [`IrohSessionBuilder::install_blobs`] 产出的 blob port。
pub struct BlobHandlers {
    pub transport: Arc<BlobTransport>,
    pub read_store: Arc<dyn BlobReadStorePort>,
    pub blob_transfer: Arc<dyn BlobTransferPort>,
}

/// Live iroh node with a spawned [`Router`].
///
/// Owns the [`Router`] so shutdown runs through a single call site; Slice 2 /
/// 3 add handlers by extending [`IrohSessionBuilder`], not by adding shutdown
/// paths here.
pub struct IrohNode {
    session_context: Arc<IrohSessionContext>,
    router: Router,
    session_protocols: Arc<SessionProtocolRegistry>,
    session_generation: Option<SessionProtocolGenerationHandle>,
    connection_observations: super::observed_connections::ObservedConnections,
    /// Relay self-healing watchdog (see `net_recovery`). `None` in LAN-only
    /// mode where there is no home relay to watch. Held so [`shutdown`] can
    /// abort it deterministically and surface a panic instead of letting a
    /// dead watchdog vanish silently.
    ///
    /// [`shutdown`]: IrohNode::shutdown
    net_recovery: Option<tokio::task::JoinHandle<()>>,
    /// 周期性连接/路径数量普查；节点关闭时 abort。
    connection_census: Option<tokio::task::JoinHandle<()>>,
    discovery_runtime: DiscoveryRuntime,
    /// Keeps the process-wide single-node reservation until shutdown is
    /// complete. Dropping a partially-built or live node releases it too.
    _run_lease: NodeRunLease,
}

impl IrohNode {
    /// 为下一代空间会话准备协议能力，但不影响当前会话。
    pub fn prepare_session(&self) -> IrohSessionBuilder {
        IrohSessionBuilder::new(Arc::clone(&self.session_context))
    }

    /// 停止当前空间会话并排空其连接，长期网络节点继续运行。
    pub async fn quiesce_session(&mut self) -> Result<(), IrohNodeError> {
        let Some(generation) = self.session_generation.as_ref() else {
            return Ok(());
        };
        self.session_protocols
            .quiesce(generation)
            .await
            .map_err(IrohNodeError::session_protocol)?;
        self.session_generation = None;
        Ok(())
    }

    /// 发布已经完整准备好的下一代空间会话。
    pub async fn activate_session(
        &mut self,
        prepared: PreparedIrohSession,
    ) -> Result<(), IrohNodeError> {
        let generation = self
            .session_protocols
            .publish(prepared.handlers)
            .await
            .map_err(IrohNodeError::session_protocol)?;
        self.session_generation = Some(generation);
        Ok(())
    }

    #[cfg(any(test, feature = "test-util"))]
    pub fn has_active_protocol_for_test(&self, alpn: &[u8]) -> bool {
        self.session_protocols.has_active_handler_for_test(alpn)
    }

    /// 测试匿名连接是否在完成协议协商后被准入拒绝。
    #[cfg(any(test, feature = "test-util"))]
    pub async fn rejects_anonymous_protocol_for_test(&self, alpn: &[u8]) -> bool {
        let client = match Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
        {
            Ok(client) => client,
            Err(_) => return false,
        };
        let rejected = match tokio::time::timeout(
            Duration::from_secs(5),
            client.connect(self.session_context.endpoint.addr(), alpn),
        )
        .await
        {
            Ok(Ok(connection)) => tokio::time::timeout(Duration::from_secs(2), connection.closed())
                .await
                .is_ok(),
            Ok(Err(_)) | Err(_) => false,
        };
        client.close().await;
        rejected
    }

    #[cfg(any(test, feature = "test-util"))]
    pub async fn accepts_protocol_for_test(&self, alpn: &[u8]) -> bool {
        let client = match Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
        {
            Ok(client) => client,
            Err(_) => return false,
        };
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.connect(self.session_context.endpoint.addr(), alpn),
        )
        .await;
        let connected = match result {
            Ok(Ok(connection)) => {
                tokio::time::timeout(Duration::from_millis(100), connection.closed())
                    .await
                    .is_err()
            }
            Ok(Err(_)) | Err(_) => false,
        };
        client.close().await;
        connected
    }
}

/// Bootstrap-time configuration for [`IrohNodeBuilder`]. Defaults cover
/// production; integration tests override the rendezvous URL (pointing at
/// a mock server) and usually disable relays (loopback-only handshake).
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct IrohRelayAccessToken(String);

impl IrohRelayAccessToken {
    pub fn new(mut value: String) -> Result<Self, IrohNodeError> {
        if value.is_empty()
            || value.len() > 4096
            || !value.is_ascii()
            || value.chars().any(char::is_control)
        {
            value.zeroize();
            return Err(IrohNodeError::InvalidRelayAccessToken);
        }
        Ok(Self(value))
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for IrohRelayAccessToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("IrohRelayAccessToken([REDACTED])")
    }
}

#[derive(Debug, Clone)]
pub struct IrohNodeConfig {
    /// Override rendezvous base URL. `None` → use
    /// [`crate::rendezvous::RENDEZVOUS_BASE_URL`].
    pub rendezvous_base_url: Option<String>,
    /// If true, bind the endpoint in **strict LAN-only mode**:
    ///
    /// 1. `RelayMode::Disabled` —— 不连任何 iroh relay；
    /// 2. **同时清掉 `presets::N0` 默认注入的 `PkarrPublisher` /
    ///    `DnsAddressLookup`**，避免向 `dns.iroh.link` 发布 endpoint info 或
    ///    通过 n0 公共 DNS 解析对端 NodeId。LAN-only 用户的合理预期是"只靠
    ///    mDNS 在局域网内发现对方"，而不是"relay 关掉但仍向公共 DNS 广播
    ///    自己的 NodeId"（见 `.planning/research/PITFALLS.md` Pitfall 5）。
    ///
    /// 副作用（**这是 LAN-only 的设计意图，不是 bug**）：
    /// - 跨网段已配对设备无法被发现（关 relay 已经如此，这里把发现路径也
    ///   收紧到 mDNS 同子网）；
    /// - 没有 mDNS 的环境（部分 Docker / NAS / 未装 avahi 的 Linux）会完全
    ///   不可达，这是用户启用 LAN-only 时承担的取舍。
    ///
    /// 集成测试也走 `disable_relays = true` 路径（loopback 走 mDNS 或显式
    /// `EndpointAddr` 注入）。production `false` 保持完整 N0 行为
    /// （pkarr publish + DNS lookup + relay fallback）。
    pub disable_relays: bool,
    /// 节点实际使用的 relay URL 列表，由调用方（产品默认或用户配置）决定，
    /// 本模块不再隐含任何上游默认列表。`disable_relays = false` 时必须非空并翻译为
    /// `RelayMode::Custom`；`disable_relays = true` 时该列表不参与 endpoint bind。
    pub relay_urls: Vec<String>,
    /// Access tokens keyed by their matching relay URL. Values are
    /// redacted in debug output and zeroized when dropped.
    pub relay_access_tokens: BTreeMap<String, IrohRelayAccessToken>,
    /// If true, allow VPN / overlay-network virtual NIC addresses (CGNAT
    /// `100.64.0.0/10`, Tailscale ULA `fd7a:115c:a1e0::/48`) to flow through
    /// the address filter as direct-connection candidates. Default `false`
    /// (drop them). Power users set this when both peers share an overlay
    /// network and want iroh to leverage that path.
    ///
    /// Clash fake-ip (`198.18.0.0/15`) and IPv4 link-local (`169.254.0.0/16`)
    /// are unconditionally filtered regardless of this flag — they have no
    /// legitimate cross-host use case.
    pub allow_overlay_network_addrs: bool,
    /// Congestion controller to use for QUIC connections.
    /// `Cubic` (default) gives excellent LAN throughput; `Bbr3` may be
    /// better on lossy WAN paths. Mapped from `Settings.network`.
    pub congestion_controller: CongestionController,
    /// 把 iroh 的 UDP 监听固定到同一端口（UniClipboard#900）。`None` 时 IPv4
    /// 与 IPv6 都由系统分配随机端口（默认）。`Some(port)` 时 IPv4 绑定
    /// `0.0.0.0:port`（必须成功，端口被占用则节点启动失败），IPv6 尽力绑定
    /// `[::]:port`（本机没有 IPv6 或该端口不可用时跳过，不影响启动）。
    ///
    /// 固定端口让对端保存的直连线索在重启后仍然有效，也让防火墙、端口转发和
    /// WireGuard 等隧道只需放行一个已知端口。取值由组装层按“环境变量覆盖设置”
    /// 决定，infra 只消费结果。
    pub bind_port: Option<u16>,
    /// Advertise a configured public socket address as one of this node's
    /// own direct-connection candidates (UniClipboard#900). `None` → only
    /// discovered/reflexive addresses are advertised (today's behavior).
    /// `Some(addr)` → inject `addr` into the advertised endpoint address at
    /// **bind time** (iroh `Builder::external_addr`), so it is present in the
    /// very first `endpoint.addr()` snapshot — i.e. before pairing/dispatch
    /// exchange the address blob with peers. With the relay disabled, a
    /// remote desktop that knows only this address can dial the node directly.
    ///
    /// Sourced from `UC_IROH_PUBLIC_ADDR` (parsed in `uc-bootstrap`).
    pub public_addr: Option<SocketAddr>,
    /// 测试专用的认证 peer 网络分区门；生产组装始终为 `None`。
    pub network_partition_gate: Option<IrohNetworkPartitionGate>,
    /// 用户为配对设备配置的地址来源；`None` 时不装配该发现来源。
    pub configured_peer_addresses: Option<Arc<ConfiguredPeerAddresses>>,
}

impl Default for IrohNodeConfig {
    /// 默认配置等于产品默认路由：启用 relay 并使用产品内置列表。
    fn default() -> Self {
        Self {
            rendezvous_base_url: None,
            disable_relays: false,
            relay_urls: RelayRouting::BuiltIn.effective_urls(&[]),
            relay_access_tokens: BTreeMap::new(),
            allow_overlay_network_addrs: false,
            congestion_controller: CongestionController::default(),
            bind_port: None,
            public_addr: None,
            network_partition_gate: None,
            configured_peer_addresses: None,
        }
    }
}

/// Snapshot the candidate set this endpoint is currently advertising and
/// log it at INFO. We compare these IPs against `connect.rs`'s
/// `selected path` log to spot when magicsock is publishing virtual-NIC
/// addresses (Clash TUN, WireGuard, Tailscale) that skew remote candidate
/// races. Two snapshots are taken — `post-bind` (just after the UDP
/// socket comes up) and `post-spawn` (after `install_*` finish, which
/// gives magicsock more time to enumerate interfaces). Refs
/// UniClipboard#486.
fn log_publish_addrs(endpoint: &Endpoint, stage: &'static str) {
    let addr = endpoint.addr();
    let ip_addrs: Vec<String> = addr
        .addrs
        .iter()
        .filter_map(|a| match a {
            TransportAddr::Ip(s) => Some(s.to_string()),
            _ => None,
        })
        .collect();
    let relay_urls: Vec<String> = addr
        .addrs
        .iter()
        .filter_map(|a| match a {
            TransportAddr::Relay(u) => Some(u.to_string()),
            _ => None,
        })
        .collect();
    uc_info!(
        stage = stage,
        ip_addr_count = ip_addrs.len(),
        relay_url_count = relay_urls.len(),
        "iroh endpoint publish snapshot (refs UniClipboard#486)"
    );
}

/// Build the QUIC `QuicTransportConfig` we attach to the shared endpoint.
///
/// Defaults are tuned assuming "internet"-shaped paths; on macOS the shared
/// Wi-Fi radio is periodically yanked away by AWDL (AirDrop / Handoff /
/// Continuity) and background SSID scans, producing 50–150ms RTT spikes
/// even on a quiet LAN with great signal. Left at defaults, CUBIC interprets
/// those spikes as persistent congestion after 3 PTOs and halves CWND, so a
/// 60 MB transfer drags to ~700 KB/s over a link capable of hundreds of
/// MB/s. These knobs give QUIC more slack for the jitter floor without
/// changing the congestion algorithm itself.
///
/// Any change here affects every transport (pairing / peer_reachability / clipboard
/// / blobs) because they share this one endpoint.
///
/// iroh 0.97 reshaped the API: the old quinn-style `&mut TransportConfig`
/// lives behind `iroh::endpoint::QuicTransportConfigBuilder` now, returned
/// by `QuicTransportConfig::builder()`. Setters are by-value chained
/// instead of `&mut self`, but the underlying knobs are the same noq
/// (the project's quinn fork) `TransportConfig` surface.
fn build_transport_config(cc: CongestionController) -> QuicTransportConfig {
    let cc_factory: Arc<dyn noq_proto::congestion::ControllerFactory + Send + Sync> = match cc {
        CongestionController::Cubic => Arc::new(CubicConfig::default()),
        CongestionController::Bbr3 => Arc::new(Bbr3Config::default()),
    };
    uc_info!(
        congestion_controller = log_vocab(&cc),
        "QUIC congestion controller selected"
    );
    QuicTransportConfig::builder()
        .congestion_controller_factory(cc_factory)
        // QUIC flow-control sized for hole-punched cross-WAN BDP. iroh-blobs
        // opens a single bidi stream per blob fetch (`open_bi`), so the
        // stream window — not the connection window — is the per-transfer
        // ceiling: max throughput ≈ stream_receive_window / RTT. Quinn's
        // default 1.25 MB is sized for a 100ms × 100 Mbps internet path; on
        // the relay fallback (~360ms RTT) it caps a single blob at ~28 Mbps,
        // and even on a successful hole-punch across continents (~200ms RTT)
        // it caps at ~50 Mbps. 32 MB supports 1 Gbps over 256ms RTT with
        // headroom — well beyond any realistic peer link. Memory cost is
        // bounded: each in-flight blob fetch can stage up to 32 MB of
        // unread chunks, but iroh-blobs reads continuously so the buffer
        // rarely fills.
        .stream_receive_window(VarInt::from_u32(32 * 1024 * 1024))
        // Mirror on the send side. Default `send_window = 8 × 1.25 MB = 10
        // MB` caps connection-total in-flight bytes at the same WAN-hostile
        // value. 64 MB keeps both directions of a sync from saturating
        // their own backpressure on long paths.
        .send_window(64 * 1024 * 1024)
        // Default 3 → 5 PTOs before declaring persistent congestion, so
        // isolated 100–150ms AWDL spikes don't collapse CWND.
        .persistent_congestion_threshold(5)
        // Default 30s idle timeout drops connections between bursty user
        // copies, forcing a fresh hole-punch on every resume. 60s keeps
        // the ConnectionPool entry warm across typical copy gaps.
        .max_idle_timeout(Some(
            Duration::from_secs(60)
                .try_into()
                .expect("60s is well within QUIC's idle-timeout encoding"),
        ))
        // QUIC-level keepalive, complementary to PeerKeepAliveWorker's
        // app-level pings: keeps the magicsock path's last-activity stamp
        // fresh so iroh doesn't tear the path down between transfers.
        // Note: iroh 0.98 changed the signature from `Option<Duration>`
        // (where `None` disabled) to bare `Duration` (always enabled),
        // because the iroh `QuicTransportConfigBuilder` already pre-sets a
        // sane default — calling this setter just overrides it.
        .keep_alive_interval(Duration::from_secs(15))
        // Multipath QUIC extension is *not* enabled — we deliberately leave
        // `max_concurrent_multipath_paths` at the noq-proto default of
        // `None` (i.e. don't call the setter). Rationale:
        //
        // 1. Our shape is 2-peer P2P. Multipath QUIC is designed for hosts
        //    with multiple ISPs simultaneously; we have one functional
        //    path between any two devices.
        //
        // 2. Empirically, enabling multipath (any value ≥ 1) triggers
        //    upstream bug noq#512 ("PTO fires when unset for an abandoned
        //    path") whenever the cross-peer link has more than one
        //    candidate (Tailscale + LAN + vmnet etc) and one of those
        //    candidates gets abandoned during handshake. The race
        //    manifests as: incoming connection arrives at the iroh
        //    endpoint, ALPN routes to the selected business protocol,
        //    `noq_proto::connection` silently swallows the PTO timer,
        //    handshake never makes progress, and `iroh::protocol`
        //    eventually 60s-timeouts at the router.accept ALPN deadline.
        //    We observed this at ~50% under release build with multipath
        //    enabled across 10 mac↔fedora rounds, but 0% on fedora
        //    self-pair (localhost UDP, no path abandonment).
        //
        // 3. iroh#4124 (the old reason this setter existed at all —
        //    PathId-budget exhaustion at 13) was about ramping the budget
        //    up. Since we're not enabling multipath, there is no PathId
        //    budget to exhaust.
        //
        // Revisit only after upstream noq#512 is fixed AND we have a
        // real product need for multipath (e.g. mobile multi-radio).
        .build()
}

/// Build the `AddrFilter` we hand to `Endpoint::builder().addr_filter(...)`.
/// The filter is applied at the `AddressLookupServices` layer (see
/// iroh#3960 / #4010), upstream of every individual lookup service, so a
/// single registration covers pkarr / mdns / static / DHT lookups in one
/// place — that's what makes this a viable replacement for "fork iroh and
/// patch magicsock" (issue #486 §三 A).
///
/// `allow_overlay` is captured by the closure so the predicate behaviour is
/// fixed at endpoint bind time — the iroh `Endpoint` does not support
/// runtime mutation of address filters; toggling
/// `Settings.network.allow_overlay_network_addrs` requires a daemon restart
/// (the same constraint as `disable_relays`).
fn build_addr_filter(allow_overlay: bool) -> AddrFilter {
    AddrFilter::new(move |addrs: &Vec<TransportAddr>| apply_addr_filter(addrs, allow_overlay))
}

fn relay_mode_from_config(config: &IrohNodeConfig) -> Result<RelayMode, IrohNodeError> {
    if config.disable_relays {
        return Ok(RelayMode::Disabled);
    }

    if config.relay_urls.is_empty() {
        return Err(IrohNodeError::InvalidRelayUrl(
            RelayUrlProblem::NoneConfigured,
        ));
    }

    let mut relay_configs = Vec::with_capacity(config.relay_urls.len());
    for raw in &config.relay_urls {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(IrohNodeError::InvalidRelayUrl(RelayUrlProblem::Empty));
        }
        let parsed = trimmed
            .parse::<RelayUrl>()
            .map_err(|err| IrohNodeError::InvalidRelayUrl(RelayUrlProblem::Parse(err)))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(IrohNodeError::InvalidRelayUrl(
                RelayUrlProblem::UnsupportedScheme,
            ));
        }
        if parsed.host_str().is_none() {
            return Err(IrohNodeError::InvalidRelayUrl(RelayUrlProblem::MissingHost));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(IrohNodeError::InvalidRelayUrl(RelayUrlProblem::Credentials));
        }
        let mut relay_config = RelayConfig::from(parsed.clone());
        if let Some(token) = config
            .relay_access_tokens
            .get(raw)
            .or_else(|| config.relay_access_tokens.get(parsed.as_str()))
        {
            relay_config = relay_config.with_auth_token(token.expose_secret());
        }
        relay_configs.push(relay_config);
    }

    Ok(RelayMode::Custom(relay_configs.into_iter().collect()))
}

/// A process-wide lease prevents two live production endpoints from sharing
/// mutable runtime state. Unlike the former `OnceLock`, the lease is owned by
/// the builder and then the live node, so a completed shutdown permits restart.
#[cfg(not(any(test, feature = "test-util")))]
static NODE_RUN_ACTIVE: Mutex<bool> = Mutex::new(false);

struct NodeRunLease;

impl NodeRunLease {
    fn acquire() -> Result<Self, IrohNodeError> {
        #[cfg(not(any(test, feature = "test-util")))]
        {
            let mut active = NODE_RUN_ACTIVE
                .lock()
                // discarded-source[lock-poisoned]: `std::sync::PoisonError<Guard>`: the poison error holds the guard and cannot be stored across threads
                .map_err(|_| IrohNodeError::RuntimeStatePoisoned)?;
            if *active {
                return Err(IrohNodeError::AlreadyRunning);
            }
            *active = true;
        }
        Ok(Self)
    }
}

impl Drop for NodeRunLease {
    fn drop(&mut self) {
        #[cfg(not(any(test, feature = "test-util")))]
        {
            // Clear the runtime configuration before releasing the lease so a
            // newly-bound node cannot inherit the previous node's LAN policy.
            super::runtime_consts::clear_lan_only();
            match NODE_RUN_ACTIVE.lock() {
                Ok(mut active) => *active = false,
                Err(_) => uc_warn!("iroh node runtime state lock poisoned during release"),
            }
        }
    }
}

/// 长期网络构造器：绑定 endpoint 并启动固定 Router。
pub struct IrohNodeBuilder {
    session_context: Arc<IrohSessionContext>,
    connection_observations: super::observed_connections::ObservedConnections,
    network_recorder: uc_observability_contract::diagnostics::connectivity::NetworkRecorder,
    router_builder: ProtocolRouterBuilder,
    session_protocols: Arc<SessionProtocolRegistry>,
    run_lease: NodeRunLease,
    discovery_runtime: DiscoveryRuntime,
}

struct IrohSessionContext {
    discovery: Arc<DiscoveryManager>,
    endpoint: Arc<Endpoint>,
    demand_recovery: Arc<DemandRecoveryCoordinator>,
    network_recovery_observations: Arc<NetworkRecoveryObservationSource>,
    config: IrohNodeConfig,
}

pub struct IrohSessionBuilder {
    admission_transport: Arc<IrohSpaceAdmissionTransport>,
    context: Arc<IrohSessionContext>,
    handlers: SessionProtocolHandlersBuilder,
    /// 本会话各协议 handler 共享的已授权入站连接登记。
    admitted_connections: AdmittedConnections,
}

pub struct PreparedIrohSession {
    handlers: SessionProtocolHandlers,
}

impl PreparedIrohSession {
    pub async fn shutdown(self) {
        self.handlers.shutdown().await;
    }
}

impl IrohSessionBuilder {
    fn new(context: Arc<IrohSessionContext>) -> Self {
        let admission_transport =
            Arc::new(IrohSpaceAdmissionTransport::new(context.endpoint.clone()));
        Self {
            admission_transport,
            context,
            handlers: SessionProtocolHandlersBuilder::new(),
            admitted_connections: AdmittedConnections::default(),
        }
    }

    /// 完成会话能力装配。结果只有整体发布后才会开始接收请求。
    pub fn finish(self) -> PreparedIrohSession {
        PreparedIrohSession {
            handlers: self.handlers.build(),
        }
    }

    /// 订阅设备级发现负责人；每次提示使用当前会话身份目录，不缓存 DeviceId 绑定。
    pub async fn connection_hints(
        &self,
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        fingerprints: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> BoxStream<'static, Result<ConnectionHint, anyhow::Error>> {
        let local = self
            .context
            .endpoint
            .watch_addr()
            .stream()
            .skip(1)
            .map(|_| Ok(ConnectionHint::NetworkChanged));
        let identity = Arc::new(PeerIdentityResolver::new(identities, fingerprints));
        let changes = self.context.discovery.subscribe_changes();
        let discovered = stream::unfold(changes, |mut changes| async move {
            match changes.recv().await {
                Ok(peer) => Some((Some(peer), changes)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => Some((None, changes)),
                Err(tokio::sync::broadcast::error::RecvError::Closed) => None,
            }
        })
        .filter_map(move |event| {
            let identity = Arc::clone(&identity);
            async move {
                let peer = match event {
                    Some(peer) => peer,
                    None => return Some(Ok(ConnectionHint::NetworkChanged)),
                };
                let endpoint = endpoint_id(&peer)?;
                match identity.resolve(endpoint.as_bytes()).await {
                    Ok(device) => Some(Ok(ConnectionHint::PeerAddressChanged(device))),
                    Err(PeerIdentityError::Unresolved | PeerIdentityError::Ambiguous) => None,
                    Err(
                        error @ (PeerIdentityError::MemberRead(_)
                        | PeerIdentityError::Fingerprint(_)),
                    ) => Some(Err(anyhow::Error::new(error))),
                }
            }
        });
        let observations = self.context.network_recovery_observations.subscribe();
        let recovery = stream::unfold(observations, |mut receiver| async move {
            use super::net_recovery::NetworkRecoveryObservation;
            let hint = match receiver.recv().await {
                Ok(NetworkRecoveryObservation::LocalRelayRecovered) => {
                    uc_application::deps::ConnectionHint::RelayRecovered
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    uc_application::deps::ConnectionHint::NetworkChanged
                }
                Ok(NetworkRecoveryObservation::CommunicationFailed(device)) => {
                    uc_application::deps::ConnectionHint::CommunicationFailed(device)
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            };
            Some((Ok(hint), receiver))
        });
        let hints = stream::select(stream::select(local, discovered), recovery);
        #[cfg(any(test, feature = "test-util"))]
        let hints = hints.filter_map({
            let gate = self.context.config.network_partition_gate.clone();
            move |hint| {
                let suppressed = gate
                    .as_ref()
                    .is_some_and(|gate| gate.opportunities_suppressed());
                futures_util::future::ready((!suppressed).then_some(hint))
            }
        });
        Box::pin(hints)
    }

    /// 提供当前准入材料能力，具体 Endpoint 留在 Iroh adapter 内部。
    pub fn admission_transport_material(
        &self,
        fingerprints: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Arc<IrohAdmissionTransportMaterial> {
        Arc::new(IrohAdmissionTransportMaterial::new(
            Arc::clone(&self.context.endpoint),
            fingerprints,
        ))
    }

    fn install_session_handler<I, A>(
        &mut self,
        alpns: I,
        handler: impl Into<Box<dyn iroh::protocol::DynProtocolHandler>>,
    ) -> Result<(), IrohNodeError>
    where
        I: IntoIterator<Item = A>,
        A: AsRef<[u8]>,
    {
        self.handlers
            .install(alpns, handler)
            .map_err(IrohNodeError::session_protocol)
    }

    /// 在 Application 构造前取得同一 endpoint 支撑的被动出站能力。
    pub fn space_admission_transport(&self) -> Arc<dyn SpaceAdmissionTransportPort> {
        self.admission_transport.clone()
    }

    /// 在 Router spawn 前一次性安装 Application 的认证消息 endpoint。
    pub fn install_space_admission(
        &mut self,
        endpoint: Arc<dyn HandleAuthenticatedSpaceAdmissionMessagePort>,
        credentials: Arc<dyn SpaceAdmissionChannelCredentialPort>,
    ) -> Result<(), IrohNodeError> {
        let handler =
            IrohSpaceAdmissionHandler::new(self.admission_transport.clone(), endpoint, credentials)
                .map_err(|source| IrohNodeError::AdmissionInstall {
                    source: anyhow::anyhow!(source),
                })?;
        self.install_session_handler([SPACE_ADMISSION_ALPN], handler)
    }
}

fn session_protocol_alpns() -> [&'static [u8]; 16] {
    [
        SPACE_ADMISSION_ALPN,
        PEER_REACHABILITY_ALPN,
        LEGACY_PEER_REACHABILITY_ALPN,
        ATTRIBUTED_CLIPBOARD_ALPN,
        MULTIPLEXED_CLIPBOARD_ALPN,
        CLIPBOARD_ALPN,
        GROUP_UPDATE_ALPN,
        LEGACY_GROUP_UPDATE_ALPN,
        MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
        MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
        MEMBERSHIP_BRANCH_RECOVERY_ALPN,
        ACTIVE_CLIPBOARD_ALPN,
        ACTIVE_CLIPBOARD_PULL_ALPN,
        TRANSFER_PROGRESS_ALPN,
        LEGACY_TRANSFER_PROGRESS_ALPN,
        BLOBS_ALPN,
    ]
}

impl IrohNodeBuilder {
    /// 创建一套尚未发布的空间会话能力。
    pub fn prepare_session(&self) -> IrohSessionBuilder {
        IrohSessionBuilder::new(Arc::clone(&self.session_context))
    }

    /// Bind the iroh endpoint, reusing the Ed25519 secret persisted by
    /// [`IrohIdentityStore`] so the endpoint's on-wire identity matches the
    /// fingerprint `LocalIdentityPort` hands out to domain code.
    ///
    /// 创建共享 Iroh endpoint；业务 ALPN 仅由后续 `install_*` 明确注册。
    #[instrument(skip_all)]
    pub async fn bind(
        identity_store: &IrohIdentityStore,
        config: IrohNodeConfig,
    ) -> Result<Self, IrohNodeError> {
        let run_lease = NodeRunLease::acquire()?;

        let secret = identity_store.ensure_secret_key()?;
        let relay_mode = relay_mode_from_config(&config)?;
        let recorder =
            uc_observability_contract::diagnostics::connectivity::NetworkRecorder::current();
        // 本机 mDNS 是可降级的发现来源：组播套接字无法建立时节点照常启动，显式地址、已保存的
        // 直连线索与启用时的云端发现仍然可用，长期 mDNS 在后台按有界退避重试。长期设备发现与
        // 配对发现使用同一接口策略，在全部可用 IPv4 接口上收发；长期发现另外在这些接口上
        // 收发 IPv6 mDNS。
        let local_id = secret.public();
        let mdns = LifetimeMdns::start(
            move || {
                MdnsAddressLookup::builder()
                    .multicast_interfaces_v4(MulticastInterfacesV4::new(|| {
                        MdnsInterfaceSnapshot::capture().into_parts().1
                    }))
                    .multicast_interfaces_v6(MulticastInterfacesV6::new(|| {
                        MdnsInterfaceSnapshot::capture().into_multicast_v6()
                    }))
                    .build(local_id)
            },
            recorder.clone(),
        );
        // Snapshot the overlay flag before consuming `config` into `Self`.
        let allow_overlay = config.allow_overlay_network_addrs;
        uc_info!(
            target: "iroh.addr_filter",
            allow_overlay = allow_overlay,
            "addr filter configured for overlay-network addresses (Tailscale 100.64/10 + fd7a:115c:a1e0::/48)"
        );
        let connection_observations =
            super::observed_connections::ObservedConnections::new(recorder.clone());
        let session_protocols = Arc::new(SessionProtocolRegistry::new());
        // 第三方构造和驱动一起隔离，避免在future创建时捕获调用请求的parent。
        let mut endpoint_builder = dispatcher::with_default(&Dispatch::none(), || {
            Endpoint::builder(presets::N0)
                .clear_address_lookup()
                .secret_key(secret)
                .relay_mode(relay_mode)
                .transport_config(build_transport_config(config.congestion_controller))
                // UniClipboard#486: drop Clash TUN / link-local IPs from every
                // address-lookup service in one shot, and drop CGNAT/Tailscale
                // overlay IPs unless the user opts in. See `build_addr_filter`.
                .addr_filter(build_addr_filter(allow_overlay))
                .hooks(session_protocols.endpoint_hooks())
        });
        // 保留 N0 原生平台的原服务及原配置，只包裹固定的采集入口。
        if !config.disable_relays {
            use super::observed_address_lookup::ObservedAddressLookupBuilder;
            use uc_observability_contract::diagnostics::connectivity::DiscoverySource;
            endpoint_builder = endpoint_builder.address_lookup(ObservedAddressLookupBuilder::new(
                iroh::address_lookup::PkarrPublisher::n0_dns(),
                DiscoverySource::Pkarr,
                recorder.clone(),
            ));
        }
        if let Some(gate) = &config.network_partition_gate {
            endpoint_builder = endpoint_builder.hooks(gate.clone());
        }
        endpoint_builder = endpoint_builder.hooks(connection_observations.clone());

        // LAN-only Mode 收紧（Pitfall 5 防御 + 与 `disable_relays` 字段 doc 一致）：
        // `presets::N0` 默认注入 `PkarrPublisher` (publish 到 dns.iroh.link) +
        // `DnsAddressLookup`，即便 `RelayMode::Disabled` 也会持续向 n0 公共 DNS
        // 广播自己的 NodeId 并解析对端。这与"LAN-only 仅靠 mDNS 在局域网内发现
        // 对方"的用户预期相悖。所以 `disable_relays = true` 路径下显式 clear
        // 掉 N0 注入的 lookup services，再单挂 mDNS。
        //
        // 同步把 LAN-only 状态固化到 `runtime_consts::LAN_ONLY` 进程常量 ——
        // `connect.rs` 出站 dial 时会读这个常量，从对端 `EndpointAddr` 中剥掉
        // `TransportAddr::Relay`。否则即便本端 `RelayMode::Disabled`，iroh 仍
        // 会用对端发布的 relay url 走中转（已在 dev 日志中观测到）。
        //
        // 取舍：跨网段已配对设备无法通过 NodeId 反查到，这是 LAN-only 的设计
        // 意图（不是 bug）。
        super::runtime_consts::install_lan_only(config.disable_relays);
        if config.disable_relays {
            endpoint_builder = endpoint_builder.clear_address_lookup();
            uc_info!(
                target: "iroh.address_lookup",
                "LAN-only mode: cleared n0 pkarr/DNS lookup services; only mDNS will publish/resolve",
            );
        }

        // UniClipboard#900 (a): 固定 UDP 端口，让对端保存的直连线索跨重启有效。
        // 用户定义的绑定替换 iroh 默认的随机端口套接字（仍监听全部网卡）。IPv4 必须
        // 成功，端口被占用时以 `Bind` 失败；IPv6 设为非必需，没有 IPv6 或端口不可用时
        // 跳过，不让只有 IPv4 的主机启动失败。
        if let Some(port) = config.bind_port {
            endpoint_builder = endpoint_builder
                .bind_addr((Ipv4Addr::UNSPECIFIED, port))
                .map_err(|err| {
                    IrohNodeError::Bind(anyhow::Error::new(err).context("pin iroh IPv4 UDP port"))
                })?
                .bind_addr_with_opts(
                    (Ipv6Addr::UNSPECIFIED, port),
                    BindOpts::default().set_is_required(false),
                )
                .map_err(|err| {
                    IrohNodeError::Bind(anyhow::Error::new(err).context("pin iroh IPv6 UDP port"))
                })?;
            uc_info!(
                target: "iroh.bind",
                "pinned iroh UDP sockets to a fixed port"
            );
        }

        // UniClipboard#900 (b): advertise a configured public address as a
        // direct-connection candidate. `external_addr` is applied at build
        // time, so the address is present in the very first `endpoint.addr()`
        // snapshot — before pairing/dispatch exchange the address blob with
        // peers (acceptance criterion). With the relay disabled, a remote
        // desktop that knows only this address can dial the node directly.
        if let Some(public_addr) = config.public_addr {
            endpoint_builder = endpoint_builder.external_addr(public_addr);
            uc_info!(
                target: "iroh.bind",
                "advertising configured public address as a direct candidate (UC_IROH_PUBLIC_ADDR)",
            );
        }

        let discovery_slot = Arc::new(OnceLock::new());
        let endpoint = async {
            endpoint_builder
                .address_lookup(DirectoryAddressLookupBuilder {
                    mdns: mdns.clone(),
                    lan_only: config.disable_relays,
                    configured: config.configured_peer_addresses.clone(),
                    recorder: recorder.clone(),
                    manager: discovery_slot.clone(),
                })
                .bind()
                .await
        }
        .with_subscriber(Dispatch::none())
        .await
        .map_err(|err| IrohNodeError::Bind(anyhow::Error::new(err)))?;
        let endpoint = Arc::new(endpoint);
        // 只有 bind 完成才将来源标记为可采集；失败构造不能留下 Enabled 假象。
        {
            use uc_observability_contract::diagnostics::connectivity::{
                LocalDiagnosticSource as Source, SourceCapability, SourceCollection,
            };
            for source in [Source::Connections, Source::ConnectionPaths] {
                recorder.register_source(
                    source,
                    SourceCapability::Partial,
                    SourceCollection::Enabled,
                );
            }
            recorder.register_source(
                Source::MdnsDiscovery,
                SourceCapability::Partial,
                // 后台重试成功时由重试任务改记为可用。
                if mdns.is_available() {
                    SourceCollection::Enabled
                } else {
                    SourceCollection::Unavailable
                },
            );
            for source in [
                Source::DnsDiscovery,
                Source::PkarrDiscovery,
                Source::RelayRecovery,
            ] {
                recorder.register_source(
                    source,
                    SourceCapability::Partial,
                    if config.disable_relays {
                        SourceCollection::Disabled
                    } else {
                        SourceCollection::Enabled
                    },
                );
            }
        }
        if let Some(gate) = &config.network_partition_gate {
            gate.install_local_endpoint_id(*endpoint.id().as_bytes());
        }
        let demand_recovery = Arc::new(DemandRecoveryCoordinator::new(
            (*endpoint).clone(),
            !config.disable_relays,
        ));
        let network_recovery_observations = Arc::new(NetworkRecoveryObservationSource::new());
        let mut router_builder = ProtocolRouterBuilder::new((*endpoint).clone());
        for alpn in session_protocol_alpns() {
            router_builder = router_builder.accept(alpn, session_protocols.dispatcher(alpn));
        }
        uc_debug!(
            disable_relays = config.disable_relays,
            allow_overlay_network_addrs = config.allow_overlay_network_addrs,
            relay_count = config.relay_urls.len(),
            rendezvous_override = config.rendezvous_base_url.is_some(),
            "iroh node bound; ready to install transport handlers"
        );
        log_publish_addrs(&endpoint, "post-bind");
        let discovery = discovery_slot
            .get()
            .cloned()
            .ok_or_else(|| IrohNodeError::Discovery {
                source: DiscoveryError::NotInitialized.into(),
            })?;
        let discovery_runtime = DiscoveryRuntime::start(discovery.clone(), mdns);
        let session_context = Arc::new(IrohSessionContext {
            discovery,
            endpoint,
            demand_recovery,
            network_recovery_observations,
            config,
        });
        Ok(Self {
            session_context,
            connection_observations,
            network_recorder: recorder,
            router_builder,
            session_protocols,
            run_lease,
            discovery_runtime,
        })
    }
}

impl IrohSessionBuilder {
    /// 构造邀请发布、地址查询与短码解析能力，不注册业务 ALPN。
    ///
    /// A single [`RendezvousClient`] is built here and shared between the
    /// invitation resolver (joiner `/resolve`) and invitation adapter
    /// (sponsor `/pairings` + `/consume`) so the
    /// whole process uses one reqwest connection pool, one timeout, and
    /// one user-agent.
    pub fn install_pairing_invitation(
        &mut self,
        device_identity: Arc<dyn DeviceIdentityPort>,
        settings: Arc<dyn SettingsPort>,
    ) -> PairingInvitationHandlers {
        let rendezvous = Arc::new(match &self.context.config.rendezvous_base_url {
            Some(url) => RendezvousClient::with_base_url(url.clone()),
            None => RendezvousClient::new(),
        });

        let resolver = Arc::new(PairingInvitationResolverAdapter::new(
            Arc::clone(&self.context.endpoint),
            Arc::clone(&rendezvous),
        ));

        let invitation_adapter = Arc::new(RendezvousPairingInvitationAdapter::new(
            Arc::clone(&self.context.endpoint),
            device_identity,
            settings,
            rendezvous,
        ));
        let invitation: Arc<dyn PairingInvitationPort> = invitation_adapter.clone();
        let invitation_addresses: Arc<dyn PairingInvitationAddressQueryPort> =
            invitation_adapter.clone();
        let invitation_by_address: Arc<dyn PairingInvitationByAddressPort> = invitation_adapter;

        PairingInvitationHandlers {
            joiner_invitation_resolver: resolver,
            invitation,
            invitation_addresses,
            invitation_by_address,
        }
    }

    /// 同一 Endpoint 的 presence 资源依赖；不构造准入与在线业务对象。
    pub fn presence_connectivity_deps(
        &self,
        addresses: Arc<dyn PeerAddressRepositoryPort>,
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        admission: Arc<dyn PeerAdmissionPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> (ConnectivityDeps, Arc<dyn PeerReachabilityEnvironmentPort>) {
        let environment = Arc::new(PresenceEnvironment::new(
            identities.clone(),
            fingerprint.clone(),
            self.admitted_connections.clone(),
            self.context.demand_recovery.clone(),
            self.context.network_recovery_observations.clone(),
        ));
        (
            ConnectivityDeps {
                backend: Arc::new(IrohConnectivityBackend::new(
                    self.context.endpoint.clone(),
                    addresses,
                    None,
                    ConnectivityProfile::Presence,
                )),
                admission: Arc::new(IrohConnectionAdmission::new(
                    identities,
                    admission,
                    fingerprint,
                    self.admitted_connections.clone(),
                    ConnectivityProfile::Presence,
                )),
            },
            environment,
        )
    }
    /// 两个协商版本一次转交同一完整 owner。
    pub fn install_peer_reachability(
        &mut self,
        owner: Arc<PeerReachabilityTransport>,
    ) -> Result<(), IrohNodeError> {
        self.install_session_handler(
            [PEER_REACHABILITY_ALPN, LEGACY_PEER_REACHABILITY_ALPN],
            IrohPeerReachabilityHandler::new(owner),
        )
    }

    /// Build a [`ConnectionChannelPort`] (Phase 96 INDIC-01).
    ///
    /// Pure read adapter — does **not** register an ALPN handler, only wires
    /// the shared endpoint + `peer_addr_repo` so callers can ask
    /// `channel_for(device_id)` to derive Direct/Relay/Offline/Unknown from
    /// the current `Endpoint::remote_info` snapshot.
    ///
    /// Safe to call before or after [`spawn`](Self::spawn) in principle, but
    /// for consistency with the other `install_*` methods (and to keep
    /// bootstrap wiring linear) we expose it as part of the builder phase.
    /// No router mutation, so coexists trivially with every other install_*.
    pub fn install_connection_channel(
        &self,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    ) -> Arc<dyn ConnectionChannelPort> {
        Arc::new(IrohConnectionChannelAdapter::new(
            Arc::clone(&self.context.endpoint),
            peer_addr_repo,
        ))
    }

    /// 提供同一 endpoint、会话登记和权威身份目录下的连接能力，供 Engine 组装负责人。
    pub fn connectivity_deps(
        &self,
        profile: ConnectivityProfile,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        peer_reachability: Arc<dyn PeerReachabilityPort>,
    ) -> ConnectivityDeps {
        ConnectivityDeps {
            backend: Arc::new(IrohConnectivityBackend::new(
                self.context.endpoint.clone(),
                peer_addr_repo,
                Some(peer_reachability),
                profile,
            )),
            admission: Arc::new(IrohConnectionAdmission::new(
                identities,
                peer_admission,
                fingerprint_factory,
                self.admitted_connections.clone(),
                profile,
            )),
        }
    }

    /// 安装普通剪贴板完整交换能力；两种 ALPN 共享同一会话负责人。
    pub fn install_clipboard(
        &mut self,
        manager: ConnectivityManager,
    ) -> Result<ClipboardHandlers, IrohNodeError> {
        let receiver = Arc::new(ClipboardReceiverAdapter::new(manager.clone()));
        self.install_session_handler(
            [
                ATTRIBUTED_CLIPBOARD_ALPN,
                MULTIPLEXED_CLIPBOARD_ALPN,
                CLIPBOARD_ALPN,
            ],
            IrohClipboardHandler::new(receiver.clone()),
        )?;
        Ok(ClipboardHandlers {
            dispatch: Arc::new(ClipboardDispatchAdapter::new(manager)),
            receiver,
        })
    }

    /// Install the encrypted group-update transport under [`GROUP_UPDATE_ALPN`].
    ///
    /// Must be called before [`spawn`](Self::spawn), while the shared router
    /// still accepts protocol handlers.
    pub fn install_group_updates(
        &mut self,
        manager: ConnectivityManager,
        receiver: Arc<dyn GroupUpdateReceiverPort>,
        legacy_only: bool,
    ) -> Result<(), IrohNodeError> {
        if legacy_only {
            // 固定 Router 为全部世代预登记路由；只少装 handler 不会使 TLS 拒绝 /2。
            // 互通验收必须同时限制握手公告，才能验证旧发送方的真实 /1 回退。
            let mut advertised: Vec<Vec<u8>> = session_protocol_alpns()
                .into_iter()
                .filter(|alpn| *alpn != GROUP_UPDATE_ALPN)
                .map(<[u8]>::to_vec)
                .collect();
            advertised.sort_by_key(|alpn| {
                if alpn.as_slice() == PEER_REACHABILITY_ALPN {
                    0
                } else if alpn.as_slice() == MULTIPLEXED_CLIPBOARD_ALPN {
                    1
                } else {
                    2
                }
            });
            self.context.endpoint.set_alpns(advertised);
        }
        let protocols = if legacy_only {
            vec![LEGACY_GROUP_UPDATE_ALPN]
        } else {
            vec![GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN]
        };
        self.install_session_handler(
            protocols,
            IrohGroupUpdateHandler::new(manager.clone(), receiver),
        )?;
        Ok(())
    }

    pub fn history_connectivity_deps(
        &self,
        addresses: Arc<dyn PeerAddressRepositoryPort>,
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        retention: Arc<dyn HistoryConnectionRetentionPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
        reachability: Arc<dyn PeerReachabilityPort>,
    ) -> HistoryConnectivityDeps {
        HistoryConnectivityDeps {
            backend: Arc::new(IrohConnectivityBackend::new(
                self.context.endpoint.clone(),
                addresses,
                Some(reachability),
                ConnectivityProfile::MembershipHistory,
            )),
            admission: Arc::new(HistoryConnectionAdmission::new(
                identities,
                retention,
                fingerprint,
                self.admitted_connections.clone(),
            )),
        }
    }
    pub fn install_membership_history_exchange(
        &mut self,
        manager: ConnectivityManager,
        receiver: Arc<dyn HistoryReceiverPort>,
    ) -> Result<(), IrohNodeError> {
        self.install_session_handler(
            [
                MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
                MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
            ],
            IrohMembershipHistoryExchangeHandler::new(manager, receiver),
        )
    }

    pub fn branch_recovery_connectivity_deps(
        &self,
        addresses: Arc<dyn PeerAddressRepositoryPort>,
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        membership: Arc<dyn BranchRecoveryPeerPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> ConnectivityDeps {
        ConnectivityDeps {
            backend: Arc::new(IrohConnectivityBackend::new(
                self.context.endpoint.clone(),
                addresses,
                None,
                ConnectivityProfile::MembershipBranchRecovery,
            )),
            admission: Arc::new(BranchRecoveryAdmission::new(
                identities,
                membership,
                fingerprint,
                self.admitted_connections.clone(),
            )),
        }
    }

    pub fn install_managed_branch_recovery(
        &mut self,
        manager: ConnectivityManager,
        receiver: Arc<BranchRecoveryReceiver>,
    ) -> Result<(), IrohNodeError> {
        self.install_session_handler(
            [MEMBERSHIP_BRANCH_RECOVERY_ALPN],
            IrohBranchRecoveryHandler::new(manager, receiver),
        )
    }

    pub fn build_membership_identity_adapter(
        &self,
        session: Arc<InMemorySession>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        settings: Arc<dyn SettingsPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Arc<IrohMembershipIdentityAdapter> {
        Arc::new(IrohMembershipIdentityAdapter::new(
            Arc::clone(&self.context.endpoint),
            session,
            device_identity,
            settings,
            fingerprint_factory,
        ))
    }

    pub fn build_verified_peer_address_refresh(
        &self,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Arc<IrohVerifiedPeerAddressRefresh> {
        Arc::new(IrohVerifiedPeerAddressRefresh::new(
            Arc::clone(&self.context.endpoint),
            peer_addr_repo,
            clock,
        ))
    }

    /// Install the active-clipboard state transport.
    ///
    /// * Registers [`IrohActiveStateHandler`] as the
    ///   [`ACTIVE_CLIPBOARD_ALPN`] `ProtocolHandler`, an independent sibling
    ///   of the bulk clipboard ALPN. Unknown peers are dropped at the
    ///   identity boundary via `remote_id()` + fingerprint resolution.
    /// * Builds the outbound dispatch adapter on the same endpoint, reusing
    ///   `peer_addr_repo` so a state send rides the same NAT/relay mapping
    ///   peer_reachability already established (mirrors `install_clipboard`).
    /// * Returns both ports as trait objects.
    ///
    /// Must be called before [`spawn`](Self::spawn). Coexists with every
    /// other `install_*` — all ALPNs share a single router.
    ///
    /// [`IrohActiveStateHandler`]: super::active_clipboard::IrohActiveStateHandler
    pub fn install_active_clipboard(
        &mut self,
        manager: ConnectivityManager,
    ) -> Result<ActiveClipboardHandlers, IrohNodeError> {
        let wire = Arc::new(ActiveClipboardWire);
        let receiver = Arc::new(ActiveStateReceiver::new(manager.clone(), wire.clone()));
        self.install_session_handler(
            [ACTIVE_CLIPBOARD_ALPN],
            IrohActiveStateHandler::new(receiver.clone()),
        )?;
        Ok(ActiveClipboardHandlers {
            dispatch: Arc::new(ActiveStateSender::new(manager, wire)),
            receiver,
        })
    }

    /// Install the active-clipboard pull transport (issue #1017 PR8).
    ///
    /// * Registers [`IrohActivePullHandler`] as the
    ///   [`ACTIVE_CLIPBOARD_PULL_ALPN`] `ProtocolHandler`, an independent
    ///   sibling of the active-clipboard state ALPN. Admission is
    ///   member-fingerprint only; the handler delegates content production to
    ///   `serve` (the application-layer serve port), which produces a fresh
    ///   transfer envelope and requires an unlocked session.
    /// * Builds the outbound pull client on the same endpoint, reusing
    ///   `peer_addr_repo` so a pull rides the same NAT/relay mapping peer_reachability
    ///   already established (mirrors `install_active_clipboard`).
    /// * Returns only the client port; the serve side has no outbound surface.
    ///
    /// Must be called before [`spawn`](Self::spawn). Coexists with every other
    /// `install_*` — all ALPNs share a single router.
    ///
    /// [`IrohActivePullHandler`]: super::active_clipboard::IrohActivePullHandler
    pub fn install_active_clipboard_pull(
        &mut self,
        manager: ConnectivityManager,
        prepare: Arc<dyn ActivePullPreparationPort>,
        content_gate: Arc<dyn CurrentSpaceMemberScopePort>,
    ) -> Result<ActiveClipboardPullHandlers, IrohNodeError> {
        let wire = Arc::new(ActiveClipboardWire);
        let receiver = Arc::new(ActivePullReceiver::new(
            manager.clone(),
            wire.clone(),
            prepare,
            content_gate,
            Arc::new(SessionActivePullLocalWork),
        ));
        self.install_session_handler(
            [ACTIVE_CLIPBOARD_PULL_ALPN],
            IrohActivePullHandler::new(receiver),
        )?;
        Ok(ActiveClipboardPullHandlers {
            client: Arc::new(ActivePullClient::new(manager, wire)),
        })
    }

    /// 将同一Application完整端点注册到两种能力；世代只关闭该handler一次。
    pub fn install_transfer_progress(
        &mut self,
        owner: Arc<TransferProgressTransport>,
    ) -> Result<(), IrohNodeError> {
        let handler = IrohTransferProgressHandler::new(owner);
        self.install_session_handler(
            [TRANSFER_PROGRESS_ALPN, LEGACY_TRANSFER_PROGRESS_ALPN],
            handler,
        )?;
        Ok(())
    }

    /// 安装 iroh-blobs 传输能力。
    ///
    /// 在同一存储上组装 SDK 能力和 Application 完整负责人，Router 只注册受管 handler。
    /// sqlite 引用仓储仍由数据库装配链提供。
    pub async fn install_blobs(
        &mut self,
        store_dir: PathBuf,
        assemble: impl FnOnce(
            Arc<dyn BlobExchangePort>,
            Arc<dyn BlobStoreLifecyclePort>,
        ) -> Arc<BlobTransport>,
    ) -> Result<BlobHandlers, IrohNodeError> {
        // 通过公开加载入口配置 SDK 内部 GC；GC 与存储工作由 Store 自己监督。
        // 完整关闭须等待 GC、已接受的 import/export、actor、元数据及私有 runtime，
        // 不能把 Router 关闭或句柄释放当作 Store 已完成关闭。
        //
        // `load_with_opts` 的第一个参数是 redb 数据库**文件**路径,不是 root
        // 目录。`Options::new(root)` 才接 root,内部派生出 `root/data`、
        // `root/temp`。直接传 `store_dir` 当 db_path 会让 redb 去把已经被
        // `Options::new` 当作 root 的目录当文件打开,actor 启动会卡死(macOS
        // 上 redb 在目录上的文件锁不会立即返回错误)。和 `FsStore::load`
        // 内部一样用 `root/blobs.db` 作为 db 文件路径。
        let mut options = iroh_blobs::store::fs::options::Options::new(&store_dir);
        options.gc = Some(iroh_blobs::store::GcConfig {
            interval: crate::network::iroh::blobs::BLOBS_GC_INTERVAL,
            add_protected: None,
        });
        let db_path = store_dir.join("blobs.db");
        let store = without_request_span(|| {
            iroh_blobs::store::fs::FsStore::load_with_opts(db_path, options)
        })
        .await
        .map_err(|err| IrohNodeError::BlobStoreInit(anyhow::Error::from(err)))?;

        // Phase E1 (transitional): sweep `auto-*` tags left behind by
        // pre-Phase-F daemons.
        //
        // iroh-blobs `AddProgress::with_tag` (the `IntoFuture` default for
        // `add_bytes` / `add_path_with_opts`) used to mint a persistent
        // `auto-<timestamp>` tag protecting every newly-published blob
        // from GC. Phase F (`IrohBlobTransferAdapter::publish*`) routes
        // through `with_named_tag` instead, so freshly written blobs no
        // longer carry an auto-tag. But upgrades from older builds inherit
        // the leftover auto-tags, and those still pin every old blob
        // against the Phase D1 GC even after the user deletes the owning
        // entry — sweeping them once at startup is what lets cache reclaim
        // recover for upgraded users.
        //
        // Sweeping is safe because no `add_*` request can land between
        // `FsStore::load_with_opts` returning and us re-acquiring the
        // router (single-threaded init). Phase F also means future
        // publishes never re-create an auto-tag, so post-upgrade this
        // sweep becomes a near-no-op (delete-prefix on an empty range);
        // we keep it for one or two release cycles to cover stragglers
        // and then can drop the call entirely.
        match without_request_span(|| store.tags().delete_prefix(b"auto-")).await {
            Ok(removed) => {
                if removed > 0 {
                    uc_info!(removed = removed, "iroh blobs: swept stale auto-* tags");
                }
            }
            Err(err) => {
                uc_warn!(
                    error_kind = "stale_tag_sweep",
                    io_error_kind = io_error_kind(&err),
                    "iroh blobs: failed to sweep stale auto-* tags (non-fatal)"
                );
            }
        }

        let exchange = Arc::new(IrohBlobExchangeAdapter::new(store.clone()));
        let owner = assemble(exchange.clone(), exchange.clone());
        self.install_session_handler([BLOBS_ALPN], IrohBlobHandler::new(owner.clone()))?;

        let adapter = Arc::new(IrohBlobTransferAdapter::new(
            Arc::clone(&self.context.endpoint),
            store,
        ));

        uc_info!(
            alpn = log_vocab(&String::from_utf8_lossy(BLOBS_ALPN)),
            gc_interval_secs = crate::network::iroh::blobs::BLOBS_GC_INTERVAL.as_secs(),
            "iroh blobs acceptor installed"
        );

        Ok(BlobHandlers {
            transport: owner,
            read_store: exchange,
            blob_transfer: adapter,
        })
    }
}

impl IrohNodeBuilder {
    /// 启动固定 [`Router`]。此时尚无活动 Space 会话。
    pub fn spawn(self) -> IrohNode {
        // Router 只捕获空 span；后续协议处理仍使用进程 subscriber 和已认证远端父关系。
        let router = tracing::dispatcher::with_default(
            &tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()),
            || self.router_builder.spawn(),
        );
        log_publish_addrs(&self.session_context.endpoint, "post-spawn");

        // Relay self-healing watchdog (see `net_recovery`). Only meaningful
        // when relays are enabled: in LAN-only mode there is no home relay by
        // design, so an empty relay-status set is expected, not a wedge. The
        // handle is retained (not detached) so `shutdown` can stop it
        // deterministically and keep a panic visible.
        let net_recovery = (!self.session_context.config.disable_relays).then(|| {
            super::net_recovery::spawn_net_recovery(
                (*self.session_context.endpoint).clone(),
                Arc::clone(&self.session_context.network_recovery_observations),
                self.network_recorder.clone(),
            )
        });

        let connection_census = Some(self.connection_observations.census().spawn());

        IrohNode {
            session_context: self.session_context,
            router,
            session_protocols: self.session_protocols,
            session_generation: None,
            connection_observations: self.connection_observations,
            net_recovery,
            connection_census,
            discovery_runtime: self.discovery_runtime,
            _run_lease: self.run_lease,
        }
    }
}

/// Bootstrap-time failures binding the iroh endpoint. Kept small on
/// purpose — deeper iroh errors are summarised into a string rather than
/// threaded as typed variants per `docs/design-docs/layers/infrastructure.md` §9.1 (infra error
/// types don't leak third-party error types upward).
#[derive(Debug, thiserror::Error)]
pub enum IrohNodeError {
    #[error("failed to initialize peer discovery")]
    Discovery {
        #[source]
        source: anyhow::Error,
    },

    #[error("an iroh node is already running in this process")]
    AlreadyRunning,

    #[error("iroh node runtime state lock is poisoned")]
    RuntimeStatePoisoned,

    #[error("failed to bind iroh endpoint")]
    Bind(#[source] anyhow::Error),

    #[error("failed to initialize iroh blob store")]
    BlobStoreInit(#[source] anyhow::Error),

    #[error("failed to configure iroh session protocols")]
    SessionProtocol {
        #[source]
        source: anyhow::Error,
    },

    #[error("failed to install the Space admission transport")]
    AdmissionInstall {
        #[source]
        source: anyhow::Error,
    },

    /// 不保存原始配置值：URL 是地址，还可能带有凭据。
    #[error("invalid custom iroh relay URL")]
    InvalidRelayUrl(#[source] RelayUrlProblem),

    #[error("invalid iroh relay access token")]
    InvalidRelayAccessToken,

    #[error(transparent)]
    Identity(#[from] LocalIdentityError),
}

/// 自定义 relay URL 不合法的固定分类。
#[derive(Debug, thiserror::Error)]
pub enum RelayUrlProblem {
    #[error("relay URL must not be empty")]
    Empty,
    #[error("no relay URL is configured while relays are enabled")]
    NoneConfigured,
    #[error("relay URL could not be parsed")]
    Parse(#[source] <RelayUrl as FromStr>::Err),
    #[error("relay URL scheme must be http or https")]
    UnsupportedScheme,
    #[error("relay URL must include a host")]
    MissingHost,
    #[error("relay URL must not include credentials")]
    Credentials,
}

impl IrohNodeError {
    fn session_protocol(source: SessionProtocolRegistryError) -> Self {
        Self::SessionProtocol {
            source: anyhow::Error::new(source),
        }
    }
}

#[cfg(test)]
#[path = "node/startup_trace_tests.rs"]
mod startup_trace_tests;

#[cfg(test)]
mod tests {
    use super::super::addr_filter::is_virtual_nic_ip;
    use super::*;

    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use uc_core::ids::{DeviceId, EntryId};
    use uc_core::ports::{SecureStorageError, SecureStoragePort};
    use uc_core::settings::model::Settings;

    use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

    #[derive(Default)]
    struct InMemorySecureStorage {
        map: StdMutex<HashMap<String, Vec<u8>>>,
    }
    impl SecureStoragePort for InMemorySecureStorage {
        fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
            Ok(self.map.lock().unwrap().get(key).cloned())
        }
        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
            self.map
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_vec());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
            self.map.lock().unwrap().remove(key);
            Ok(())
        }
    }

    struct FixedDeviceIdentity(DeviceId);
    impl DeviceIdentityPort for FixedDeviceIdentity {
        fn current_device_id(&self) -> DeviceId {
            self.0.clone()
        }
    }

    struct InMemorySettings(StdMutex<Settings>);
    #[async_trait]
    impl SettingsPort for InMemorySettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.0.lock().unwrap().clone())
        }
        async fn save(&self, s: &Settings) -> anyhow::Result<()> {
            *self.0.lock().unwrap() = s.clone();
            Ok(())
        }
    }

    pub(super) fn identity_store() -> Arc<IrohIdentityStore> {
        Arc::new(IrohIdentityStore::new(
            Arc::new(InMemorySecureStorage::default()),
            Arc::new(Sha256IdentityFingerprintFactory),
        ))
    }

    async fn spawn_session(network: IrohNodeBuilder, builder: IrohSessionBuilder) -> IrohNode {
        let mut node = network.spawn();
        node.activate_session(builder.finish())
            .await
            .expect("activate session");
        node
    }

    fn install_presence_fixture(
        builder: &mut IrohSessionBuilder,
        addresses: Arc<dyn PeerAddressRepositoryPort>,
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        authority: Arc<dyn PeerAdmissionPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
        clock: Arc<dyn ClockPort>,
        contacts: tokio::sync::broadcast::Sender<uc_application::deps::KnownPeerContact>,
    ) -> Result<Arc<dyn PeerReachabilityPort>, IrohNodeError> {
        let address_refresh =
            builder.build_verified_peer_address_refresh(addresses.clone(), clock.clone());
        let (deps, environment) = builder.presence_connectivity_deps(
            addresses,
            identities,
            authority.clone(),
            fingerprint,
        );
        let manager = uc_application::deps::ConnectivityManager::new(
            deps.backend,
            deps.admission,
            uc_application::deps::ConnectivityPolicy {
                presence_retention: true,
                inbound_per_connection: 2,
                flights_per_session: 4,
                ..uc_application::deps::ConnectivityPolicy::default()
            },
        );
        let owner = PeerReachabilityTransport::new(
            manager,
            authority,
            uc_application::deps::PeerReachabilityTransportPorts {
                wire: Arc::new(super::super::connectivity::PeerReachabilityWire),
                environment,
                address_refresh,
            },
            clock,
            contacts,
        );
        builder.install_peer_reachability(owner.clone())?;
        Ok(owner)
    }

    /// 向系统要一个当前空闲的 UDP 端口后立即释放，让 iroh 绑定同一端口号。
    /// 存在很小的竞争窗口，测试可以接受。
    fn free_udp_port() -> u16 {
        let probe = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).expect("probe socket");
        probe.local_addr().expect("probe addr").port()
    }

    /// UniClipboard#900: `bind_port` 把 IPv4 与（可用时的）IPv6 都固定到同一端口。
    #[tokio::test]
    async fn bind_pins_fixed_udp_port() {
        let port = free_udp_port();

        let store = identity_store();
        let cfg = IrohNodeConfig {
            bind_port: Some(port),
            ..Default::default()
        };
        let builder = IrohNodeBuilder::bind(&store, cfg).await.expect("bind");
        let bound = builder.session_context.endpoint.bound_sockets();
        assert!(
            bound.iter().any(|s| s.is_ipv4() && s.port() == port),
            "expected pinned IPv4 port {port} in bound sockets {bound:?}"
        );
        assert!(
            bound
                .iter()
                .filter(|s| s.is_ipv6())
                .all(|s| s.port() == port),
            "an IPv6 socket, when present, must use the same fixed port {port}: {bound:?}"
        );
    }

    /// relay 关闭、没有任何地址发现（相当于没有组播的 WireGuard）：对端以同一身份和
    /// 固定端口重启后，仅凭此前保存的直连线索仍能重新连上。保存的线索来自开启 relay
    /// 时的公告，因此同时带有 relay；剥掉 relay 后直连线索必须仍在。
    #[tokio::test]
    async fn stored_direct_hint_reaches_a_peer_restarted_on_its_fixed_port_without_discovery() {
        let port = free_udp_port();
        let store = identity_store();
        let config = || IrohNodeConfig {
            disable_relays: true,
            bind_port: Some(port),
            ..Default::default()
        };
        let wait_for_direct = |endpoint: Arc<Endpoint>| async move {
            for _ in 0..200 {
                if endpoint.addr().ip_addrs().next().is_some() {
                    return endpoint.addr();
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("endpoint never advertised a direct address");
        };

        let first = IrohNodeBuilder::bind(&store, config())
            .await
            .expect("first bind");
        let advertised = wait_for_direct(Arc::clone(&first.session_context.endpoint)).await;
        let relay: iroh::RelayUrl = "https://relay.example.invalid/".parse().unwrap();
        let stored =
            super::super::persistable_addr::to_persistable_addr(iroh::EndpointAddr::from_parts(
                advertised.id,
                advertised
                    .addrs
                    .iter()
                    .cloned()
                    .chain([iroh::TransportAddr::Relay(relay)]),
            ));
        assert!(stored.ip_addrs().any(|addr| addr.port() == port));
        let first_session = first.prepare_session();
        spawn_session(first, first_session)
            .await
            .shutdown()
            .await
            .unwrap();

        let second = IrohNodeBuilder::bind(&store, config())
            .await
            .expect("restart on the same fixed port");
        let restarted = Arc::clone(&second.session_context.endpoint);
        assert_eq!(restarted.id(), stored.id);
        // 只验证传输可达；协议路由器尚未启动，这里直接接受一个测试连接。
        restarted.set_alpns(vec![PEER_REACHABILITY_ALPN.to_vec()]);
        let acceptor = tokio::spawn(async move {
            let incoming = restarted.accept().await?;
            let connection = incoming.await.ok()?;
            connection.closed().await;
            Some(())
        });
        let dialer = Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .relay_mode(iroh::RelayMode::Disabled)
                .clear_address_lookup()
                .bind()
                .await
                .expect("bind dialer"),
        );
        let lan_only_hint = iroh::EndpointAddr::from_parts(
            stored.id,
            stored.addrs.iter().filter(|addr| addr.is_ip()).cloned(),
        );

        let connection = tokio::time::timeout(
            Duration::from_secs(15),
            super::super::connect::connect_with_staggered_retry(
                dialer,
                lan_only_hint,
                PEER_REACHABILITY_ALPN,
                "presence",
                uc_observability_contract::diagnostics::connectivity::AddressInputSource::Stored,
            ),
        )
        .await
        .expect("dial finishes")
        .expect("the stored direct hint reaches the restarted peer");
        drop(connection);
        acceptor.abort();
        drop(second);
    }

    /// UniClipboard#900: `public_addr` injects a configured public address
    /// into the advertised endpoint address at bind time, so it is present in
    /// the very first `endpoint.addr()` snapshot — before any pairing exchange.
    #[tokio::test]
    async fn bind_advertises_configured_public_addr() {
        // A public, non-virtual IPv4 — never dropped by the address filter.
        let public_addr = SocketAddr::V4(std::net::SocketAddrV4::new(
            Ipv4Addr::new(1, 2, 3, 4),
            12345,
        ));
        let store = identity_store();
        let cfg = IrohNodeConfig {
            public_addr: Some(public_addr),
            ..Default::default()
        };
        let builder = IrohNodeBuilder::bind(&store, cfg).await.expect("bind");
        let addr = builder.session_context.endpoint.addr();
        assert!(
            addr.ip_addrs().any(|a| *a == public_addr),
            "configured public addr {public_addr} not advertised in {addr:?}"
        );
    }

    #[tokio::test]
    async fn bind_install_invitation_discovery_spawn_and_shutdown_cleanly() {
        let store = identity_store();
        let network = IrohNodeBuilder::bind(&store, IrohNodeConfig::default())
            .await
            .expect("bind");
        let mut builder = network.prepare_session();
        let handlers = builder.install_pairing_invitation(
            Arc::new(FixedDeviceIdentity(DeviceId::new("device-1"))),
            Arc::new(InMemorySettings(StdMutex::new(Settings::default()))),
        );
        // Ports are handed out as trait objects so ownership (and hence
        // the session adapter) survives past the node's spawn.
        drop(handlers);
        let node = spawn_session(network, builder).await;
        assert!(!node.accepts_protocol_for_test(PEER_REACHABILITY_ALPN).await);
        // Clean shutdown exits without hanging; the test runner's default
        // timeout would catch a deadlock.
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn native_network_diagnostics_survive_driver_isolation_without_retaining_connections() {
        use opentelemetry::logs::AnyValue;
        use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
        use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
        use tracing_subscriber::layer::SubscriberExt;
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let dispatch = tracing::Dispatch::new(
            tracing_subscriber::registry().with(OpenTelemetryTracingBridge::new(&provider)),
        );
        let store = identity_store();
        let builder = IrohNodeBuilder::bind(
            &store,
            IrohNodeConfig {
                disable_relays: true,
                ..Default::default()
            },
        )
        .with_subscriber(dispatch.clone())
        .await
        .expect("node");
        builder
            .session_context
            .endpoint
            .set_alpns(vec![b"diagnostic-probe".to_vec()]);
        for _ in 0..100 {
            if !builder.session_context.endpoint.addr().addrs.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!builder.session_context.endpoint.addr().addrs.is_empty());
        let client = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .clear_address_lookup()
            .bind()
            .await
            .expect("client");
        let server_endpoint = Arc::clone(&builder.session_context.endpoint);
        let incoming = tokio::spawn(async move {
            server_endpoint
                .accept()
                .await
                .expect("incoming")
                .await
                .expect("handshake")
        });
        let connected = client
            .connect(builder.session_context.endpoint.addr(), b"diagnostic-probe")
            .await
            .expect("connected");
        drop(incoming.await.expect("accept task"));
        tokio::time::timeout(Duration::from_secs(3), connected.closed())
            .await
            .expect("诊断观察不能延长服务器连接寿命");
        client.close().await;
        let session = builder.prepare_session();
        spawn_session(builder, session)
            .await
            .shutdown()
            .with_subscriber(dispatch)
            .await
            .unwrap();
        let records = exporter.get_emitted_logs().expect("logs");
        let names: Vec<_> = records
            .iter()
            .filter_map(|record| {
                record
                    .record
                    .attributes_iter()
                    .find_map(|(key, value)| match value {
                        AnyValue::String(value) if key.as_str() == "event.name" => {
                            Some(value.to_string())
                        }
                        _ => None,
                    })
            })
            .collect();
        assert!(
            names.iter().any(|name| name == "address.publish_requested"),
            "mDNS 发布请求没有进入安全采集入口"
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "connection.established")
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| name.as_str() == "connection.closed")
                .count(),
            1
        );
        assert!(!format!("{records:?}").contains(&client.id().to_string()));
    }

    #[tokio::test]
    async fn bind_is_idempotent_across_builds_for_same_store() {
        // The endpoint id is derived from the Ed25519 secret, so a second
        // bind against the same store must see the same id (rotating it
        // would break every peer that already remembered us).
        let store = identity_store();
        let first = IrohNodeBuilder::bind(&store, IrohNodeConfig::default())
            .await
            .expect("first bind");
        let first_id = first.session_context.endpoint.id();
        let first_session = first.prepare_session();
        let first_node = spawn_session(first, first_session).await;
        first_node.shutdown().await.unwrap();

        let second = IrohNodeBuilder::bind(&store, IrohNodeConfig::default())
            .await
            .expect("second bind");
        assert_eq!(second.session_context.endpoint.id(), first_id);
        let second_session = second.prepare_session();
        spawn_session(second, second_session)
            .await
            .shutdown()
            .await
            .unwrap();
    }

    #[derive(Default)]
    struct EmptyPeerAddressRepo;
    #[async_trait]
    impl PeerAddressRepositoryPort for EmptyPeerAddressRepo {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            Ok(None)
        }
        async fn upsert(
            &self,
            _record: &uc_core::ports::PeerAddressRecord,
        ) -> Result<(), uc_core::ports::PeerAddressError> {
            Ok(())
        }
        async fn list(
            &self,
        ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            Ok(Vec::new())
        }
        async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
            Ok(())
        }
    }

    struct FixedClock(i64);
    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    #[tokio::test]
    async fn invitation_discovery_and_peer_reachability_coexist_on_same_node() {
        let store = identity_store();
        let network = IrohNodeBuilder::bind(&store, IrohNodeConfig::default())
            .await
            .expect("bind");
        let mut builder = network.prepare_session();

        let _invitation = builder.install_pairing_invitation(
            Arc::new(FixedDeviceIdentity(DeviceId::new("device-coexist"))),
            Arc::new(InMemorySettings(StdMutex::new(Settings::default()))),
        );

        let peer_reachability: Arc<dyn PeerReachabilityPort> = install_presence_fixture(
            &mut builder,
            Arc::new(EmptyPeerAddressRepo),
            Arc::new(EmptyMemberRepo),
            Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
            Arc::new(uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock(1_700_000_000_000)),
            tokio::sync::broadcast::channel(1).0,
        )
        .expect("install peer reachability");

        // current_state before any dial is Unknown — proves the adapter
        // survived the type-erasure round trip through install_peer_reachability.
        let unknown_state = peer_reachability
            .current_state(&DeviceId::new("never-dialed"))
            .await;
        assert_eq!(unknown_state, uc_core::ports::ReachabilityState::Unknown,);

        let node = spawn_session(network, builder).await;
        assert!(node.accepts_protocol_for_test(PEER_REACHABILITY_ALPN).await);
        assert!(!node.accepts_protocol_for_test(LEGACY_CLIPBOARD_ALPN).await);
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn replaces_space_session_without_rebinding_endpoint() {
        let store = identity_store();
        let network = IrohNodeBuilder::bind(&store, IrohNodeConfig::default())
            .await
            .expect("bind");
        let mut first = network.prepare_session();
        install_presence_fixture(
            &mut first,
            Arc::new(EmptyPeerAddressRepo),
            Arc::new(EmptyMemberRepo),
            Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
            Arc::new(uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock(1_700_000_000_000)),
            tokio::sync::broadcast::channel(1).0,
        )
        .expect("install first session");
        let mut node = spawn_session(network, first).await;
        let endpoint_id = node.session_context.endpoint.id();
        assert!(node.has_active_protocol_for_test(PEER_REACHABILITY_ALPN));
        assert!(!node.has_active_protocol_for_test(b"unknown/test"));
        assert!(node.accepts_protocol_for_test(PEER_REACHABILITY_ALPN).await);

        let mut second = node.prepare_session();
        install_presence_fixture(
            &mut second,
            Arc::new(EmptyPeerAddressRepo),
            Arc::new(EmptyMemberRepo),
            Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
            Arc::new(uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock(1_700_000_000_001)),
            tokio::sync::broadcast::channel(1).0,
        )
        .expect("install second session");

        node.quiesce_session().await.expect("quiesce first session");
        assert!(!node.has_active_protocol_for_test(PEER_REACHABILITY_ALPN));
        assert!(!node.accepts_protocol_for_test(PEER_REACHABILITY_ALPN).await);
        node.activate_session(second.finish())
            .await
            .expect("activate second session");

        assert_eq!(node.session_context.endpoint.id(), endpoint_id);
        assert!(node.has_active_protocol_for_test(PEER_REACHABILITY_ALPN));
        assert!(node.accepts_protocol_for_test(PEER_REACHABILITY_ALPN).await);
        node.shutdown().await.unwrap();
    }

    #[derive(Default)]
    struct EmptyMemberRepo;
    crate::network::iroh::inbound_peer::member_table_identity_directory!(EmptyMemberRepo);
    #[async_trait]
    impl uc_core::membership::MemberRepositoryPort for EmptyMemberRepo {
        async fn get(
            &self,
            _device_id: &DeviceId,
        ) -> Result<Option<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
        {
            Ok(None)
        }
        async fn list(
            &self,
        ) -> Result<Vec<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
        {
            Ok(Vec::new())
        }
        async fn save(
            &self,
            _member: &uc_core::membership::SpaceMember,
        ) -> Result<(), uc_core::membership::MembershipError> {
            Ok(())
        }
        async fn remove(
            &self,
            _device_id: &DeviceId,
        ) -> Result<bool, uc_core::membership::MembershipError> {
            Ok(false)
        }
    }

    #[tokio::test]
    async fn invitation_peer_reachability_and_clipboard_coexist_on_same_router() {
        // Presence 与 clipboard 共享 Router，invitation discovery 只共享 endpoint。
        // survive the trait-object round trip and the router spawns /
        // shuts down cleanly when all three transports are installed.
        let store = identity_store();
        let network = IrohNodeBuilder::bind(&store, IrohNodeConfig::default())
            .await
            .expect("bind");
        let mut builder = network.prepare_session();

        let _invitation = builder.install_pairing_invitation(
            Arc::new(FixedDeviceIdentity(DeviceId::new("device-triple"))),
            Arc::new(InMemorySettings(StdMutex::new(Settings::default()))),
        );

        let peer_addr_repo: Arc<dyn PeerAddressRepositoryPort> = Arc::new(EmptyPeerAddressRepo);
        let peer_reachability = install_presence_fixture(
            &mut builder,
            Arc::clone(&peer_addr_repo),
            Arc::new(EmptyMemberRepo),
            Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
            Arc::new(uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock(1_700_000_000_000)),
            tokio::sync::broadcast::channel(1).0,
        )
        .expect("install peer reachability");

        let ClipboardHandlers { dispatch, receiver } = builder
            .install_clipboard({
                let deps = builder.connectivity_deps(
                    ConnectivityProfile::Clipboard,
                    peer_addr_repo,
                    Arc::new(EmptyMemberRepo),
                    Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
                    Arc::new(
                        uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory,
                    ),
                    peer_reachability,
                );
                ConnectivityManager::new(
                    deps.backend,
                    deps.admission,
                    ConnectivityPolicy::default(),
                )
            })
            .expect("install clipboard");

        // Dispatch against an unknown device — the repo is empty so we
        // hit the `Offline` short-circuit without touching the wire. This
        // verifies the trait object round-tripped and is usable.
        let offline_err = dispatch
            .dispatch(
                &DeviceId::new("never-paired"),
                &uc_core::ports::ClipboardHeader {
                    source_entry: None,
                    version: uc_core::ports::ClipboardHeader::CURRENT_VERSION,
                    snapshot_hash: "0".repeat(64),
                    captured_at_ms: 0,
                    origin_device_id: "self".to_string(),
                    origin_device_name: "Self".to_string(),
                    payload_version: 3,
                },
                uc_core::ports::SyncPayload {
                    ciphertext: bytes::Bytes::from_static(b"x"),
                },
            )
            .await
            .outcome
            .expect_err("no peer_addr → Offline");
        assert!(
            matches!(offline_err, uc_core::ports::ClipboardDispatchError::Offline),
            "expected Offline, got {offline_err:?}"
        );

        // Receiver's subscribe handle is ready for the ingest use case.
        let _inbound_rx = receiver.subscribe();

        let node = spawn_session(network, builder).await;
        node.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn invitation_peer_reachability_clipboard_and_blobs_coexist_on_same_router() {
        let store = identity_store();
        let network = IrohNodeBuilder::bind(&store, IrohNodeConfig::default())
            .await
            .expect("bind");
        let mut builder = network.prepare_session();

        let _invitation = builder.install_pairing_invitation(
            Arc::new(FixedDeviceIdentity(DeviceId::new("device-quad"))),
            Arc::new(InMemorySettings(StdMutex::new(Settings::default()))),
        );

        let peer_addr_repo: Arc<dyn PeerAddressRepositoryPort> = Arc::new(EmptyPeerAddressRepo);
        let peer_reachability = install_presence_fixture(
            &mut builder,
            Arc::clone(&peer_addr_repo),
            Arc::new(EmptyMemberRepo),
            Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
            Arc::new(uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock(1_700_000_000_000)),
            tokio::sync::broadcast::channel(1).0,
        )
        .expect("install peer reachability");

        let _clipboard = builder
            .install_clipboard({
                let deps = builder.connectivity_deps(
                    ConnectivityProfile::Clipboard,
                    peer_addr_repo,
                    Arc::new(EmptyMemberRepo),
                    Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
                    Arc::new(
                        uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory,
                    ),
                    peer_reachability.clone(),
                );
                ConnectivityManager::new(
                    deps.backend,
                    deps.admission,
                    ConnectivityPolicy::default(),
                )
            })
            .expect("install clipboard");

        let tempdir = tempfile::tempdir().expect("tempdir");
        let BlobHandlers { blob_transfer, .. } = builder
            .install_blobs(tempdir.path().join("iroh-blobs"), {
                let deps = builder.connectivity_deps(
                    ConnectivityProfile::Blob,
                    Arc::new(EmptyPeerAddressRepo),
                    Arc::new(EmptyMemberRepo),
                    Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
                    Arc::new(
                        uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory,
                    ),
                    peer_reachability.clone(),
                );
                move |exchange, lifecycle| {
                    BlobTransport::new(deps.backend, deps.admission, exchange, lifecycle)
                }
            })
            .await
            .expect("install blobs");

        let digest = blob_transfer
            .publish(
                bytes::Bytes::from_static(b"router-four-alpns"),
                uc_core::ports::blob::TagReason::ClipboardEntry(uc_core::ids::EntryId::from_str(
                    "router-blob-test",
                )),
            )
            .await
            .expect("publish through blob port");
        assert!(blob_transfer.has(&digest).await.expect("has digest"));

        let node = spawn_session(network, builder).await;
        node.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn quiesce_waits_for_managed_blob_work_after_router_waiter_is_cancelled() {
        use tokio::sync::Semaphore;
        use uc_application::deps::{
            BlobExchangeError, BlobExchangeFailure, BlobReceived, BlobValidatedProgress,
            KnownPeerIdentity, MembershipLedgerError, StreamLease,
        };
        use uc_core::ports::blob::BlobDigest;
        struct Directory(KnownPeerIdentity);
        #[async_trait]
        impl PeerIdentityDirectoryPort for Directory {
            async fn known_peer_identities(
                &self,
            ) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
                Ok(vec![self.0.clone()])
            }
        }
        struct SlowExport {
            entered: Semaphore,
            cancelled: Semaphore,
            finish: Semaphore,
        }
        #[async_trait]
        impl BlobExchangePort for SlowExport {
            async fn receive(
                &self,
                _: StreamLease,
                _: BlobDigest,
                _: tokio_util::sync::CancellationToken,
                _: Arc<dyn BlobValidatedProgress>,
            ) -> Result<BlobReceived, BlobExchangeError> {
                panic!("unexpected receive");
            }
            async fn provide(
                &self,
                stream: StreamLease,
                _: tokio::time::Instant,
                cancel: tokio_util::sync::CancellationToken,
            ) -> Result<(), BlobExchangeError> {
                self.entered.add_permits(1);
                cancel.cancelled().await;
                self.cancelled.add_permits(1);
                self.finish
                    .acquire()
                    .await
                    .expect("actual export finish")
                    .forget();
                drop(stream);
                Err(BlobExchangeError::refused(BlobExchangeFailure::Cancelled))
            }
        }
        let network = IrohNodeBuilder::bind(
            &identity_store(),
            IrohNodeConfig {
                disable_relays: true,
                ..IrohNodeConfig::default()
            },
        )
        .await
        .expect("bind");
        let mut builder = network.prepare_session();
        let client = Endpoint::builder(presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
            .expect("client");
        let fingerprint =
            Arc::new(uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory);
        let directory = Arc::new(Directory(KnownPeerIdentity {
            device_id: DeviceId::new("blob-requester"),
            identity_fingerprint: fingerprint
                .from_public_key(client.id().as_bytes())
                .expect("fingerprint"),
        }));
        let backend = Arc::new(IrohConnectivityBackend::new(
            builder.context.endpoint.clone(),
            Arc::new(EmptyPeerAddressRepo),
            None,
            ConnectivityProfile::Blob,
        ));
        let admission = Arc::new(IrohConnectionAdmission::new(
            directory,
            Arc::new(crate::network::iroh::StaticPeerAdmission(true)),
            fingerprint,
            builder.admitted_connections.clone(),
            ConnectivityProfile::Blob,
        ));
        let slow = Arc::new(SlowExport {
            entered: Semaphore::new(0),
            cancelled: Semaphore::new(0),
            finish: Semaphore::new(0),
        });
        let provided = slow.clone();
        let directory = tempfile::tempdir().expect("store directory");
        builder
            .install_blobs(directory.path().join("blobs"), move |_, lifecycle| {
                BlobTransport::new(backend, admission, provided, lifecycle)
            })
            .await
            .expect("managed blobs");
        let mut node = spawn_session(network, builder).await;
        let connection = client
            .connect(node.session_context.endpoint.addr(), BLOBS_ALPN)
            .await
            .expect("connection");
        let (mut send, _receive) = connection.open_bi().await.expect("stream");
        // 本例只验证会话与实际工作寿命，SDK 请求／响应由 managed_tests 的真实下载器覆盖。
        send.write_all(&[0]).await.expect("first stream data");
        tokio::time::timeout(Duration::from_secs(2), slow.entered.acquire())
            .await
            .expect("provider started")
            .expect("started signal")
            .forget();
        {
            let stopping = node.quiesce_session();
            tokio::pin!(stopping);
            tokio::select! {
                result = &mut stopping => panic!("quiesce abandoned actual work: {result:?}"),
                signal = slow.cancelled.acquire() => signal.expect("cancel signal").forget(),
            }
            assert!(
                tokio::time::timeout(Duration::from_millis(50), &mut stopping)
                    .await
                    .is_err()
            );
            slow.finish.add_permits(1);
            tokio::time::timeout(Duration::from_secs(2), stopping)
                .await
                .expect("quiesce completed")
                .expect("quiesce");
        }
        client.close().await;
        node.shutdown().await.expect("shutdown");
    }

    // ──────────────────────────────────────────────────────────────────
    // is_virtual_nic_ip / build_addr_filter unit tests
    // ──────────────────────────────────────────────────────────────────

    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

    fn v4(ip: &str) -> IpAddr {
        IpAddr::V4(ip.parse::<Ipv4Addr>().unwrap())
    }
    fn v6(ip: &str) -> IpAddr {
        IpAddr::V6(ip.parse::<Ipv6Addr>().unwrap())
    }

    /// Always-filtered classes are filtered regardless of the overlay flag.
    #[test]
    fn always_filtered_classes_ignore_overlay_flag() {
        for &allow in &[false, true] {
            // 198.18.0.0/15 — Clash fake-ip
            assert!(
                is_virtual_nic_ip(v4("198.18.0.1"), allow),
                "198.18.0.1 must be filtered (allow_overlay={allow})"
            );
            assert!(
                is_virtual_nic_ip(v4("198.19.255.255"), allow),
                "198.19.255.255 must be filtered (allow_overlay={allow})"
            );
            // 169.254.0.0/16 — IPv4 link-local
            assert!(
                is_virtual_nic_ip(v4("169.254.1.1"), allow),
                "169.254.1.1 must be filtered (allow_overlay={allow})"
            );
        }
    }

    /// CGNAT/Tailscale 100.64/10 is filtered when overlay is denied,
    /// allowed when overlay is permitted.
    #[test]
    fn cgnat_v4_respects_overlay_flag() {
        // allow_overlay = false → filtered
        assert!(is_virtual_nic_ip(v4("100.64.0.1"), false));
        assert!(is_virtual_nic_ip(v4("100.100.100.100"), false));
        assert!(is_virtual_nic_ip(v4("100.127.255.255"), false));

        // allow_overlay = true → allowed (predicate returns false)
        assert!(!is_virtual_nic_ip(v4("100.64.0.1"), true));
        assert!(!is_virtual_nic_ip(v4("100.100.100.100"), true));
        assert!(!is_virtual_nic_ip(v4("100.127.255.255"), true));
    }

    /// Tailscale IPv6 ULA fd7a:115c:a1e0::/48 mirrors the CGNAT v4 case.
    #[test]
    fn tailscale_ula_v6_respects_overlay_flag() {
        // First three 16-bit segments must match: fd7a:115c:a1e0
        assert!(is_virtual_nic_ip(v6("fd7a:115c:a1e0::1"), false));
        assert!(is_virtual_nic_ip(v6("fd7a:115c:a1e0:ab12:cd34::"), false));
        assert!(!is_virtual_nic_ip(v6("fd7a:115c:a1e0::1"), true));
        assert!(!is_virtual_nic_ip(v6("fd7a:115c:a1e0:ab12:cd34::"), true));
    }

    /// Real-world LAN/WAN IPs are never filtered, regardless of flag.
    #[test]
    fn real_world_addresses_pass_through() {
        for &allow in &[false, true] {
            for ip in [
                v4("10.0.0.1"),
                v4("192.168.1.42"),
                v4("172.16.0.5"),
                v4("8.8.8.8"),
                v4("100.63.255.255"), // just below 100.64/10
                v4("100.128.0.0"),    // just above 100.64/10
                v4("198.17.255.255"), // just below 198.18/15
                v4("198.20.0.0"),     // just above 198.18/15
                v4("169.253.255.255"),
                v4("169.255.0.0"),
                v6("2001:db8::1"),
                v6("fe80::1"),
                v6("fd7a:115c:a1e1::1"), // off-by-one outside Tailscale ULA
                v6("fc00::1"),           // generic ULA, not Tailscale
            ] {
                assert!(
                    !is_virtual_nic_ip(ip, allow),
                    "{ip} must NOT be filtered (allow_overlay={allow})"
                );
            }
        }
    }

    /// apply_addr_filter: no virtual addresses present → returns Borrowed
    /// (untouched).
    #[test]
    fn addr_filter_passes_through_clean_set() {
        let addrs = vec![
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "192.168.1.1".parse().unwrap(),
                4242,
            ))),
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "10.0.0.5".parse().unwrap(),
                4242,
            ))),
        ];
        let kept = apply_addr_filter(&addrs, false);
        assert_eq!(
            kept.len(),
            addrs.len(),
            "clean set must be passed through unchanged"
        );
    }

    /// apply_addr_filter: drops virtual NIC addrs when allow_overlay=false.
    #[test]
    fn addr_filter_drops_overlay_when_disallowed() {
        let addrs = vec![
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "192.168.1.1".parse().unwrap(),
                4242,
            ))),
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "100.64.0.7".parse().unwrap(),
                4242,
            ))), // Tailscale 100.x — must be dropped
            TransportAddr::Ip(SocketAddr::V6(SocketAddrV6::new(
                "fd7a:115c:a1e0::1".parse().unwrap(),
                4242,
                0,
                0,
            ))), // Tailscale ULA — must be dropped
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "198.18.0.1".parse().unwrap(),
                4242,
            ))), // Clash — always dropped
        ];
        let kept: Vec<TransportAddr> = apply_addr_filter(&addrs, false).into_owned();
        assert_eq!(kept.len(), 1, "only the real LAN IP should survive");
        match &kept[0] {
            TransportAddr::Ip(s) => assert_eq!(s.ip().to_string(), "192.168.1.1"),
            _ => panic!("expected Ip variant"),
        }
    }

    /// apply_addr_filter: keeps overlay addrs when allow_overlay=true,
    /// but still drops always-filtered classes.
    #[test]
    fn addr_filter_keeps_overlay_when_allowed_but_still_drops_clash() {
        let addrs = vec![
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "192.168.1.1".parse().unwrap(),
                4242,
            ))),
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "100.64.0.7".parse().unwrap(),
                4242,
            ))), // overlay — kept now
            TransportAddr::Ip(SocketAddr::V6(SocketAddrV6::new(
                "fd7a:115c:a1e0::1".parse().unwrap(),
                4242,
                0,
                0,
            ))), // overlay — kept now
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "198.18.0.1".parse().unwrap(),
                4242,
            ))), // Clash — always dropped
            TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
                "169.254.0.5".parse().unwrap(),
                4242,
            ))), // link-local — always dropped
        ];
        let kept: Vec<TransportAddr> = apply_addr_filter(&addrs, true).into_owned();
        assert_eq!(kept.len(), 3, "real LAN + 2 overlay candidates kept");
        let ips: Vec<String> = kept
            .iter()
            .filter_map(|a| match a {
                TransportAddr::Ip(s) => Some(s.ip().to_string()),
                _ => None,
            })
            .collect();
        assert!(ips.contains(&"192.168.1.1".to_string()));
        assert!(ips.contains(&"100.64.0.7".to_string()));
        assert!(ips.iter().any(|s| s == "fd7a:115c:a1e0::1"));
        assert!(!ips.iter().any(|s| s == "198.18.0.1"));
        assert!(!ips.iter().any(|s| s == "169.254.0.5"));
    }

    /// 记录经过地址过滤后真正到达发布服务的数据。
    #[derive(Debug, Clone, Default)]
    struct RecordingLookup {
        published: Arc<StdMutex<Vec<Vec<TransportAddr>>>>,
    }

    impl iroh::address_lookup::AddressLookup for RecordingLookup {
        fn publish(&self, data: &iroh::address_lookup::EndpointData) {
            self.published
                .lock()
                .unwrap()
                .push(data.addrs().cloned().collect());
        }
    }

    fn publish_through_production_filter(
        allow_overlay: bool,
        sets: &[Vec<TransportAddr>],
    ) -> Vec<Vec<TransportAddr>> {
        let recorder = RecordingLookup::default();
        let lookup = iroh::address_lookup::FilteredAddressLookup::new(
            recorder.clone(),
            build_addr_filter(allow_overlay),
        );
        for set in sets {
            iroh::address_lookup::AddressLookup::publish(
                &lookup,
                &iroh::address_lookup::EndpointData::new(set.clone()),
            );
        }
        let published = recorder.published.lock().unwrap().clone();
        published
    }

    fn mixed_candidates() -> Vec<TransportAddr> {
        let ip = |text: &str| TransportAddr::Ip(text.parse().unwrap());
        vec![
            ip("203.0.113.10:4242"),
            ip("[2001:db8::7]:4242"),
            ip("192.168.31.72:4242"),
            ip("100.64.0.7:4242"),
            ip("[fd7a:115c:a1e0::1]:4242"),
            ip("198.18.0.1:4242"),
            ip("169.254.0.5:4242"),
            TransportAddr::Relay("https://relay.example.invalid".parse().unwrap()),
        ]
    }

    /// 公网 IPv4、IPv6、私网与 relay 同时存在时，发布只移除虚拟网卡候选，
    /// 且 ticket 过滤保留同一组 IP；overlay 开关只改变 overlay 候选。
    #[test]
    fn publish_keeps_public_private_and_relay_and_matches_ticket() {
        for allow_overlay in [false, true] {
            let published = publish_through_production_filter(allow_overlay, &[mixed_candidates()]);
            let kept = &published[0];
            let ips: Vec<String> = kept
                .iter()
                .filter_map(|addr| match addr {
                    TransportAddr::Ip(socket) => Some(socket.to_string()),
                    _ => None,
                })
                .collect();
            for expected in [
                "203.0.113.10:4242",
                "[2001:db8::7]:4242",
                "192.168.31.72:4242",
            ] {
                assert!(ips.iter().any(|ip| ip == expected), "{expected} must stay");
            }
            assert_eq!(
                kept.iter()
                    .filter(|addr| matches!(addr, TransportAddr::Relay(_)))
                    .count(),
                1
            );
            for dropped in ["198.18.0.1:4242", "169.254.0.5:4242"] {
                assert!(!ips.iter().any(|ip| ip == dropped), "{dropped} must go");
            }
            for overlay in ["100.64.0.7:4242", "[fd7a:115c:a1e0::1]:4242"] {
                assert_eq!(ips.iter().any(|ip| ip == overlay), allow_overlay);
            }

            let secret = iroh::SecretKey::from_bytes(&[0x41; 32]);
            let ticket = crate::network::iroh::addr_filter::filter_endpoint_addr(
                iroh::EndpointAddr::from_parts(secret.public(), mixed_candidates()),
                allow_overlay,
            );
            let mut ticket_addrs: Vec<_> = ticket.addrs.iter().cloned().collect();
            let mut published_addrs = kept.clone();
            ticket_addrs.sort();
            published_addrs.sort();
            assert_eq!(ticket_addrs, published_addrs);
        }
    }

    /// 同一过滤器连续收到切网前后的两组地址，新公网候选照常发布，
    /// 不依赖创建过滤器时的网络状态。
    #[test]
    fn publish_follows_address_changes_on_the_same_filter() {
        let ip = |text: &str| TransportAddr::Ip(text.parse().unwrap());
        let before = vec![ip("192.168.31.72:4242"), ip("203.0.113.10:4242")];
        let after = vec![ip("10.20.30.40:4242"), ip("198.51.100.9:4242")];
        let published = publish_through_production_filter(false, &[before.clone(), after.clone()]);
        assert_eq!(published, vec![before, after]);
    }

    #[test]
    fn relay_mode_rejects_empty_relay_list_when_enabled() {
        let cfg = IrohNodeConfig {
            disable_relays: false,
            relay_urls: Vec::new(),
            ..Default::default()
        };
        assert!(matches!(
            relay_mode_from_config(&cfg),
            Err(IrohNodeError::InvalidRelayUrl(
                RelayUrlProblem::NoneConfigured
            ))
        ));
    }

    /// 内置列表脱离 `RelayMode::Default` 后，仍须与所用 iroh 版本的上游默认条目逐项一致
    /// （含 QUIC 地址发现配置）；升级 iroh 时若上游默认变化，此处失败即提示显式审阅内置列表。
    #[test]
    fn builtin_relay_list_matches_iroh_default_relay_map() {
        let cfg = IrohNodeConfig {
            disable_relays: false,
            relay_urls: uc_core::settings::relay_routing::RelayRouting::BuiltIn.effective_urls(&[]),
            ..Default::default()
        };
        let RelayMode::Custom(builtin) = relay_mode_from_config(&cfg).expect("relay mode") else {
            panic!("built-in routing must build an explicit relay map");
        };
        let upstream = iroh::defaults::prod::default_relay_map();
        assert_eq!(builtin.len(), upstream.len());
        for upstream_relay in upstream.relays::<Vec<_>>() {
            let ours = builtin
                .get(&upstream_relay.url)
                .expect("built-in list must contain every upstream default relay");
            assert_eq!(ours.quic, upstream_relay.quic);
            assert!(ours.auth_token.is_none());
        }
    }

    #[test]
    fn relay_mode_custom_urls_builds_custom_map() {
        let cfg = IrohNodeConfig {
            disable_relays: false,
            relay_urls: vec![
                "https://relay-a.example.com.".to_string(),
                "https://relay-b.example.com.".to_string(),
            ],
            ..Default::default()
        };
        let mode = relay_mode_from_config(&cfg).expect("relay mode");
        match mode {
            RelayMode::Custom(map) => {
                let urls = map.urls::<Vec<_>>();
                assert_eq!(urls.len(), 2);
                assert!(urls
                    .iter()
                    .any(|url| url.as_str() == "https://relay-a.example.com./"));
                assert!(urls
                    .iter()
                    .any(|url| url.as_str() == "https://relay-b.example.com./"));
            }
            other => panic!("expected custom relay mode, got {other:?}"),
        }
    }

    #[test]
    fn relay_mode_applies_tokens_only_to_matching_relays() {
        let relay_a = "https://relay-a.example.com./";
        let relay_b = "https://relay-b.example.com./";
        let cfg = IrohNodeConfig {
            disable_relays: false,
            relay_urls: vec![relay_a.to_string(), relay_b.to_string()],
            relay_access_tokens: std::collections::BTreeMap::from([(
                relay_a.to_string(),
                IrohRelayAccessToken::new("relay-a-token".to_string()).expect("valid token"),
            )]),
            ..Default::default()
        };

        let mode = relay_mode_from_config(&cfg).expect("relay mode");
        let RelayMode::Custom(map) = mode else {
            panic!("expected custom relay mode");
        };
        let relay_a_url: RelayUrl = relay_a.parse().expect("relay A URL");
        let relay_b_url: RelayUrl = relay_b.parse().expect("relay B URL");
        assert_eq!(
            map.get(&relay_a_url)
                .expect("relay A config")
                .auth_token
                .as_deref(),
            Some("relay-a-token")
        );
        assert_eq!(
            map.get(&relay_b_url)
                .expect("relay B config")
                .auth_token
                .as_deref(),
            None
        );
    }

    #[test]
    fn relay_access_token_rejects_values_that_cannot_be_sent_as_an_http_header() {
        let error = IrohRelayAccessToken::new("令牌".to_string()).expect_err("invalid token");

        assert!(matches!(error, IrohNodeError::InvalidRelayAccessToken));
    }

    #[test]
    fn relay_mode_lan_only_ignores_custom_urls() {
        let cfg = IrohNodeConfig {
            disable_relays: true,
            relay_urls: vec!["https://relay.example.com.".to_string()],
            ..Default::default()
        };
        let mode = relay_mode_from_config(&cfg).expect("relay mode");
        assert!(matches!(mode, RelayMode::Disabled));
    }

    #[test]
    fn relay_mode_rejects_invalid_custom_url() {
        let cfg = IrohNodeConfig {
            disable_relays: false,
            relay_urls: vec!["not a url".to_string()],
            ..Default::default()
        };
        let err = relay_mode_from_config(&cfg).expect_err("invalid relay url");
        assert!(matches!(err, IrohNodeError::InvalidRelayUrl(_)));
    }

    #[test]
    fn relay_mode_error_omits_the_configured_url_and_credentials() {
        let cfg = IrohNodeConfig {
            disable_relays: false,
            relay_urls: vec!["https://user:secret-pass@relay.example.com".to_string()],
            ..Default::default()
        };
        let err = relay_mode_from_config(&cfg).expect_err("credentials rejected");

        assert!(matches!(
            err,
            IrohNodeError::InvalidRelayUrl(RelayUrlProblem::Credentials)
        ));
        let rendered = format!("{err} {err:?}");
        assert!(!rendered.contains("secret-pass"));
        assert!(!rendered.contains("relay.example.com"));
    }

    #[test]
    fn relay_mode_rejects_custom_url_with_unsupported_scheme() {
        let cfg = IrohNodeConfig {
            disable_relays: false,
            relay_urls: vec!["ftp://relay.example.com".to_string()],
            ..Default::default()
        };
        let err = relay_mode_from_config(&cfg).expect_err("invalid relay scheme");
        assert!(matches!(err, IrohNodeError::InvalidRelayUrl(_)));
    }

    #[tokio::test]
    #[ignore = "requires a protected relay and access token"]
    async fn endpoint_connects_to_protected_relay_with_scoped_token() {
        let relay_url = std::env::var("RELAY_ENDPOINT_TARGET").expect("relay target");
        let access_token = std::env::var("RELAY_ENDPOINT_TOKEN").expect("relay token");
        let config = IrohNodeConfig {
            relay_urls: vec![relay_url.clone()],
            relay_access_tokens: BTreeMap::from([(
                relay_url,
                IrohRelayAccessToken::new(access_token).expect("valid token"),
            )]),
            ..Default::default()
        };
        let store = identity_store();
        let builder = IrohNodeBuilder::bind(&store, config)
            .await
            .expect("bind endpoint");

        tokio::time::timeout(
            Duration::from_secs(10),
            builder.session_context.endpoint.online(),
        )
        .await
        .expect("endpoint did not connect to protected relay");
        builder.session_context.endpoint.close().await;
    }

    /// 成员被移除后，生产装配下所有已授权的长连接都必须被关闭。
    mod removed_member_connections {
        use super::*;
        use std::sync::atomic::{AtomicBool, Ordering};
        use uc_application::deps::ClipboardDelivery;
        use uc_core::membership::{
            MemberRepositoryPort, MembershipError, PeerAdmissionError, PeerAdmissionPort,
            SpaceMember,
        };
        use uc_core::ports::security::IdentityFingerprintFactoryPort;
        use uc_core::ports::PeerAddressRecord;
        use uc_core::MemberSyncPreferences;
        use uc_sync_protocol::clipboard as clipboard_wire;
        use uc_sync_protocol::transfer_progress::{self as progress_wire, ProgressFrame};

        use crate::network::iroh::transfer_progress_v2::TransferProgressInboundAdapter;
        use uc_application::deps::{
            transfer_progress_connectivity_policy, ProgressPageReply, ProgressProjectionEvent,
            ProgressProjectionKey, ProgressProjectionRegistry, ProgressProjectionWorker,
            ProgressReceiveError, ProgressSnapshotProviderPort, ProgressSnapshotQuery,
        };
        use uc_core::file_transfer::{ProgressIdentity, ProgressItem, ProgressItemKind};
        use uc_infra_local::SystemElapsedClock;

        struct RejectUnexpectedProgressQuery;
        #[async_trait]
        impl ProgressSnapshotProviderPort for RejectUnexpectedProgressQuery {
            async fn snapshot_page(
                &self,
                _: &DeviceId,
                _: ProgressSnapshotQuery,
            ) -> Result<ProgressPageReply, ProgressReceiveError> {
                panic!("sender-only membership fixture must not receive snapshot queries")
            }
        }

        const STEP: Duration = Duration::from_secs(5);
        const CLOSE_BUDGET: Duration = Duration::from_secs(3);

        #[derive(Default)]
        struct Members(StdMutex<Vec<SpaceMember>>);
        crate::network::iroh::inbound_peer::member_table_identity_directory!(Members);
        #[async_trait]
        impl MemberRepositoryPort for Members {
            async fn get(
                &self,
                device_id: &DeviceId,
            ) -> Result<Option<SpaceMember>, MembershipError> {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|member| &member.device_id == device_id)
                    .cloned())
            }
            async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
                Ok(self.0.lock().unwrap().clone())
            }
            async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
                self.0.lock().unwrap().push(member.clone());
                Ok(())
            }
            async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError> {
                let mut members = self.0.lock().unwrap();
                let before = members.len();
                members.retain(|member| &member.device_id != device_id);
                Ok(members.len() != before)
            }
        }

        #[derive(Default)]
        struct PeerAddresses(StdMutex<Vec<PeerAddressRecord>>);
        #[async_trait]
        impl PeerAddressRepositoryPort for PeerAddresses {
            async fn get(
                &self,
                device: &DeviceId,
            ) -> Result<Option<PeerAddressRecord>, uc_core::ports::PeerAddressError> {
                Ok(self
                    .0
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|record| &record.device_id == device)
                    .cloned())
            }
            async fn upsert(
                &self,
                record: &PeerAddressRecord,
            ) -> Result<(), uc_core::ports::PeerAddressError> {
                self.0.lock().unwrap().push(record.clone());
                Ok(())
            }
            async fn list(
                &self,
            ) -> Result<Vec<PeerAddressRecord>, uc_core::ports::PeerAddressError> {
                Ok(self.0.lock().unwrap().clone())
            }
            async fn remove(
                &self,
                _device: &DeviceId,
            ) -> Result<(), uc_core::ports::PeerAddressError> {
                Ok(())
            }
        }

        struct Admission(AtomicBool);
        #[async_trait]
        impl PeerAdmissionPort for Admission {
            async fn is_admitted(&self, _device: &DeviceId) -> Result<bool, PeerAdmissionError> {
                Ok(self.0.load(Ordering::SeqCst))
            }
        }

        /// 生产装配的节点 A、一个已加入的成员 B（普通 endpoint），以及撤销成员所需的句柄。
        struct Fixture {
            node: IrohNode,
            peer: Endpoint,
            node_addr: iroh::EndpointAddr,
            members: Arc<Members>,
            admission: Arc<Admission>,
            peer_device: DeviceId,
            reachability: Arc<dyn PeerReachabilityPort>,
            progress: tokio::sync::broadcast::Receiver<ProgressProjectionEvent>,
            _progress_worker: ProgressProjectionWorker,
            clipboard: ClipboardHandlers,
        }

        async fn wait_for_addrs(endpoint: &Endpoint) {
            for _ in 0..100 {
                if !endpoint.addr().addrs.is_empty() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            panic!("endpoint never published direct addresses");
        }

        impl Fixture {
            async fn start() -> Self {
                let peer_key = iroh::SecretKey::from_bytes(&[0x61; 32]);
                let peer_device = DeviceId::new("removed-member");
                let members = Arc::new(Members::default());
                members
                    .save(&SpaceMember {
                        device_id: peer_device.clone(),
                        device_name: "Peer".to_string(),
                        identity_fingerprint: Sha256IdentityFingerprintFactory
                            .from_public_key(peer_key.public().as_bytes())
                            .expect("fingerprint"),
                        joined_at: chrono::Utc::now(),
                        sync_preferences: MemberSyncPreferences::default(),
                    })
                    .await
                    .expect("save member");
                let admission = Arc::new(Admission(AtomicBool::new(true)));
                let peer = Endpoint::builder(iroh::endpoint::presets::N0)
                    .secret_key(peer_key)
                    .relay_mode(RelayMode::Disabled)
                    .bind()
                    .await
                    .expect("bind peer");
                wait_for_addrs(&peer).await;
                let peer_addresses = Arc::new(PeerAddresses::default());
                peer_addresses
                    .upsert(&PeerAddressRecord {
                        device_id: peer_device.clone(),
                        addr_blob: postcard::to_stdvec(&peer.addr()).expect("encode address"),
                        observed_at: chrono::Utc::now(),
                    })
                    .await
                    .expect("seed peer address");
                let identities: Arc<dyn PeerIdentityDirectoryPort> =
                    crate::network::iroh::inbound_peer::member_table_directory(members.clone());
                let fingerprints: Arc<dyn IdentityFingerprintFactoryPort> =
                    Arc::new(Sha256IdentityFingerprintFactory);

                let store = identity_store();
                let network = IrohNodeBuilder::bind(&store, IrohNodeConfig::default())
                    .await
                    .expect("bind");
                let node_endpoint = Arc::clone(&network.session_context.endpoint);
                let mut builder = network.prepare_session();
                let peer_addr_repo: Arc<dyn PeerAddressRepositoryPort> = peer_addresses;
                let reachability = install_presence_fixture(
                    &mut builder,
                    Arc::clone(&peer_addr_repo),
                    identities.clone(),
                    admission.clone(),
                    fingerprints.clone(),
                    Arc::new(FixedClock(1_700_000_000_000)),
                    tokio::sync::broadcast::channel(1).0,
                )
                .expect("install peer reachability");
                let clipboard = builder
                    .install_clipboard({
                        let deps = builder.connectivity_deps(
                            ConnectivityProfile::Clipboard,
                            Arc::clone(&peer_addr_repo),
                            identities.clone(),
                            admission.clone(),
                            fingerprints.clone(),
                            reachability.clone(),
                        );
                        ConnectivityManager::new(
                            deps.backend,
                            deps.admission,
                            ConnectivityPolicy::default(),
                        )
                    })
                    .expect("install clipboard");
                let clock = Arc::new(SystemElapsedClock);
                let (registry, progress_worker) = ProgressProjectionRegistry::new(clock.clone());
                registry
                    .begin_intent(EntryId::from(
                        Uuid::from_bytes(
                            (ProgressProjectionKey {
                                peer: peer_device,
                                source: ProgressIdentity::new([7; 16]),
                            })
                            .source
                            .bytes(),
                        )
                        .to_string(),
                    ))
                    .accept(
                        ProgressProjectionKey {
                            peer: peer_device,
                            source: ProgressIdentity::new([7; 16]),
                        },
                        vec![ProgressItem {
                            kind: ProgressItemKind::File,
                            index: 0,
                        }],
                    )
                    .expect("original send intent");
                let progress = registry.subscribe();
                let deps = builder.connectivity_deps(
                    ConnectivityProfile::TransferProgress,
                    peer_addr_repo,
                    identities,
                    admission.clone(),
                    fingerprints,
                    reachability.clone(),
                );
                let manager = ConnectivityManager::new(
                    deps.backend,
                    deps.admission,
                    transfer_progress_connectivity_policy(clock.clone()),
                );
                let protocol = Arc::new(TransferProgressInboundAdapter::new(
                    Arc::new(registry.clone()),
                    Arc::new(registry.clone()),
                    Arc::new(RejectUnexpectedProgressQuery),
                ));
                builder
                    .install_transfer_progress(registry.inbound_transport(manager, protocol, clock))
                    .expect("install transfer progress");
                let node = spawn_session(network, builder).await;

                wait_for_addrs(&node_endpoint).await;
                Self {
                    node,
                    peer,
                    node_addr: node_endpoint.addr(),
                    members,
                    admission,
                    peer_device,
                    reachability,
                    progress,
                    _progress_worker: progress_worker,
                    clipboard,
                }
            }

            /// 成员被移除：成员表与准入同时撤销，随后由连接 owner 调用 `forget`。
            async fn remove_member(&self) {
                self.members
                    .remove(&self.peer_device)
                    .await
                    .expect("remove member");
                self.admission.0.store(false, Ordering::SeqCst);
                self.reachability.forget(&self.peer_device).await;
            }

            async fn assert_closed(connection: &iroh::endpoint::Connection, alpn: &str) {
                assert!(
                    tokio::time::timeout(CLOSE_BUDGET, connection.closed())
                        .await
                        .is_ok(),
                    "{alpn} connection of a removed member is still open"
                );
            }
        }

        #[tokio::test]
        async fn transfer_progress_connection_is_closed_when_the_member_is_removed() {
            let mut fixture = Fixture::start().await;
            let connection = fixture
                .peer
                .connect(fixture.node_addr.clone(), LEGACY_TRANSFER_PROGRESS_ALPN)
                .await
                .expect("connect");
            let mut send = connection.open_uni().await.expect("open stream");
            progress_wire::write_frame(
                &mut send,
                &ProgressFrame {
                    transfer_id_bytes: [7; 16],
                    bytes_transferred: 1,
                    total_bytes: Some(2),
                    status: uc_core::file_transfer::OutboundProgressStatus::InProgress,
                },
            )
            .await
            .expect("write frame");
            send.finish().expect("finish");
            // 对照：成员有效时事件被投递，证明连接已通过授权。
            tokio::time::timeout(STEP, fixture.progress.recv())
                .await
                .expect("event arrives")
                .expect("event");

            fixture.remove_member().await;

            Fixture::assert_closed(&connection, "transfer-progress").await;
            fixture.node.shutdown().await.unwrap();
        }

        /// 对端收下帧但不回执；成员被移除后，在途的出站发送必须被中止并返回失败。
        #[tokio::test]
        async fn in_flight_dispatch_is_aborted_when_the_member_is_removed() {
            use iroh::protocol::{AcceptError, ProtocolHandler};

            #[derive(Debug, Clone)]
            struct SilentHandler(Arc<tokio::sync::Notify>);
            impl ProtocolHandler for SilentHandler {
                async fn accept(
                    &self,
                    connection: iroh::endpoint::Connection,
                ) -> Result<(), AcceptError> {
                    let (_send, mut recv) = connection
                        .accept_bi()
                        .await
                        .map_err(|err| AcceptError::from_err(err))?;
                    let _ = clipboard_wire::read_frame(&mut recv).await;
                    self.0.notify_one();
                    let _ = connection.closed().await;
                    Ok(())
                }
            }

            let fixture = Fixture::start().await;
            let frame_received = Arc::new(tokio::sync::Notify::new());
            let peer_router = iroh::protocol::Router::builder(fixture.peer.clone())
                .accept(CLIPBOARD_ALPN, SilentHandler(Arc::clone(&frame_received)))
                .spawn();

            let dispatch = Arc::clone(&fixture.clipboard.dispatch);
            let target = fixture.peer_device.clone();
            let sending = tokio::spawn(async move {
                dispatch
                    .dispatch(
                        &target,
                        &uc_core::ports::ClipboardHeader {
                            source_entry: None,
                            version: uc_core::ports::ClipboardHeader::CURRENT_VERSION,
                            snapshot_hash: "9".repeat(64),
                            captured_at_ms: 1_700_000_000_000,
                            origin_device_id: "local".to_string(),
                            origin_device_name: "Local".to_string(),
                            payload_version: 3,
                        },
                        uc_core::ports::SyncPayload {
                            ciphertext: bytes::Bytes::from_static(b"in-flight"),
                        },
                    )
                    .await
            });
            // 对照：帧已到对端，发送停在等待回执。
            tokio::time::timeout(STEP, frame_received.notified())
                .await
                .expect("peer receives the frame");

            fixture.remove_member().await;

            let report = tokio::time::timeout(CLOSE_BUDGET, sending)
                .await
                .expect("the in-flight dispatch of a removed member is still waiting")
                .expect("dispatch task joins");
            assert!(report.outcome.is_err());
            peer_router.shutdown().await.ok();
            fixture.node.shutdown().await.unwrap();
        }

        async fn pending_multiplexed_deliveries(
            fixture: &Fixture,
        ) -> (iroh::endpoint::Connection, Vec<ClipboardDelivery>) {
            let mut inbound = fixture.clipboard.receiver.subscribe();
            let connection = fixture
                .peer
                .connect(fixture.node_addr.clone(), MULTIPLEXED_CLIPBOARD_ALPN)
                .await
                .unwrap();
            assert_eq!(connection.alpn(), MULTIPLEXED_CLIPBOARD_ALPN);
            for payload in [b"first".as_slice(), b"second".as_slice()] {
                let (mut send, _recv) = connection.open_bi().await.unwrap();
                clipboard_wire::write_frame(
                    &mut send,
                    &uc_core::ports::ClipboardHeader {
                        source_entry: None,
                        version: uc_core::ports::ClipboardHeader::CURRENT_VERSION,
                        snapshot_hash: "8".repeat(64),
                        captured_at_ms: 1_700_000_000_000,
                        origin_device_id: "peer".into(),
                        origin_device_name: "Peer".into(),
                        payload_version: 3,
                    },
                    None,
                    &bytes::Bytes::copy_from_slice(payload),
                )
                .await
                .unwrap();
                send.finish().unwrap();
            }
            let mut deliveries = Vec::new();
            for _ in 0..2 {
                deliveries.push(
                    tokio::time::timeout(STEP, inbound.recv())
                        .await
                        .unwrap()
                        .unwrap(),
                );
            }
            (connection, deliveries)
        }

        #[tokio::test]
        async fn multiplexed_streams_close_together_when_the_member_is_removed() {
            let fixture = Fixture::start().await;
            let (connection, deliveries) = pending_multiplexed_deliveries(&fixture).await;
            fixture.remove_member().await;
            Fixture::assert_closed(&connection, "multiplexed-clipboard").await;
            drop(deliveries);
            fixture.node.shutdown().await.unwrap();
        }

        #[tokio::test]
        async fn multiplexed_streams_quiesce_without_rebinding_the_endpoint() {
            let mut fixture = Fixture::start().await;
            let endpoint_id = fixture.node.session_context.endpoint.id();
            let (connection, deliveries) = pending_multiplexed_deliveries(&fixture).await;
            tokio::time::timeout(CLOSE_BUDGET, fixture.node.quiesce_session())
                .await
                .unwrap()
                .unwrap();
            Fixture::assert_closed(&connection, "retired-multiplexed-clipboard").await;
            assert_eq!(fixture.node.session_context.endpoint.id(), endpoint_id);
            drop(deliveries);
            fixture.node.shutdown().await.unwrap();
        }

        #[tokio::test]
        async fn unsettled_clipboard_connection_is_closed_when_the_member_is_removed() {
            let fixture = Fixture::start().await;
            let mut inbound = fixture.clipboard.receiver.subscribe();
            let connection = fixture
                .peer
                .connect(fixture.node_addr.clone(), CLIPBOARD_ALPN)
                .await
                .expect("connect");
            let (mut send, _recv) = connection.open_bi().await.expect("open stream");
            clipboard_wire::write_frame(
                &mut send,
                &uc_core::ports::ClipboardHeader {
                    source_entry: None,
                    version: uc_core::ports::ClipboardHeader::CURRENT_VERSION,
                    snapshot_hash: "9".repeat(64),
                    captured_at_ms: 1_700_000_000_000,
                    origin_device_id: "removed-member".to_string(),
                    origin_device_name: "Peer".to_string(),
                    payload_version: 3,
                },
                None,
                &bytes::Bytes::from_static(b"in-flight"),
            )
            .await
            .expect("write frame");
            send.finish().expect("finish");
            // 对照：帧已通过授权并交给应用，但尚未结算。
            let delivery = tokio::time::timeout(STEP, inbound.recv())
                .await
                .expect("frame arrives")
                .expect("delivery");

            fixture.remove_member().await;

            Fixture::assert_closed(&connection, "clipboard").await;
            drop(delivery);
            fixture.node.shutdown().await.unwrap();
        }
    }
}
