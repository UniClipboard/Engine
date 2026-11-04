//! 产品内置 relay 默认值与 relay 路由决策。
//!
//! 内置列表是随版本发布的产品默认值，由本模块唯一持有：它不写入用户设置，
//! 也不依赖 iroh 的 `RelayMode::Default` 隐含列表。
//!
//! 内置条目分为两类：公共 relay 直接可用，绑定后立即安装；产品自建 relay
//! 必须先向 Cloud 取得短期匿名凭据，由受管 worker 在取证成功后追加。

/// 内置 relay 的访问方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinRelayAccess {
    /// 上游公共 relay：直接配置即可连接，不经过产品控制面。
    Direct,
    /// 产品自建 relay：必须先从 Cloud 取得短期匿名凭据。
    AnonymousCredential,
}

/// 一条产品内置 relay。`region` 是稳定的区域标识，展示名称由宿主本地化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinRelay {
    pub id: &'static str,
    pub region: &'static str,
    pub url: &'static str,
    pub access: BuiltinRelayAccess,
}

/// 受管目录优先选择的稳定 relay ID。
pub const PREFERRED_MANAGED_RELAY_ID: &str = "cn-shanghai-1";

/// 内置 relay 列表，URL 使用规范化形式（与自定义列表的规范化结果可直接比较）。
pub const BUILTIN_RELAYS: [BuiltinRelay; 5] = [
    BuiltinRelay {
        id: "n0-na-east",
        region: "na-east",
        url: "https://use1-1.relay.n0.iroh.link./",
        access: BuiltinRelayAccess::Direct,
    },
    BuiltinRelay {
        id: "n0-na-west",
        region: "na-west",
        url: "https://usw1-1.relay.n0.iroh.link./",
        access: BuiltinRelayAccess::Direct,
    },
    BuiltinRelay {
        id: "n0-eu",
        region: "eu",
        url: "https://euc1-1.relay.n0.iroh.link./",
        access: BuiltinRelayAccess::Direct,
    },
    BuiltinRelay {
        id: "n0-asia-pacific",
        region: "asia-pacific",
        url: "https://aps1-1.relay.n0.iroh.link./",
        access: BuiltinRelayAccess::Direct,
    },
    BuiltinRelay {
        id: PREFERRED_MANAGED_RELAY_ID,
        region: "cn-shanghai",
        url: "https://sha1.uc-relay.z2blog.com/",
        access: BuiltinRelayAccess::AnonymousCredential,
    },
];

/// 可直接安装的内置 relay；无需凭据，绑定后立即生效。
pub fn direct_builtin_relays() -> impl Iterator<Item = &'static BuiltinRelay> {
    BUILTIN_RELAYS
        .iter()
        .filter(|relay| relay.access == BuiltinRelayAccess::Direct)
}

/// 需要匿名凭据的内置受管 relay 初始候选；有效 Cloud 目录可以替换或移除它们。
pub fn managed_builtin_relays() -> impl Iterator<Item = &'static BuiltinRelay> {
    BUILTIN_RELAYS
        .iter()
        .filter(|relay| relay.access == BuiltinRelayAccess::AnonymousCredential)
}

/// 设置决定的 relay 路由方式。优先级：禁用（仅局域网）> 自定义 > 内置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayRouting {
    BuiltIn,
    Custom,
    Disabled,
}

impl RelayRouting {
    pub fn resolve(allow_relay_fallback: bool, custom_relay_urls: &[String]) -> Self {
        if !allow_relay_fallback {
            Self::Disabled
        } else if custom_relay_urls.is_empty() {
            Self::BuiltIn
        } else {
            Self::Custom
        }
    }

    /// 该路由方式下可直接安装的 relay URL；禁用时为空。自定义列表按原样返回。
    ///
    /// 内置路由只包含公共 relay；产品自建 relay 由受管 worker 在取得匿名凭据后追加。
    pub fn effective_urls(self, custom_relay_urls: &[String]) -> Vec<String> {
        match self {
            Self::Disabled => Vec::new(),
            Self::Custom => custom_relay_urls.to_vec(),
            Self::BuiltIn => direct_builtin_relays()
                .map(|relay| relay.url.to_owned())
                .collect(),
        }
    }
}
