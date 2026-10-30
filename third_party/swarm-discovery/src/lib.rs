#![doc = include_str!("../README.md")]

mod guardian;
mod receiver;
mod sender;
mod socket;
mod updater;

use acto::{AcTokio, ActoHandle, ActoRef, ActoRuntime, SupervisionRef, TokioJoinHandle};
use hickory_proto::rr::Name;
use socket::{InterfaceProbe, SocketError, Sockets};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    str::FromStr,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::runtime::Handle;

type Callback = Box<dyn FnMut(&str, &Peer) + Send + 'static>;

pub(crate) type TxtData = BTreeMap<String, Option<String>>;

/// Errors that can occur when spawning a swarm discovery service.
#[derive(Debug, Error)]
pub enum SpawnError {
    #[error(transparent)]
    Sockets {
        #[from]
        source: SocketError,
    },
    #[error("Cannot construct service name from name '{name}' and protocol '{protocol}'")]
    ServiceName {
        #[source]
        source: hickory_proto::ProtoError,
        name: String,
        protocol: Protocol,
    },
    #[error("Cannot construct name from peer ID {peer_id}")]
    NameFromPeerId {
        #[source]
        source: hickory_proto::ProtoError,
        peer_id: String,
    },
    #[error("Cannot append service name '{service_name}' to peer ID")]
    AppendServiceName {
        #[source]
        source: hickory_proto::ProtoError,
        service_name: Name,
    },
}

/// Errors that can occur when validating a txt attribute.
#[derive(Debug, Error)]
pub enum TxtAttributeError {
    #[error("Key may not be empty")]
    EmptyKey,
    #[error("Key-value pair is too long, must be shorter than 254 bytes")]
    TooLong,
}

/// Builder for a swarm discovery service.
///
/// # Example
///
/// ```rust
/// use if_addrs::get_if_addrs;
/// use swarm_discovery::Discoverer;
/// use tokio::runtime::Builder;
///
/// // create Tokio runtime
/// let rt = Builder::new_multi_thread()
///     .enable_all()
///     .build()
///     .expect("build runtime");
///
/// // make up some peer ID
/// let peer_id = "peer_id42".to_owned();
///
/// // get local addresses and make up some port
/// let addrs = get_if_addrs().unwrap().into_iter().map(|i| i.addr.ip()).collect::<Vec<_>>();
/// let port = 1234;
///
/// // start announcing and discovering
/// let _guard = Discoverer::new("swarm".to_owned(), peer_id)
///     .with_addrs(port, addrs)
///     .with_callback(|peer_id, peer| {
///         println!("discovered {}: {:?}", peer_id, peer);
///     })
///     .spawn(rt.handle())
///     .expect("discoverer spawn");
/// ```
pub struct Discoverer {
    name: String,
    protocol: Protocol,
    peer_id: String,
    peers: BTreeMap<String, Peer>,
    callback: Callback,
    tau: Duration,
    phi: f32,
    class: IpClass,
    multicast_interfaces: Vec<Ipv4Addr>,
    interface_probe: InterfaceProbe,
}

/// A peer discovered by the swarm discovery service.
///
/// The discovery yields service instances, which are located by a port and a list of IP addresses.
/// Both IPv4 and IPv6 addresses may be present, depending on the configuration via [Discoverer::with_ip_class].
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Peer {
    addrs: Vec<(IpAddr, u16)>,
    last_seen: Instant,
    txt: TxtData,
}

impl Peer {
    /// Creates a new [`Peer`] with no addresses or TXT attributes.
    ///
    /// The last seen timestamp is set to the current time.
    pub(crate) fn new() -> Self {
        Peer {
            addrs: Default::default(),
            last_seen: Instant::now(),
            txt: Default::default(),
        }
    }

    /// Known addresses of this peer, or empty slice in case the peer has expired.
    pub fn addrs(&self) -> &[(IpAddr, u16)] {
        &self.addrs
    }

    /// Returns true if this peer has expired.
    pub fn is_expiry(&self) -> bool {
        self.addrs.is_empty()
    }

    /// Return the age of this peer snapshot.
    ///
    /// Note that observations performed after this Peer structure was handed to
    /// your code are not taken into account; this yields the age of this Peer
    /// snapshot.
    pub fn age(&self) -> Duration {
        self.last_seen.elapsed()
    }

    /// Returns an iterator of the TXT attributes set by the peer.
    ///
    /// See [`Discoverer::with_txt_attributes`] for details on the encoding of
    /// these attributes.
    pub fn txt_attributes(&self) -> impl Iterator<Item = (&str, Option<&str>)> + '_ {
        self.txt
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_ref().map(|v| v.as_str())))
    }

    /// Returns the value for a TXT attribute for this peer.
    ///
    /// Returns `None` if the attribute is missing.
    /// Returns `Some(None)` if the attribute is a boolean, i.e. has no value.
    /// Returns `Some(Some(value))` if the attribute has a value.
    ///
    /// See [`Discoverer::with_txt_attributes`] for details on the encoding of
    /// these attributes.
    pub fn txt_attribute(&self, name: &str) -> Option<Option<&str>> {
        self.txt.get(name).map(|x| x.as_deref())
    }
}

/// This selects which sockets will be created by the [Discoverer].
///
/// Responses will be sent on that socket which received the query.
/// Queries will prefer v4 when available.
/// Default is [IpClass::Auto], which means the socket will figure out what ip classes are available on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IpClass {
    /// Require the socket to bind to ipv4.
    V4Only,
    /// Require the socket to bind to ipv6.
    V6Only,
    /// Require the socket to bind to both ipv4 and ipv6.
    V4AndV6,
    /// Allow the socket to attempt to bind to both ipv4 and ipv6.
    ///
    /// Only error if the socket is unable to bind to either.
    #[default]
    Auto,
}

impl IpClass {
    /// Returns `true` if IPv4 is enabled.
    pub fn has_v4(self) -> bool {
        matches!(self, Self::V4Only | Self::V4AndV6)
    }

    /// Returns `true` if IPv6 is enabled.
    pub fn has_v6(self) -> bool {
        matches!(self, Self::V6Only | Self::V4AndV6)
    }
}

/// This selects which protocol suffix to use for the service name.
///
/// Default is [Protocol::Udp].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Protocol {
    #[default]
    Udp,
    Tcp,
}

impl Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Protocol::Udp => write!(f, "_udp"),
            Protocol::Tcp => write!(f, "_tcp"),
        }
    }
}

impl Discoverer {
    /// Creates a new builder for a swarm discovery service.
    ///
    /// The `name` is the name of the mDNS service, meaning that it will be discoverable under the name `_name._udp.local.`.
    /// The `peer_id` is the unique identifier of this peer, which will be discoverable under the name `peer_id._name._udp.local.`.
    pub fn new(name: String, peer_id: String) -> Self {
        Self {
            name,
            protocol: Protocol::default(),
            peer_id,
            peers: BTreeMap::new(),
            callback: Box::new(|_, _| {}),
            tau: Duration::from_secs(10),
            phi: 1.0,
            class: IpClass::default(),
            multicast_interfaces: Vec::new(),
            interface_probe: InterfaceProbe::default(),
        }
    }

    /// Creates a new builder with default cadence and response rate for human interactive applications.
    ///
    /// This sets τ=0.7sec and φ=2.5, see [Discoverer::new] for the `name` and `peer_id` arguments.
    pub fn new_interactive(name: String, peer_id: String) -> Self {
        Self::new(name, peer_id)
            .with_cadence(Duration::from_millis(700))
            .with_response_rate(2.5)
    }

    /// Set the protocol suffix to use for the service name.
    ///
    /// Note that this does not change the protocol used for discovery, which is always UDP-based mDNS.
    /// Default is [Protocol::Udp].
    pub fn with_protocol(mut self, protocol: Protocol) -> Self {
        self.protocol = protocol;
        self
    }

    /// Register the local peer's port and IP addresses, may be called multiple times with additive effect.
    ///
    /// If this method is not called, the local peer will not advertise itself.
    /// It can still discover others.
    pub fn with_addrs(mut self, port: u16, addrs: impl IntoIterator<Item = IpAddr>) -> Self {
        let me = self
            .peers
            .entry(self.peer_id.clone())
            .or_insert_with(Peer::new);
        me.addrs.extend(addrs.into_iter().map(|addr| (addr, port)));
        me.addrs.sort_unstable();
        me.addrs.dedup();
        self
    }

    /// Sets TXT attributes for this peer.
    ///
    /// This crate supports a single TXT record per peer, which contains a list
    /// of key-value pairs of UTF-8 strings. The value is optional: when missing,
    /// the attribute is a flag, simply identified as being present.
    ///
    /// The formatting of the TXT record follows [RFC 6763], with the following
    /// differences to the RFC:
    ///  * Keys and values are interpreted as UTF-8 strings (not only US-ASCII)
    ///  * Keys and values are case-sensitive (not case-insensitive)
    ///
    /// Key and value of each pair may not be longer than 254 bytes combined.
    /// Returns an error if the length is exceeded.
    ///
    /// The total length of all attributes is not checked here. You should make sure
    /// to keep the total length of all attributes at a few hundred bytes so that
    /// the resulting DNS packet does not exceed the UDP MTU.
    ///
    /// [RFC 6763]: https://datatracker.ietf.org/doc/html/rfc6763#section-6
    pub fn with_txt_attributes(
        mut self,
        attributes: impl IntoIterator<Item = (String, Option<String>)>,
    ) -> Result<Self, TxtAttributeError> {
        let me = self
            .peers
            .entry(self.peer_id.clone())
            .or_insert_with(Peer::new);
        for (key, value) in attributes.into_iter() {
            validate_txt_attribute(&key, value.as_deref())?;
            me.txt.insert(key, value);
        }
        Ok(self)
    }

    /// Register a callback to be called when a peer is discovered or its addresses change.
    ///
    /// When a peer is removed, the callback will be called with an empty list of addresses.
    /// This happens after not receiving any responses for a time period greater than three
    /// times the estimated swarm size divided by the response frequency.
    pub fn with_callback(mut self, callback: impl FnMut(&str, &Peer) + Send + 'static) -> Self {
        self.callback = Box::new(callback);
        self
    }

    /// Set the discovery time target.
    ///
    /// After roughly this time a new peer should have discovered some parts of the swarm.
    /// The worst-case latency is 1.2•τ.
    ///
    /// Note that the product τ•φ must be greater than 1 for the rate limiting to work correctly.
    /// For human interactive applications it is recommended to set τ=0.7s and φ=2.5 (see [Discoverer::new_interactive]).
    ///
    /// The default is 10 seconds.
    pub fn with_cadence(mut self, tau: Duration) -> Self {
        self.tau = tau;
        self
    }

    /// Set the response frequency target in Hz.
    ///
    /// While query-response cycles follow the configured cadence (see [Discoverer::with_cadence]),
    /// the response rate determines the (soft) maximum of how many responses should be received per second.
    ///
    /// With cadence 10sec, setting this to 1.0Hz means that at most 10 responses will be received per cycle.
    /// Setting it to 0.5Hz means that up to roughly 5 responses will be received per cycle.
    ///
    /// Note that the product τ•φ must be greater than 1 for the rate limiting to work correctly.
    /// For human interactive applications it is recommended to set τ=0.7s and φ=2.5 (see [Discoverer::new_interactive]).
    ///
    /// The default is 1.0Hz.
    pub fn with_response_rate(mut self, phi: f32) -> Self {
        self.phi = phi;
        self
    }

    /// Set which IP classes to use.
    ///
    /// The default is to use both IPv4 and IPv6, where IPv4 is preferred for sending queries.
    /// Responses will be sent using that class which the query used.
    pub fn with_ip_class(mut self, class: IpClass) -> Self {
        self.class = class;
        self
    }

    /// Set which IPv4 addresses to use for sending multicast messages.
    ///
    /// By default (empty vector), multicast messages are sent only on the default interface.
    /// Provide a list of local IPv4 addresses to send multicast messages on specific interfaces.
    ///
    /// This improves discovery in multi-homed environments where peers may be on different
    /// network segments. Note that this only affects IPv4; IPv6 multicast uses the default
    /// interface unless [`DropGuard::set_interfaces_v6`] is used (UniClipboard patch).
    pub fn with_multicast_interfaces_v4(mut self, interfaces: Vec<Ipv4Addr>) -> Self {
        self.multicast_interfaces = interfaces;
        self
    }

    #[cfg(test)]
    fn with_interface_probe(mut self, probe: InterfaceProbe) -> Self {
        self.interface_probe = probe;
        self
    }

    /// Start the discovery service.
    ///
    /// This will spawn asynchronous tasks and return a guard which will stop the discovery when dropped.
    /// Changing the configuration is done by stopping the discovery and starting a new one.
    pub fn spawn(self, handle: &Handle) -> Result<DropGuard, SpawnError> {
        let _entered = handle.enter();
        let sockets = Sockets::new(
            self.class,
            self.multicast_interfaces.clone(),
            self.interface_probe.clone(),
        )?;
        tracing::trace!(?sockets, "created new sockets");

        let service_name = Name::from_str(&format!("_{}.{}.local.", self.name, self.protocol))
            .map_err(|source| SpawnError::ServiceName {
                source,
                name: self.name.clone(),
                protocol: self.protocol,
            })?;
        // need to test this here so it won't fail in the actor
        Name::from_str(&self.peer_id)
            .map_err(|source| SpawnError::NameFromPeerId {
                source,
                peer_id: self.peer_id.clone(),
            })?
            .append_domain(&service_name)
            .map_err(|source| SpawnError::AppendServiceName {
                source,
                service_name: service_name.clone(),
            })?;

        let rt = AcTokio::from_handle("swarm-discovery", handle.clone());
        let SupervisionRef { me, handle } = rt.spawn_actor("guardian", move |ctx| {
            guardian::guardian(ctx, self, sockets, service_name)
        });

        Ok(DropGuard {
            task: Some(handle),
            aref: me,
            _rt: rt,
        })
    }
}

/// A guard which will keep the discovery running until it is dropped.
///
/// You can also use this guard to modify the local addresses while the discovery is running.
#[must_use = "dropping this value will stop the mDNS discovery"]
pub struct DropGuard {
    task: Option<TokioJoinHandle<()>>,
    aref: ActoRef<guardian::Input>,
    _rt: AcTokio,
}

impl DropGuard {
    /// Remove all local addresses and stop advertising.
    pub fn remove_all(&self) {
        self.aref.send(guardian::Input::RemoveAll);
    }

    /// Remove a specific port from the local addresses.
    pub fn remove_port(&self, port: u16) {
        self.aref.send(guardian::Input::RemovePort(port));
    }

    /// Remove a specific address from the local addresses.
    pub fn remove_addr(&self, addr: IpAddr) {
        self.aref.send(guardian::Input::RemoveAddr(addr));
    }

    /// Add a port and addresses to the local addresses.
    pub fn add(&self, port: u16, addrs: Vec<IpAddr>) {
        self.aref.send(guardian::Input::AddAddr(port, addrs));
    }

    /// Sets a TXT attribute for this peer.
    ///
    /// See [`Discoverer::with_txt_attributes`] for details on the encoding of
    /// these attributes.
    ///
    /// Key and value together may not be longer than 254 bytes. Returns an
    /// error if the length is exceeded.
    ///
    /// The total length of all attributes is not checked here. You should make sure
    /// to keep the total length of all attributes at a few hundred bytes so that
    /// the resulting DNS packet does not exceed the UDP MTU.
    pub fn set_txt_attribute(
        &self,
        key: String,
        value: Option<String>,
    ) -> Result<(), TxtAttributeError> {
        validate_txt_attribute(&key, value.as_deref())?;
        self.aref.send(guardian::Input::SetTxt(key, value));
        Ok(())
    }

    /// Removes a TXT attribute.
    pub fn remove_txt_attribute(&self, key: String) {
        self.aref.send(guardian::Input::RemoveTxt(key));
    }

    /// Add a new IPv4 interface for multicast operations.
    ///
    /// This allows adding network interfaces dynamically after the discovery service
    /// has started. Useful for systems where network interfaces may come up after
    /// the application starts.
    ///
    /// Note: This only affects IPv4. IPv6 interfaces are managed by
    /// [`Self::set_interfaces_v6`] (UniClipboard patch).
    pub fn add_interface_v4(&self, interface: Ipv4Addr) {
        self.aref
            .send(guardian::Input::AddInterface(IpAddr::V4(interface)));
    }

    /// Remove an IPv4 interface from multicast operations.
    ///
    /// This stops sending multicast messages on the specified interface.
    ///
    /// Note: This only affects IPv4. IPv6 interfaces are managed by
    /// [`Self::set_interfaces_v6`] (UniClipboard patch).
    pub fn remove_interface_v4(&self, interface: Ipv4Addr) {
        self.aref
            .send(guardian::Input::RemoveInterface(IpAddr::V4(interface)));
    }

    /// 声明期望参与组播的完整 IPv4 接口集合（UniClipboard 补丁）。
    ///
    /// 后台把已安装接口调整为该集合：移除多余接口时先停止并等待其接收者退出，再释放
    /// 套接字；只有套接字、接收者以及主通配套接字上的组播成员关系都已建立才算安装成功。
    /// 仍有接口未安装时按 1 秒起、最长 60 秒的退避重试，直到安装成功或该接口离开期望集合；
    /// 重复提交相同集合会立即再尝试一次。首次调用后，`add_interface_v4`/`remove_interface_v4`
    /// 也修改这个集合。
    ///
    /// 只影响 IPv4；IPv6 接口见 [`Self::set_interfaces_v6`]。
    pub fn set_interfaces_v4(&self, interfaces: impl IntoIterator<Item = Ipv4Addr>) {
        let interfaces: BTreeSet<Ipv4Addr> = interfaces.into_iter().collect();
        self.aref.send(guardian::Input::SetInterfaces(interfaces));
    }

    /// 声明期望参与组播的完整 IPv6 接口集合（UniClipboard 补丁）。
    ///
    /// 每项为（操作系统接口索引，该接口上的一个本机地址）。IPv6 的组播成员关系与发送接口
    /// 都按接口索引生效，因此集合以接口索引为键：每个接口只建立一个发送套接字，并在主通配
    /// IPv6 套接字上为该索引加入一次组播组；同一索引出现多次时以最后一项为准，索引 0
    /// （“默认接口”）被忽略。地址只用于绑定该接口的发送套接字，使报文源端口为 5353，
    /// 不影响通告的记录；link-local 地址以接口索引为 scope 绑定。已安装接口的地址变化时，
    /// 先完整移除该接口再重新安装。
    ///
    /// 安装完整性、退避重试与移除顺序与 [`Self::set_interfaces_v4`] 相同，两个协议族共用同一
    /// 退避节奏。没有主通配 IPv6 套接字（IPv6 不可用或 [`IpClass::V4Only`]）时不安装 IPv6
    /// 接口。从未调用此接口时保持上游行为：IPv6 组播只使用默认接口。
    pub fn set_interfaces_v6(&self, interfaces: impl IntoIterator<Item = (u32, Ipv6Addr)>) {
        let interfaces: BTreeMap<u32, Ipv6Addr> = interfaces
            .into_iter()
            .filter(|(index, _)| *index != 0)
            .collect();
        self.aref.send(guardian::Input::SetInterfacesV6(interfaces));
    }
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        self.task.take().unwrap().abort();
    }
}

fn validate_txt_attribute(key: &str, value: Option<&str>) -> Result<(), TxtAttributeError> {
    if key.is_empty() {
        Err(TxtAttributeError::EmptyKey)
    } else if key.len() + value.as_ref().map(|v| v.len()).unwrap_or_default() > 254 {
        Err(TxtAttributeError::TooLong)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        net::{Ipv4Addr, Ipv6Addr},
        sync::Arc,
    };
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_change_addresses() {
        let handle = tokio::runtime::Handle::current();

        let peer_id1 = "test_peer1".to_string();
        let peer_id2 = "test_peer2".to_string();

        let (tx, mut rx) = mpsc::channel(10);

        // First Discoverer (the one we're testing)
        let discoverer1 = Discoverer::new("test_service".to_string(), peer_id1.clone())
            .with_addrs(8000, vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))])
            .with_multicast_interfaces_v4(vec![Ipv4Addr::new(127, 0, 0, 1)])
            .with_cadence(Duration::from_secs(1))
            .with_response_rate(1.0);

        let guard1 = discoverer1
            .spawn(&handle)
            .expect("Failed to spawn discoverer1");

        // Second Discoverer (to verify the changes). It uses the same
        // multicast interface as the first one: multicast sent on the
        // loopback interface is only delivered to members on the loopback
        // interface, which also keeps this test independent of the host's
        // default route.
        let discoverer2 = Discoverer::new("test_service".to_string(), peer_id2)
            .with_multicast_interfaces_v4(vec![Ipv4Addr::new(127, 0, 0, 1)])
            .with_callback(move |id, peer| {
                if id == peer_id1 {
                    tx.try_send(peer.clone()).ok();
                }
            });

        let _guard2 = discoverer2
            .spawn(&handle)
            .expect("Failed to spawn discoverer2");

        // Wait for initial discovery with a timeout
        let initial_peer = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Timeout waiting for initial peer")
            .expect("Failed to receive initial peer");
        assert_eq!(initial_peer.addrs().len(), 1);
        assert_eq!(
            initial_peer.addrs()[0],
            (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8000)
        );

        // Change addresses
        guard1.add(
            9000,
            vec![IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1))],
        );
        guard1.remove_port(8000);

        // Wait for the update to be discovered
        let updated_peer = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(peer) = rx.recv().await {
                    if peer.addrs().len() == 1 && peer.addrs()[0].1 == 9000 {
                        return Ok(peer);
                    }
                } else {
                    return Err("Failed to receive updated peer");
                }
            }
        })
        .await
        .expect("Timeout waiting for updated peer")
        .expect("Failed to receive updated peer");

        assert_eq!(updated_peer.addrs().len(), 1);
        assert_eq!(
            updated_peer.addrs()[0],
            (IpAddr::V6(Ipv6Addr::new(0, 0, 0, 0, 0, 0, 0, 1)), 9000)
        );

        // Stop the discoverers
        drop(guard1);
    }

    /// 轮询条件直到成立；超时时以条件名称失败。
    async fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
    }

    /// 只在回环网卡上收发的后来者；被测发现服务收到它即说明接收路径仍然工作。
    fn spawn_late_peer(service: String, interface: Ipv4Addr) -> DropGuard {
        Discoverer::new(service, "late_peer".to_string())
            .with_addrs(8000, [IpAddr::V4(interface)])
            .with_multicast_interfaces_v4(vec![interface])
            .with_cadence(Duration::from_millis(500))
            .spawn(&tokio::runtime::Handle::current())
            .expect("spawn late peer")
    }

    fn late_peer_callback(tx: mpsc::Sender<()>) -> impl FnMut(&str, &Peer) + Send + 'static {
        move |id, peer| {
            if id == "late_peer" && !peer.is_expiry() {
                tx.try_send(()).ok();
            }
        }
    }

    /// UniClipboard 补丁：反复增删同一受控接口后，接收者与套接字数量回到基线，
    /// 保留接口上的发现照常工作。
    ///
    /// Linux 上整个 127.0.0.0/8 都位于回环网卡，因此 127.0.0.2 可作为不影响主机网络的
    /// 受控接口；其他平台没有可用的第二个回环地址，跳过本测试。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn removed_interface_releases_receiver_and_socket_while_retained_interface_discovers() {
        let retained = Ipv4Addr::LOCALHOST;
        let churned = Ipv4Addr::new(127, 0, 0, 2);
        let service = format!("churn{}", std::process::id());
        let state = Arc::new(socket::probe::State::default());
        let (tx, mut rx) = mpsc::channel(10);
        let guard = Discoverer::new(service.clone(), "churning_peer".to_string())
            .with_cadence(Duration::from_millis(500))
            .with_interface_probe(state.probe())
            .with_callback(late_peer_callback(tx))
            .spawn(&tokio::runtime::Handle::current())
            .expect("spawn churning discoverer");
        guard.set_interfaces_v4([retained]);
        wait_until("retained interface installed", || state.live(retained) == 1).await;

        for round in 1..=5 {
            guard.set_interfaces_v4([retained, churned]);
            wait_until("controlled interface installed", || {
                state.live(churned) == 1
            })
            .await;
            guard.set_interfaces_v4([retained]);
            wait_until("controlled interface released", || state.live(churned) == 0).await;
            assert_eq!(state.created(churned), round);
        }
        assert_eq!(state.live(retained), 1);
        assert_eq!(state.created(retained), 1);

        let _late = spawn_late_peer(service, retained);
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("retained interface still discovers new peers")
            .expect("callback channel open");
    }

    /// UniClipboard 补丁：初始创建与首次协调都失败后，期望集合保持不变，后台重试
    /// 在套接字真正建立后才把接口算作已安装。
    #[tokio::test]
    async fn failed_interface_installation_is_retried_without_a_new_interface_set() {
        let interface = Ipv4Addr::LOCALHOST;
        let service = format!("retry{}", std::process::id());
        let state = Arc::new(socket::probe::State::default());
        state.fail_next(2);
        let (tx, mut rx) = mpsc::channel(10);
        let guard = Discoverer::new(service.clone(), "retrying_peer".to_string())
            .with_multicast_interfaces_v4(vec![interface])
            .with_cadence(Duration::from_millis(500))
            .with_interface_probe(state.probe())
            .with_callback(late_peer_callback(tx))
            .spawn(&tokio::runtime::Handle::current())
            .expect("spawn retrying discoverer");
        assert_eq!(state.attempts(interface), 1, "initial creation attempted");
        assert_eq!(state.live(interface), 0, "initial failure is not installed");

        guard.set_interfaces_v4([interface]);
        wait_until("first reconciliation attempt", || {
            state.attempts(interface) >= 2
        })
        .await;
        assert_eq!(
            state.live(interface),
            0,
            "a failed addition is not installed"
        );

        // 不再提交新的期望集合：只靠后台重试完成安装。
        wait_until("retried installation", || state.live(interface) == 1).await;
        assert_eq!(state.attempts(interface), 3);

        let _late = spawn_late_peer(service, interface);
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("installed interface discovers peers")
            .expect("callback channel open");
        drop(guard);
    }

    /// `left` 最后一次退出组播组之后 `addr` 的加入操作，返回（成功数，失败数）。
    fn joins_after_leave(
        state: &socket::probe::State,
        left: Ipv4Addr,
        addr: Ipv4Addr,
    ) -> (usize, usize) {
        use socket::probe::Membership;
        let log = state.memberships();
        let Some(leave) = log
            .iter()
            .rposition(|(op, a, _)| *op == Membership::Leave && *a == left)
        else {
            return (0, 0);
        };
        log[leave + 1..]
            .iter()
            .filter(|(op, a, _)| *op == Membership::Join && *a == addr)
            .fold((0, 0), |(ok, failed), (_, _, joined)| {
                if *joined {
                    (ok + 1, failed)
                } else {
                    (ok, failed + 1)
                }
            })
    }

    /// 启动只使用期望集合的发现服务，并等待 `interfaces` 全部完成安装（含组播成员关系）。
    async fn spawn_with_interfaces(
        service: &str,
        state: &Arc<socket::probe::State>,
        interfaces: [Ipv4Addr; 2],
    ) -> DropGuard {
        use socket::probe::Membership;
        let guard = Discoverer::new(service.to_string(), "membership_peer".to_string())
            .with_cadence(Duration::from_millis(500))
            .with_interface_probe(state.probe())
            .spawn(&tokio::runtime::Handle::current())
            .expect("spawn membership discoverer");
        guard.set_interfaces_v4(interfaces);
        wait_until("both interfaces installed", || {
            let log = state.memberships();
            interfaces.iter().all(|addr| {
                state.live(*addr) == 1
                    && log
                        .iter()
                        .any(|(op, a, ok)| *op == Membership::Join && a == addr && *ok)
            })
        })
        .await;
        guard
    }

    /// UniClipboard 补丁：组播成员关系按操作系统接口生效。移除同一网卡上的另一地址后，
    /// 仍保留地址必须在主通配套接字上重新加入组播组，而不是因为接收者仍在而被跳过。
    ///
    /// 断言只依赖观察点记录的加入与退出顺序，不依赖 Linux 的跨套接字组播投递；Linux 上
    /// 127.0.0.1 与 127.0.0.2 同属回环网卡，其他平台没有可用的第二个回环地址，跳过本测试。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn removing_an_address_rejoins_the_group_for_an_address_retained_on_the_same_interface() {
        let retained = Ipv4Addr::LOCALHOST;
        let removed = Ipv4Addr::new(127, 0, 0, 2);
        let state = Arc::new(socket::probe::State::default());
        let service = format!("rejoin{}", std::process::id());
        let guard = spawn_with_interfaces(&service, &state, [retained, removed]).await;

        guard.set_interfaces_v4([retained]);
        wait_until("removed interface released", || state.live(removed) == 0).await;
        wait_until("retained address rejoined after the removal", || {
            joins_after_leave(&state, removed, retained).0 >= 1
        })
        .await;
        assert_eq!(state.live(retained), 1);
        assert_eq!(state.created(retained), 1, "retained socket is not rebuilt");
    }

    /// UniClipboard 补丁：保留地址重新加入组播组失败时，接口算作未完整安装，后台安装
    /// 重试在不提交新期望集合的情况下补齐成员关系。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_failed_rejoin_for_a_retained_address_is_retried_without_a_new_interface_set() {
        let retained = Ipv4Addr::LOCALHOST;
        let removed = Ipv4Addr::new(127, 0, 0, 2);
        let state = Arc::new(socket::probe::State::default());
        let service = format!("rejoinretry{}", std::process::id());
        let guard = spawn_with_interfaces(&service, &state, [retained, removed]).await;

        // 移除时的重新加入与同一轮协调中的立即重试都失败。
        state.fail_next_joins(retained, 2);
        guard.set_interfaces_v4([retained]);
        wait_until("removed interface released", || state.live(removed) == 0).await;
        wait_until("failed rejoin retried in the background", || {
            joins_after_leave(&state, removed, retained).0 >= 1
        })
        .await;
        assert_eq!(joins_after_leave(&state, removed, retained).1, 2);
        assert_eq!(state.created(retained), 1, "retained socket is not rebuilt");
    }

    /// 本机 IPv6 回环接口的（接口索引，::1）；主机未启用 IPv6 时返回 `None`。
    #[cfg(target_os = "linux")]
    fn loopback_v6() -> Option<(u32, Ipv6Addr)> {
        let interface = if_addrs::get_if_addrs()
            .ok()?
            .into_iter()
            .find(|interface| interface.addr.ip() == IpAddr::V6(Ipv6Addr::LOCALHOST))?;
        Some((interface.index?, Ipv6Addr::LOCALHOST))
    }

    /// 启动同时要求 IPv4 与 IPv6 主套接字的发现服务，只使用 IPv6 期望集合。
    #[cfg(target_os = "linux")]
    fn spawn_v6_discoverer(service: &str, state: &Arc<socket::probe::State>) -> DropGuard {
        Discoverer::new(service.to_string(), "ipv6_peer".to_string())
            .with_ip_class(IpClass::V4AndV6)
            .with_cadence(Duration::from_millis(500))
            .with_interface_probe(state.probe())
            .spawn(&tokio::runtime::Handle::current())
            .expect("spawn IPv6 discoverer")
    }

    /// 该接口在主通配套接字上是否有成功的加入记录。
    #[cfg(target_os = "linux")]
    fn joined(state: &socket::probe::State, key: socket::probe::Key) -> bool {
        use socket::probe::Membership;
        state
            .memberships()
            .iter()
            .any(|(op, k, ok)| *op == Membership::Join && *k == key && *ok)
    }

    /// 本组测试在 Linux 回环接口上用 ::1 建立真实套接字。Linux 回环接口不能收发 IPv6 组播，
    /// 断言只依赖观察点记录（套接字弱引用、发送尝试与成员操作），不依赖组播投递；主机未启用
    /// IPv6 时跳过。
    #[cfg(target_os = "linux")]
    macro_rules! require_loopback_v6 {
        () => {
            match loopback_v6() {
                Some(interface) => interface,
                None => {
                    eprintln!("skipped: host has no IPv6 loopback");
                    return;
                }
            }
        };
    }

    /// UniClipboard 补丁：IPv6 接口按接口索引安装，发送套接字、接收者与主通配套接字上的
    /// 组播成员关系都建立，查询经该接口套接字发出；移出期望集合后套接字被全部持有者释放，
    /// 并在主通配套接字上退出该接口的组播组。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn ipv6_interface_is_installed_by_index_and_released_when_removed() {
        use socket::probe::{Key, Membership};
        let (index, addr) = require_loopback_v6!();
        let key = Key::V6(index);
        let state = Arc::new(socket::probe::State::default());
        let guard = spawn_v6_discoverer(&format!("v6install{}", std::process::id()), &state);

        guard.set_interfaces_v6([(index, addr)]);
        wait_until("IPv6 interface installed", || {
            state.live(key) == 1 && joined(&state, key)
        })
        .await;
        wait_until("query sent on the IPv6 interface socket", || {
            state.sends(key) >= 1
        })
        .await;

        guard.set_interfaces_v6([]);
        wait_until("IPv6 interface released", || state.live(key) == 0).await;
        let log = state.memberships();
        assert_eq!(
            log.last().map(|(op, k, _)| (*op, *k)),
            Some((Membership::Leave, key)),
            "the group is left on the main socket after the socket is released"
        );
        assert_eq!(state.created(key), 1);
    }

    /// UniClipboard 补丁：IPv6 接口套接字的首次创建与第一次重试都失败后，期望集合保持不变，
    /// 后台按退避重试，直到套接字真正建立才算已安装。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn failed_ipv6_interface_installation_is_retried_without_a_new_interface_set() {
        use socket::probe::Key;
        let (index, addr) = require_loopback_v6!();
        let key = Key::V6(index);
        let state = Arc::new(socket::probe::State::default());
        state.fail_next(2);
        let guard = spawn_v6_discoverer(&format!("v6retry{}", std::process::id()), &state);

        guard.set_interfaces_v6([(index, addr)]);
        wait_until("first installation attempt", || state.attempts(key) >= 1).await;
        assert_eq!(state.live(key), 0, "a failed addition is not installed");

        // 不再提交新的期望集合：只靠后台重试完成安装。
        wait_until("retried installation", || {
            state.live(key) == 1 && joined(&state, key)
        })
        .await;
        assert_eq!(state.attempts(key), 3);
    }

    /// UniClipboard 补丁：主通配 IPv6 套接字为接口索引加入组播组失败时，接口算作未完整安装，
    /// 后台重试在不提交新期望集合、也不重建发送套接字的情况下补齐成员关系。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_failed_ipv6_join_is_retried_without_a_new_interface_set() {
        use socket::probe::{Key, Membership};
        let (index, addr) = require_loopback_v6!();
        let key = Key::V6(index);
        let state = Arc::new(socket::probe::State::default());
        // 建立套接字时的加入与同一轮协调中的立即重试都失败。
        state.fail_next_joins(key, 2);
        let guard = spawn_v6_discoverer(&format!("v6join{}", std::process::id()), &state);

        guard.set_interfaces_v6([(index, addr)]);
        wait_until("failed join retried in the background", || {
            joined(&state, key)
        })
        .await;
        let failed_joins = state
            .memberships()
            .iter()
            .filter(|(op, k, ok)| *op == Membership::Join && *k == key && !*ok)
            .count();
        assert_eq!(failed_joins, 2);
        assert_eq!(state.created(key), 1, "the interface socket is not rebuilt");
        assert_eq!(state.live(key), 1);
    }

    /// UniClipboard 补丁：同一接口索引的期望地址变化时，旧发送套接字先被完整释放再按新地址
    /// 安装；新地址无法绑定时按退避重试，恢复可用地址后重新安装。
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn changing_the_address_of_an_ipv6_interface_rebuilds_its_socket() {
        use socket::probe::Key;
        let (index, addr) = require_loopback_v6!();
        let key = Key::V6(index);
        let state = Arc::new(socket::probe::State::default());
        let guard = spawn_v6_discoverer(&format!("v6rebind{}", std::process::id()), &state);

        guard.set_interfaces_v6([(index, addr)]);
        wait_until("IPv6 interface installed", || state.live(key) == 1).await;

        // 文档地址不属于本机，绑定失败。
        let unassigned: Ipv6Addr = "2001:db8::1".parse().expect("documentation address");
        guard.set_interfaces_v6([(index, unassigned)]);
        wait_until("old socket released and rebinding attempted", || {
            state.live(key) == 0 && state.attempts(key) >= 2
        })
        .await;

        guard.set_interfaces_v6([(index, addr)]);
        wait_until("IPv6 interface reinstalled", || state.live(key) == 1).await;
        assert_eq!(state.created(key), 2);
    }
}
