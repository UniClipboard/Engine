//! 分支恢复连接只接受已知的当前成员身份，业务资格仍由恢复负责人核验。
use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    AdmissionAttempt, AdmissionGrant, BranchRecoveryPeerPort, ConnectionProtocol,
    ConnectivityError, InboundAdmissionFence, InboundIdentityReservation, PeerConnection,
    PeerIdentityDirectoryPort, SessionConnectionAdmissionPort,
};
use uc_core::ids::DeviceId;
use uc_core::ports::security::IdentityFingerprintFactoryPort;

use super::super::admitted_connections::{
    AdmittedConnections, ConnectionRegistration, RevocationEpoch,
};
use super::super::inbound_peer::PeerIdentityResolver;

#[derive(Clone)]
pub struct BranchRecoveryAdmission {
    identity: Arc<PeerIdentityResolver>,
    membership: Arc<dyn BranchRecoveryPeerPort>,
    connections: AdmittedConnections,
}

impl BranchRecoveryAdmission {
    pub fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        membership: Arc<dyn BranchRecoveryPeerPort>,
        fingerprint: Arc<dyn IdentityFingerprintFactoryPort>,
        connections: AdmittedConnections,
    ) -> Self {
        Self {
            identity: Arc::new(PeerIdentityResolver::new(identities, fingerprint)),
            membership,
            connections,
        }
    }

    async fn check(&self, peer: DeviceId, epoch: RevocationEpoch) -> Result<(), ConnectivityError> {
        if !self.connections.current(&peer, epoch) {
            return Err(ConnectivityError::Denied);
        }
        let admitted = self
            .membership
            .is_current_recovery_peer(&peer)
            .await
            .map_err(|source| ConnectivityError::Admission(Box::new(source)))?;
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
        if connection.protocol() != ConnectionProtocol::MembershipBranchRecovery {
            return Err(ConnectivityError::Denied);
        }
        let actual = self
            .identity
            .resolve(&connection.authenticated_key())
            .await
            .map_err(|source| ConnectivityError::Admission(Box::new(source)))?;
        if actual != peer {
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

struct Grant {
    owner: BranchRecoveryAdmission,
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

    fn check_frozen(&self) -> Result<(), ConnectivityError> {
        if self.owner.connections.current(&self.peer, self.epoch) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }

    async fn check_current(&self) -> Result<(), ConnectivityError> {
        self.owner.check(self.peer, self.epoch).await?;
        let actual = self
            .owner
            .identity
            .resolve(&self.connection.authenticated_key())
            .await
            .map_err(|source| ConnectivityError::Admission(Box::new(source)))?;
        if actual != self.peer {
            return Err(ConnectivityError::Denied);
        }
        self.owner.check(self.peer, self.epoch).await
    }
}

struct Attempt {
    owner: BranchRecoveryAdmission,
    peer: DeviceId,
    epoch: RevocationEpoch,
}

#[async_trait]
impl AdmissionAttempt for Attempt {
    async fn check_current(&self) -> Result<(), ConnectivityError> {
        self.owner.check(self.peer, self.epoch).await
    }

    async fn adopt(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.owner.adopt(self.peer, self.epoch, connection).await
    }
}

struct Fence {
    connections: AdmittedConnections,
    revision: u64,
}

impl InboundAdmissionFence for Fence {
    fn check_current(&self, peer: &DeviceId) -> Result<(), ConnectivityError> {
        if self.connections.current_since(peer, self.revision) {
            Ok(())
        } else {
            Err(ConnectivityError::Denied)
        }
    }
}

#[async_trait]
impl SessionConnectionAdmissionPort for BranchRecoveryAdmission {
    fn inbound_fence(&self) -> Option<Box<dyn InboundAdmissionFence>> {
        Some(Box::new(Fence {
            connections: self.connections.clone(),
            revision: self.connections.revision(),
        }))
    }

    fn begin(&self, peer: DeviceId) -> Box<dyn AdmissionAttempt> {
        Box::new(Attempt {
            owner: self.clone(),
            peer,
            epoch: self.connections.epoch(&peer),
        })
    }

    async fn adopt_inbound(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        let peer = self
            .identity
            .resolve(&connection.authenticated_key())
            .await
            .map_err(|source| ConnectivityError::Admission(Box::new(source)))?;
        reservation.identified(peer)?;
        let grant = self
            .adopt(peer, self.connections.epoch(&peer), connection)
            .await?;
        reservation.identified(peer)?;
        Ok(grant)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use tokio::sync::Semaphore;
    use uc_application::deps::{
        BranchRecoveryPeerPort, KnownPeerIdentity, MembershipLedgerError,
        PeerIdentityDirectoryPort, SessionConnectionAdmissionPort,
    };
    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

    use super::super::super::inbound_peer::PeerIdentityError;
    use super::super::stream::memory_backend_fixture::recovery_connection_pair;
    use super::{
        AdmittedConnections, BranchRecoveryAdmission, ConnectivityError, DeviceId, Fence,
        InboundAdmissionFence,
    };

    struct Members {
        identity: KnownPeerIdentity,
        active: AtomicBool,
    }

    #[async_trait]
    impl PeerIdentityDirectoryPort for Members {
        async fn known_peer_identities(
            &self,
        ) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
            Ok(vec![self.identity.clone()])
        }
    }

    #[async_trait]
    impl BranchRecoveryPeerPort for Members {
        async fn is_current_recovery_peer(
            &self,
            peer: &DeviceId,
        ) -> Result<bool, MembershipLedgerError> {
            Ok(*peer == self.identity.device_id && self.active.load(Ordering::SeqCst))
        }
    }

    #[tokio::test]
    async fn admitted_connection_retains_original_revocation_epoch_after_readmission() {
        let key = [9; 32];
        let peer = DeviceId::new("peer");
        let fingerprint = Arc::new(Sha256IdentityFingerprintFactory);
        let members = Arc::new(Members {
            identity: KnownPeerIdentity {
                device_id: peer,
                identity_fingerprint: fingerprint.from_public_key(&key).unwrap(),
            },
            active: AtomicBool::new(true),
        });
        let connections = AdmittedConnections::default();
        let admission = BranchRecoveryAdmission::new(
            members.clone(),
            members.clone(),
            fingerprint,
            connections.clone(),
        );
        let (old_connection, _) = recovery_connection_pair(1, key);
        let old_fence = admission.inbound_fence();
        let old = admission
            .adopt_inbound(old_connection, &old_fence)
            .await
            .unwrap();
        assert!(old.check_frozen().is_ok());
        assert!(old.check_current().await.is_ok());

        members.active.store(false, Ordering::SeqCst);
        connections.close_device(&peer, b"test retirement");
        members.active.store(true, Ordering::SeqCst);
        let (new_connection, _) = recovery_connection_pair(2, key);
        let new_fence = admission.inbound_fence();
        let new = admission
            .adopt_inbound(new_connection, &new_fence)
            .await
            .unwrap();
        assert!(old.check_frozen().is_err());
        assert!(old.check_current().await.is_err());
        assert!(new.check_frozen().is_ok());
        assert!(new.check_current().await.is_ok());
    }

    #[tokio::test]
    async fn identity_read_started_before_revocation_cannot_admit_rejoined_peer() {
        struct DelayedDirectory {
            identity: KnownPeerIdentity,
            first: AtomicBool,
            entered: Semaphore,
            release: Semaphore,
        }

        #[async_trait]
        impl PeerIdentityDirectoryPort for DelayedDirectory {
            async fn known_peer_identities(
                &self,
            ) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
                if self.first.swap(false, Ordering::SeqCst) {
                    self.entered.add_permits(1);
                    self.release.acquire().await.unwrap().forget();
                }
                Ok(vec![self.identity.clone()])
            }
        }

        let key = [9; 32];
        let peer = DeviceId::new("peer");
        let fingerprint = Arc::new(Sha256IdentityFingerprintFactory);
        let identity = KnownPeerIdentity {
            device_id: peer,
            identity_fingerprint: fingerprint.from_public_key(&key).unwrap(),
        };
        let directory = Arc::new(DelayedDirectory {
            identity: identity.clone(),
            first: AtomicBool::new(true),
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        let members = Arc::new(Members {
            identity,
            active: AtomicBool::new(true),
        });
        let connections = AdmittedConnections::default();
        let admission = BranchRecoveryAdmission::new(
            directory.clone(),
            members.clone(),
            fingerprint,
            connections.clone(),
        );
        let (old_connection, _) = recovery_connection_pair(1, key);
        let fence = admission.inbound_fence();
        let old = tokio::spawn({
            let admission = admission.clone();
            async move { admission.adopt_inbound(old_connection, &fence).await }
        });
        tokio::time::timeout(Duration::from_secs(5), directory.entered.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
        members.active.store(false, Ordering::SeqCst);
        connections.close_device(&peer, b"test retirement");
        members.active.store(true, Ordering::SeqCst);
        directory.release.add_permits(1);
        assert!(matches!(old.await.unwrap(), Err(ConnectivityError::Denied)));

        let (new_connection, _) = recovery_connection_pair(2, key);
        let new_fence = admission.inbound_fence();
        let new = admission
            .adopt_inbound(new_connection, &new_fence)
            .await
            .unwrap();
        assert!(new.check_frozen().is_ok());
        assert!(new.check_current().await.is_ok());
    }

    #[tokio::test]
    async fn recovery_admission_rejects_unknown_ambiguous_and_wrong_peer() {
        struct Directory(Vec<KnownPeerIdentity>);

        #[async_trait]
        impl PeerIdentityDirectoryPort for Directory {
            async fn known_peer_identities(
                &self,
            ) -> Result<Vec<KnownPeerIdentity>, MembershipLedgerError> {
                Ok(self.0.clone())
            }
        }

        let key = [9; 32];
        let peer = DeviceId::new("peer");
        let other = DeviceId::new("other");
        let fingerprint = Arc::new(Sha256IdentityFingerprintFactory);
        let identity = KnownPeerIdentity {
            device_id: peer,
            identity_fingerprint: fingerprint.from_public_key(&key).unwrap(),
        };
        let members = Arc::new(Members {
            identity: identity.clone(),
            active: AtomicBool::new(true),
        });
        for (identities, ambiguous) in [
            (Vec::new(), false),
            (
                vec![
                    identity.clone(),
                    KnownPeerIdentity {
                        device_id: other,
                        identity_fingerprint: identity.identity_fingerprint.clone(),
                    },
                ],
                true,
            ),
        ] {
            let admission = BranchRecoveryAdmission::new(
                Arc::new(Directory(identities)),
                members.clone(),
                fingerprint.clone(),
                AdmittedConnections::default(),
            );
            let (connection, _) = recovery_connection_pair(1, key);
            let fence = admission.inbound_fence();
            let Err(ConnectivityError::Admission(source)) =
                admission.adopt_inbound(connection, &fence).await
            else {
                panic!("身份解析失败必须拒绝恢复连接并保留来源");
            };
            let source = source.downcast_ref::<PeerIdentityError>();
            assert!(if ambiguous {
                matches!(source, Some(PeerIdentityError::Ambiguous))
            } else {
                matches!(source, Some(PeerIdentityError::Unresolved))
            });
        }
        let admission = BranchRecoveryAdmission::new(
            Arc::new(Directory(vec![identity])),
            members,
            fingerprint,
            AdmittedConnections::default(),
        );
        let (connection, _) = recovery_connection_pair(2, key);
        assert!(matches!(
            admission.begin(other).adopt(connection).await,
            Err(ConnectivityError::Denied)
        ));
    }

    #[test]
    fn retired_fence_stays_invalid_after_peer_is_readmitted() {
        let connections = AdmittedConnections::default();
        let peer = DeviceId::new("peer");
        let old = Fence {
            connections: connections.clone(),
            revision: connections.revision(),
        };
        assert!(old.check_current(&peer).is_ok());
        connections.close_device(&peer, b"test retirement");
        let current = Fence {
            connections: connections.clone(),
            revision: connections.revision(),
        };
        assert!(old.check_current(&peer).is_err());
        assert!(current.check_current(&peer).is_ok());
        connections.close_all(b"test session shutdown");
        assert!(old.check_current(&peer).is_err());
        assert!(current.check_current(&peer).is_err());
    }
}
