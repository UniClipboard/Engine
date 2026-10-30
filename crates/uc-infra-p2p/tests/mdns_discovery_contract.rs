//! 真实 mDNS adapter 契约；测试地址仅用于检查观察，不冒充内容传输验收。

use std::time::Duration;

use futures_util::StreamExt;
use iroh::{
    address_lookup::{AddressLookup, EndpointData},
    SecretKey, TransportAddr,
};
use iroh_mdns_address_lookup::{
    DiscoveryEvent, MdnsAddressLookup, MulticastInterfacesV4, MulticastInterfacesV6,
};

fn pair() -> (MdnsAddressLookup, MdnsAddressLookup, iroh::EndpointId) {
    let service = format!("uni-test-{}", uuid::Uuid::new_v4().simple());
    let id = SecretKey::generate().public();
    let receiver = MdnsAddressLookup::builder()
        .service_name(service.clone())
        .advertise(false)
        .build(SecretKey::generate().public())
        .unwrap();
    let sender = MdnsAddressLookup::builder()
        .service_name(service)
        .build(id)
        .unwrap();
    (receiver, sender, id)
}

#[tokio::test]
async fn actively_resolved_peers_still_reach_discovery_subscribers() {
    let (receiver, sender, id) = pair();
    let mut passive = receiver.subscribe().await;
    let mut active = receiver.resolve(id).unwrap();
    sender.publish(&EndpointData::from_iter([TransportAddr::Ip(
        "192.0.2.10:4242".parse().unwrap(),
    )]));
    let resolved = tokio::time::timeout(Duration::from_secs(10), active.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(resolved.endpoint_id(), id);
    let event = tokio::time::timeout(Duration::from_secs(3), passive.next())
        .await
        .expect("主动查询不能抑制发现观察")
        .unwrap();
    assert!(
        matches!(event, DiscoveryEvent::Discovered { endpoint_info, .. } if endpoint_info.endpoint_id == id)
    );
}

#[tokio::test]
async fn address_updates_replace_cache_and_late_subscriptions_receive_snapshot() {
    let (receiver, sender, id) = pair();
    let mut active = receiver.resolve(id).unwrap();
    let old = "192.0.2.10:4242".parse::<std::net::SocketAddr>().unwrap();
    let new = "192.0.2.11:4243".parse::<std::net::SocketAddr>().unwrap();
    sender.publish(&EndpointData::from_iter([TransportAddr::Ip(old)]));
    tokio::time::timeout(Duration::from_secs(10), active.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    sender.publish(&EndpointData::from_iter([TransportAddr::Ip(new)]));
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(item) = active.next().await {
            if item
                .unwrap()
                .to_endpoint_addr()
                .ip_addrs()
                .any(|addr| addr == &new)
            {
                return;
            }
        }
        panic!("查询流未收到地址变化");
    })
    .await
    .unwrap();
    let mut subsequent = receiver.resolve(id).unwrap();
    let item = tokio::time::timeout(Duration::from_secs(3), subsequent.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        item.to_endpoint_addr().ip_addrs().any(|addr| addr == &new),
        "后续查询必须得到更新后的缓存"
    );
    assert!(!item.to_endpoint_addr().ip_addrs().any(|addr| addr == &old));
    let mut passive = receiver.subscribe().await;
    let event = tokio::time::timeout(Duration::from_secs(3), passive.next())
        .await
        .expect("迟订阅必须得到当前快照")
        .unwrap();
    assert!(
        matches!(event, DiscoveryEvent::Discovered { endpoint_info, .. } if endpoint_info.to_endpoint_addr().ip_addrs().any(|addr| addr == &new))
    );
}

#[tokio::test]
async fn shutdown_closes_subscriptions_even_when_service_clones_are_retained() {
    let (receiver, sender, _) = pair();
    let retained = receiver.clone();
    let mut events = receiver.subscribe().await;
    receiver.shutdown().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(1), events.next())
        .await
        .unwrap()
        .is_none());
    retained.shutdown().await.unwrap();
    sender.shutdown().await.unwrap();
}

/// 本机非回环 IPv4 地址；与 Engine 的接口快照策略一致，只用于测试接口来源。
fn local_ipv4_interfaces() -> Vec<std::net::Ipv4Addr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|interface| match interface.addr.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
            _ => None,
        })
        .collect()
}

/// 接口来源在启动时读取一次，并在本机发布地址变化时再次读取以增删组播接口；
/// 在全部本机 IPv4 接口上收发时发现照常工作。
#[tokio::test]
async fn multicast_interfaces_are_read_at_start_and_on_local_address_changes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let service = format!("uni-test-{}", uuid::Uuid::new_v4().simple());
    let reads = Arc::new(AtomicUsize::new(0));
    let interfaces = {
        let reads = Arc::clone(&reads);
        MulticastInterfacesV4::new(move || {
            reads.fetch_add(1, Ordering::SeqCst);
            local_ipv4_interfaces()
        })
    };
    let id = SecretKey::generate().public();
    let receiver = MdnsAddressLookup::builder()
        .service_name(service.clone())
        .advertise(false)
        .multicast_interfaces_v4(MulticastInterfacesV4::new(local_ipv4_interfaces))
        .build(SecretKey::generate().public())
        .unwrap();
    let sender = MdnsAddressLookup::builder()
        .service_name(service)
        .multicast_interfaces_v4(interfaces)
        .build(id)
        .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1, "read once at start");

    let mut active = receiver.resolve(id).unwrap();
    sender.publish(&EndpointData::from_iter([TransportAddr::Ip(
        "192.0.2.10:4242".parse().unwrap(),
    )]));
    let resolved = tokio::time::timeout(Duration::from_secs(10), active.next())
        .await
        .expect("discovery works with configured interfaces")
        .unwrap()
        .unwrap();
    assert_eq!(resolved.endpoint_id(), id);
    tokio::time::timeout(Duration::from_secs(3), async {
        while reads.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a local address change refreshes the interface set");

    receiver.shutdown().await.unwrap();
    sender.shutdown().await.unwrap();
}

/// 本机非回环 IPv6 接口的（接口索引，地址），每个接口取一个地址；只用于测试接口来源。
fn local_ipv6_interfaces() -> Vec<(u32, std::net::Ipv6Addr)> {
    let mut interfaces = std::collections::BTreeMap::new();
    for interface in if_addrs::get_if_addrs().unwrap_or_default() {
        if let (std::net::IpAddr::V6(ip), Some(index)) = (interface.addr.ip(), interface.index) {
            if !ip.is_loopback() && !ip.is_unspecified() {
                interfaces.entry(index).or_insert(ip);
            }
        }
    }
    interfaces.into_iter().collect()
}

/// IPv6 接口来源与 IPv4 一样在启动时读取一次，并在本机发布地址变化时再次读取；
/// 在本机 IPv6 接口上收发时发现照常工作。
#[tokio::test]
async fn ipv6_multicast_interfaces_are_read_at_start_and_on_local_address_changes() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let service = format!("uni-test-{}", uuid::Uuid::new_v4().simple());
    let reads = Arc::new(AtomicUsize::new(0));
    let interfaces = {
        let reads = Arc::clone(&reads);
        MulticastInterfacesV6::new(move || {
            reads.fetch_add(1, Ordering::SeqCst);
            local_ipv6_interfaces()
        })
    };
    let id = SecretKey::generate().public();
    let receiver = MdnsAddressLookup::builder()
        .service_name(service.clone())
        .advertise(false)
        .multicast_interfaces_v6(MulticastInterfacesV6::new(local_ipv6_interfaces))
        .build(SecretKey::generate().public())
        .unwrap();
    let sender = MdnsAddressLookup::builder()
        .service_name(service)
        .multicast_interfaces_v6(interfaces)
        .build(id)
        .unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 1, "read once at start");

    let mut active = receiver.resolve(id).unwrap();
    sender.publish(&EndpointData::from_iter([TransportAddr::Ip(
        "192.0.2.10:4242".parse().unwrap(),
    )]));
    let resolved = tokio::time::timeout(Duration::from_secs(10), active.next())
        .await
        .expect("discovery works with configured IPv6 interfaces")
        .unwrap()
        .unwrap();
    assert_eq!(resolved.endpoint_id(), id);
    tokio::time::timeout(Duration::from_secs(3), async {
        while reads.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a local address change refreshes the IPv6 interface set");

    receiver.shutdown().await.unwrap();
    sender.shutdown().await.unwrap();
}
