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
    AdmissionAttempt, AdmissionGrant, ConnectionProtocol, ConnectivityError, InboundAdmissionFence,
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
                ConnectivityProfile::TransferProgress => InboundPeerProtocol::TransferProgress,
                ConnectivityProfile::Presence => InboundPeerProtocol::Presence,
                ConnectivityProfile::ActiveClipboardPull => {
                    InboundPeerProtocol::ActiveClipboardPull
                }
                ConnectivityProfile::ActiveClipboardState => InboundPeerProtocol::ActiveClipboard,
                ConnectivityProfile::Clipboard => InboundPeerProtocol::Clipboard,
                ConnectivityProfile::MembershipHistory => InboundPeerProtocol::MembershipHistory,
                ConnectivityProfile::MembershipBranchRecovery => {
                    InboundPeerProtocol::MembershipBranchRecovery
                }
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
        if self.protocol == InboundPeerProtocol::TransferProgress
            && !matches!(
                connection.protocol(),
                ConnectionProtocol::TransferProgressV1 | ConnectionProtocol::TransferProgressV2
            )
        {
            return Err(ConnectivityError::Denied);
        }
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
        // Blob 可直接交付本机缓存，没有连接 grant；请求本身也须核对当前成员权威。
        if self.owner.protocol == InboundPeerProtocol::Blob {
            return self.owner.check(self.peer, self.epoch).await;
        }
        // 其他协议仍由请求围栏验证快照，连接 grant 读取当前成员权威并负责整体关闭。
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
        if !matches!(
            self.owner.protocol,
            InboundPeerProtocol::Blob | InboundPeerProtocol::TransferProgress
        ) {
            return Ok(false);
        }
        if self.owner.protocol == InboundPeerProtocol::TransferProgress
            && self.connection.protocol() != ConnectionProtocol::TransferProgressV2
        {
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
