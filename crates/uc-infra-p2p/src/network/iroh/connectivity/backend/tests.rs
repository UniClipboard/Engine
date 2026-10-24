use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, RelayMode};
use tokio::sync::{broadcast, mpsc};
use uc_application::deps::{ConnectionMode, ConnectionProtocol};
use uc_core::ports::{
    PeerAddressError, PeerAddressRecord, PeerReachabilityChanged, PeerReachabilityError,
    ReachabilityState,
};

use super::{
    ConnectivityBackend, ConnectivityProfile, DeviceId, IrohConnectivityBackend,
    PeerAddressRepositoryPort, PeerReachabilityPort,
};
use crate::network::iroh::connectivity::IrohPeerConnection;
use uc_sync_protocol::clipboard::MULTIPLEXED_CLIPBOARD_ALPN;
use uc_sync_protocol::group_update::{GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN};

struct Address(EndpointAddr);
#[async_trait]
impl PeerAddressRepositoryPort for Address {
    async fn get(&self, device: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        Ok(Some(PeerAddressRecord {
            device_id: *device,
            addr_blob: postcard::to_stdvec(&self.0).unwrap(),
            observed_at: chrono::Utc::now(),
        }))
    }
    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        Ok(Vec::new())
    }
    async fn upsert(&self, _: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        Ok(())
    }
    async fn remove(&self, _: &DeviceId) -> Result<(), PeerAddressError> {
        Ok(())
    }
}
struct Reachability(broadcast::Sender<PeerReachabilityChanged>);
#[async_trait]
impl PeerReachabilityPort for Reachability {
    async fn ensure_reachable(
        &self,
        _: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        Ok(ReachabilityState::Unknown)
    }
    async fn current_state(&self, _: &DeviceId) -> ReachabilityState {
        ReachabilityState::Unknown
    }
    fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
        self.0.subscribe()
    }
}

#[derive(Debug, Clone)]
struct Capture(mpsc::Sender<(ConnectionProtocol, ConnectionMode)>);
impl ProtocolHandler for Capture {
    async fn accept(&self, native: Connection) -> Result<(), AcceptError> {
        let connection = IrohPeerConnection::wrap(native);
        self.0
            .send((connection.protocol(), connection.mode()))
            .await
            .unwrap();
        connection.closed().await;
        Ok(())
    }
}

async fn endpoint() -> Arc<Endpoint> {
    let endpoint = Arc::new(
        Endpoint::builder(iroh::endpoint::presets::N0)
            .relay_mode(RelayMode::Disabled)
            .bind()
            .await
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while endpoint.addr().ip_addrs().next().is_none() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    endpoint
}

#[tokio::test]
async fn clipboard_and_group_profiles_negotiate_distinct_connections_to_the_same_peer() {
    let server = endpoint().await;
    let client = endpoint().await;
    let (tx, mut rx) = mpsc::channel(2);
    let router = Router::builder((*server).clone())
        .accept(MULTIPLEXED_CLIPBOARD_ALPN, Capture(tx.clone()))
        .accept(GROUP_UPDATE_ALPN, Capture(tx))
        .spawn();
    let addresses = Arc::new(Address(server.addr()));
    let reachability = Arc::new(Reachability(broadcast::channel(8).0));
    let clipboard = IrohConnectivityBackend::new(
        client.clone(),
        addresses.clone(),
        Some(reachability.clone()),
        ConnectivityProfile::Clipboard,
    );
    let group = IrohConnectivityBackend::new(
        client.clone(),
        addresses,
        Some(reachability),
        ConnectivityProfile::GroupUpdate,
    );
    let clipboard = clipboard
        .connect(DeviceId::new("receiver"), None, CancellationToken::new())
        .await
        .unwrap();
    let group = group
        .connect(DeviceId::new("receiver"), None, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        clipboard.protocol(),
        ConnectionProtocol::MultiplexedClipboard
    );
    assert_eq!(group.protocol(), ConnectionProtocol::GroupUpdate);
    assert_eq!(clipboard.mode(), ConnectionMode::Multiplexed);
    assert_eq!(group.mode(), ConnectionMode::SingleExchange);
    assert_ne!(clipboard.id(), group.id());
    let first = rx.recv().await.unwrap();
    let second = rx.recv().await.unwrap();
    assert!([first, second].contains(&(
        ConnectionProtocol::GroupUpdate,
        ConnectionMode::SingleExchange
    )));
    clipboard.close();
    group.close();
    router.shutdown().await.unwrap();
    client.close().await;
}

#[tokio::test]
async fn legacy_only_peer_reports_the_actual_group_protocol_after_negotiation() {
    let server = endpoint().await;
    let client = endpoint().await;
    let (tx, mut rx) = mpsc::channel(1);
    let router = Router::builder((*server).clone())
        .accept(LEGACY_GROUP_UPDATE_ALPN, Capture(tx))
        .spawn();
    let backend = IrohConnectivityBackend::new(
        client.clone(),
        Arc::new(Address(server.addr())),
        Some(Arc::new(Reachability(broadcast::channel(8).0))),
        ConnectivityProfile::GroupUpdate,
    );
    let connection = backend
        .connect(DeviceId::new("receiver"), None, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(connection.protocol(), ConnectionProtocol::LegacyGroupUpdate);
    assert_eq!(connection.mode(), ConnectionMode::SingleExchange);
    assert_eq!(
        rx.recv().await.unwrap(),
        (
            ConnectionProtocol::LegacyGroupUpdate,
            ConnectionMode::SingleExchange
        )
    );
    connection.close();
    router.shutdown().await.unwrap();
    client.close().await;
}
