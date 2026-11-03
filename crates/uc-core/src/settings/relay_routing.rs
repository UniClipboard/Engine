//! 产品内置 relay 默认值与 relay 路由决策。
//!
//! 内置列表是随版本发布的产品默认值，由本模块唯一持有：它不写入用户设置，
//! 也不依赖 iroh 的 `RelayMode::Default` 隐含列表。此条目只是 Cloud 目录不可用时的
//! 初始候选；有效 Cloud 目录可以替换或移除它，连接仍须先取得匿名凭据。

/// 一条产品内置 relay。`region` 是稳定的区域标识，展示名称由宿主本地化。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinRelay {
    pub id: &'static str,
    pub region: &'static str,
    pub url: &'static str,
}

/// 内置 relay 列表，URL 使用规范化形式（与自定义列表的规范化结果可直接比较）。
pub const BUILTIN_RELAYS: [BuiltinRelay; 1] = [BuiltinRelay {
    id: "cn-shanghai-1",
    region: "cn-shanghai",
    url: "https://sha1.uc-relay.z2blog.com/",
}];

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
