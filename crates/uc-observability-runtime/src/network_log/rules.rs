//! 已审定的第三方网络事件调用点与字段提取规则。
//!
//! 规则绑定库的精确版本目录（`iroh-1.3.0`）和调用点（`_events` 结构化 target，或文件与行号）。
//! 升级依赖后版本目录不再匹配，调用点退回为“未审查”并只留下无字段的静态编号，直到重新审定。
//! 输出只含规则声明的固定事件名与固定类别、数量、布尔值或本次运行内的随机引用；
//! 地址、公钥、URL、证书、CID、令牌和自由文本一律不出现在输出里。

use tracing::Level;

/// 采集深度。标准档常驻；详细档只在限时窗口内放行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CaptureTier {
    Standard,
    Detailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Library {
    Iroh,
    IrohRelay,
    IrohBlobs,
    Noq,
    NoqProto,
    NoqUdp,
}

impl Library {
    pub(crate) const COUNT: usize = 6;

    pub(crate) const ALL: [Self; Self::COUNT] = [
        Self::Iroh,
        Self::IrohRelay,
        Self::IrohBlobs,
        Self::Noq,
        Self::NoqProto,
        Self::NoqUdp,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Iroh => "iroh",
            Self::IrohRelay => "iroh_relay",
            Self::IrohBlobs => "iroh_blobs",
            Self::Noq => "noq",
            Self::NoqProto => "noq_proto",
            Self::NoqUdp => "noq_udp",
        }
    }

    fn from_path(path: &str) -> Option<Self> {
        // 只比较第一段，按包边界匹配；`iroh_x` 或 `irohx` 不会被当成 iroh。
        match path.split("::").next()? {
            "iroh" => Some(Self::Iroh),
            "iroh_relay" => Some(Self::IrohRelay),
            "iroh_blobs" => Some(Self::IrohBlobs),
            "noq" => Some(Self::Noq),
            "noq_proto" => Some(Self::NoqProto),
            "noq_udp" => Some(Self::NoqUdp),
            _ => None,
        }
    }

    /// target 与 module_path 必须指向同一个库；任何一边缺失或不一致都不属于网络库事件。
    pub(crate) fn of(metadata: &tracing::Metadata<'_>) -> Option<Self> {
        let by_target = Self::from_path(metadata.target())?;
        (Self::from_path(metadata.module_path()?)? == by_target).then_some(by_target)
    }
}

/// 调用点的定位方式。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Site {
    /// iroh 为事件消费者保留的结构化 target。
    Target(&'static str),
    /// 库内的文件与行号（相对 `src/`）。
    Line { file: &'static str, line: u32 },
}

/// 从原始字段值提取输出值的方式。
#[derive(Debug, Clone, Copy)]
pub(crate) enum Kind {
    /// 连接标识 → 本次运行内的随机连接引用。
    ConnRef,
    /// 对端标识 → 本次运行内的随机对端引用。
    PeerRef,
    /// 只输出配置里登记过的 ALPN，其余为 `unrecognized`。
    Alpn,
    /// 值以其中一个变体名开头（Debug 形式），输出该变体名；不匹配为 `other`。
    Variant(&'static [&'static str]),
    /// 值包含其中一个片段则输出对应类别；否则 `other`。
    Contains(&'static [(&'static str, &'static str)]),
    /// 传输路径或地址的类别：`ip_v4`、`ip_v6`、`relay`、`custom`、`none`、`other`。
    Transport,
    /// 按分隔符数条目（空集合为 0）。
    CountSeparated(&'static str),
    /// 按片段出现次数计数。
    CountToken(&'static str),
    Bool,
    /// 不超过上限的无符号整数。
    Number(u64),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FieldRule {
    /// 事件里的原始字段名。
    pub(crate) source: &'static str,
    /// 输出字段名。
    pub(crate) output: &'static str,
    pub(crate) kind: Kind,
}

const fn field(source: &'static str, output: &'static str, kind: Kind) -> FieldRule {
    FieldRule {
        source,
        output,
        kind,
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Rule {
    /// 输出的固定事件名。
    pub(crate) event: &'static str,
    pub(crate) library: Library,
    /// 精确的库版本目录；路径里没有这个目录的事件不匹配。
    pub(crate) version_dir: &'static str,
    pub(crate) site: Site,
    pub(crate) level: Level,
    pub(crate) tier: CaptureTier,
    pub(crate) fields: &'static [FieldRule],
    /// 令牌桶：突发容量与每个令牌的回填毫秒数。
    pub(crate) burst: u32,
    pub(crate) refill_ms: u32,
}

const IROH: &str = "iroh-1.3.0";
const NOQ_PROTO: &str = "noq-proto-1.3.0";

/// 根 `Cargo.toml` 的 `[patch.crates-io]` 固定的 iroh 提交（前 7 位，即 cargo 签出目录名）。
/// 固定后的 iroh 不在 `iroh-1.3.0` 这样的注册表目录里，而在 git 签出目录里；规则仍按 `IROH`
/// 逻辑版本匹配，但只接受这里列出的提交，换提交必须同时评审规则并更新这里。
const PINNED_IROH_REVS: &[&str] = &["c8efae0"];

const CONNECTION_ERROR_VARIANTS: &[&str] = &[
    "VersionMismatch",
    "TransportError",
    "ConnectionClosed",
    "ApplicationClosed",
    "Reset",
    "TimedOut",
    "LocallyClosed",
    "CidsExhausted",
];

const PATH_ABANDON_VARIANTS: &[&str] = &[
    "ApplicationClosed",
    "ValidationFailed",
    "TimedOut",
    "UnusableAfterNetworkChange",
    "RemoteAbandoned",
];

const PATH_STATUS_VARIANTS: &[&str] = &["Available", "Backup"];

const HANDSHAKE_CLASSES: &[(&str, &str)] = &[
    (
        "peer doesn't support any known protocol",
        "no_application_protocol",
    ),
    (
        "peer doesn't support any known application protocol",
        "no_application_protocol",
    ),
    ("timed out", "timed_out"),
    ("connection closed", "connection_closed"),
    ("reset", "reset"),
    ("version", "version_mismatch"),
    ("transport error", "transport_error"),
];

const QUIC_STATES: &[(&str, &str)] = &[
    ("handshake", "handshake"),
    ("established", "established"),
    ("draining", "draining"),
    ("drained", "drained"),
];

pub(crate) const RULES: &[Rule] = &[
    Rule {
        event: "connection.connecting",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::conn::connecting"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[
            field("remote_id", "peer_ref", Kind::PeerRef),
            field("alpn", "alpn", Kind::Alpn),
        ],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "connection.connected",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::conn::connected"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[
            field("conn_id", "conn_ref", Kind::ConnRef),
            field("side", "side", Kind::Variant(&["Client", "Server"])),
            field("remote_id", "peer_ref", Kind::PeerRef),
            field("alpn", "alpn", Kind::Alpn),
        ],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "connection.incoming",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::conn::incoming"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[field("remote_addr", "addr_kind", Kind::Transport)],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "connection.closed",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::conn::closed"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[
            field("conn_id", "conn_ref", Kind::ConnRef),
            field("remote_id", "peer_ref", Kind::PeerRef),
            field("reason", "reason", Kind::Variant(CONNECTION_ERROR_VARIANTS)),
        ],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "path.selected",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::path::selected"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[
            field("remote", "peer_ref", Kind::PeerRef),
            field("network_path", "path_kind", Kind::Transport),
            field("prev_network_path", "prev_path_kind", Kind::Transport),
        ],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "path.abandoned",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::path::abandoned"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[
            field("remote", "peer_ref", Kind::PeerRef),
            field("conn_id", "conn_ref", Kind::ConnRef),
            field("path_id", "path_id", Kind::Number(64)),
            field("network_path", "path_kind", Kind::Transport),
            field("reason", "reason", Kind::Variant(PATH_ABANDON_VARIANTS)),
        ],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "relay.connected",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::relay::connected"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[field("home_relay", "home_relay", Kind::Bool)],
        burst: 8,
        refill_ms: 1_000,
    },
    Rule {
        event: "relay.home_changed",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::relay::home_changed"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[field("home_relay", "home_relay", Kind::Bool)],
        burst: 8,
        refill_ms: 1_000,
    },
    Rule {
        event: "network.link_change",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::link_change"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[field("is_major", "is_major", Kind::Bool)],
        burst: 8,
        refill_ms: 1_000,
    },
    Rule {
        event: "network.direct_addrs_changed",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::direct_addrs"),
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[field("addrs", "count", Kind::CountToken("DirectAddr {"))],
        burst: 8,
        refill_ms: 1_000,
    },
    Rule {
        event: "path.open",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::path::open"),
        level: Level::DEBUG,
        tier: CaptureTier::Detailed,
        fields: &[
            field("remote", "peer_ref", Kind::PeerRef),
            field("conn_id", "conn_ref", Kind::ConnRef),
            field("path_id", "path_id", Kind::Number(64)),
            field("network_path", "path_kind", Kind::Transport),
        ],
        burst: 32,
        refill_ms: 100,
    },
    Rule {
        event: "path.status_changed",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::path::set_status"),
        level: Level::DEBUG,
        tier: CaptureTier::Detailed,
        fields: &[
            field("remote", "peer_ref", Kind::PeerRef),
            field("conn_id", "conn_ref", Kind::ConnRef),
            field("path_id", "path_id", Kind::Number(64)),
            field("network_path", "path_kind", Kind::Transport),
            field("status", "status", Kind::Variant(PATH_STATUS_VARIANTS)),
            field(
                "prev_status",
                "prev_status",
                Kind::Variant(PATH_STATUS_VARIANTS),
            ),
        ],
        burst: 32,
        refill_ms: 100,
    },
    Rule {
        event: "holepunch.init",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Target("iroh::_events::qnt::init"),
        level: Level::DEBUG,
        tier: CaptureTier::Detailed,
        fields: &[
            field("remote", "peer_ref", Kind::PeerRef),
            field(
                "local_candidates",
                "local_count",
                Kind::CountSeparated(", "),
            ),
            field(
                "remote_candidates",
                "remote_count",
                Kind::CountSeparated(", "),
            ),
        ],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "router.accepting_failed",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Line {
            file: "src/protocol.rs",
            line: 640,
        },
        level: Level::WARN,
        tier: CaptureTier::Standard,
        fields: &[field(
            "message",
            "reason",
            Kind::Contains(HANDSHAKE_CLASSES),
        )],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "quic.handshake_rejected",
        library: Library::NoqProto,
        version_dir: NOQ_PROTO,
        site: Site::Line {
            file: "src/endpoint.rs",
            line: 690,
        },
        level: Level::DEBUG,
        tier: CaptureTier::Standard,
        fields: &[field(
            "message",
            "reason",
            Kind::Contains(HANDSHAKE_CLASSES),
        )],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "quic.state_changed",
        library: Library::NoqProto,
        version_dir: NOQ_PROTO,
        site: Site::Line {
            file: "src/connection/state.rs",
            line: 59,
        },
        level: Level::TRACE,
        tier: CaptureTier::Detailed,
        fields: &[field("message", "state", Kind::Contains(QUIC_STATES))],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "quic.state_changed",
        library: Library::NoqProto,
        version_dir: NOQ_PROTO,
        site: Site::Line {
            file: "src/connection/state.rs",
            line: 64,
        },
        level: Level::TRACE,
        tier: CaptureTier::Detailed,
        fields: &[field("message", "state", Kind::Contains(QUIC_STATES))],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "quic.state_changed",
        library: Library::NoqProto,
        version_dir: NOQ_PROTO,
        site: Site::Line {
            file: "src/connection/state.rs",
            line: 109,
        },
        level: Level::TRACE,
        tier: CaptureTier::Detailed,
        fields: &[field("message", "state", Kind::Contains(QUIC_STATES))],
        burst: 16,
        refill_ms: 250,
    },
    Rule {
        event: "quic.state_changed",
        library: Library::NoqProto,
        version_dir: NOQ_PROTO,
        site: Site::Line {
            file: "src/connection/state.rs",
            line: 155,
        },
        level: Level::TRACE,
        tier: CaptureTier::Detailed,
        fields: &[field("message", "state", Kind::Contains(QUIC_STATES))],
        burst: 16,
        refill_ms: 250,
    },
    // 固定提交 `c8efae0` 的 `remote_state.rs:1089`：路径打开因 CID/路径号耗尽被推迟后重试。
    // 只输出事件本身（原始字段是候选地址，一律丢弃）；出现频率就是 #214 H1 的直接证据。
    Rule {
        event: "path.open_deferred",
        library: Library::Iroh,
        version_dir: IROH,
        site: Site::Line {
            file: "src/socket/remote_map/remote_state.rs",
            line: 1089,
        },
        level: Level::TRACE,
        tier: CaptureTier::Detailed,
        fields: &[],
        burst: 8,
        refill_ms: 1000,
    },
    Rule {
        event: "quic.packet_discarded",
        library: Library::NoqProto,
        version_dir: NOQ_PROTO,
        site: Site::Line {
            file: "src/connection/mod.rs",
            line: 2356,
        },
        level: Level::TRACE,
        tier: CaptureTier::Detailed,
        fields: &[field("path_id", "path_id", Kind::Number(64))],
        burst: 16,
        refill_ms: 250,
    },
];

/// 事件的库、版本目录和库内相对路径。路径里没有 `name-x.y.z/` 目录时版本未知。
pub(crate) struct CallsitePath<'a> {
    pub(crate) version_dir: Option<&'a str>,
    /// 去掉构建机目录前缀后的库内路径（形如 `src/protocol.rs`）；未知时为 `None`。
    pub(crate) in_crate: Option<&'a str>,
}

pub(crate) fn callsite_path(file: Option<&str>) -> CallsitePath<'_> {
    let Some(file) = file else {
        return CallsitePath {
            version_dir: None,
            in_crate: None,
        };
    };
    if let Some(pinned) = pinned_iroh_checkout(file) {
        return pinned;
    }
    let mut rest = file;
    while let Some(index) = rest.find(is_separator) {
        let (segment, tail) = rest.split_at(index);
        if looks_like_versioned_dir(segment) {
            return CallsitePath {
                version_dir: Some(segment),
                in_crate: Some(&tail[1..]),
            };
        }
        rest = &tail[1..];
    }
    CallsitePath {
        version_dir: None,
        in_crate: None,
    }
}

/// `file!()` 在 Windows 构建里使用反斜杠，Unix 构建使用正斜杠；两者都是路径分隔符。
fn is_separator(character: char) -> bool {
    matches!(character, '/' | '\\')
}

/// 库内路径与审查登记的路径是否相同，忽略分隔符写法。
fn same_path(actual: &str, reviewed: &str) -> bool {
    actual.len() == reviewed.len()
        && actual
            .chars()
            .zip(reviewed.chars())
            .all(|(a, b)| a == b || (is_separator(a) && is_separator(b)))
}

/// cargo 的 git 签出路径 `…/iroh-<16 位十六进制>/<提交前 7 位>/iroh/src/…`，且提交在允许列表内时，
/// 按逻辑版本 `IROH` 匹配；其他 git 来源、其他提交或其他子包都不匹配。
fn pinned_iroh_checkout(file: &str) -> Option<CallsitePath<'_>> {
    let mut rest = file;
    while let Some(index) = rest.find(is_separator) {
        let (segment, tail) = rest.split_at(index);
        rest = &tail[1..];
        let Some(hash) = segment.strip_prefix("iroh-") else {
            continue;
        };
        if hash.len() != 16 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            continue;
        }
        let mut parts = rest.splitn(3, is_separator);
        let (rev, package, in_crate) = (parts.next()?, parts.next()?, parts.next()?);
        return (PINNED_IROH_REVS.contains(&rev) && package == "iroh").then_some(CallsitePath {
            version_dir: Some(IROH),
            in_crate: Some(in_crate),
        });
    }
    None
}

/// `iroh-1.3.0`、`noq-proto-1.3.0` 这种“名称-主.次.修订”目录。
fn looks_like_versioned_dir(segment: &str) -> bool {
    let Some(index) = segment.rfind('-') else {
        return false;
    };
    let (name, version) = segment.split_at(index);
    let version = &version[1..];
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        && version.split('.').count() >= 3
        && version
            .split('.')
            .take(3)
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

/// 找到匹配当前调用点的规则。
pub(crate) fn rule_for(metadata: &tracing::Metadata<'_>) -> Option<&'static Rule> {
    let library = Library::of(metadata)?;
    let path = callsite_path(metadata.file());
    RULES.iter().find(|rule| {
        rule.library == library
            && rule.level == *metadata.level()
            && path.version_dir == Some(rule.version_dir)
            && match rule.site {
                Site::Target(target) => metadata.target() == target,
                Site::Line { file, line } => {
                    path.in_crate.is_some_and(|actual| same_path(actual, file))
                        && metadata.line() == Some(line)
                }
            }
    })
}

pub(crate) fn rule_index(rule: &Rule) -> usize {
    RULES
        .iter()
        .position(|candidate| std::ptr::eq(candidate, rule))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGISTRY: &str =
        "/home/u/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/iroh-1.3.0/src/socket.rs";
    const PINNED: &str =
        "/home/u/.cargo/git/checkouts/iroh-0123456789abcdef/c8efae0/iroh/src/socket.rs";

    #[test]
    fn registry_layout_keeps_matching_by_version_directory() {
        let path = callsite_path(Some(REGISTRY));
        assert_eq!(path.version_dir, Some(IROH));
        assert_eq!(path.in_crate, Some("src/socket.rs"));
    }

    #[test]
    fn pinned_git_checkout_matches_the_logical_version() {
        let path = callsite_path(Some(PINNED));
        assert_eq!(path.version_dir, Some(IROH));
        assert_eq!(path.in_crate, Some("src/socket.rs"));
    }

    #[test]
    fn unlisted_revision_package_or_hash_shape_does_not_match() {
        for file in [
            "/c/git/checkouts/iroh-0123456789abcdef/1111111/iroh/src/socket.rs",
            "/c/git/checkouts/iroh-0123456789abcdef/c8efae0/iroh-relay/src/lib.rs",
            "/c/git/checkouts/iroh-0123456789abcde/c8efae0/iroh/src/socket.rs",
            "/c/git/checkouts/iroh-0123456789abcdeg/c8efae0/iroh/src/socket.rs",
            "/c/git/checkouts/iroh-0123456789abcdef/c8efae0/iroh",
        ] {
            let path = callsite_path(Some(file));
            assert_eq!(path.version_dir, None, "{file}");
            assert_eq!(path.in_crate, None, "{file}");
        }
    }

    #[test]
    fn allow_list_contains_the_revision_pinned_in_the_root_manifest() {
        let manifest = include_str!("../../../../Cargo.toml");
        let pinned = manifest
            .lines()
            .filter(|line| line.starts_with("iroh = { git"))
            .filter_map(|line| line.split("rev = \"").nth(1)?.split('"').next())
            .collect::<Vec<_>>();
        assert_eq!(
            pinned.len(),
            1,
            "root manifest must pin iroh to one revision"
        );
        assert!(
            PINNED_IROH_REVS.contains(&&pinned[0][..7]),
            "PINNED_IROH_REVS must list {}",
            &pinned[0][..7]
        );
    }
}
