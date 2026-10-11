use std::{sync::Arc, time::Duration};

use futures_util::StreamExt;
use iroh::{
    address_lookup::MemoryLookup, endpoint::presets, Endpoint, EndpointAddr, RelayMode, SecretKey,
};
use tokio::time::Instant;
use uc_application::deps::{DiscoveredPeer, DiscoveryEvent, DiscoveryManager, PeerDirectory};

use super::{
    cloud::CloudDiscoveryProvider, decode, lookup::DirectoryAddressLookup, peer_id, CLOUD, MDNS,
};

#[test]
fn cached_observation_does_not_gain_a_new_lifetime_when_read_again() {
    use iroh::address_lookup::{EndpointInfo, Item};
    use std::time::{SystemTime, UNIX_EPOCH};
    let address = EndpointAddr::new(SecretKey::generate().public());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    let expired = Item::new(
        EndpointInfo::from(address.clone()),
        "mdns",
        Some(now - 61_000_000),
    );
    assert!(
        super::observation(expired).is_err(),
        "读取缓存不能重新授予已过期观察六十秒有效期"
    );
    let nearly_expired = Item::new(EndpointInfo::from(address), "mdns", Some(now - 59_000_000));
    let DiscoveryEvent::Found(found) = super::observation(nearly_expired).unwrap() else {
        panic!("预期候选观察");
    };
    assert!(found.ttl <= Duration::from_secs(1));
}

#[test]
fn locator_cannot_redirect_a_candidate_to_another_transport_identity() {
    let peer = peer_id(SecretKey::generate().public());
    let address = EndpointAddr::new(SecretKey::generate().public());
    assert!(decode(&peer, &postcard::to_stdvec(&address).unwrap()).is_err());
}

#[tokio::test]
async fn directory_lookup_establishes_real_multistream_connection_with_bidirectional_bytes() {
    const ALPN: &[u8] = b"uni-test/discovery";
    let server = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let address = server.addr();
    assert!(!address.addrs.is_empty());
    let directory = Arc::new(PeerDirectory::new(8));
    directory
        .observe(
            MDNS,
            DiscoveryEvent::Found(DiscoveredPeer {
                peer: peer_id(server.id()),
                locator: postcard::to_stdvec(&address).unwrap(),
                ttl: Duration::from_secs(30),
            }),
            Instant::now(),
        )
        .unwrap();
    let native_cloud = MemoryLookup::new();
    native_cloud.add_endpoint_info(address);
    let manager = Arc::new(
        DiscoveryManager::new(
            vec![Arc::new(CloudDiscoveryProvider(native_cloud))],
            directory.clone(),
        )
        .unwrap(),
    );
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .address_lookup(DirectoryAddressLookup {
            manager: manager.clone(),
            mdns: Arc::new(MemoryLookup::new()),
        })
        .bind()
        .await
        .unwrap();
    let serving = tokio::spawn({
        let server = server.clone();
        async move {
            let connection = server.accept().await.unwrap().await.unwrap();
            for _ in 0..2 {
                let (mut send, mut recv) = connection.accept_bi().await.unwrap();
                let content = recv.read_to_end(128).await.unwrap();
                assert_eq!(content, b"test discovery content");
                send.write_all(b"test peer reply").await.unwrap();
                send.finish().unwrap();
            }
            connection.closed().await;
        }
    });
    let connection = tokio::time::timeout(
        Duration::from_secs(10),
        client.connect(EndpointAddr::new(server.id()), ALPN),
    )
    .await
    .unwrap()
    .unwrap();
    for _ in 0..2 {
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(b"test discovery content").await.unwrap();
        send.finish().unwrap();
        assert_eq!(recv.read_to_end(128).await.unwrap(), b"test peer reply");
    }
    // 同一 lookup 的真实结果也进入目录；本测试只证明桥与可替换来源，非真实云端验收。
    let mut cloud = manager.resolve(peer_id(server.id()));
    while let Some(candidate) = cloud.next().await {
        if candidate.unwrap().source == CLOUD {
            break;
        }
    }
    directory
        .observe(
            MDNS,
            DiscoveryEvent::Lost {
                peer: peer_id(server.id()),
            },
            Instant::now(),
        )
        .unwrap();
    assert_eq!(
        directory
            .candidates(&peer_id(server.id()), Instant::now())
            .unwrap()[0]
            .source,
        CLOUD
    );
    client.close().await;
    serving.await.unwrap();
    server.close().await;
    manager.shutdown().unwrap();
}
