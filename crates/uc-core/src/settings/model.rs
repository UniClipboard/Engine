use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DurationSeconds};

/// v1 -> v2: one-time rewrite of a stock v1-default `retention_policy`
/// (`ByAge(30d) + ByCount(500)`) to the new default (`ByAge(180d)`, no count
/// cap). See `uc_infra_local::settings::migration::MigrationV1ToV2`.
///
/// v2 -> v3: originally rewrote the legacy startup booleans into a startup
/// mode. Startup preferences now belong to the host product, so this step
/// only advances the version. See
/// `uc_infra_local::settings::migration::MigrationV2ToV3`.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

// 所有 settings struct 统一使用 `#[serde(default)]`：缺字段时回退到
// `Default::default()`（在 `defaults.rs` 中实现），保证向后兼容。
// 详见 issue #581：旧版本 settings.json 缺新增字段会让 daemon 启动失败。
/// Engine 拥有的通用设置。
///
/// 宿主产品外壳偏好（启动方式、自动启动、更新、主题、语言、产品分析许可、
/// 快捷键与快捷面板）归宿主产品保存，不属于 Engine 设置。旧版 settings.json
/// 中残留的这些键在加载时被忽略，下一次保存时不再写出。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GeneralSettings {
    pub device_name: Option<String>,
    /// Enables a persistent local diagnostic logging profile on the next process start.
    ///
    /// This only changes local log verbosity. It does not change sync behavior,
    /// network policy, telemetry/analytics gates, or clipboard content handling.
    pub debug_mode: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ContentTypes {
    pub text: bool,
    pub image: bool,
    pub link: bool,
    pub file: bool,
    pub code_snippet: bool,
    pub rich_text: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SyncSettings {
    pub sync_enabled: bool,
    #[serde(alias = "auto_sync")]
    pub auto_sync_enabled: bool,
    pub sync_frequency: SyncFrequency,
    pub content_types: ContentTypes,
    /// Whether restoring a historical entry makes its content the active
    /// clipboard for the rest of the synced devices.
    ///
    /// When enabled, putting a past entry back on this device's clipboard
    /// announces it as the latest active content so peers converge on it
    /// (subject to the per-device send preferences). Default `false`:
    /// re-activating old content is an explicit opt-in so a routine restore
    /// never silently changes what other devices consider current.
    pub sync_on_restore: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SyncFrequency {
    Realtime,
    Interval,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetentionRule {
    /// 按时间清理
    ByAge {
        #[serde_as(as = "DurationSeconds<u64>")]
        max_age: Duration,
    },

    /// 按总数量上限
    ByCount { max_items: usize },

    /// 按内容类型的最大存活时间
    ByContentType {
        content_type: ContentTypes,
        #[serde_as(as = "DurationSeconds<u64>")]
        max_age: Duration,
    },

    /// 按磁盘占用大小
    ByTotalSize { max_bytes: u64 },

    /// 敏感内容快速过期
    Sensitive {
        #[serde_as(as = "DurationSeconds<u64>")]
        max_age: Duration,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleEvaluation {
    AnyMatch, // OR（推荐，默认）
    AllMatch, // AND（极少用）
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "snake_case")]
pub struct RetentionPolicy {
    pub enabled: bool,
    pub rules: Vec<RetentionRule>,
    pub skip_pinned: bool,
    pub evaluation: RuleEvaluation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "snake_case")]
pub struct SecuritySettings {
    /// 是否启用本地数据加密
    pub encryption_enabled: bool,

    /// 是否已经在 keyring 中设置过口令
    ///
    /// 仅用于 UI 与流程判断
    /// 不代表当前口令是否“可用”
    pub passphrase_configured: bool,

    /// 是否启用启动时自动解锁
    ///
    /// 仅用于 UI 与流程判断
    /// 需要用户在系统弹窗中选择“始终允许”才能静默生效
    pub auto_unlock_enabled: bool,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PairingSettings {
    #[serde_as(as = "DurationSeconds<u64>")]
    pub step_timeout: Duration,
    #[serde_as(as = "DurationSeconds<u64>")]
    pub user_verification_timeout: Duration,
    #[serde_as(as = "DurationSeconds<u64>")]
    pub session_timeout: Duration,
    pub max_retries: u8,
    pub protocol_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct FileSyncSettings {
    pub file_sync_enabled: bool,
    pub small_file_threshold: u64,
    pub max_file_size: u64,
    /// Total-byte ceiling for a single file-set capture (all members summed).
    ///
    /// Orthogonal to [`Self::max_file_size`], which bounds one member: this
    /// bounds the *set*. A capture whose members' sizes sum above this is
    /// admitted to local history but excluded from sync (its file-set identity
    /// falls back to path text and the outbound path skips it). Guards the
    /// capture hot path — and, once directory capture lands, the member-count
    /// explosion a deep tree would cause — against unbounded hashing/transfer.
    pub max_file_set_total_bytes: u64,
    /// Member-count ceiling for a single file-set capture.
    ///
    /// Same admit-locally / exclude-from-sync semantics as
    /// [`Self::max_file_set_total_bytes`]. A flat multi-file copy rarely
    /// approaches this; a copied directory tree can, which is why the cap
    /// exists before directory traversal is introduced.
    pub max_file_set_member_count: u64,
    pub file_cache_quota_per_device: u64,
    pub file_retention_hours: u32,
    pub file_auto_cleanup: bool,
    /// Optional directory where inbound files are written.
    ///
    /// When `None` or empty, inbound files stay in the application-managed
    /// storage layout and remain subject to retention cleanup. When set to a
    /// usable directory, inbound files are written there instead; such files
    /// are owned by the user and are never removed by retention cleanup.
    pub auto_save_dir: Option<String>,
}

/// Algorithm used for network flow control.
///
/// Cubic works best on local networks; Bbr3 may perform better on
/// long-distance or lossy links. Requires daemon restart to take effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CongestionController {
    /// Loss-based algorithm, excellent on LAN and well-provisioned paths.
    #[default]
    Cubic,
    /// Bandwidth-probing algorithm that avoids halving the congestion
    /// window on sporadic loss; better for high-latency or lossy links.
    Bbr3,
}

impl std::fmt::Display for CongestionController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cubic => f.write_str("cubic"),
            Self::Bbr3 => f.write_str("bbr3"),
        }
    }
}

impl std::str::FromStr for CongestionController {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "cubic" => Ok(Self::Cubic),
            "bbr3" | "bbr" => Ok(Self::Bbr3),
            other => Err(format!(
                "unknown congestion controller: {other:?} (expected cubic or bbr3)"
            )),
        }
    }
}

// ======================================================================
// NetworkSettings —— LAN-only Mode（v0.7.0）的后端持久化字段
//
// 反向命名规则（Pitfall 1 防御 — 见 .planning/research/PITFALLS.md §Pitfall 1）：
// - UI = "LAN-only Mode = ON"
// - 后端 = `allow_relay_fallback = false`
// - infra (iroh) = `IrohNodeConfig.disable_relays = true`
//
// 三层语义两次反转。**全工程只允许在 `uc-bootstrap/src/network_policy.rs`
// 唯一一处取反**；DTO ↔ View ↔ core 三层只搬运 `allow_relay_fallback`
// 业务正向语义，不取反。
// ======================================================================

/// 网络相关设置（LAN-only Mode 字段族）。
///
/// `#[serde(default)]` 让缺字段时回退到 `Default::default()`：
/// - `allow_relay_fallback = true`（允许 fallback，breaking change 警惕）
/// - `allow_overlay_network_addrs = false`（默认过滤虚拟网卡候选）
/// - `custom_relay_urls = []`（空列表使用产品内置 relay，见 `relay_routing`）
///
/// 修改默认值前请先 grep `LAN-only Mode` 文档与 changelog。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct NetworkSettings {
    /// 是否允许 iroh 在直连失败时回落到公网中继。
    /// `true`（默认）= 允许 fallback，跨网段设备仍可通过 relay 同步；
    /// `false` = LAN-only，禁用 relay，跨网段设备会失联。
    /// 业务正向语义：UI "LAN-only Mode = ON" → 此字段 = `false`。
    pub allow_relay_fallback: bool,

    /// 是否允许把 VPN / overlay 类虚拟网卡地址（CGNAT 100.64.0.0/10、
    /// Tailscale ULA fd7a:115c:a1e0::/48）作为 iroh 直连候选。
    ///
    /// 默认 `false`：默认过滤，避免对端不在同一 tailnet 时把死候选发给 peer，
    /// 拖慢 path-validation、占用 PathId 预算。两端确实都接入同一 VPN
    /// （如 Tailscale）希望让 iroh 借用该 overlay 网络互联时改为 `true`。
    ///
    /// 注意：`198.18.0.0/15`（Clash fake-ip）、`169.254.0.0/16`（IPv4 link-local）
    /// 与本字段无关，永远过滤。
    ///
    /// 修改后需重启 daemon 生效（iroh endpoint bind-time 常量）。
    pub allow_overlay_network_addrs: bool,

    /// 自定义 iroh relay 节点 URL 列表。
    ///
    /// 空列表表示使用产品内置 relay（`relay_routing::BUILTIN_RELAYS`，不写入本字段）；
    /// 非空时只使用这些用户配置的 relay 节点，整体替换内置列表。仅在
    /// `allow_relay_fallback = true` 时生效；LAN-only 模式下 relay 整体禁用，
    /// 但列表仍会被保留，方便用户稍后重新开启 relay fallback。
    ///
    /// 每个值应为 relay 的 HTTP(S) URL，例如
    /// `https://relay.example.com.`。修改后需重启 daemon 生效（iroh endpoint
    /// bind-time 常量）。
    pub custom_relay_urls: Vec<String>,

    /// Algorithm for network flow control.
    ///
    /// `Cubic`（default）works best on local and well-provisioned networks;
    /// `Bbr3` may perform better on long-distance or lossy links at the
    /// cost of reduced throughput on LAN paths. Requires daemon restart.
    #[serde(default)]
    pub congestion_controller: CongestionController,
}

// ======================================================================
// MobileSyncSettings —— 移动端同步（v1：iOS Shortcut）的总开关。
//
// 字段刻意从一个 `enabled` 起步：v1 SPEC 要求 listen_port / bind_address
// 由 daemon 端常量 + profile offset 自动推导，不暴露给用户配置；
// install_method 是 register flow 的一次性选择而非持久化偏好。这里只
// 持久化"用户是否打开移动端同步监听"这一项，其余调用所需信息（当前
// LAN URL、可用 install_method）由 application 层 use case 在 view 中
// 实时拼装。
//
// 修改 enabled 后需要重启 daemon 才会生效（v1 不做热重载，详见
// `.context/mobile-sync/SPEC.md` §5）。
// ======================================================================

/// 移动端同步功能的设置族。
///
/// v3 SyncClipboard 兼容版(SPEC §14.10)按"功能开关 + 广告 URL 细节"两层
/// 拆分。daemon socket bind 行为是常量推导:`enabled && lan_listen_enabled`
/// 时 bind `0.0.0.0:lan_port`,否则不起 listener。
///
/// * `enabled` —— 总开关。关闭时 daemon 完全不起 LAN listener,即使
///   `lan_listen_enabled=true` 也无效。
/// * `lan_listen_enabled` —— LAN listener 子开关。仅在 `enabled=true` 时
///   生效。让用户能在不撤销已登记设备的情况下临时停用 listener
///   (例如出差换不可信网络时)。
/// * `lan_advertise_ip` —— 用户选定的、要写进 SyncClipboard install URL /
///   二维码的 IPv4 字符串(`192.168.1.5` 这类)。仅决定 iPhone 客户端看到
///   的 base_url,**不**决定 daemon socket 绑哪里(daemon 永远绑 `0.0.0.0`)。
///   从 list-interfaces 候选里挑一个 RFC1918 私有地址。`None` 对应 UI 的
///   "自动"选项,展示 / register_device base_url 都退回 `0.0.0.0`(iPhone
///   连不上,需用户挑一个具体 IP 后再添加设备)。
/// * `lan_advertise_base_url` —— 用户指定的完整广告地址(scheme + host +
///   可选 port)。`Some` 时优先于 `lan_advertise_ip` 决定对外公布的连接
///   地址;`None` 时回退到由 advertise IP 与端口拼出的地址形态。
/// * `lan_port` —— 自定义端口。`None` 时取默认 `42720`(SPEC §3.2)。
///
/// 任意字段变更后都需要重启 daemon 才能生效(v1 不做配置热重载,详见
/// `.context/mobile-sync/SPEC.md` §1.2.5)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MobileSyncSettings {
    /// 是否启用移动端同步 LAN 监听总开关。
    /// 默认 `false`:移动端同步对未配对的局域网邻居有暴露面,必须由
    /// 用户在设置页显式开启。
    pub enabled: bool,

    /// LAN listener 子开关。`enabled=true` 时由本字段决定 daemon 启动
    /// 后是否真的把 listener spawn 起来(绑 0.0.0.0:lan_port);否则不起
    /// listener。默认 `false`。
    pub lan_listen_enabled: bool,

    /// 写进 install URL / 二维码的 LAN IPv4 字符串(用户从 list-interfaces
    /// 挑一个)。仅决定 iPhone 端 base_url, 不决定 daemon socket bind
    /// (daemon 永远绑 `0.0.0.0`)。`None` 对应 UI 的"自动"选项,退回
    /// `0.0.0.0`(iPhone 连不上,需挑具体 IP)。
    pub lan_advertise_ip: Option<String>,

    /// 用户指定的完整广告地址(scheme + host + 可选 port)。作为对移动
    /// 客户端公布的连接地址,`Some` 时优先于 [`Self::lan_advertise_ip`];
    /// `None` 时回退到由 advertise IP 与端口拼出的地址形态。仅影响对外
    /// 公布的地址本身,不改变本机实际监听的地址。
    pub lan_advertise_base_url: Option<String>,

    /// 用户自定义的 LAN 监听端口。`None` 时取默认 `42720`(SPEC §3.2)。
    pub lan_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    #[serde(default = "oldest_known_schema_version")]
    pub schema_version: u32,

    #[serde(default)]
    pub general: GeneralSettings,

    #[serde(default)]
    pub sync: SyncSettings,

    #[serde(default)]
    pub retention_policy: RetentionPolicy,

    #[serde(default)]
    pub security: SecuritySettings,

    #[serde(default)]
    pub pairing: PairingSettings,

    #[serde(default)]
    pub file_sync: FileSyncSettings,

    #[serde(default)]
    pub network: NetworkSettings,

    #[serde(default)]
    pub mobile_sync: MobileSyncSettings,
}

/// Schema version assumed for a settings.json missing the `schema_version`
/// field entirely.
///
/// No version of this app has ever written `Settings` without this field —
/// it has existed since the settings system's introduction, alongside
/// `retention_policy` — so this only matters for hand-edited or externally
/// generated files. It deliberately does NOT default to
/// `CURRENT_SCHEMA_VERSION`: that would make a versionless file look
/// already-migrated and silently skip the entire migration chain, hiding
/// exactly the kind of stale-default rewrite `MigrationV1ToV2` exists to
/// perform. Defaulting to the oldest version instead runs it through every
/// migration, which is the safe direction to be wrong in.
///
/// # Examples
///
/// ```
/// use uc_core::settings::model::Settings;
///
/// let s: Settings = serde_json::from_str("{}").unwrap();
/// assert_eq!(s.schema_version, 1);
/// ```
fn oldest_known_schema_version() -> u32 {
    1
}
