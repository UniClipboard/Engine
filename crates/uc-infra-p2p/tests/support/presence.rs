//! 准入外部验收使用生产 builder 和同一完整 owner 图，不保留原生业务构造。
use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, UdpSocket},
    sync::{Arc, Mutex},
};
use uc_application::deps::{
    ConnectivityManager, ConnectivityPolicy, KnownPeerContact, PeerIdentityDirectoryPort,
    PeerReachabilityTransport, PeerReachabilityTransportPorts,
};
use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{ClockPort, PeerAddressRepositoryPort, SecureStorageError, SecureStoragePort};
use uc_infra_p2p::network::iroh::connectivity::PeerReachabilityWire;
use uc_infra_p2p::network::iroh::{
    IrohIdentityStore, IrohNode, IrohNodeBuilder, IrohNodeConfig, IDENTITY_STORE_KEY,
};
struct MemorySecrets(Mutex<HashMap<String, Vec<u8>>>);
impl SecureStoragePort for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, bytes: &[u8]) -> Result<(), SecureStorageError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), bytes.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}
pub struct PresenceNode {
    pub owner: Arc<PeerReachabilityTransport>,
    pub address: iroh::EndpointAddr,
    node: IrohNode,
}
pub async fn start(
    addresses: Arc<dyn PeerAddressRepositoryPort>,
    identities: Arc<dyn PeerIdentityDirectoryPort>,
    authority: Arc<dyn PeerAdmissionPort>,
    fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
    clock: Arc<dyn ClockPort>,
    contacts: tokio::sync::broadcast::Sender<KnownPeerContact>,
) -> PresenceNode {
    let secret = iroh::SecretKey::generate();
    let storage = Arc::new(MemorySecrets(Mutex::new(HashMap::from([(
        IDENTITY_STORE_KEY.to_owned(),
        secret.to_bytes().to_vec(),
    )]))));
    let identity = IrohIdentityStore::new(storage, fingerprint.clone());
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let network = IrohNodeBuilder::bind(
        &identity,
        IrohNodeConfig {
            disable_relays: true,
            relay_urls: Vec::new(),
            bind_port: Some(port),
            ..IrohNodeConfig::default()
        },
    )
    .await
    .unwrap();
    let mut builder = network.prepare_session();
    let address_refresh =
        builder.build_verified_peer_address_refresh(addresses.clone(), clock.clone());
    let (deps, environment) =
        builder.presence_connectivity_deps(addresses, identities, authority.clone(), fingerprint);
    let manager = ConnectivityManager::new(
        deps.backend,
        deps.admission,
        ConnectivityPolicy {
            presence_retention: true,
            inbound_per_connection: 2,
            flights_per_session: 4,
            ..ConnectivityPolicy::default()
        },
    );
    let owner = PeerReachabilityTransport::new(
        manager,
        authority,
        PeerReachabilityTransportPorts {
            wire: Arc::new(PeerReachabilityWire),
            environment,
            address_refresh,
        },
        clock,
        contacts,
    );
    builder.install_peer_reachability(owner.clone()).unwrap();
    let mut node = network.spawn();
    node.activate_session(builder.finish()).await.unwrap();
    let address = iroh::EndpointAddr::new(secret.public())
        .with_ip_addr(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
    PresenceNode {
        owner,
        address,
        node,
    }
}
impl PresenceNode {
    pub async fn shutdown(self) {
        self.node.shutdown().await.unwrap();
    }
}
