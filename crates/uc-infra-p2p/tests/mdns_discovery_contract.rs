//! 真实 mDNS adapter 契约；测试地址仅用于检查观察，不冒充内容传输验收。

use std::time::Duration;

use futures_util::StreamExt;
use iroh::{
    address_lookup::{AddressLookup, EndpointData},
    SecretKey, TransportAddr,
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};

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
