//! iroh 地址候选过滤规则的唯一事实来源。
//!
//! **为什么需要这个模块**：虚拟网卡地址（Clash fake-ip 198.18/15、IPv4
//! link-local 169.254/16、CGNAT/Tailscale overlay 100.64/10 与
//! fd7a:115c:a1e0::/48）在三条路径上都得用完全相同的判定 ——
//!
//! 1. `node.rs::build_addr_filter` 决定本端 endpoint 发布哪些地址；
//! 2. `invitation_adapter::serialize_filtered_endpoint_ticket` 决定哪些
//!    地址进 sponsor ticket；
//! 3. `dev pairing addrs / issue --addr` 列出候选给开发者诊断。
//!
//! 任何一条路径独立维护过滤规则都会产生分叉 —— UniClipboard#486 的 Fedora
//! 配对超时就是 sponsor ticket 序列化了一个被 endpoint AddrFilter 滤掉的
//! 地址。把判定集中在这里，让上述三处共享同一份代码，从根上消除分叉的可能。
//!
//! LAN-only Mode 的出站拨号范围（[`is_local_network_ip`] 与
//! [`restrict_to_local_network`]）也在这里定义：存储线索、邀请路由与地址发现结果
//! 进入 iroh 前统一按同一份规则收敛，不在各条拨号路径上各写一份。

use std::borrow::Cow;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use uc_observability_contract::uc_debug;

use iroh::{EndpointAddr, TransportAddr};

/// 判断某个 IP 是否属于需要过滤的虚拟网卡候选。
pub(crate) fn is_virtual_nic_ip(ip: IpAddr, allow_overlay: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            if (o[0] == 198 && (o[1] & 0xfe) == 18) || (o[0] == 169 && o[1] == 254) {
                return true;
            }
            if !allow_overlay && o[0] == 100 && (o[1] & 0xc0) == 64 {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            let segs = v6.segments();
            if !allow_overlay && segs[0] == 0xfd7a && segs[1] == 0x115c && segs[2] == 0xa1e0 {
                return true;
            }
            false
        }
    }
}

fn should_filter_transport_addr(addr: &TransportAddr, allow_overlay: bool) -> bool {
    match addr {
        TransportAddr::Ip(socket) => is_virtual_nic_ip(socket.ip(), allow_overlay),
        _ => false,
    }
}

fn log_dropped_addrs(dropped: &[String], allow_overlay: bool) {
    if dropped.is_empty() {
        return;
    }
    uc_debug!(
        target: "iroh.addr_filter",
        allow_overlay = allow_overlay,
        dropped_count = dropped.len(),
        "filtered virtual-NIC addresses from candidate set",
    );
}

/// 过滤 iroh `AddrFilter` 收到的本机待发布候选地址集合。
///
/// 只移除虚拟网卡候选：Clash fake-ip 198.18/15、IPv4 link-local
/// 169.254/16，以及 `allow_overlay=false` 时的 CGNAT 100.64/10 与
/// Tailscale ULA fd7a:115c:a1e0::/48，详见 `is_virtual_nic_ip`。公网、私网、
/// IPv6 与 relay 候选原样保留，路径选择交给 iroh；发布阶段没有对端上下文，
/// 不能据此猜测可达性。过滤器不持有网络状态，切网后的新地址直接按同一规则
/// 处理。
///
/// 签名形态 `&Vec<TransportAddr> -> Cow<Vec<TransportAddr>>` 由
/// `iroh::address_lookup::AddrFilter::new` 的回调契约决定 —— **不要**
/// clippy-fix 成 `&[TransportAddr]`, 否则会和 iroh 的 `Fn` 签名失配。
pub(crate) fn apply_addr_filter(
    addrs: &Vec<TransportAddr>,
    allow_overlay: bool,
) -> Cow<'_, Vec<TransportAddr>> {
    let dropped: Vec<String> = addrs
        .iter()
        .filter_map(|addr| match addr {
            TransportAddr::Ip(socket) if is_virtual_nic_ip(socket.ip(), allow_overlay) => {
                Some(socket.to_string())
            }
            _ => None,
        })
        .collect();
    if dropped.is_empty() {
        return Cow::Borrowed(addrs);
    }

    log_dropped_addrs(&dropped, allow_overlay);
    Cow::Owned(
        addrs
            .iter()
            .filter(|addr| !should_filter_transport_addr(addr, allow_overlay))
            .cloned()
            .collect(),
    )
}

/// 过滤完整的 `EndpointAddr`，用于把本端地址写进可交给远端拨号的 ticket。
pub(crate) fn filter_endpoint_addr(addr: EndpointAddr, allow_overlay: bool) -> EndpointAddr {
    let EndpointAddr { id, addrs } = addr;
    let mut kept = Vec::new();
    let mut dropped = Vec::new();

    for addr in addrs {
        if should_filter_transport_addr(&addr, allow_overlay) {
            if let TransportAddr::Ip(socket) = &addr {
                dropped.push(socket.to_string());
            }
        } else {
            kept.push(addr);
        }
    }

    log_dropped_addrs(&dropped, allow_overlay);
    EndpointAddr::from_parts(id, kept)
}

/// 判断 IP 是否只在本地网络内路由，即 LAN-only Mode 允许出站拨号的范围。
///
/// 允许：IPv4 私网（10/8、172.16/12、192.168/16）、环回、link-local 169.254/16、
/// CGNAT 100.64/10（Tailscale 等 overlay 隧道），IPv6 环回、ULA fc00::/7（含
/// Tailscale fd7a:115c:a1e0::/48）与 link-local fe80::/10；IPv4-mapped IPv6 按其
/// IPv4 地址判定。公网地址（含 relay 开启时经 QAD 观察到的反射地址与公网网卡地址）、
/// Clash fake-ip 198.18/15（流量会进入代理而离开局域网）、组播、广播与未指定地址
/// 一律不允许。
///
/// 这是拨号侧判定，与 [`is_virtual_nic_ip`] 的发布侧过滤互相独立：169.254/16 不在
/// 本端发布，但它只在链路内可达，出现在对端线索中时不会让流量离开局域网；overlay
/// 地址是否发布由 `allow_overlay` 决定，对端发布了才会出现在线索中，拨号侧不再按该
/// 开关二次过滤。
pub(crate) fn is_local_network_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_local_network_ipv4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_local_network_ipv4(v4),
            None => is_local_network_ipv6(v6),
        },
    }
}

fn is_local_network_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    let cgnat = o[0] == 100 && (o[1] & 0xc0) == 64;
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || cgnat
}

fn is_local_network_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
}

/// LAN-only Mode 的拨号候选：只保留 [`is_local_network_ip`] 范围内的直连地址，
/// 去掉 relay 与其他传输地址。身份保持不变；结果为空时由 iroh 的地址发现补充
/// （同样经过本函数），都没有时拨号自然失败。
pub(crate) fn restrict_to_local_network(addr: EndpointAddr) -> EndpointAddr {
    let EndpointAddr { id, addrs } = addr;
    let mut dropped = 0usize;
    let kept: Vec<TransportAddr> = addrs
        .into_iter()
        .filter(|addr| {
            let keep =
                matches!(addr, TransportAddr::Ip(socket) if is_local_network_ip(socket.ip()));
            dropped += usize::from(!keep);
            keep
        })
        .collect();
    if dropped > 0 {
        uc_debug!(
            target: "iroh.addr_filter",
            dropped_count = dropped,
            "dropped non-local dial candidates in LAN-only mode",
        );
    }
    EndpointAddr::from_parts(id, kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::net::{SocketAddr, SocketAddrV4, SocketAddrV6};

    fn ip(addr: &str) -> TransportAddr {
        TransportAddr::Ip(addr.parse::<SocketAddr>().expect("socket address"))
    }

    #[test]
    fn local_network_ranges_are_dialable_in_lan_only_mode() {
        for local in [
            "10.8.0.2",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.1.5",
            "127.0.0.1",
            "169.254.10.20",
            "100.64.0.1",
            "100.127.255.254",
            "::1",
            "fd7a:115c:a1e0::1",
            "fc00::1",
            "fe80::1",
            "::ffff:192.168.1.5",
        ] {
            let parsed: IpAddr = local.parse().expect("ip");
            assert!(is_local_network_ip(parsed), "{local} must stay dialable");
        }
    }

    #[test]
    fn public_and_proxy_ranges_are_not_dialable_in_lan_only_mode() {
        for remote in [
            "1.2.3.4",
            "8.8.8.8",
            "172.32.0.1",
            "100.128.0.1",
            "198.18.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.251",
            "2001:db8::1",
            "2606:4700::1111",
            "fec0::1",
            "ff02::fb",
            "::",
            "::ffff:1.2.3.4",
        ] {
            let parsed: IpAddr = remote.parse().expect("ip");
            assert!(!is_local_network_ip(parsed), "{remote} must not be dialed");
        }
    }

    #[test]
    fn restricting_to_local_network_keeps_private_and_tunnel_hints_only() {
        let id = iroh::SecretKey::from_bytes(&[0x42; 32]).public();
        let relay = TransportAddr::Relay("https://relay.example.com/".parse().expect("relay"));
        let wireguard = ip("10.8.0.2:51820");
        let lan = TransportAddr::Ip(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::new(192, 168, 1, 5),
            50754,
        )));
        let tailscale = ip("[fd7a:115c:a1e0::5]:41641");
        let link_local = TransportAddr::Ip(SocketAddr::V6(SocketAddrV6::new(
            "fe80::5".parse().expect("ipv6"),
            41641,
            0,
            2,
        )));
        let public_v4 = ip("1.2.3.4:50754");
        let public_v6 = ip("[2606:4700::1111]:50754");

        let restricted = restrict_to_local_network(EndpointAddr::from_parts(
            id,
            [
                relay,
                public_v4,
                wireguard.clone(),
                public_v6,
                lan.clone(),
                tailscale.clone(),
                link_local.clone(),
            ],
        ));

        assert_eq!(
            restricted,
            EndpointAddr::from_parts(id, [wireguard, lan, tailscale, link_local])
        );
    }
}
