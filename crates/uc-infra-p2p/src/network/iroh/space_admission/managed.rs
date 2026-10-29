//! 非成员准入通道仅提供本协议的有界传输机会，业务权限仍由 OPAQUE 与续接证明授予。
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::{Endpoint, EndpointAddr};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, ConnectionProtocol, ConnectivityBackend, ConnectivityError,
    ConnectivityManager, ConnectivityPolicy, InboundIdentityReservation, PeerConnection,
    SessionConnectionAdmissionPort,
};
use uc_observability_contract::diagnostics::DiagnosticTaskKind;

use super::super::connectivity::IrohPeerConnection;
use super::connection::connect;

/// QUIC 认证端点的资源键；不与成员 DeviceId 互换。
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct AdmissionEndpoint(pub [u8; 32]);
pub(super) const EXCHANGE_DEADLINE: Duration = Duration::from_secs(120);
pub(super) const MAX_EXCHANGES: usize = 8;

pub(super) type AdmissionManager = ConnectivityManager<AdmissionEndpoint>;

pub(super) fn manager(endpoint: Arc<Endpoint>, capacity: usize) -> AdmissionManager {
    ConnectivityManager::new(
        Arc::new(Backend(endpoint)),
        Arc::new(ChannelAdmission),
        ConnectivityPolicy {
            supervise_local_work: true,
            connections_per_peer_direction: Some(2),
            outbound_per_peer: 2,
            inbound_per_connection: 1,
            inbound_per_session: capacity,
            waiters_per_peer: 2,
            waiters_per_session: capacity,
            flights_per_session: capacity,
            task_kind: DiagnosticTaskKind::SpaceAdmissionConnectivityWorker,
            ..ConnectivityPolicy::default()
        },
    )
}

struct Backend(Arc<Endpoint>);
#[async_trait]
impl ConnectivityBackend<AdmissionEndpoint> for Backend {
    async fn connect(
        &self,
        target: AdmissionEndpoint,
        address_hint: Option<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        let encoded = address_hint.ok_or(ConnectivityError::AddressMissing)?;
        let address: EndpointAddr = postcard::from_bytes(&encoded)
            .map_err(|source| ConnectivityError::AddressLookup(Box::new(source)))?;
        if address.id.as_bytes() != &target.0 {
            return Err(ConnectivityError::Denied);
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ConnectivityError::Stopped),
            result = connect(&self.0, address) => result
                .map(IrohPeerConnection::wrap)
                .map_err(|source| ConnectivityError::Backend(Box::new(source))),
        }
    }
}

/// 只核对认证端点及专属协议；不得用此 grant 调用任何普通成员能力。
struct ChannelAdmission;
struct Attempt(AdmissionEndpoint);
struct ChannelGrant {
    peer: AdmissionEndpoint,
    connection: Arc<dyn PeerConnection>,
}
fn check(
    connection: &dyn PeerConnection,
    peer: AdmissionEndpoint,
) -> Result<(), ConnectivityError> {
    if connection.protocol() != ConnectionProtocol::SpaceAdmission
        || connection.authenticated_key() != peer.0
        || connection.is_closed()
    {
        return Err(ConnectivityError::Denied);
    }
    Ok(())
}
#[async_trait]
impl AdmissionAttempt<AdmissionEndpoint> for Attempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        Ok(())
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant<AdmissionEndpoint>>, ConnectivityError> {
        check(connection.as_ref(), self.0)?;
        Ok(Box::new(ChannelGrant {
            peer: self.0,
            connection,
        }))
    }
}
#[async_trait]
impl AdmissionGrant<AdmissionEndpoint> for ChannelGrant {
    fn peer(&self) -> AdmissionEndpoint {
        self.peer
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        check(self.connection.as_ref(), self.peer)
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort<AdmissionEndpoint> for ChannelAdmission {
    fn begin(&self, target: AdmissionEndpoint) -> Box<dyn AdmissionAttempt<AdmissionEndpoint>> {
        Box::new(Attempt(target))
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation<AdmissionEndpoint>,
    ) -> Result<Box<dyn AdmissionGrant<AdmissionEndpoint>>, ConnectivityError> {
        let peer = AdmissionEndpoint(connection.authenticated_key());
        reservation.identified(peer)?;
        check(connection.as_ref(), peer)?;
        Ok(Box::new(ChannelGrant { peer, connection }))
    }
}
