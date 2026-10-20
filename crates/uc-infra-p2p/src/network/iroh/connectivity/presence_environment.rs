//! presence 复用既有身份目录、统一撤销与网络恢复能力的桥。
use super::super::admitted_connections::AdmittedConnections;
use super::super::inbound_peer::{
    record_inbound_rejection, PeerIdentityError, PeerIdentityResolver,
};
use super::super::net_recovery::{
    DemandRecoveryCoordinator, NetworkRecoveryObservation, NetworkRecoveryObservationSource,
};
use async_trait::async_trait;
use std::sync::Arc;
use uc_application::deps::{
    ConnectivityError, PeerIdentityDirectoryPort, PeerReachabilityEnvironmentPort,
};
use uc_core::ids::DeviceId;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_observability_contract::diagnostics::connectivity::InboundPeerProtocol;

pub(crate) struct PresenceEnvironment {
    identity: PeerIdentityResolver,
    connections: AdmittedConnections,
    recovery: Arc<DemandRecoveryCoordinator>,
    observations: Arc<NetworkRecoveryObservationSource>,
}
impl PresenceEnvironment {
    pub(crate) fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
        connections: AdmittedConnections,
        recovery: Arc<DemandRecoveryCoordinator>,
        observations: Arc<NetworkRecoveryObservationSource>,
    ) -> Self {
        Self {
            identity: PeerIdentityResolver::new(identities, fingerprint),
            connections,
            recovery,
            observations,
        }
    }
}
#[async_trait]
impl PeerReachabilityEnvironmentPort for PresenceEnvironment {
    async fn identify(&self, key: &[u8; 32]) -> Result<DeviceId, ConnectivityError> {
        self.identity
            .resolve(key)
            .await
            .map_err(|source| match source {
                PeerIdentityError::Unresolved | PeerIdentityError::Ambiguous => {
                    record_inbound_rejection(InboundPeerProtocol::Presence, source.rejection());
                    ConnectivityError::Denied
                }
                source => ConnectivityError::Admission(Box::new(source)),
            })
    }
    async fn recover_for_demand(&self) {
        self.recovery.recover_for_demand().await;
    }
    fn forget(&self, peer: &DeviceId) {
        self.connections.close_device(peer, b"forgotten");
    }
    fn disconnect_all(&self) {
        self.connections.close_all(b"space_left");
    }
    fn communication_failed(&self, peer: DeviceId) {
        self.observations
            .publish(NetworkRecoveryObservation::CommunicationFailed(peer));
    }
}
