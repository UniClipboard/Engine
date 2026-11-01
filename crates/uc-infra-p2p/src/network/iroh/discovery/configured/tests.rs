use std::{
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use iroh::{endpoint::presets, Endpoint, EndpointAddr, RelayMode, SecretKey, TransportAddr};
use uc_application::deps::{
    DiscoveryEvent, DiscoveryManager, DiscoveryProvider, KnownPeerIdentity, MembershipLedgerError,
    PeerDirectory, PeerIdentityDirectoryPort,
};
use uc_core::ids::DeviceId;
use uc_core::ports::{security::IdentityFingerprintFactoryPort, SettingsPort};
use uc_core::settings::model::{ConfiguredPeerAddress, Settings};
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;
use uc_observability_contract::diagnostics::connectivity::NetworkRecorder;

use super::super::{
    decode, lookup::DirectoryAddressLookup, lookup::DirectoryAddressLookupBuilder, peer_id,
    LifetimeMdns,
};
use super::{ConfiguredAddressProvider, ConfiguredPeerAddresses};

struct Directory(Vec<KnownPeerIdentity>);
#[async_trait]
impl PeerIdentityDirectoryPort for Directory {
    async fn known_peer_identities(&self) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
        Ok(self.0.clone())
    }
}

struct FixedSettings(Mutex<Settings>);
#[async_trait]
impl SettingsPort for FixedSettings {
    async fn load(&self) -> anyhow::Result<Settings> {
        Ok(self.0.lock().unwrap().clone())
    }
    async fn save(&self, settings: &Settings) -> anyhow::Result<()> {
        *self.0.lock().unwrap() = settings.clone();
        Ok(())
    }
}

fn configured(
    endpoint: iroh::EndpointId,
    device: &str,
    ip: &str,
    port: u16,
) -> Arc<ConfiguredPeerAddresses> {
    let mut settings = Settings::default();
    settings.network.peer_addresses.insert(
        device.to_string(),
        ConfiguredPeerAddress {
            ip: ip.to_string(),
            port,
        },
    );
    Arc::new(ConfiguredPeerAddresses::new(
        Arc::new(Directory(vec![KnownPeerIdentity {
            device_id: DeviceId::new(device),
            identity_fingerprint: Sha256IdentityFingerprintFactory
                .from_public_key(endpoint.as_bytes())
                .unwrap(),
        }])),
        Arc::new(Sha256IdentityFingerprintFactory),
        Arc::new(FixedSettings(Mutex::new(settings))),
    ))
}

async fn resolved(
    provider: &ConfiguredAddressProvider,
    endpoint: iroh::EndpointId,
) -> Vec<EndpointAddr> {
    let Some(events) = provider.resolve(&peer_id(endpoint)) else {
        return Vec::new();
    };
    events
        .map(|event| match event.unwrap() {
            DiscoveryEvent::Found(found) => decode(&found.peer, &found.locator).unwrap(),
            _ => panic!("configured source only reports found addresses"),
        })
        .collect()
        .await
}

/// 按传输身份找到设备并在查询时读取其配置条目：IP 字面量交出该身份的直连候选；
/// 主机名、不属于已知设备或没有条目的身份没有候选。
#[tokio::test]
async fn resolves_the_configured_entry_of_the_identified_device_at_query_time() {
    let id = SecretKey::generate().public();
    let literal = ConfiguredAddressProvider(configured(id, "device-a", "10.8.0.2", 51821));
    assert_eq!(
        resolved(&literal, id).await,
        vec![EndpointAddr::from_parts(
            id,
            [TransportAddr::Ip("10.8.0.2:51821".parse().unwrap())]
        )]
    );
    let stranger = SecretKey::generate().public();
    assert!(resolved(&literal, stranger).await.is_empty());

    // 主机名不是 IP 字面量：不解析、不产生 DNS 查询，也不交出候选。
    let hostname = ConfiguredAddressProvider(configured(id, "device-a", "localhost", 51821));
    assert!(resolved(&hostname, id).await.is_empty());

    let other_device = ConfiguredAddressProvider(configured(id, "device-a", "10.8.0.2", 51821));
    let mut settings = Settings::default();
    settings.network.peer_addresses.insert(
        "device-b".into(),
        ConfiguredPeerAddress {
            ip: "10.8.0.3".into(),
            port: 51821,
        },
    );
    other_device.0.settings.save(&settings).await.unwrap();
    assert!(
        resolved(&other_device, id).await.is_empty(),
        "settings are read at query time"
    );
}

/// LAN-only 下配置地址与其他来源一样经过目录 lookup 的本地网络范围收敛：公网地址不交给 iroh。
#[tokio::test]
async fn lan_only_directory_lookup_drops_a_configured_public_address() {
    use iroh::address_lookup::AddressLookup;

    let id = SecretKey::generate().public();
    for (ip, lan_only, expected) in [
        ("203.0.113.7", true, EndpointAddr::new(id)),
        (
            "203.0.113.7",
            false,
            EndpointAddr::from_parts(
                id,
                [TransportAddr::Ip("203.0.113.7:51821".parse().unwrap())],
            ),
        ),
        (
            "10.8.0.2",
            true,
            EndpointAddr::from_parts(id, [TransportAddr::Ip("10.8.0.2:51821".parse().unwrap())]),
        ),
    ] {
        let manager = Arc::new(
            DiscoveryManager::new(
                vec![Arc::new(ConfiguredAddressProvider(configured(
                    id, "device-a", ip, 51821,
                )))],
                Arc::new(PeerDirectory::new(8)),
            )
            .unwrap(),
        );
        let lookup = DirectoryAddressLookup {
            manager: manager.clone(),
            mdns: Arc::new(iroh::address_lookup::MemoryLookup::new()),
            lan_only,
        };
        let item = lookup
            .resolve(id)
            .expect("directory resolves")
            .next()
            .await
            .expect("one candidate")
            .expect("candidate decodes");
        assert_eq!(item.into_endpoint_addr(), expected, "lan_only={lan_only}");
        manager.shutdown().unwrap();
    }
}

/// 只知道对端身份、没有 mDNS 与已保存线索时，LAN-only 节点经用户配置的地址拨通对端。
#[tokio::test]
async fn lan_only_endpoint_dials_a_peer_known_only_by_its_configured_address() {
    const ALPN: &[u8] = b"uni-test/configured";
    let server = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let port = server
        .bound_sockets()
        .into_iter()
        .find(|socket| socket.is_ipv4())
        .expect("IPv4 socket")
        .port();
    let mdns = LifetimeMdns::start_with_backoff(
        || {
            Err(iroh::address_lookup::AddressLookupBuilderError::from_err(
                "mdns",
                std::io::Error::other("unavailable"),
            ))
        },
        NetworkRecorder::current(),
        Duration::from_secs(3600),
        Duration::from_secs(3600),
    );
    let client = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .address_lookup(DirectoryAddressLookupBuilder {
            mdns: mdns.clone(),
            lan_only: true,
            configured: Some(configured(server.id(), "device-a", "127.0.0.1", port)),
            recorder: NetworkRecorder::current(),
            manager: Arc::new(OnceLock::new()),
        })
        .bind()
        .await
        .unwrap();
    let accepting = tokio::spawn({
        let server = server.clone();
        async move {
            let connection = server.accept().await?.await.ok()?;
            connection.closed().await;
            Some(())
        }
    });
    let connection = tokio::time::timeout(
        Duration::from_secs(10),
        client.connect(EndpointAddr::new(server.id()), ALPN),
    )
    .await
    .expect("dial finishes")
    .expect("configured address reaches the peer");
    assert_eq!(connection.remote_id(), server.id());
    drop(connection);
    accepting.abort();
    mdns.abort();
    client.close().await;
    server.close().await;
}
