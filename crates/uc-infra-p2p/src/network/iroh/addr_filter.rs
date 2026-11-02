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

#[cfg(feature = "network-test-probe")]
use super::testing_network_probe::subnets::{self as subnet_probe, SubnetStage};
use anyhow::Error;
use arc_swap::ArcSwapOption;
use if_addrs::IfAddr;
use std::borrow::Cow;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio::task::{spawn_blocking, JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;
use uc_observability_contract::{error_source::io_error_kind, uc_debug};

use iroh::endpoint::EndpointHooks;
use iroh::{EndpointAddr, TransportAddr};

const SUBNET_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
const SUBNET_MAX_AGE: Duration = Duration::from_secs(30);
const MAX_LOCAL_SUBNETS: usize = 256;

struct SubnetSnapshot {
    subnets: Vec<LocalSubnet>,
    valid_until: Instant,
}

#[derive(Default)]
struct SubnetControl {
    generation: u64,
    stopped: bool,
}

struct SubnetView {
    #[cfg(feature = "network-test-probe")]
    probe_alias: u64,
    current: ArcSwapOption<SubnetSnapshot>,
    control: Mutex<SubnetControl>,
    changed: Notify,
    cancel: CancellationToken,
}

impl SubnetView {
    /// 已知网络变化先撤销旧许可。此锁仅保护发布，不覆盖 I/O 或 await。
    fn invalidate(&self) -> Option<u64> {
        let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        self.current.store(None);
        if control.stopped {
            return None;
        }
        control.generation = control.generation.wrapping_add(1);
        #[cfg(feature = "network-test-probe")]
        subnet_probe::invalidated(self.probe_alias, control.generation);
        Some(control.generation)
    }

    fn stop(&self) {
        let mut control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        control.stopped = true;
        self.current.store(None);
        self.cancel.cancel();
        #[cfg(feature = "network-test-probe")]
        subnet_probe::stopped(self.probe_alias, control.generation);
    }

    async fn refresh(&self) -> Result<(), JoinError> {
        let generation = {
            let control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
            if control.stopped {
                return Ok(());
            }
            control.generation
        };
        // 普通续期不撤销有效视图；已知变化已先清空，慢读取也不能延长旧许可。
        let read_started = Instant::now();
        #[cfg(feature = "network-test-probe")]
        let probe_alias = self.probe_alias;
        #[cfg(feature = "network-test-probe")]
        subnet_probe::read_scheduled(probe_alias, generation, read_started + SUBNET_MAX_AGE);
        let read = spawn_blocking(move || {
            #[cfg(feature = "network-test-probe")]
            let fault = subnet_probe::before_native(probe_alias, generation);
            let result = read_local_subnets();
            #[cfg(feature = "network-test-probe")]
            let result = subnet_probe::after_native(
                probe_alias,
                generation,
                fault,
                result,
                |subnets, ip| (subnets.len(), in_local_subnet(ip, subnets)),
            );
            result
        })
        .await;
        let mut subnets = match read {
            Ok(Ok(subnets)) => subnets,
            Ok(Err(source)) => {
                #[cfg(feature = "network-test-probe")]
                subnet_probe::observe(
                    probe_alias,
                    generation,
                    SubnetStage::ReadFailed,
                    Some(false),
                );
                report_subnet_read_failure(&source);
                Vec::new()
            }
            Err(source) => {
                #[cfg(feature = "network-test-probe")]
                subnet_probe::observe(
                    probe_alias,
                    generation,
                    SubnetStage::ReadJoinFailed,
                    Some(false),
                );
                self.stop();
                return Err(source);
            }
        };
        subnets.sort_unstable();
        subnets.dedup();
        let snapshot = (subnets.len() <= MAX_LOCAL_SUBNETS).then(|| {
            Arc::new(SubnetSnapshot {
                subnets,
                valid_until: read_started + SUBNET_MAX_AGE,
            })
        });
        let control = self.control.lock().unwrap_or_else(PoisonError::into_inner);
        if !control.stopped && control.generation == generation {
            #[cfg(feature = "network-test-probe")]
            let observed = snapshot.clone();
            self.current.store(snapshot);
            #[cfg(feature = "network-test-probe")]
            subnet_probe::published(
                self.probe_alias,
                generation,
                observed.as_ref().map_or(0, |view| view.subnets.len()),
                observed.as_ref().map(|view| view.valid_until),
                |ip| {
                    observed
                        .as_ref()
                        .is_some_and(|view| in_local_subnet(ip, &view.subnets))
                },
            );
        } else {
            #[cfg(feature = "network-test-probe")]
            subnet_probe::observe(
                self.probe_alias,
                generation,
                if control.stopped {
                    SubnetStage::StoppedDiscarded
                } else {
                    SubnetStage::StaleDiscarded
                },
                None,
            );
        }
        Ok(())
    }

    fn contains(&self, ip: IpAddr) -> bool {
        let snapshot = self.current.load();
        snapshot.as_ref().is_some_and(|snapshot| {
            Instant::now() < snapshot.valid_until && in_local_subnet(ip, &snapshot.subnets)
        })
    }
}

/// LAN-only 的最终发送边界。发送轮询只读取不可变视图，不枚举接口或等待写锁。
pub(super) struct LocalNetworkSendPolicy {
    view: Arc<SubnetView>,
}

impl fmt::Debug for LocalNetworkSendPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LocalNetworkSendPolicy")
    }
}

impl EndpointHooks for LocalNetworkSendPolicy {
    fn allow_transport_send(&self, remote: &TransportAddr) -> bool {
        permits_local_transport(remote, |ip| self.view.contains(ip))
    }

    fn on_network_change(&self) {
        self.view.invalidate();
        self.view.changed.notify_one();
    }
}

/// 节点拥有唯一刷新任务；任务和 hook 均不持有 Endpoint。
pub(super) struct LocalSubnetRuntime {
    view: Arc<SubnetView>,
    task: Option<JoinHandle<Result<(), JoinError>>>,
}

impl LocalSubnetRuntime {
    /// 先交出默认拒绝动态子网的 hook；endpoint 的原生监视器启动后才读取第一份视图。
    pub(super) fn prepare() -> Self {
        Self {
            view: Arc::new(SubnetView {
                #[cfg(feature = "network-test-probe")]
                probe_alias: subnet_probe::owner_created(),
                current: ArcSwapOption::empty(),
                control: Mutex::new(SubnetControl::default()),
                changed: Notify::new(),
                cancel: CancellationToken::new(),
            }),
            task: None,
        }
    }

    pub(super) async fn start(&mut self) -> Result<(), JoinError> {
        self.view.refresh().await?;
        let running = Arc::clone(&self.view);
        let task = tokio::spawn(async move {
            let result = async {
                loop {
                    tokio::select! {
                        biased;
                        _ = running.cancel.cancelled() => return Ok(()),
                        _ = running.changed.notified() => {},
                        _ = tokio::time::sleep(SUBNET_REFRESH_INTERVAL) => {},
                    }
                    running.refresh().await?;
                }
            }
            .await;
            #[cfg(feature = "network-test-probe")]
            subnet_probe::worker_finished(running.probe_alias, result.is_ok());
            result
        });
        self.task = Some(task);
        Ok(())
    }

    pub(super) fn policy(&self) -> LocalNetworkSendPolicy {
        LocalNetworkSendPolicy {
            view: Arc::clone(&self.view),
        }
    }

    pub(super) async fn shutdown(&mut self) -> Result<(), Error> {
        self.view.stop();
        if let Some(task) = self.task.take() {
            let result = task.await;
            #[cfg(feature = "network-test-probe")]
            subnet_probe::worker_joined(
                self.view.probe_alias,
                result.as_ref().is_ok_and(Result::is_ok),
            );
            result??;
        }
        Ok(())
    }
}

impl Drop for LocalSubnetRuntime {
    fn drop(&mut self) {
        // 已开始的阻塞读取由任务继续等待；停止标记禁止迟到结果发布，也不再开始新读取。
        self.view.stop();
    }
}

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

/// 本机接口当前配置的一个子网：接口地址与前缀长度。
type LocalSubnet = (IpAddr, u8);

/// 接口地址所在的子网只按前缀长度判定：Windows 上 if-addrs 的 `netmask` 可能取自主机
/// 前缀（例如 /128），与表示链路前缀的 `prefixlen`（例如 /64）不一致。
fn local_subnet(addr: IfAddr) -> LocalSubnet {
    let prefix = match &addr {
        IfAddr::V4(v4) => v4.prefixlen,
        IfAddr::V6(v6) => v6.prefixlen,
    };
    (addr.ip(), prefix)
}

fn read_local_subnets() -> io::Result<Vec<LocalSubnet>> {
    if_addrs::get_if_addrs().map(|interfaces| {
        interfaces
            .into_iter()
            .map(|interface| local_subnet(interface.addr))
            .collect()
    })
}

fn report_subnet_read_failure(error: &io::Error) {
    uc_debug!(
        target: "iroh.addr_filter",
        error_kind = "address_enumeration",
        io_error_kind = io_error_kind(error),
        "local subnets unavailable; LAN-only dialing uses fixed ranges only",
    );
}

/// 读取本机接口当前配置的子网；读取失败时只按固定范围判定。
fn local_subnets() -> Vec<LocalSubnet> {
    match read_local_subnets() {
        Ok(subnets) => subnets,
        Err(error) => {
            report_subnet_read_failure(&error);
            Vec::new()
        }
    }
}

/// 地址是否落在本机某个接口当前所在的子网内（例如没有 ULA 的 IPv6 局域网中对端的
/// 全局地址）。Clash fake-ip 198.18/15 即使有本机接口也不算本地网络。前缀长度只接受
/// IPv4 1..=32、IPv6 1..=128，其余（含 0）不匹配任何地址。
fn in_local_subnet(ip: IpAddr, subnets: &[LocalSubnet]) -> bool {
    let ip = ip.to_canonical();
    !is_virtual_nic_ip(ip, true)
        && subnets.iter().any(|&(net, prefix)| match (ip, net) {
            (IpAddr::V4(ip), IpAddr::V4(net)) if (1..=32).contains(&prefix) => {
                (u32::from(ip) ^ u32::from(net)) >> (32 - prefix) == 0
            }
            (IpAddr::V6(ip), IpAddr::V6(net)) if (1..=128).contains(&prefix) => {
                (u128::from(ip) ^ u128::from(net)) >> (128 - prefix) == 0
            }
            _ => false,
        })
}

/// LAN-only Mode 的拨号候选：只保留 [`is_local_network_ip`] 范围内、或落在本机接口
/// 当前子网内的直连地址，去掉 relay 与其他传输地址。身份保持不变；结果为空时由
/// iroh 的地址发现补充（同样经过本函数），都没有时拨号自然失败。只有存在固定范围之外的
/// 直连候选时才读取本机接口，不缓存。
pub(crate) fn restrict_to_local_network(addr: EndpointAddr) -> EndpointAddr {
    restrict_to_local_network_within(addr, local_subnets)
}

fn permits_local_transport(
    addr: &TransportAddr,
    in_current_subnet: impl FnOnce(IpAddr) -> bool,
) -> bool {
    matches!(addr, TransportAddr::Ip(socket)
        if is_local_network_ip(socket.ip()) || in_current_subnet(socket.ip()))
}

fn restrict_to_local_network_within(
    addr: EndpointAddr,
    load_subnets: impl Fn() -> Vec<LocalSubnet>,
) -> EndpointAddr {
    let EndpointAddr { id, addrs } = addr;
    let mut dropped = 0usize;
    let mut subnets = None;
    let kept: Vec<TransportAddr> = addrs
        .into_iter()
        .filter(|addr| {
            let keep = permits_local_transport(addr, |ip| {
                in_local_subnet(ip, subnets.get_or_insert_with(&load_subnets))
            });
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

    /// 没有 ULA 的 IPv6 局域网：对端的全局地址落在本机接口子网内时保留，同族但不在
    /// 本机任何子网内的全局地址仍然丢弃。
    #[test]
    fn global_addresses_on_a_local_interface_subnet_stay_dialable() {
        let id = iroh::SecretKey::from_bytes(&[0x43; 32]).public();
        let subnets: Vec<LocalSubnet> = vec![
            ("2001:db8:1:2::10".parse().expect("ipv6"), 64),
            ("203.0.113.10".parse().expect("ipv4"), 24),
            ("198.18.0.1".parse().expect("fake-ip"), 15),
        ];
        let on_link_v6 = ip("[2001:db8:1:2::20]:50754");
        let on_link_v4 = ip("203.0.113.20:50754");

        let restricted = restrict_to_local_network_within(
            EndpointAddr::from_parts(
                id,
                [
                    on_link_v6.clone(),
                    ip("[2001:db8:1:3::20]:50754"),
                    on_link_v4.clone(),
                    ip("203.0.114.20:50754"),
                    ip("198.18.0.2:50754"),
                ],
            ),
            || subnets.clone(),
        );

        assert_eq!(
            restricted,
            EndpointAddr::from_parts(id, [on_link_v6, on_link_v4])
        );
    }

    #[test]
    fn local_interfaces_are_read_only_for_candidates_outside_the_fixed_ranges() {
        let id = iroh::SecretKey::from_bytes(&[0x44; 32]).public();
        let reads = std::cell::Cell::new(0);
        let load = || {
            reads.set(reads.get() + 1);
            Vec::new()
        };
        let fixed = EndpointAddr::from_parts(id, [ip("10.8.0.2:51820"), ip("[fe80::1]:1")]);
        assert_eq!(restrict_to_local_network_within(fixed.clone(), load), fixed);
        assert_eq!(reads.get(), 0);

        let public = [ip("203.0.113.7:1"), ip("[2001:db8::1]:1")];
        restrict_to_local_network_within(EndpointAddr::from_parts(id, public), load);
        assert_eq!(reads.get(), 1, "read once per filter call");
    }

    /// 子网只由前缀长度构造：Windows 上报告的 `netmask` 可能是主机前缀 /128。
    #[test]
    fn the_subnet_mask_comes_from_the_prefix_length() {
        let windows = local_subnet(IfAddr::V6(if_addrs::Ifv6Addr {
            ip: "2001:db8:1:2::10".parse().expect("ipv6"),
            netmask: Ipv6Addr::from(u128::MAX),
            prefixlen: 64,
            broadcast: None,
        }));
        let on_link: IpAddr = "2001:db8:1:2::20".parse().expect("ipv6");
        assert!(in_local_subnet(on_link, &[windows]));

        for (net, prefix, inside, outside) in [
            ("203.0.113.10", 24, Some("203.0.113.200"), "203.0.114.1"),
            ("203.0.113.10", 32, Some("203.0.113.10"), "203.0.113.11"),
            ("203.0.113.10", 0, None, "203.0.113.11"),
            ("203.0.113.10", 33, None, "203.0.113.10"),
            ("2001:db8::1", 128, Some("2001:db8::1"), "2001:db8::2"),
            ("2001:db8::1", 48, Some("2001:db8:0:ffff::1"), "2001:db9::1"),
            ("2001:db8::1", 129, None, "2001:db8::1"),
        ] {
            let subnet = [(net.parse().expect("net"), prefix)];
            if let Some(inside) = inside {
                assert!(
                    in_local_subnet(inside.parse().expect("ip"), &subnet),
                    "{inside}/{prefix}"
                );
            }
            assert!(
                !in_local_subnet(outside.parse().expect("ip"), &subnet),
                "{outside}/{prefix}"
            );
        }
    }
}
