//! mDNS 使用的本机网络接口快照：配对发现与长期设备发现共用同一接口策略。
//!
//! 发布端和解析端必须使用同一组 IPv4 发送接口。只把地址写进 mDNS 记录并
//! 不会让 `swarm-discovery` 从对应接口发送数据；多网卡主机必须显式提供
//! 组播接口（配对用 `with_multicast_interfaces_v4`，长期发现用
//! `MdnsAddressLookupBuilder::multicast_interfaces_v4`）。没有组播能力的接口
//! （例如 WireGuard）只会在建立该接口的套接字时被跳过，不影响其他接口。

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};

use uc_observability_contract::{error_source::io_error_kind, uc_warn};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MdnsInterfaceSnapshot {
    advertised_addrs: Vec<IpAddr>,
    multicast_v4: Vec<Ipv4Addr>,
}

impl MdnsInterfaceSnapshot {
    pub(crate) fn capture() -> Self {
        match if_addrs::get_if_addrs() {
            Ok(interfaces) => Self::from_addrs(interfaces.into_iter().map(|item| item.addr.ip())),
            Err(error) => {
                uc_warn!(
                    error_kind = "address_enumeration",
                    io_error_kind = io_error_kind(&error),
                    "if-addrs enumerate failed; mDNS will use the default interface"
                );
                Self::default()
            }
        }
    }

    pub(crate) fn into_parts(self) -> (Vec<IpAddr>, Vec<Ipv4Addr>) {
        (self.advertised_addrs, self.multicast_v4)
    }

    fn from_addrs(addrs: impl IntoIterator<Item = IpAddr>) -> Self {
        let advertised_addrs: Vec<IpAddr> = addrs
            .into_iter()
            .filter(|ip| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let multicast_v4 = advertised_addrs
            .iter()
            .filter_map(|ip| match ip {
                IpAddr::V4(ip) => Some(*ip),
                IpAddr::V6(_) => None,
            })
            .collect();

        Self {
            advertised_addrs,
            multicast_v4,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multihomed_snapshot_keeps_default_and_direct_ethernet_ipv4() {
        let snapshot = MdnsInterfaceSnapshot::from_addrs([
            "192.168.50.21".parse().expect("default network address"),
            "10.172.6.2".parse().expect("direct ethernet address"),
            "10.172.6.2".parse().expect("duplicate ethernet address"),
            "::1".parse().expect("IPv6 loopback"),
        ]);

        assert_eq!(
            snapshot.multicast_v4,
            vec![
                "10.172.6.2"
                    .parse::<Ipv4Addr>()
                    .expect("direct ethernet address"),
                "192.168.50.21"
                    .parse::<Ipv4Addr>()
                    .expect("default network address"),
            ]
        );
        assert_eq!(snapshot.advertised_addrs.len(), 2);
    }

    #[test]
    fn snapshot_excludes_addresses_that_cannot_be_interface_sources() {
        let snapshot = MdnsInterfaceSnapshot::from_addrs([
            "0.0.0.0".parse().expect("unspecified IPv4"),
            "127.0.0.1".parse().expect("IPv4 loopback"),
            "224.0.0.251".parse().expect("IPv4 multicast"),
            "10.172.6.2".parse().expect("direct ethernet address"),
            "2001:db8::1".parse().expect("IPv6 address"),
        ]);

        assert_eq!(
            snapshot.multicast_v4,
            vec!["10.172.6.2"
                .parse::<Ipv4Addr>()
                .expect("direct ethernet address")]
        );
        assert_eq!(snapshot.advertised_addrs.len(), 2);
    }
}
