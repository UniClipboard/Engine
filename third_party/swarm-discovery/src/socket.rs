use crate::IpClass;
use hickory_proto::op::Message;
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::{HashMap, HashSet},
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    sync::{Arc, Mutex, PoisonError, RwLock},
};
use thiserror::Error;
use tokio::net::UdpSocket;

pub const MDNS_PORT: u16 = 5353;
pub const MDNS_IPV4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
pub const MDNS_IPV6: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 0xfb);

#[derive(Debug)]
#[doc(hidden)]
pub enum IP {
    Ipv4,
    Ipv6,
}

impl std::fmt::Display for IP {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IP::Ipv4 => write!(f, "IPv4"),
            IP::Ipv6 => write!(f, "IPv6"),
        }
    }
}

/// Errors that can occur when creating and configuring an IPv4 or IPv6 socket.
#[derive(Debug, Error)]
pub enum SocketError {
    #[error("{domain}: error creating new socket for")]
    NewSocket {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("{domain}: error setting the reuse address")]
    ReuseAddress {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[cfg(unix)]
    #[error("{domain}: error setting the reuse port")]
    ReusePort {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("{domain}: error binding the socket for")]
    Bind {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("{domain}: error setting multicast loop")]
    SetMulticastLoop {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("{domain}: error joining multicast")]
    JoinMulticast {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("{domain}: error setting the multicast ttl")]
    MulticastTtl {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("{domain}: error setting the multicast interface")]
    SetMulticastIf {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("{domain}: error setting the socket to non-blocking mode")]
    SetNonBlocking {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("{domain}: error creating a UDP socket from a standard socket")]
    UdpSocket {
        domain: IP,
        #[source]
        source: std::io::Error,
    },
    #[error("Cannot bind to IPv4 or IPv6")]
    CannotBind,
}

pub fn socket_v4(interface_addr: Option<Ipv4Addr>) -> Result<UdpSocket, SocketError> {
    // Make sure we bind to a specific interface if specified
    let bind_addr = match interface_addr {
        Some(addr) => SocketAddrV4::new(addr, MDNS_PORT).into(),
        None => SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, MDNS_PORT).into(),
    };

    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|source| {
        SocketError::NewSocket {
            domain: IP::Ipv4,
            source,
        }
    })?;
    socket
        .set_reuse_address(true)
        .map_err(|source| SocketError::ReuseAddress {
            domain: IP::Ipv4,
            source,
        })?;
    // Best-effort: some platforms (e.g. esp-idf/lwIP) declare SO_REUSEPORT but
    // leave it unimplemented (setsockopt returns ENOPROTOOPT). It is not needed
    // for a single process binding the mDNS port, and a genuine "port in use"
    // still surfaces at bind(), so tolerate a failure here instead of aborting.
    #[cfg(unix)]
    if let Err(e) = socket.set_reuse_port(true) {
        tracing::debug!("set_reuse_port (IPv4) failed, continuing: {e}");
    }
    socket
        .bind(&bind_addr)
        .map_err(|source| SocketError::Bind {
            domain: IP::Ipv4,
            source,
        })?;
    socket
        .set_multicast_loop_v4(true)
        .map_err(|source| SocketError::SetMulticastLoop {
            domain: IP::Ipv4,
            source,
        })?;

    // Join the multicast group once, on the interface of the default route.
    //
    // Group membership is per interface, so packets arriving on any other
    // interface are not delivered to this socket. IP_MULTICAST_ALL would
    // paper over this on Linux when another membership exists, but that
    // socket option is Linux-only; on macOS and other BSD-derived systems
    // reception from non-default interfaces does not work. The per-interface
    // sockets cannot take over reception either: they are bound to a unicast
    // address and therefore never match multicast-destined packets. Reception
    // on further interfaces is instead set up by
    // `Sockets::join_group_on_main_v4`, which joins the group per interface on
    // this main wildcard socket.
    socket
        .join_multicast_v4(&MDNS_IPV4, &interface_addr.unwrap_or(Ipv4Addr::UNSPECIFIED))
        .map_err(|source| SocketError::JoinMulticast {
            domain: IP::Ipv4,
            source,
        })?;

    // Pin multicast egress to the requested interface. Binding to the
    // interface address alone does not reliably select the outgoing
    // interface for multicast: the kernel falls back to the routing table,
    // which can pick a different interface or fail with "no route to host"
    // (for example for interfaces without a multicast route entry).
    if let Some(addr) = interface_addr {
        socket
            .set_multicast_if_v4(&addr)
            .map_err(|source| SocketError::SetMulticastIf {
                domain: IP::Ipv4,
                source,
            })?;
    }

    socket
        .set_multicast_ttl_v4(16)
        .map_err(|source| SocketError::MulticastTtl {
            domain: IP::Ipv4,
            source,
        })?;
    socket
        .set_nonblocking(true)
        .map_err(|source| SocketError::SetNonBlocking {
            domain: IP::Ipv4,
            source,
        })?;
    UdpSocket::from_std(std::net::UdpSocket::from(socket)).map_err(|source| {
        SocketError::UdpSocket {
            domain: IP::Ipv4,
            source,
        }
    })
}

pub fn socket_v6() -> Result<UdpSocket, SocketError> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)).map_err(|source| {
        SocketError::NewSocket {
            domain: IP::Ipv6,
            source,
        }
    })?;
    socket
        .set_reuse_address(true)
        .map_err(|source| SocketError::ReuseAddress {
            domain: IP::Ipv6,
            source,
        })?;
    // See the IPv4 path: set_reuse_port is best-effort (unimplemented on some
    // platforms, e.g. esp-idf/lwIP), so tolerate a failure here.
    #[cfg(unix)]
    if let Err(e) = socket.set_reuse_port(true) {
        tracing::debug!("set_reuse_port (IPv6) failed, continuing: {e}");
    }
    socket
        .bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, MDNS_PORT)).into())
        .map_err(|source| SocketError::Bind {
            domain: IP::Ipv6,
            source,
        })?;
    socket
        .set_multicast_loop_v6(true)
        .map_err(|source| SocketError::SetMulticastLoop {
            domain: IP::Ipv6,
            source,
        })?;

    // Join multicast on the default interface (interface index 0)
    socket
        .join_multicast_v6(&MDNS_IPV6, 0)
        .map_err(|source| SocketError::JoinMulticast {
            domain: IP::Ipv6,
            source,
        })?;

    socket
        .set_nonblocking(true)
        .map_err(|source| SocketError::SetNonBlocking {
            domain: IP::Ipv6,
            source,
        })?;
    UdpSocket::from_std(std::net::UdpSocket::from(socket)).map_err(|source| {
        SocketError::UdpSocket {
            domain: IP::Ipv6,
            source,
        }
    })
}

/// UniClipboard 补丁：为单个 IPv6 接口创建发送套接字。
///
/// IPv6 的组播发送接口按接口索引选择（`IPV6_MULTICAST_IF`），因此套接字以接口索引
/// 区分；`addr` 只决定绑定的源地址，使发送的报文源端口为 5353。绑定单播地址的套接字
/// 不会收到发往组播组的报文，接收仍由主通配套接字按接口加入组播组完成，不会重复接收。
/// link-local 地址只在其所属接口内有意义，绑定时带上接口索引作为 scope。
pub fn socket_v6_interface(index: u32, addr: Ipv6Addr) -> Result<UdpSocket, SocketError> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)).map_err(|source| {
        SocketError::NewSocket {
            domain: IP::Ipv6,
            source,
        }
    })?;
    socket
        .set_reuse_address(true)
        .map_err(|source| SocketError::ReuseAddress {
            domain: IP::Ipv6,
            source,
        })?;
    #[cfg(unix)]
    if let Err(e) = socket.set_reuse_port(true) {
        tracing::debug!("set_reuse_port (IPv6 interface) failed, continuing: {e}");
    }
    let scope_id = if addr.is_unicast_link_local() {
        index
    } else {
        0
    };
    socket
        .bind(&SocketAddr::V6(SocketAddrV6::new(addr, MDNS_PORT, 0, scope_id)).into())
        .map_err(|source| SocketError::Bind {
            domain: IP::Ipv6,
            source,
        })?;
    socket
        .set_multicast_loop_v6(true)
        .map_err(|source| SocketError::SetMulticastLoop {
            domain: IP::Ipv6,
            source,
        })?;
    socket
        .set_multicast_if_v6(index)
        .map_err(|source| SocketError::SetMulticastIf {
            domain: IP::Ipv6,
            source,
        })?;
    socket
        .set_nonblocking(true)
        .map_err(|source| SocketError::SetNonBlocking {
            domain: IP::Ipv6,
            source,
        })?;
    UdpSocket::from_std(std::net::UdpSocket::from(socket)).map_err(|source| {
        SocketError::UdpSocket {
            domain: IP::Ipv6,
            source,
        }
    })
}

/// UniClipboard 补丁：接口套接字创建与主通配套接字组播成员操作的测试观察点。
///
/// 生产构建中为零大小类型，不改变行为；单元测试用它注入创建或加入失败，以弱引用
/// 观察每个接口套接字是否已被所有持有者释放，并记录加入与退出组播组的顺序。
#[derive(Clone, Debug, Default)]
pub(crate) struct InterfaceProbe(#[cfg(test)] Option<Arc<probe::State>>);

impl InterfaceProbe {
    fn create(&self, addr: Ipv4Addr) -> Result<Arc<UdpSocket>, SocketError> {
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.before_create(addr, IP::Ipv4)?;
        }
        let socket = Arc::new(socket_v4(Some(addr))?);
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.created_socket(addr, &socket);
        }
        Ok(socket)
    }

    fn create_v6(&self, index: u32, addr: Ipv6Addr) -> Result<Arc<UdpSocket>, SocketError> {
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.before_create(probe::Key::V6(index), IP::Ipv6)?;
        }
        let socket = Arc::new(socket_v6_interface(index, addr)?);
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.created_socket(probe::Key::V6(index), &socket);
        }
        Ok(socket)
    }

    fn join(&self, socket: &UdpSocket, addr: Ipv4Addr) -> io::Result<()> {
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.before_join(addr)?;
        }
        let result = socket.join_multicast_v4(MDNS_IPV4, addr);
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.record(probe::Membership::Join, addr, &result);
        }
        result
    }

    fn leave(&self, socket: &UdpSocket, addr: Ipv4Addr) -> io::Result<()> {
        let result = socket.leave_multicast_v4(MDNS_IPV4, addr);
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.record(probe::Membership::Leave, addr, &result);
        }
        result
    }

    fn join_v6(&self, socket: &UdpSocket, index: u32) -> io::Result<()> {
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.before_join(probe::Key::V6(index))?;
        }
        let result = socket.join_multicast_v6(&MDNS_IPV6, index);
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.record(probe::Membership::Join, probe::Key::V6(index), &result);
        }
        result
    }

    fn leave_v6(&self, socket: &UdpSocket, index: u32) -> io::Result<()> {
        let result = socket.leave_multicast_v6(&MDNS_IPV6, index);
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.record(probe::Membership::Leave, probe::Key::V6(index), &result);
        }
        result
    }

    /// 记录一次在 IPv6 接口套接字上的发送尝试（无论成功与否）。
    fn sent_v6(&self, _index: u32) {
        #[cfg(test)]
        if let Some(state) = &self.0 {
            state.sent(probe::Key::V6(_index));
        }
    }
}

#[cfg(test)]
pub(crate) mod probe {
    use super::{InterfaceProbe, SocketError, IP};
    use std::{
        collections::HashMap,
        io,
        net::Ipv4Addr,
        sync::{Arc, Mutex, Weak},
    };
    use tokio::net::UdpSocket;

    /// 主通配套接字上的组播成员操作。
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Membership {
        Join,
        Leave,
    }

    /// 观察点记录的接口：IPv4 按地址，IPv6 按操作系统接口索引。
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub(crate) enum Key {
        V4(Ipv4Addr),
        V6(u32),
    }

    impl From<Ipv4Addr> for Key {
        fn from(addr: Ipv4Addr) -> Self {
            Self::V4(addr)
        }
    }

    impl PartialEq<Ipv4Addr> for Key {
        fn eq(&self, addr: &Ipv4Addr) -> bool {
            *self == Self::V4(*addr)
        }
    }

    #[derive(Debug, Default)]
    pub(crate) struct State {
        inner: Mutex<Inner>,
    }

    #[derive(Debug, Default)]
    struct Inner {
        failures_remaining: usize,
        attempts: Vec<Key>,
        created: Vec<(Key, Weak<UdpSocket>)>,
        join_failures: HashMap<Key, usize>,
        /// 按发生顺序记录的组播成员操作及其是否成功（含注入的失败）。
        memberships: Vec<(Membership, Key, bool)>,
        /// 在接口套接字上的发送尝试。
        sends: Vec<Key>,
    }

    impl State {
        pub(crate) fn probe(self: &Arc<Self>) -> InterfaceProbe {
            InterfaceProbe(Some(Arc::clone(self)))
        }

        /// 让接下来的 `count` 次接口套接字创建失败。
        pub(crate) fn fail_next(&self, count: usize) {
            self.inner.lock().unwrap().failures_remaining = count;
        }

        pub(crate) fn attempts(&self, key: impl Into<Key>) -> usize {
            let key = key.into();
            let inner = self.inner.lock().unwrap();
            inner.attempts.iter().filter(|a| **a == key).count()
        }

        pub(crate) fn created(&self, key: impl Into<Key>) -> usize {
            let key = key.into();
            let inner = self.inner.lock().unwrap();
            inner.created.iter().filter(|(a, _)| *a == key).count()
        }

        /// 仍被任意持有者（接口表、接收者或发送中的克隆）引用的接口套接字数量。
        pub(crate) fn live(&self, key: impl Into<Key>) -> usize {
            let key = key.into();
            let inner = self.inner.lock().unwrap();
            inner
                .created
                .iter()
                .filter(|(a, socket)| *a == key && socket.strong_count() > 0)
                .count()
        }

        /// 迄今在该接口套接字上的发送尝试次数。
        pub(crate) fn sends(&self, key: impl Into<Key>) -> usize {
            let key = key.into();
            let inner = self.inner.lock().unwrap();
            inner.sends.iter().filter(|a| **a == key).count()
        }

        pub(super) fn before_create(
            &self,
            key: impl Into<Key>,
            domain: IP,
        ) -> Result<(), SocketError> {
            let mut inner = self.inner.lock().unwrap();
            inner.attempts.push(key.into());
            if inner.failures_remaining > 0 {
                inner.failures_remaining -= 1;
                return Err(SocketError::NewSocket {
                    domain,
                    source: std::io::Error::other("injected interface socket failure"),
                });
            }
            Ok(())
        }

        pub(super) fn created_socket(&self, key: impl Into<Key>, socket: &Arc<UdpSocket>) {
            let mut inner = self.inner.lock().unwrap();
            inner.created.push((key.into(), Arc::downgrade(socket)));
        }

        /// 让该接口接下来的 `count` 次组播组加入失败。
        pub(crate) fn fail_next_joins(&self, key: impl Into<Key>, count: usize) {
            self.inner
                .lock()
                .unwrap()
                .join_failures
                .insert(key.into(), count);
        }

        /// 迄今记录的组播成员操作。
        pub(crate) fn memberships(&self) -> Vec<(Membership, Key, bool)> {
            self.inner.lock().unwrap().memberships.clone()
        }

        pub(super) fn before_join(&self, key: impl Into<Key>) -> io::Result<()> {
            let key = key.into();
            let mut inner = self.inner.lock().unwrap();
            let Some(remaining) = inner.join_failures.get_mut(&key).filter(|n| **n > 0) else {
                return Ok(());
            };
            *remaining -= 1;
            inner.memberships.push((Membership::Join, key, false));
            Err(io::Error::other("injected multicast join failure"))
        }

        pub(super) fn record(&self, op: Membership, key: impl Into<Key>, result: &io::Result<()>) {
            let ok = match result {
                Ok(()) => true,
                Err(e) => op == Membership::Join && e.kind() == io::ErrorKind::AddrInUse,
            };
            self.inner
                .lock()
                .unwrap()
                .memberships
                .push((op, key.into(), ok));
        }

        pub(super) fn sent(&self, key: Key) {
            self.inner.lock().unwrap().sends.push(key);
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sockets {
    v4: Option<Arc<UdpSocket>>,
    v6: Option<Arc<UdpSocket>>,
    interface_sockets_v4: Arc<RwLock<HashMap<Ipv4Addr, Arc<UdpSocket>>>>,
    /// UniClipboard 补丁：仍在接口表中、但主通配套接字尚未确认为其加入组播组的地址。
    ///
    /// 组播成员关系属于（套接字、组播组、操作系统接口），而不是接口表中的单个地址：
    /// 同一网卡上的两个地址共用一个成员关系，任一地址退出都会让该网卡离开组播组。
    /// 因此退出后要为其余地址重新加入；加入失败的地址记在这里，由 guardian 的安装重试补齐。
    unjoined_v4: Arc<Mutex<HashSet<Ipv4Addr>>>,
    /// UniClipboard 补丁：IPv6 接口表，按操作系统接口索引维护，值为发送套接字绑定的本机地址
    /// 与套接字。IPv6 的组播成员关系与发送接口都按接口索引生效，同一网卡上的多个地址共用
    /// 一个条目，不会出现 IPv4 那样由另一地址带走成员关系的情况。
    interface_sockets_v6: Arc<RwLock<HashMap<u32, (Ipv6Addr, Arc<UdpSocket>)>>>,
    /// UniClipboard 补丁：仍在 IPv6 接口表中、但主通配套接字尚未确认为其加入组播组的接口索引。
    unjoined_v6: Arc<Mutex<HashSet<u32>>>,
    probe: InterfaceProbe,
}

impl Sockets {
    pub(crate) fn new(
        class: IpClass,
        multicast_interfaces: Vec<Ipv4Addr>,
        probe: InterfaceProbe,
    ) -> Result<Self, SocketError> {
        // Create interface-specific sockets for multi-interface mode
        let mut interface_sockets_v4 = HashMap::new();
        for addr in &multicast_interfaces {
            match probe.create(*addr) {
                Ok(socket) => {
                    tracing::debug!("Created interface-specific socket for {}", addr);
                    interface_sockets_v4.insert(*addr, socket);
                }
                Err(e) => {
                    tracing::warn!("Failed to create interface socket for {}: {}", addr, e);
                }
            }
        }
        let interface_sockets_v4 = Arc::new(RwLock::new(interface_sockets_v4));

        let sockets = match class {
            IpClass::Auto => {
                let socket = Self {
                    v4: socket_v4(None).ok().map(Arc::new),
                    v6: socket_v6().ok().map(Arc::new),
                    interface_sockets_v4: interface_sockets_v4.clone(),
                    unjoined_v4: Arc::default(),
                    interface_sockets_v6: Arc::default(),
                    unjoined_v6: Arc::default(),
                    probe,
                };
                if socket.v4.is_none() && socket.v6.is_none() {
                    return Err(SocketError::CannotBind);
                }
                socket
            }
            _ => Self {
                v4: class
                    .has_v4()
                    .then(|| socket_v4(None).map(Arc::new))
                    .transpose()?,
                v6: class
                    .has_v6()
                    .then(|| socket_v6().map(Arc::new))
                    .transpose()?,
                interface_sockets_v4: interface_sockets_v4.clone(),
                unjoined_v4: Arc::default(),
                interface_sockets_v6: Arc::default(),
                unjoined_v6: Arc::default(),
                probe,
            },
        };
        for addr in sockets.get_all_interface_addresses_v4() {
            // 失败已记入 `unjoined_v4`；期望集合模式下由 guardian 重试。
            let _ = sockets.join_group_on_main_v4(addr);
        }
        Ok(sockets)
    }

    pub fn v4(&self) -> Option<Arc<UdpSocket>> {
        self.v4.as_ref().map(Arc::clone)
    }

    pub fn v6(&self) -> Option<Arc<UdpSocket>> {
        self.v6.as_ref().map(Arc::clone)
    }

    /// Joins the multicast group on the given interface on the main wildcard
    /// IPv4 socket.
    ///
    /// Group membership is per interface: the wildcard socket initially only
    /// joins the group on the interface of the default route, so multicast
    /// arriving on other interfaces is not delivered to it (except on
    /// platforms with `IP_MULTICAST_ALL` semantics and an existing
    /// system-wide membership). The per-interface sockets cannot take over
    /// reception either, because they are bound to a unicast address and
    /// therefore never match multicast-destined packets. Joining the group
    /// per interface on the wildcard socket makes reception work on all
    /// interfaces.
    ///
    /// UniClipboard 补丁：返回加入结果，并按结果维护 `unjoined_v4`。该接口已是成员
    /// （`AddrInUse`，例如创建套接字时的默认接口或同一网卡上的另一地址已加入）视为成功。
    fn join_group_on_main_v4(&self, addr: Ipv4Addr) -> io::Result<()> {
        let result = match &self.v4 {
            Some(v4) => match self.probe.join(v4, addr) {
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => Ok(()),
                result => result,
            },
            None => Ok(()),
        };
        let mut unjoined = self
            .unjoined_v4
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match &result {
            Ok(()) => {
                unjoined.remove(&addr);
            }
            Err(e) => {
                tracing::debug!("could not join multicast group on interface: {}", e);
                unjoined.insert(addr);
            }
        }
        result
    }

    /// Leaves the multicast group on the given interface on the main wildcard
    /// IPv4 socket. See [`Self::join_group_on_main_v4`].
    fn leave_group_on_main_v4(&self, addr: Ipv4Addr) {
        if let Some(v4) = &self.v4 {
            if let Err(e) = self.probe.leave(v4, addr) {
                tracing::debug!("could not leave multicast group on interface: {}", e);
            }
        }
    }

    /// UniClipboard 补丁：先退出再加入，使该地址所在的操作系统接口重新成为成员。
    ///
    /// 不依赖各平台对重复加入返回的错误码：接口仍是成员时，退出后立即重新加入；
    /// 成员关系已被同一网卡上的另一地址带走时，退出失败而加入恢复它。同一网卡上的
    /// 其他地址共用该成员关系，加入后同样恢复。
    fn rejoin_group_on_main_v4(&self, addr: Ipv4Addr) -> io::Result<()> {
        if let Some(v4) = &self.v4 {
            let _ = self.probe.leave(v4, addr);
        }
        self.join_group_on_main_v4(addr)
    }

    /// UniClipboard 补丁：确认接口地址在主通配套接字上的组播成员关系。尚未确认时重新
    /// 加入一次；失败表示接口尚未完整安装，guardian 按安装退避重试。
    pub(crate) fn ensure_group_on_main_v4(&self, addr: Ipv4Addr) -> Result<(), SocketError> {
        let unjoined = self
            .unjoined_v4
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&addr);
        if !unjoined {
            return Ok(());
        }
        self.rejoin_group_on_main_v4(addr)
            .map_err(|source| SocketError::JoinMulticast {
                domain: IP::Ipv4,
                source,
            })
    }

    /// Add a new IPv4 interface for multicast operations.
    /// Returns Ok(()) if the socket was successfully created and added.
    pub fn add_interface_v4(&self, addr: Ipv4Addr) -> Result<(), SocketError> {
        // Check if interface already exists
        if self
            .interface_sockets_v4
            .read()
            .unwrap()
            .contains_key(&addr)
        {
            return Ok(());
        }

        // Create the interface-specific socket for sending
        let socket = self.probe.create(addr)?;

        {
            let mut interfaces = self.interface_sockets_v4.write().unwrap();
            // need to recheck since we dropped the lock in between
            interfaces.entry(addr).or_insert_with(|| {
                tracing::info!("Added interface {} for multicast", addr);
                socket
            });
        }
        // 套接字已可用于发送；加入失败记入 `unjoined_v4`，由 `ensure_group_on_main_v4` 补齐。
        let _ = self.join_group_on_main_v4(addr);
        Ok(())
    }

    /// Remove an IPv4 interface from multicast operations.
    /// Returns true if the interface was found and removed.
    pub fn remove_interface_v4(&self, addr: Ipv4Addr) -> bool {
        let mut interfaces = self.interface_sockets_v4.write().unwrap();

        if interfaces.contains_key(&addr) {
            let socket = interfaces.remove(&addr);
            let retained: Vec<Ipv4Addr> = interfaces.keys().copied().collect();
            drop(interfaces);
            // drop socket outside the lock
            drop(socket);
            self.unjoined_v4
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&addr);
            self.leave_group_on_main_v4(addr);
            // UniClipboard 补丁：退出按操作系统接口生效，同一网卡上仍保留的地址也随之失去
            // 成员关系；逐个重新加入，失败的地址留在 `unjoined_v4` 等待安装重试。
            for retained in retained {
                let _ = self.rejoin_group_on_main_v4(retained);
            }
            tracing::info!("Removed interface {} from multicast", addr);

            true
        } else {
            false
        }
    }

    /// UniClipboard 补丁：在主通配 IPv6 套接字上为接口索引加入组播组，按结果维护
    /// `unjoined_v6`。该接口已是成员（`AddrInUse`，例如创建主套接字时按索引 0 加入的
    /// 默认接口）视为成功。
    fn join_group_on_main_v6(&self, index: u32) -> io::Result<()> {
        let result = match &self.v6 {
            Some(v6) => match self.probe.join_v6(v6, index) {
                Err(e) if e.kind() == io::ErrorKind::AddrInUse => Ok(()),
                result => result,
            },
            None => Ok(()),
        };
        let mut unjoined = self
            .unjoined_v6
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match &result {
            Ok(()) => {
                unjoined.remove(&index);
            }
            Err(e) => {
                tracing::debug!("could not join IPv6 multicast group on interface: {}", e);
                unjoined.insert(index);
            }
        }
        result
    }

    fn leave_group_on_main_v6(&self, index: u32) {
        if let Some(v6) = &self.v6 {
            if let Err(e) = self.probe.leave_v6(v6, index) {
                tracing::debug!("could not leave IPv6 multicast group on interface: {}", e);
            }
        }
    }

    /// UniClipboard 补丁：确认接口索引在主通配 IPv6 套接字上的组播成员关系。尚未确认时
    /// 与 IPv4 一致先退出再加入一次，不依赖各平台对重复加入返回的错误码；失败表示接口
    /// 尚未完整安装，guardian 按安装退避重试。
    pub(crate) fn ensure_group_on_main_v6(&self, index: u32) -> Result<(), SocketError> {
        let unjoined = self
            .unjoined_v6
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&index);
        if !unjoined {
            return Ok(());
        }
        if let Some(v6) = &self.v6 {
            let _ = self.probe.leave_v6(v6, index);
        }
        self.join_group_on_main_v6(index)
            .map_err(|source| SocketError::JoinMulticast {
                domain: IP::Ipv6,
                source,
            })
    }

    /// UniClipboard 补丁：是否存在主通配 IPv6 套接字。不存在时 IPv6 接口无从接收，
    /// guardian 不安装 IPv6 接口。
    pub(crate) fn has_v6(&self) -> bool {
        self.v6.is_some()
    }

    /// UniClipboard 补丁：为接口索引建立绑定到 `addr` 的发送套接字并加入组播组。
    ///
    /// 该索引已有套接字时直接返回；绑定地址变化由 guardian 先移除再重新安装。加入失败
    /// 记入 `unjoined_v6`，由 `ensure_group_on_main_v6` 补齐。
    pub(crate) fn add_interface_v6(&self, index: u32, addr: Ipv6Addr) -> Result<(), SocketError> {
        if self
            .interface_sockets_v6
            .read()
            .unwrap()
            .contains_key(&index)
        {
            return Ok(());
        }
        let socket = self.probe.create_v6(index, addr)?;
        self.interface_sockets_v6
            .write()
            .unwrap()
            .entry(index)
            .or_insert((addr, socket));
        let _ = self.join_group_on_main_v6(index);
        Ok(())
    }

    /// UniClipboard 补丁：释放接口索引的发送套接字并在主通配套接字上退出该接口的组播组。
    ///
    /// 成员关系按接口索引生效，其他接口不受影响，无需像 IPv4 那样为其余条目重新加入。
    /// 唯一的共用情况是主套接字创建时按索引 0 加入的默认接口：移除默认接口的索引后，
    /// 默认接口也不再接收，与 IPv4 移除默认接口地址时一致。
    pub(crate) fn remove_interface_v6(&self, index: u32) -> bool {
        let socket = self.interface_sockets_v6.write().unwrap().remove(&index);
        if socket.is_none() {
            return false;
        }
        drop(socket);
        self.unjoined_v6
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&index);
        self.leave_group_on_main_v6(index);
        true
    }

    /// UniClipboard 补丁：接口索引的发送套接字。
    pub(crate) fn get_interface_socket_v6(&self, index: u32) -> Option<Arc<UdpSocket>> {
        self.interface_sockets_v6
            .read()
            .unwrap()
            .get(&index)
            .map(|(_, socket)| Arc::clone(socket))
    }

    /// UniClipboard 补丁：接口索引的发送套接字当前绑定的本机地址。
    pub(crate) fn interface_addr_v6(&self, index: u32) -> Option<Ipv6Addr> {
        self.interface_sockets_v6
            .read()
            .unwrap()
            .get(&index)
            .map(|(addr, _)| *addr)
    }

    /// Get the socket for a specific IPv4 interface
    pub fn get_interface_socket_v4(&self, addr: Ipv4Addr) -> Option<Arc<UdpSocket>> {
        let interfaces = self.interface_sockets_v4.read().unwrap();
        match interfaces.get(&addr) {
            Some(sock) => Some(Arc::clone(sock)),
            None => None,
        }
    }

    /// Get all interface addresses that have sockets
    pub fn get_all_interface_addresses_v4(&self) -> Vec<Ipv4Addr> {
        let interfaces = self.interface_sockets_v4.read().unwrap();
        interfaces.keys().copied().collect()
    }

    pub async fn send_msg(&self, msg: &Message, mode: Mode) {
        let bytes = match msg.to_vec() {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("error serializing mDNS: {}", e);
                return;
            }
        };

        let has_interfaces_v4 = !self.interface_sockets_v4.read().unwrap().is_empty();
        let has_interfaces_v6 = !self.interface_sockets_v6.read().unwrap().is_empty();

        // UniClipboard 补丁：任一协议族有接口套接字时进入多接口模式，各协议族分别选择
        // 接口套接字或主套接字。只有 IPv4 接口时与上游行为一致：V4 走全部 IPv4 接口，
        // Any 另外经主 IPv6 套接字发送，V6 只经主 IPv6 套接字发送。
        if has_interfaces_v4 || has_interfaces_v6 {
            if matches!(mode, Mode::V4 | Mode::Any) {
                if has_interfaces_v4 {
                    tracing::debug!(
                        "Using multi-interface mode for IPv4 sending, {} interfaces available",
                        self.interface_sockets_v4.read().unwrap().len()
                    );
                    self.send_msg_multi_interface_v4(&bytes, msg).await;
                } else if let Some(v4) = &self.v4 {
                    Self::send_on_main(v4, IpAddr::from(MDNS_IPV4), &bytes, msg).await;
                }
            }
            if matches!(mode, Mode::V6 | Mode::Any) {
                if has_interfaces_v6 {
                    self.send_msg_multi_interface_v6(&bytes, msg).await;
                } else if let Some(v6) = &self.v6 {
                    Self::send_on_main(v6, IpAddr::from(MDNS_IPV6), &bytes, msg).await;
                }
            }
        } else {
            // Single interface mode or IPv6-only
            let (socket, addr) = match mode {
                Mode::V4 => (self.v4.as_ref().unwrap(), IpAddr::from(MDNS_IPV4)),
                Mode::V6 => (self.v6.as_ref().unwrap(), IpAddr::from(MDNS_IPV6)),
                Mode::Any => {
                    if let Some(v4) = &self.v4 {
                        (v4, IpAddr::from(MDNS_IPV4))
                    } else {
                        (self.v6.as_ref().unwrap(), IpAddr::from(MDNS_IPV6))
                    }
                }
            };
            Self::send_on_main(socket, addr, &bytes, msg).await;
        }
    }

    async fn send_on_main(socket: &UdpSocket, addr: IpAddr, bytes: &[u8], msg: &Message) {
        if let Err(e) = socket.send_to(bytes, (addr, MDNS_PORT)).await {
            tracing::warn!("error sending mDNS: {}", e);
        } else {
            tracing::debug!(
                q = msg.queries.len(),
                an = msg.answers.len(),
                ad = msg.additionals.len(),
                "sent {} bytes",
                bytes.len()
            );
        }
    }

    /// UniClipboard 补丁：在全部 IPv6 接口套接字上发送，目标地址带接口索引作为 scope。
    /// 单个接口发送失败（例如该接口没有组播路由）不影响其他接口；日志不含地址或接口索引。
    async fn send_msg_multi_interface_v6(&self, bytes: &[u8], msg: &Message) {
        let interfaces: Vec<(u32, Arc<UdpSocket>)> = self
            .interface_sockets_v6
            .read()
            .unwrap()
            .iter()
            .map(|(index, (_, socket))| (*index, Arc::clone(socket)))
            .collect();
        let mut sent_count = 0;
        for (index, socket) in interfaces {
            let target = SocketAddrV6::new(MDNS_IPV6, MDNS_PORT, 0, index);
            let result = socket.send_to(bytes, target).await;
            self.probe.sent_v6(index);
            match result {
                Err(e) => tracing::debug!("error sending mDNS on an IPv6 interface: {}", e),
                Ok(_) => {
                    tracing::debug!(
                        q = msg.queries.len(),
                        an = msg.answers.len(),
                        ad = msg.additionals.len(),
                        "sent {} bytes on IPv6 interface",
                        bytes.len()
                    );
                    sent_count += 1;
                }
            }
        }
        if sent_count == 0 {
            tracing::debug!("failed to send mDNS on any IPv6 interface in multi-interface mode");
        }
    }

    async fn send_msg_multi_interface_v4(&self, bytes: &[u8], msg: &Message) {
        let mut sent_count = 0;

        // Send on all IPv4 interface-specific sockets
        let interfaces = self.interface_sockets_v4.read().unwrap().clone();
        for (addr, socket) in interfaces.iter() {
            if let Err(e) = socket.send_to(bytes, (MDNS_IPV4, MDNS_PORT)).await {
                tracing::error!("error sending mDNS on interface {}: {}", addr, e);
            } else {
                tracing::debug!(
                    addr = %addr,
                    q = msg.queries.len(),
                    an = msg.answers.len(),
                    ad = msg.additionals.len(),
                    "sent {} bytes on interface",
                    bytes.len()
                );
                sent_count += 1;
            }
        }

        if sent_count == 0 {
            tracing::error!("failed to send mDNS on any IPv4 interface in multi-interface mode");
        }
    }
}

#[derive(Debug)]
pub enum Mode {
    V4,
    V6,
    Any,
}
