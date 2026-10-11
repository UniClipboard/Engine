//! Single owner of "given an iroh `EndpointId`, what connection path is this
//! peer on right now?".
//!
//! [`path_for`] is the one place that probes `endpoint.remote_info(id)` and
//! turns the snapshot into a peer-level [`ConnectionPath`] verdict (direct /
//! relay / unknown / offline) for the device list. [`connection_channel`]
//! classifies one `Connection` by its selected path and is what clipboard
//! delivery reports for a single send or receive.
//!
//! The classification step [`derive_path_from_addrs`] is a pure function over
//! [`KnownAddr`] values, a minimal view of iroh's `TransportAddrInfo` (whose
//! fields only iroh can construct), kept private so the probe can't be
//! bypassed; the truth-table is unit-testable without standing up an
//! endpoint.

use std::net::IpAddr;

use iroh::{endpoint::Connection, Endpoint, EndpointId, TransportAddr};

use uc_core::ports::connection_channel::{ConnectionChannel, ConnectionPath};

/// Probe `endpoint` for the peer's current [`ConnectionPath`].
///
/// Snapshots `remote_info(id)` once and classifies it via
/// [`derive_path_from_addrs`]. No `RemoteInfo` means magicsock has never
/// observed this peer, so it is reported as `Offline`. This is the peer-level
/// summary for the device list; it must not be used as evidence of which path
/// a single connection used — see [`connection_channel`].
pub(crate) async fn path_for(endpoint: &Endpoint, id: EndpointId) -> ConnectionPath {
    match endpoint.remote_info(id).await {
        Some(info) => derive_path_from_addrs(info.addrs().map(KnownAddr::from_iroh)),
        None => ConnectionPath {
            channel: ConnectionChannel::Offline,
            address: None,
        },
    }
}

/// Classify the path the given connection has selected right now.
///
/// `Active` addresses in `remote_info` only prove a path is open, not that
/// this connection sends over it, so content delivery reads the connection's
/// own selected path instead. No selected path (or an already closed
/// connection) is `Unknown`. The result is a snapshot at call time and does
/// not prove the path stayed unchanged for the whole exchange.
pub(crate) fn connection_channel(connection: &Connection) -> ConnectionChannel {
    let paths = connection.paths();
    match paths.iter().find(|path| path.is_selected()) {
        Some(path) if path.is_relay() => ConnectionChannel::Relay,
        Some(path) if path.is_ip() => match path.remote_addr() {
            TransportAddr::Ip(socket) if is_filtered_ip(socket.ip()) => ConnectionChannel::Unknown,
            _ => ConnectionChannel::Direct,
        },
        _ => ConnectionChannel::Unknown,
    }
}

/// One address of a peer plus whether iroh reports it as `Active`.
struct KnownAddr<'a> {
    addr: &'a TransportAddr,
    active: bool,
}

impl<'a> KnownAddr<'a> {
    fn from_iroh(info: &'a iroh::endpoint::TransportAddrInfo) -> Self {
        Self {
            addr: info.addr(),
            active: matches!(info.usage(), iroh::endpoint::TransportAddrUsage::Active),
        }
    }
}

/// Derive the active [`ConnectionPath`] from a peer's `remote_info` address
/// snapshot.
///
/// `Active` only means a path is open, not that traffic uses it, so when a
/// direct and a relay path are both `Active` the summary is `Unknown`. Having
/// a `RemoteInfo` but no `Active` path ⇒ `Unknown` (mid-handshake / probing);
/// nothing known ⇒ `Offline`.
fn derive_path_from_addrs<'a, I>(addrs: I) -> ConnectionPath
where
    I: IntoIterator<Item = KnownAddr<'a>>,
{
    let mut saw_any = false;
    let mut active_direct: Option<String> = None;
    let mut active_relay: Option<String> = None;

    for a in addrs {
        saw_any = true;
        match (a.active, a.addr) {
            (true, TransportAddr::Ip(s)) if !is_filtered_ip(s.ip()) && active_direct.is_none() => {
                active_direct = Some(s.to_string());
            }
            (true, TransportAddr::Relay(u)) if active_relay.is_none() => {
                active_relay = Some(u.to_string());
            }
            // Inactive / discovery candidates: do not promote, but keep
            // `saw_any = true` so the empty-set tail returns Offline only
            // when literally nothing is known.
            _ => {}
        }
    }

    if active_direct.is_some() && active_relay.is_some() {
        // Direct 与 Relay 同时打开不能证明流量走哪条，保守汇报 Unknown。
        ConnectionPath {
            channel: ConnectionChannel::Unknown,
            address: None,
        }
    } else if let Some(address) = active_direct {
        ConnectionPath {
            channel: ConnectionChannel::Direct,
            address: Some(address),
        }
    } else if let Some(address) = active_relay {
        ConnectionPath {
            channel: ConnectionChannel::Relay,
            address: Some(address),
        }
    } else if saw_any {
        // 有 RemoteInfo 但没有 Active 路径 ⇒ 还在握手 / probe
        ConnectionPath {
            channel: ConnectionChannel::Unknown,
            address: None,
        }
    } else {
        ConnectionPath {
            channel: ConnectionChannel::Offline,
            address: None,
        }
    }
}

/// 仅过滤不适合对用户展示为可用直连的地址。Tailscale / overlay 地址保留,
/// 这样设备列表能显示实际走的 100.x / fd7a:: 路径。**仅在 channel 判定处
/// 过滤**,不影响 outbound dial 候选(那个是 `node.rs` `AddrFilter` 的职责)。
fn is_filtered_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            // 与 node.rs::is_virtual_nic_ip 同步的 IPv4 假段(Clash /
            // link-local) —— UI 不应当把这些 path 当成可用直连:
            // Clash 198.18.0.0/15 是劫持 fake-ip, 169.254/16 link-local
            // 仅本机有意义。Tailscale 100.64.0.0/10 是真实 overlay 路径,
            // 应当作为 Direct 展示。
            (o[0] == 198 && (o[1] & 0xfe) == 18) || (o[0] == 169 && o[1] == 254)
        }
        IpAddr::V6(v6) => {
            let segs = v6.octets();
            // fe80::/10 link-local
            segs[0] == 0xfe && (segs[1] & 0xc0) == 0x80
        }
    }
}

#[cfg(test)]
mod tests {
    //! `is_filtered_ip` 是纯函数,直接喂合成数据覆盖全 truth-table。
    //! `derive_path_from_addrs` 的真实 semantics 由 bootstrap + Phase 96
    //! e2e 验收覆盖 —— `TransportAddrInfo` 是借用 iroh 内部类型的 borrowed
    //! view,直接构造困难,不在这里重做。

    use super::*;
    use std::net::Ipv4Addr;
    use std::net::Ipv6Addr;

    fn ip(text: &str) -> TransportAddr {
        TransportAddr::Ip(text.parse().unwrap())
    }

    fn relay() -> TransportAddr {
        TransportAddr::Relay("https://relay.example.invalid".parse().unwrap())
    }

    fn derive(addrs: &[(&TransportAddr, bool)]) -> ConnectionPath {
        derive_path_from_addrs(
            addrs
                .iter()
                .map(|&(addr, active)| KnownAddr { addr, active }),
        )
    }

    #[test]
    fn peer_summary_is_unknown_when_direct_and_relay_are_both_active() {
        let (direct, relay) = (ip("203.0.113.10:4242"), relay());
        let path = derive(&[(&direct, true), (&relay, true)]);
        assert_eq!(path.channel, ConnectionChannel::Unknown);
        assert_eq!(path.address, None);
    }

    #[test]
    fn peer_summary_reports_the_single_active_path() {
        let (direct, relay) = (ip("203.0.113.10:4242"), relay());
        let only_direct = derive(&[(&direct, true), (&relay, false)]);
        assert_eq!(only_direct.channel, ConnectionChannel::Direct);
        assert_eq!(only_direct.address.as_deref(), Some("203.0.113.10:4242"));
        let only_relay = derive(&[(&direct, false), (&relay, true)]);
        assert_eq!(only_relay.channel, ConnectionChannel::Relay);
    }

    #[test]
    fn peer_summary_ignores_unusable_direct_and_handles_empty_or_inactive() {
        let (fake_ip, relay) = (ip("198.18.0.1:4242"), relay());
        // 不可用的直连地址不参与汇总，不会把单独的 Relay 变成 Unknown。
        let path = derive(&[(&fake_ip, true), (&relay, true)]);
        assert_eq!(path.channel, ConnectionChannel::Relay);
        assert_eq!(
            derive(&[(&relay, false)]).channel,
            ConnectionChannel::Unknown
        );
        assert_eq!(derive(&[]).channel, ConnectionChannel::Offline);
    }

    #[test]
    fn ipv4_filter_truth_table() {
        // Real LAN — 不过滤
        assert!(!is_filtered_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10))));
        assert!(!is_filtered_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))));
        assert!(!is_filtered_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        // Clash fake-ip 198.18.0.0/15
        assert!(is_filtered_ip(IpAddr::V4(Ipv4Addr::new(198, 18, 0, 1))));
        assert!(is_filtered_ip(IpAddr::V4(Ipv4Addr::new(198, 19, 255, 254))));
        // CGNAT / Tailscale 100.64.0.0/10 是真实 IP 直连路径,不应过滤。
        assert!(!is_filtered_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(!is_filtered_ip(IpAddr::V4(Ipv4Addr::new(
            100, 127, 255, 254
        ))));
        // 100.63 / 100.128 不在 /10 内
        assert!(!is_filtered_ip(IpAddr::V4(Ipv4Addr::new(100, 63, 0, 1))));
        assert!(!is_filtered_ip(IpAddr::V4(Ipv4Addr::new(100, 128, 0, 1))));
        // link-local 169.254/16
        assert!(is_filtered_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1))));
    }

    #[test]
    fn ipv6_filter_covers_ula_and_link_local() {
        // fe80::/10 link-local
        assert!(is_filtered_ip(IpAddr::V6(Ipv6Addr::new(
            0xfe80, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_filtered_ip(IpAddr::V6(Ipv6Addr::new(
            0xfebf, 0, 0, 0, 0, 0, 0, 1
        ))));
        // fec0 不属于 fe80::/10(高 10 bit 是 0xfec, 不等于 0xfe80..0xfebf)
        assert!(!is_filtered_ip(IpAddr::V6(Ipv6Addr::new(
            0xfec0, 0, 0, 0, 0, 0, 0, 1
        ))));
        // fc00::/7 ULA(fc00..fdff) 可能是 Tailscale 等真实 overlay 直连路径。
        assert!(!is_filtered_ip(IpAddr::V6(Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(!is_filtered_ip(IpAddr::V6(Ipv6Addr::new(
            0xfd99, 0, 0, 0, 0, 0, 0, 1
        ))));
        // fe00 不属于 fc00::/7
        assert!(!is_filtered_ip(IpAddr::V6(Ipv6Addr::new(
            0xfe00, 0, 0, 0, 0, 0, 0, 1
        ))));
        // 普通全球可路由 IPv6 — 不过滤
        assert!(!is_filtered_ip(IpAddr::V6(Ipv6Addr::new(
            0x2001, 0xdb8, 0, 0, 0, 0, 0, 1
        ))));
    }
}
