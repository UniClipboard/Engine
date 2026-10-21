use super::super::admitted_connections::{
    AdmittedConnections, ConnectionRegistration, RevocationEpoch,
};
use super::super::inbound_peer::{
    record_inbound_rejection, InboundPeerRejection, PeerIdentityResolver,
};
use super::backend::ConnectivityProfile;
use async_trait::async_trait;
use std::sync::Arc;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, ConnectivityError, InboundAdmissionFence,
    InboundIdentityReservation, PeerConnection, PeerIdentityDirectoryPort,
    SessionConnectionAdmissionPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_observability_contract::diagnostics::connectivity::InboundPeerProtocol;

#[derive(Clone)]
pub struct IrohConnectionAdmission {
    identities: Arc<PeerIdentityResolver>,
    admission: Arc<dyn PeerAdmissionPort>,
    connections: AdmittedConnections,
    protocol: InboundPeerProtocol,
}
impl IrohConnectionAdmission {
    pub fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        admission: Arc<dyn PeerAdmissionPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
        connections: AdmittedConnections,
        profile: ConnectivityProfile,
    ) -> Self {
        Self {
            identities: Arc::new(PeerIdentityResolver::new(identities, fingerprint)),
            admission,
            connections,
            protocol: match profile {
                ConnectivityProfile::Blob => InboundPeerProtocol::Blob,
                ConnectivityProfile::Presence => InboundPeerProtocol::Presence,
                ConnectivityProfile::ActiveClipboardPull => {
                    InboundPeerProtocol::ActiveClipboardPull
                }
                ConnectivityProfile::ActiveClipboardState => InboundPeerProtocol::ActiveClipboard,
                ConnectivityProfile::Clipboard => InboundPeerProtocol::Clipboard,
                ConnectivityProfile::MembershipHistory => InboundPeerProtocol::MembershipHistory,
                ConnectivityProfile::GroupUpdate | ConnectivityProfile::LegacyGroupUpdate => {
                    InboundPeerProtocol::GroupUpdate
                }
            },
        }
    }
    async fn check(&self, peer: DeviceId, epoch: RevocationEpoch) -> Result<(), ConnectivityError> {
        if !self.connections.current(&peer, epoch) {
            return Err(ConnectivityError::Denied);
        }
        let admitted = self
            .admission
            .is_admitted(&peer)
            .await
            .map_err(|error| ConnectivityError::Admission(Box::new(error)))?;
        if !admitted || !self.connections.current(&peer, epoch) {
            return Err(ConnectivityError::Denied);
        }
        Ok(())
    }
    async fn adopt(
        &self,
        peer: DeviceId,
        epoch: RevocationEpoch,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let actual = self
            .identities
            .resolve(&connection.authenticated_key())
            .await
            .map_err(|error| ConnectivityError::Admission(Box::new(error)))?;
        if actual != peer {
            return Err(ConnectivityError::Denied);
        }
        self.register_and_check(peer, epoch, connection).await
    }
    async fn register_and_check(
        &self,
        peer: DeviceId,
        epoch: RevocationEpoch,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let registration = self
            .connections
            .register_managed_since(&peer, connection.clone(), epoch)
            .ok_or(ConnectivityError::Denied)?;
        self.check(peer, epoch).await?;
        Ok(Box::new(Grant {
            owner: self.clone(),
            peer,
            epoch,
            connection,
            _registration: registration,
        }))
    }
}
struct Attempt {
    owner: IrohConnectionAdmission,
    peer: DeviceId,
    epoch: RevocationEpoch,
}
#[async_trait]
impl AdmissionAttempt for Attempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        // 请求围栏只验证自身快照；当前成员权威读取与整体关闭由连接 grant 统一负责。
        if self.owner.connections.current(&self.peer, self.epoch) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.owner.adopt(self.peer, self.epoch, connection).await
    }
}
struct Grant {
    owner: IrohConnectionAdmission,
    peer: DeviceId,
    epoch: RevocationEpoch,
    connection: Arc<dyn PeerConnection>,
    _registration: ConnectionRegistration,
}
#[async_trait]
impl AdmissionGrant for Grant {
    fn peer(&self) -> DeviceId {
        self.peer
    }
    async fn can_retain(&self) -> Result<bool, ConnectivityError> {
        if self.owner.protocol != InboundPeerProtocol::Blob {
            return Ok(false);
        }
        self.check_current().await?;
        Ok(true)
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        let result = self.owner.check(self.peer, self.epoch).await;
        if result.is_err()
            && (self.owner.protocol != InboundPeerProtocol::Presence
                || matches!(result, Err(ConnectivityError::Denied)))
        {
            self.connection.close();
        }
        result
    }
}
struct ContentInboundFence {
    connections: AdmittedConnections,
    revision: u64,
}
impl InboundAdmissionFence for ContentInboundFence {
    fn check_current(&self, peer: &DeviceId) -> Result<(), ConnectivityError> {
        if self.connections.current_since(peer, self.revision) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for IrohConnectionAdmission {
    fn inbound_fence(&self) -> Option<Box<dyn InboundAdmissionFence>> {
        Some(Box::new(ContentInboundFence {
            connections: self.connections.clone(),
            revision: self.connections.revision(),
        }))
    }
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(Attempt {
            owner: self.clone(),
            peer: target,
            epoch: self.connections.epoch(&target),
        })
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let peer = self
            .identities
            .resolve(&connection.authenticated_key())
            .await
            .map_err(|error| {
                record_inbound_rejection(self.protocol, error.rejection());
                ConnectivityError::Admission(Box::new(error))
            })?;
        reservation.identified(peer)?;
        // 先交接额度并检查入口围栏，再冻结登记用的撤销基线。
        let result = self
            .register_and_check(peer, self.connections.epoch(&peer), connection)
            .await;
        if let Err(error) = &result {
            let rejection = if matches!(error, ConnectivityError::Denied) {
                InboundPeerRejection::LedgerDenied
            } else {
                InboundPeerRejection::LedgerUnavailable
            };
            record_inbound_rejection(self.protocol, rejection);
        }
        result
    }
}
