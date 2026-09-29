//! 仅局域网模式下用户声明的可信网段。
//!
//! 可信网段决定哪些 IP 可以作为直连候选，本端发布与对端拨号必须共用这一份判定。
//! 网段只接受私有地址空间：IPv4 的 RFC1918 与 CGNAT，IPv6 的 ULA。公网段、
//! `0.0.0.0/0` 这类覆盖公网的网段以及链路本地、Clash fake-ip 都不在私有地址空间内，
//! 因此无法被声明为可信，不会成为绕过仅局域网承诺的入口。

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// 可被声明为可信的私有地址空间。
const TRUSTABLE_V4_BLOCKS: [(Ipv4Addr, u8); 4] = [
    (Ipv4Addr::new(10, 0, 0, 0), 8),
    (Ipv4Addr::new(172, 16, 0, 0), 12),
    (Ipv4Addr::new(192, 168, 0, 0), 16),
    (Ipv4Addr::new(100, 64, 0, 0), 10),
];
const TRUSTABLE_V6_BLOCK: (Ipv6Addr, u8) = (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7);

/// 单条 CIDR 网段。地址部分已按前缀清零，展示形式即规范形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IpNetwork {
    network: IpAddr,
    prefix: u8,
}

/// 单条网段不能成为可信网段的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TrustedNetworkRejection {
    #[error("not a valid CIDR network")]
    InvalidFormat,
    #[error("network is outside private address space")]
    NotPrivate,
    #[error("network is listed more than once")]
    Duplicate,
}

/// 可信网段列表中某一条目被拒绝。错误只携带位置与原因，不携带网段内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("trusted network entry {index}: {rejection}")]
pub struct TrustedNetworkEntryError {
    pub index: usize,
    pub rejection: TrustedNetworkRejection,
}

/// 已校验的可信网段集合。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedNetworks {
    networks: Vec<IpNetwork>,
}

/// 从持久条目宽松解析的结果；被跳过的条目只以数量报告。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LenientTrustedNetworks {
    pub networks: TrustedNetworks,
    pub rejected_count: usize,
}

impl IpNetwork {
    /// 解析 `地址/前缀`，并要求整个网段位于私有地址空间内。
    pub fn parse(raw: &str) -> Result<Self, TrustedNetworkRejection> {
        let (addr, prefix) = raw
            .trim()
            .split_once('/')
            .ok_or(TrustedNetworkRejection::InvalidFormat)?;
        let addr: IpAddr = addr
            .parse()
            // 用户输入的纯格式校验：解析错误只说明格式不合法，分类已完整表达。
            .map_err(|_| TrustedNetworkRejection::InvalidFormat)?;
        let prefix: u8 = prefix
            .parse()
            // 用户输入的纯格式校验：前缀不是 0-255 的整数即格式不合法。
            .map_err(|_| TrustedNetworkRejection::InvalidFormat)?;
        let max_prefix = match addr {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix > max_prefix {
            return Err(TrustedNetworkRejection::InvalidFormat);
        }
        let network = Self {
            network: mask(addr, prefix),
            prefix,
        };
        if !network.is_private() {
            return Err(TrustedNetworkRejection::NotPrivate);
        }
        Ok(network)
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {
                mask(ip, self.prefix) == self.network
            }
            _ => false,
        }
    }

    fn is_private(&self) -> bool {
        match self.network {
            IpAddr::V4(_) => TRUSTABLE_V4_BLOCKS
                .iter()
                .any(|(block, prefix)| self.within(IpAddr::V4(*block), *prefix)),
            IpAddr::V6(_) => {
                let (block, prefix) = TRUSTABLE_V6_BLOCK;
                self.within(IpAddr::V6(block), prefix)
            }
        }
    }

    fn within(&self, block: IpAddr, block_prefix: u8) -> bool {
        self.prefix >= block_prefix && mask(self.network, block_prefix) == block
    }
}

impl fmt::Display for IpNetwork {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.network, self.prefix)
    }
}

impl TrustedNetworks {
    /// 严格校验持久条目，用于保存设置。任何一条无效都拒绝整份列表。
    pub fn parse<S: AsRef<str>>(entries: &[S]) -> Result<Self, TrustedNetworkEntryError> {
        let mut networks: Vec<IpNetwork> = Vec::with_capacity(entries.len());
        for (index, raw) in entries.iter().enumerate() {
            let network = IpNetwork::parse(raw.as_ref())
                .map_err(|rejection| TrustedNetworkEntryError { index, rejection })?;
            if networks.contains(&network) {
                return Err(TrustedNetworkEntryError {
                    index,
                    rejection: TrustedNetworkRejection::Duplicate,
                });
            }
            networks.push(network);
        }
        Ok(Self { networks })
    }

    /// 宽松解析持久条目，用于网络启动。手工编辑造成的无效条目被跳过而不阻断启动。
    pub fn parse_lenient<S: AsRef<str>>(entries: &[S]) -> LenientTrustedNetworks {
        let mut result = LenientTrustedNetworks::default();
        for raw in entries {
            match IpNetwork::parse(raw.as_ref()) {
                Ok(network) if !result.networks.networks.contains(&network) => {
                    result.networks.networks.push(network);
                }
                Ok(_) => {}
                Err(_) => result.rejected_count += 1,
            }
        }
        result
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.networks.iter().any(|network| network.contains(ip))
    }

    pub fn len(&self) -> usize {
        self.networks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }
}

fn mask(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let bits = u32::from(v4);
            let masked = match prefix {
                0 => 0,
                p => bits & (u32::MAX << (32 - u32::from(p))),
            };
            IpAddr::V4(Ipv4Addr::from(masked))
        }
        IpAddr::V6(v6) => {
            let bits = u128::from(v6);
            let masked = match prefix {
                0 => 0,
                p => bits & (u128::MAX << (128 - u32::from(p))),
            };
            IpAddr::V6(Ipv6Addr::from(masked))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(raw: &str) -> IpAddr {
        raw.parse().unwrap()
    }

    #[test]
    fn private_networks_parse_to_canonical_form() {
        assert_eq!(
            IpNetwork::parse("10.8.0.7/24").unwrap().to_string(),
            "10.8.0.0/24"
        );
        assert_eq!(
            IpNetwork::parse(" 192.168.1.0/24 ").unwrap().to_string(),
            "192.168.1.0/24"
        );
        assert_eq!(
            IpNetwork::parse("172.16.0.0/12").unwrap().to_string(),
            "172.16.0.0/12"
        );
        assert_eq!(
            IpNetwork::parse("100.64.0.0/10").unwrap().to_string(),
            "100.64.0.0/10"
        );
        assert_eq!(
            IpNetwork::parse("fd7a:115c:a1e0::/48").unwrap().to_string(),
            "fd7a:115c:a1e0::/48"
        );
        assert_eq!(
            IpNetwork::parse("10.0.0.5/32").unwrap().to_string(),
            "10.0.0.5/32"
        );
    }

    #[test]
    fn public_catch_all_and_excluded_ranges_are_rejected() {
        for raw in [
            "0.0.0.0/0",
            "::/0",
            "8.8.8.0/24",
            "10.0.0.0/7",
            "2001:db8::/32",
            "169.254.0.0/16",
            "198.18.0.0/15",
            "fe80::/10",
            "100.0.0.0/8",
        ] {
            assert_eq!(
                IpNetwork::parse(raw),
                Err(TrustedNetworkRejection::NotPrivate),
                "{raw} must not be trustable"
            );
        }
    }

    #[test]
    fn malformed_entries_are_rejected() {
        for raw in [
            "",
            "10.0.0.0",
            "10.0.0.0/33",
            "fd00::/129",
            "10.0.0.0/x",
            "host/24",
        ] {
            assert_eq!(
                IpNetwork::parse(raw),
                Err(TrustedNetworkRejection::InvalidFormat),
                "{raw} must be invalid"
            );
        }
    }

    #[test]
    fn contains_matches_only_the_same_family_and_prefix() {
        let networks = TrustedNetworks::parse(&["10.8.0.0/24", "fd7a:115c:a1e0::/48"]).unwrap();
        assert!(networks.contains(ip("10.8.0.2")));
        assert!(!networks.contains(ip("10.8.1.2")));
        assert!(networks.contains(ip("fd7a:115c:a1e0:ab12::1")));
        assert!(!networks.contains(ip("fd7a:115c:a1e1::1")));
        assert!(!networks.contains(ip("::ffff:10.8.0.2")));
    }

    #[test]
    fn strict_parse_reports_position_without_contents() {
        let error = TrustedNetworks::parse(&["10.8.0.0/24", "8.8.8.0/24"]).unwrap_err();
        assert_eq!(
            error,
            TrustedNetworkEntryError {
                index: 1,
                rejection: TrustedNetworkRejection::NotPrivate
            }
        );
        assert!(!error.to_string().contains("8.8.8"));

        let duplicate = TrustedNetworks::parse(&["10.8.0.0/24", "10.8.0.9/24"]).unwrap_err();
        assert_eq!(duplicate.rejection, TrustedNetworkRejection::Duplicate);
    }

    #[test]
    fn lenient_parse_skips_invalid_entries_and_counts_them() {
        let parsed =
            TrustedNetworks::parse_lenient(&["10.8.0.0/24", "0.0.0.0/0", "garbage", "10.8.0.1/24"]);
        assert_eq!(parsed.networks.len(), 1);
        assert_eq!(parsed.rejected_count, 2);
    }
}
