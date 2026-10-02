//! 产品内置 relay 默认值与 relay 路由决策。
//!
//! 内置列表是随版本发布的产品默认值，由本模块唯一持有：它不写入用户设置，
//! 也不依赖 iroh 的 `RelayMode::Default` 隐含列表。升级 Engine 即更新内置
//! 列表，用户自定义列表与仅局域网开关不受影响。

/// 一条产品内置 relay。`region` 是稳定的区域标识，展示名称由宿主本地化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinRelay {
    pub region: &'static str,
    pub url: &'static str,
}

/// 内置 relay 列表，URL 使用规范化形式（与自定义列表的规范化结果可直接比较）。
pub const BUILTIN_RELAYS: [BuiltinRelay; 4] = [
    BuiltinRelay {
        region: "na-east",
        url: "https://use1-1.relay.n0.iroh.link./",
    },
    BuiltinRelay {
        region: "na-west",
        url: "https://usw1-1.relay.n0.iroh.link./",
    },
    BuiltinRelay {
        region: "eu",
        url: "https://euc1-1.relay.n0.iroh.link./",
    },
    BuiltinRelay {
        region: "asia-pacific",
        url: "https://aps1-1.relay.n0.iroh.link./",
    },
];

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

    /// 该路由方式下节点实际使用的 relay URL；禁用时为空。自定义列表按原样返回。
    pub fn effective_urls(self, custom_relay_urls: &[String]) -> Vec<String> {
        match self {
            Self::Disabled => Vec::new(),
            Self::Custom => custom_relay_urls.to_vec(),
            Self::BuiltIn => BUILTIN_RELAYS
                .iter()
                .map(|relay| relay.url.to_owned())
                .collect(),
        }
    }
}
