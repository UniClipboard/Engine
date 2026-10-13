//! 历史身份绑定不使用普通内容 gate；当前成员资格仅决定是否可保留连接。
use super::super::admitted_connections::{
    AdmittedConnections, ConnectionRegistration, RevocationEpoch,
};
use super::super::inbound_peer::{
    record_inbound_rejection, PeerIdentityError, PeerIdentityResolver,
};
use async_trait::async_trait;
use std::sync::Arc;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, ConnectivityError, HistoryConnectionRetentionPort,
    HistoryPeerBindingPort, InboundAdmissionFence, PeerConnection, PeerIdentityDirectoryPort,
    SessionConnectionAdmissionPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::MembershipHistoryMessage;
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_observability_contract::diagnostics::connectivity::InboundPeerProtocol;

#[derive(Clone)]
pub struct HistoryConnectionAdmission {
    identity: Arc<PeerIdentityResolver>,
    retention: Arc<dyn HistoryConnectionRetentionPort>,
    connections: AdmittedConnections,
}
impl HistoryConnectionAdmission {
    pub fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        retention: Arc<dyn HistoryConnectionRetentionPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
        connections: AdmittedConnections,
    ) -> Self {
        Self {
            identity: Arc::new(PeerIdentityResolver::new(identities, fingerprint)),
            retention,
            connections,
        }
    }
    fn register(
        &self,
        peer: DeviceId,
        epoch: RevocationEpoch,
        connection: Arc<dyn PeerConnection>,
        verified: bool,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let registration = self
            .connections
            .register_managed_since(&peer, connection.clone(), epoch)
            .ok_or(ConnectivityError::Denied)?;
        Ok(Box::new(HistoryGrant {
            owner: self.clone(),
            peer,
            epoch,
            connection,
            verified,
            _registration: registration,
        }))
    }
}
struct HistoryGrant {
    owner: HistoryConnectionAdmission,
    peer: DeviceId,
    epoch: RevocationEpoch,
    connection: Arc<dyn PeerConnection>,
    verified: bool,
    _registration: ConnectionRegistration,
}
#[async_trait]
impl AdmissionGrant for HistoryGrant {
    fn peer(&self) -> DeviceId {
        self.peer
    }
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        if !self.owner.connections.current(&self.peer, self.epoch) {
            self.connection.close();
            return Err(ConnectivityError::Denied);
        }
        if self.verified {
            let actual = self
                .owner
                .identity
                .resolve(&self.connection.authenticated_key())
                .await
                .map_err(|source| ConnectivityError::Admission(Box::new(source)))?;
            if actual != self.peer {
                self.connection.close();
                return Err(ConnectivityError::Denied);
            }
        }
        if !self.owner.connections.current(&self.peer, self.epoch) {
            self.connection.close();
            return Err(ConnectivityError::Denied);
        }
        Ok(())
    }
    async fn can_retain(&self) -> Result<bool, ConnectivityError> {
        self.check_current().await?;
        if !self.verified {
            return Ok(false);
        }
        let actual = self
            .owner
            .identity
            .resolve(&self.connection.authenticated_key())
            .await
            .map_err(|source| ConnectivityError::Admission(Box::new(source)))?;
        if actual != self.peer {
            return Err(ConnectivityError::Denied);
        }
        let allowed = self
            .owner
            .retention
            .may_retain(&self.peer)
            .await
            .map_err(|source| ConnectivityError::Admission(Box::new(source)))?;
        self.check_current().await?;
        Ok(allowed)
    }
}
struct HistoryAttempt {
    owner: HistoryConnectionAdmission,
    target: DeviceId,
    epoch: RevocationEpoch,
}
#[async_trait]
impl AdmissionAttempt for HistoryAttempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        if self.owner.connections.current(&self.target, self.epoch) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.check_current().await?;
        // 未解析目录时 TLS 已按唯一地址解析器的 EndpointId 认证目标，但资格不得升级为可保留。
        let verified = match self
            .owner
            .identity
            .resolve(&connection.authenticated_key())
            .await
        {
            Ok(peer) if peer == self.target => true,
            Ok(_) => return Err(ConnectivityError::Denied),
            Err(PeerIdentityError::Unresolved) => false,
            Err(source) => return Err(ConnectivityError::Admission(Box::new(source))),
        };
        self.owner
            .register(self.target, self.epoch, connection, verified)
    }
}
struct HistoryInboundFence {
    connections: AdmittedConnections,
    revision: u64,
}
impl InboundAdmissionFence for HistoryInboundFence {
    fn check_current(&self, peer: &DeviceId) -> Result<(), ConnectivityError> {
        if self.connections.current_since(peer, self.revision) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
}
#[async_trait]
impl SessionConnectionAdmissionPort for HistoryConnectionAdmission {
    fn inbound_fence(&self) -> Option<Box<dyn InboundAdmissionFence>> {
        Some(Box::new(HistoryInboundFence {
            connections: self.connections.clone(),
            revision: self.connections.revision(),
        }))
    }
    fn begin(&self, target: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(HistoryAttempt {
            owner: self.clone(),
            target,
            epoch: self.connections.epoch(&target),
        })
    }
    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let peer = self
            .identity
            .resolve(&connection.authenticated_key())
            .await
            .map_err(|source| ConnectivityError::Admission(Box::new(source)))?;
        self.register(peer, self.connections.epoch(&peer), connection, true)
    }
}
#[async_trait]
impl HistoryPeerBindingPort for HistoryConnectionAdmission {
    async fn bind(
        &self,
        connection: Arc<dyn PeerConnection>,
        message: &MembershipHistoryMessage,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let (peer, verified) = match self.identity.resolve(&connection.authenticated_key()).await {
            Ok(peer) => (peer, true),
            Err(PeerIdentityError::Unresolved) => {
                let peer =
                    introduced_device(message, &self.identity, &connection.authenticated_key())
                        .ok_or(ConnectivityError::Denied)?;
                (peer, false)
            }
            Err(source) => {
                record_inbound_rejection(
                    InboundPeerProtocol::MembershipHistory,
                    source.rejection(),
                );
                return Err(ConnectivityError::Admission(Box::new(source)));
            }
        };
        // 临时声明只作为撤销标签登记连接，不写身份目录，也不授予普通成员资格。
        self.register(peer, self.connections.epoch(&peer), connection, verified)
    }
}
pub(super) fn introduced_device(
    message: &MembershipHistoryMessage,
    identity: &PeerIdentityResolver,
    public_key: &[u8; 32],
) -> Option<DeviceId> {
    let admission = match message {
        MembershipHistoryMessage::SummaryV3(summary) => &summary.sender_admission,
        MembershipHistoryMessage::SuffixPageV4(page) => page.sender_admission(),
        _ => return None,
    };
    identity
        .fingerprint_matches(public_key, &admission.identity_fingerprint)
        .then_some(admission.device_id)
}
