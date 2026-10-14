//! 测试使用真实后端和准入，显式提供未关注的地址/在线能力。
use super::super::admitted_connections::AdmittedConnections;
use super::super::connectivity::{
    ConnectivityProfile, IrohConnectionAdmission, IrohConnectivityBackend,
};
use super::super::StaticPeerAdmission;
use async_trait::async_trait;
use iroh::Endpoint;
use std::sync::Arc;
use tokio::sync::broadcast;
use uc_application::deps::{
    ConnectivityManager, ConnectivityPolicy, CurrentSpaceMemberScope, CurrentSpaceMemberScopeError,
    CurrentSpaceMemberScopePort, PeerIdentityDirectoryPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::MemberRepositoryPort;
use uc_core::ports::{
    PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort, PeerReachabilityChanged,
    PeerReachabilityError, PeerReachabilityPort, ReachabilityState,
};
use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

pub(super) struct EmptyAddresses;
#[async_trait]
impl PeerAddressRepositoryPort for EmptyAddresses {
    async fn get(&self, _: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
        Ok(None)
    }
    async fn upsert(&self, _: &PeerAddressRecord) -> Result<(), PeerAddressError> {
        Ok(())
    }
    async fn remove(&self, _: &DeviceId) -> Result<(), PeerAddressError> {
        Ok(())
    }
    async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
        Ok(vec![])
    }
}
struct UnknownReachability(broadcast::Sender<PeerReachabilityChanged>);
#[async_trait]
impl PeerReachabilityPort for UnknownReachability {
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
pub(super) fn manager(
    endpoint: Arc<Endpoint>,
    addresses: Arc<dyn PeerAddressRepositoryPort>,
    identities: Arc<dyn PeerIdentityDirectoryPort>,
    admitted: bool,
    profile: ConnectivityProfile,
) -> ConnectivityManager {
    let (events, _) = broadcast::channel(64);
    ConnectivityManager::new(
        Arc::new(IrohConnectivityBackend::new(
            endpoint,
            addresses,
            Arc::new(UnknownReachability(events)),
            profile,
        )),
        Arc::new(IrohConnectionAdmission::new(
            identities,
            Arc::new(StaticPeerAdmission(admitted)),
            Arc::new(Sha256IdentityFingerprintFactory),
            AdmittedConnections::default(),
            profile,
        )),
        ConnectivityPolicy {
            inbound_per_connection: 1,
            ..ConnectivityPolicy::default()
        },
    )
}

struct TestMemberScope {
    members: Arc<dyn MemberRepositoryPort>,
    allowed: bool,
}
#[async_trait]
impl CurrentSpaceMemberScopePort for TestMemberScope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        let peers = if self.allowed {
            self.members
                .list()
                .await
                .map_err(CurrentSpaceMemberScopeError::recovery_required_from)?
                .into_iter()
                .map(|member| member.device_id)
                .collect()
        } else {
            vec![]
        };
        Ok(CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: true,
            usable_peer_device_ids: peers,
            paused_peer_devices: vec![],
        })
    }
}
pub(super) fn content_scope(
    members: Arc<dyn MemberRepositoryPort>,
    allowed: bool,
) -> Arc<dyn CurrentSpaceMemberScopePort> {
    Arc::new(TestMemberScope { members, allowed })
}
