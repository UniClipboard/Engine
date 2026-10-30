//! mDNS 使用的本机网络接口快照：配对发现与长期设备发现共用同一接口策略。
//!
//! 发布端和解析端必须使用同一组 IPv4 发送接口。只把地址写进 mDNS 记录并
//! 不会让 `swarm-discovery` 从对应接口发送数据；多网卡主机必须显式提供
//! 组播接口（配对用 `with_multicast_interfaces_v4`，长期发现用
//! `MdnsAddressLookupBuilder::multicast_interfaces_v4`）。没有组播能力的接口
//! （例如 WireGuard）只会在建立该接口的套接字时被跳过，不影响其他接口。
//!
//! 长期发现还在同一组接口上收发 IPv6 mDNS（`multicast_interfaces_v6`）：只选择带有合格
//! IPv4 地址的接口，不因为 IPv6 扩大接口范围。IPv6 组播按操作系统接口索引生效，每个接口
//! 只给出一个用于绑定发送套接字的地址。

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use uc_observability_contract::{error_source::io_error_kind, uc_warn};

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct MdnsInterfaceSnapshot {
    advertised_addrs: Vec<IpAddr>,
    multicast_v4: Vec<Ipv4Addr>,
    /// （接口索引，绑定地址），按接口索引排序。
    multicast_v6: Vec<(u32, Ipv6Addr)>,
}

impl MdnsInterfaceSnapshot {
    pub(crate) fn capture() -> Self {
        match if_addrs::get_if_addrs() {
            Ok(interfaces) => Self::from_interfaces(
                interfaces
                    .into_iter()
                    .map(|item| (item.name, item.index, item.addr.ip())),
            ),
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

    /// 长期发现的 IPv6 组播接口：（接口索引，绑定发送套接字的本机地址）。
    pub(crate) fn into_multicast_v6(self) -> Vec<(u32, Ipv6Addr)> {
        self.multicast_v6
    }

    /// 由（接口名，接口索引，地址）列表建立快照；同一接口的每个地址各占一项。
    fn from_interfaces(
        interfaces: impl IntoIterator<Item = (String, Option<u32>, IpAddr)>,
    ) -> Self {
        let eligible: Vec<(String, Option<u32>, IpAddr)> = interfaces
            .into_iter()
            .filter(|(_, _, ip)| !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast())
            .collect();
        let advertised_addrs: Vec<IpAddr> = eligible
            .iter()
            .map(|(_, _, ip)| *ip)
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

        // IPv6 只用于 IPv4 已选中的接口。每个接口绑定一个地址：优先 link-local，通常不随
        // 前缀或临时地址轮换而变化，避免反复重建套接字；没有 link-local 时取最小的
        // 合格地址。没有接口索引的地址无法按接口加入组播组。
        // Windows 的 IPv4 与 IPv6 索引可能不同，以同一适配器的名称关联资格，
        // 加入 IPv6 组播时仍使用 IPv6 地址条目的索引。名称只在内存中比较，不进入日志。
        let ipv4_interfaces: BTreeSet<&str> = eligible
            .iter()
            .filter_map(|(name, _, ip)| ip.is_ipv4().then_some(name.as_str()))
            .collect();
        let mut multicast_v6: BTreeMap<u32, Ipv6Addr> = BTreeMap::new();
        for (name, index, ip) in &eligible {
            let (Some(index), IpAddr::V6(ip)) = (index, ip) else {
                continue;
            };
            if *index == 0 || !ipv4_interfaces.contains(name.as_str()) {
                continue;
            }
            let preference = |addr: &Ipv6Addr| (!addr.is_unicast_link_local(), *addr);
            multicast_v6
                .entry(*index)
                .and_modify(|bound| {
                    if preference(ip) < preference(bound) {
                        *bound = *ip;
                    }
                })
                .or_insert(*ip);
        }

        Self {
            advertised_addrs,
            multicast_v4,
            multicast_v6: multicast_v6.into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl MdnsInterfaceSnapshot {
        fn from_addrs(addrs: impl IntoIterator<Item = IpAddr>) -> Self {
            Self::from_interfaces(addrs.into_iter().map(|ip| (String::new(), None, ip)))
        }
    }

    fn interface(index: u32, addr: &str) -> (String, Option<u32>, IpAddr) {
        (
            format!("nic{index}"),
            Some(index),
            addr.parse().expect("interface address"),
        )
    }

    /// IPv6 只用于带合格 IPv4 地址的接口，每个接口一项并优先绑定 link-local；回环接口、
    /// 只有 IPv6 的接口与没有接口索引的地址都不参与。
    #[test]
    fn ipv6_interfaces_follow_the_ipv4_interface_policy_and_prefer_link_local() {
        let snapshot = MdnsInterfaceSnapshot::from_interfaces([
            interface(1, "127.0.0.1"),
            interface(1, "::1"),
            interface(2, "192.168.50.21"),
            interface(2, "2001:db8::21"),
            interface(2, "fe80::21"),
            interface(2, "fd00::21"),
            interface(3, "fd00::3"),
            interface(3, "fe80::3"),
            (
                "unindexed".to_string(),
                None,
                "10.172.6.2".parse().expect("unindexed IPv4"),
            ),
            (
                "unindexed".to_string(),
                None,
                "fe80::9".parse().expect("unindexed IPv6"),
            ),
        ]);

        assert_eq!(
            snapshot.into_multicast_v6(),
            vec![(2, "fe80::21".parse::<Ipv6Addr>().expect("link-local"))]
        );
    }

    /// 同一适配器的两个地址族可以具有不同索引；资格按名称关联，组播使用 IPv6 索引。
    #[test]
    fn ipv6_interface_uses_its_family_index_when_ipv4_index_differs() {
        let snapshot = MdnsInterfaceSnapshot::from_interfaces([
            (
                "adapter".to_string(),
                Some(2),
                "192.168.1.2".parse().unwrap(),
            ),
            ("adapter".to_string(), Some(12), "fe80::2".parse().unwrap()),
            ("other".to_string(), Some(2), "fe80::3".parse().unwrap()),
            ("adapter".to_string(), Some(0), "fe80::4".parse().unwrap()),
        ]);
        assert_eq!(
            snapshot.into_multicast_v6(),
            vec![(12, "fe80::2".parse().unwrap())]
        );
    }

    /// 没有 link-local 的接口绑定最小的合格地址。
    #[test]
    fn ipv6_interface_without_link_local_binds_its_lowest_eligible_address() {
        let snapshot = MdnsInterfaceSnapshot::from_interfaces([
            interface(4, "10.1.0.2"),
            interface(4, "fd00::20"),
            interface(4, "2001:db8::4"),
            interface(4, "ff02::fb"),
        ]);

        assert_eq!(
            snapshot.into_multicast_v6(),
            vec![(4, "2001:db8::4".parse::<Ipv6Addr>().expect("global"))]
        );
    }

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
