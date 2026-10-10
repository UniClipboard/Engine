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

use std::borrow::Cow;
use std::net::IpAddr;
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
