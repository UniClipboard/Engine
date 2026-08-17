use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tracing::{debug, warn};
use uc_core::membership::{
    CurrentWorkspaceLocalMembership, CurrentWorkspacePeerScopePort, GroupRevocationPort,
    GroupUpdateDispatchError, GroupUpdateDispatchPort, MemberRepositoryPort, PeerAdmissionPort,
    PendingGroupUpdate,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

use super::connect_with_staggered_retry;

pub const GROUP_UPDATE_ALPN: &[u8] = b"uniclipboard/group-update/1";
const MAX_UPDATE_SIZE: usize = 4 * 1024 * 1024;
const GROUP_UPDATE_IO_TIMEOUT: Duration = Duration::from_secs(10);
const ACK_ACCEPTED: u8 = 1;
const ACK_REJECTED: u8 = 2;

async fn run_outbound_io_phase<T, E>(
    timeout: Duration,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, GroupUpdateDispatchError> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| GroupUpdateDispatchError::Transport)?
        .map_err(|_| GroupUpdateDispatchError::Transport)
}

pub struct IrohGroupUpdateAdapter {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    handler_state: Arc<HandlerState>,
}

struct HandlerState {
    member_repo: Arc<dyn MemberRepositoryPort>,
    peer_admission: Arc<dyn PeerAdmissionPort>,
    current_peer_scope: Arc<dyn CurrentWorkspacePeerScopePort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    group_revocation: Arc<dyn GroupRevocationPort>,
}

impl IrohGroupUpdateAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        current_peer_scope: Arc<dyn CurrentWorkspacePeerScopePort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        group_revocation: Arc<dyn GroupRevocationPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
            handler_state: Arc::new(HandlerState {
                member_repo,
                peer_admission,
                current_peer_scope,
                fingerprint_factory,
                group_revocation,
            }),
        }
    }

    pub fn handler(&self) -> IrohGroupUpdateHandler {
        IrohGroupUpdateHandler {
            state: Arc::clone(&self.handler_state),
        }
    }

    async fn resolve_addr(&self, update: &PendingGroupUpdate) -> Option<EndpointAddr> {
        match self.peer_addr_repo.get(update.recipient()).await {
            Ok(Some(record)) => postcard::from_bytes(&record.addr_blob).ok(),
            Ok(None) => None,
            Err(error) => {
                warn!(error = %error, "group update address lookup failed");
                None
            }
        }
    }
}

#[async_trait]
impl GroupUpdateDispatchPort for IrohGroupUpdateAdapter {
    async fn dispatch_group_update(
        &self,
        update: &PendingGroupUpdate,
    ) -> Result<(), GroupUpdateDispatchError> {
        if update.payload().len() > MAX_UPDATE_SIZE {
            return Err(GroupUpdateDispatchError::Transport);
        }
        let addr = self
            .resolve_addr(update)
            .await
            .ok_or(GroupUpdateDispatchError::Offline)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            addr,
            GROUP_UPDATE_ALPN,
            "group-update",
        )
        .await
        .map_err(|_| GroupUpdateDispatchError::Offline)?;
        let (mut send, mut recv) =
            run_outbound_io_phase(GROUP_UPDATE_IO_TIMEOUT, connection.open_bi()).await?;
        let length = u32::try_from(update.payload().len())
            .map_err(|_| GroupUpdateDispatchError::Transport)?;
        run_outbound_io_phase(
            GROUP_UPDATE_IO_TIMEOUT,
            send.write_all(&length.to_be_bytes()),
        )
        .await?;
        run_outbound_io_phase(GROUP_UPDATE_IO_TIMEOUT, send.write_all(update.payload())).await?;
        send.finish()
            .map_err(|_| GroupUpdateDispatchError::Transport)?;
        let mut ack = [0u8; 1];
        run_outbound_io_phase(GROUP_UPDATE_IO_TIMEOUT, recv.read_exact(&mut ack)).await?;
        match ack[0] {
            ACK_ACCEPTED => Ok(()),
            ACK_REJECTED => Err(GroupUpdateDispatchError::Rejected),
            _ => Err(GroupUpdateDispatchError::Transport),
        }
    }
}

#[derive(Clone)]
pub struct IrohGroupUpdateHandler {
    state: Arc<HandlerState>,
}

impl std::fmt::Debug for IrohGroupUpdateHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohGroupUpdateHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohGroupUpdateHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer_device_id = self
            .state
            .resolve_device(connection.remote_id().as_bytes())
            .await;
        let (mut send, mut recv) =
            match tokio::time::timeout(GROUP_UPDATE_IO_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                Ok(Err(error)) => {
                    debug!(error = %error, "group update stream accept failed");
                    return Ok(());
                }
                Err(_) => {
                    debug!("group update stream accept timed out");
                    return Ok(());
                }
            };
        let Some(peer_device_id) = peer_device_id else {
            emit_ack(&mut send, ACK_REJECTED).await;
            let _ = connection.closed().await;
            return Ok(());
        };
        if !self.state.may_deliver_recovery(&peer_device_id).await {
            warn!(
                peer = %peer_device_id.as_str(),
                "group update: peer is not admitted by current space protection"
            );
            emit_ack(&mut send, ACK_REJECTED).await;
            let _ = connection.closed().await;
            return Ok(());
        }
        let mut length = [0u8; 4];
        if !matches!(
            tokio::time::timeout(GROUP_UPDATE_IO_TIMEOUT, recv.read_exact(&mut length)).await,
            Ok(Ok(_))
        ) {
            emit_ack(&mut send, ACK_REJECTED).await;
            let _ = connection.closed().await;
            return Ok(());
        }
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_UPDATE_SIZE {
            emit_ack(&mut send, ACK_REJECTED).await;
            let _ = connection.closed().await;
            return Ok(());
        }
        let mut payload = vec![0u8; length];
        if !matches!(
            tokio::time::timeout(GROUP_UPDATE_IO_TIMEOUT, recv.read_exact(&mut payload)).await,
            Ok(Ok(_))
        ) {
            emit_ack(&mut send, ACK_REJECTED).await;
            let _ = connection.closed().await;
            return Ok(());
        }
        let ack = if self
            .state
            .group_revocation
            .apply_group_epoch_update(&payload)
            .await
            .is_ok()
        {
            ACK_ACCEPTED
        } else {
            ACK_REJECTED
        };
        emit_ack(&mut send, ack).await;
        let _ = connection.closed().await;
        Ok(())
    }
}

impl HandlerState {
    async fn resolve_device(&self, public_key: &[u8; 32]) -> Option<uc_core::ids::DeviceId> {
        let fingerprint = self.fingerprint_factory.from_public_key(public_key).ok()?;
        let members = match self.member_repo.list().await {
            Ok(members) => members,
            Err(error) => {
                warn!(error = %error, "group update member lookup failed");
                return None;
            }
        };
        members
            .into_iter()
            .find(|member| member.identity_fingerprint == fingerprint)
            .map(|member| member.device_id)
    }

    async fn is_admitted(&self, device_id: &uc_core::ids::DeviceId) -> bool {
        match self.peer_admission.is_admitted(device_id).await {
            Ok(admitted) => admitted,
            Err(error) => {
                warn!(error = %error, peer = %device_id.as_str(), "group update: peer admission check failed");
                false
            }
        }
    }

    async fn may_deliver_recovery(&self, device_id: &uc_core::ids::DeviceId) -> bool {
        if self.is_admitted(device_id).await {
            return true;
        }
        matches!(
            self.current_peer_scope.snapshot().await,
            Ok(snapshot)
                if snapshot.local_membership == CurrentWorkspaceLocalMembership::Active
                    && snapshot.peer_device_ids.contains(device_id)
        )
    }
}

async fn emit_ack(send: &mut iroh::endpoint::SendStream, ack: u8) {
    if matches!(
        tokio::time::timeout(GROUP_UPDATE_IO_TIMEOUT, send.write_all(&[ack])).await,
        Ok(Ok(()))
    ) {
        let _ = send.finish();
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use iroh::{RelayMode, SecretKey};
    use mockall::mock;
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        CurrentWorkspaceLocalMembership, CurrentWorkspacePeerScopeError,
        CurrentWorkspacePeerScopePort, CurrentWorkspacePeerScopeSource,
        CurrentWorkspacePeerSnapshot, GroupEpoch, GroupRevocationResult, KeyEpochError,
        MemberRepositoryPort, MembershipError, RevocationId, SpaceMember,
    };
    use uc_core::ports::{PeerAddressError, PeerAddressRecord};
    use uc_core::MemberSyncPreferences;

    use super::*;
    use crate::security::Sha256IdentityFingerprintFactory;

    mock! {
        GroupRevocation {}

        #[async_trait]
        impl GroupRevocationPort for GroupRevocation {
            async fn revoke_group_member(&self, target: &DeviceId, retained_recipients: &[DeviceId], now_ms: i64) -> Result<GroupRevocationResult, KeyEpochError>;
            async fn acknowledge_group_update(&self, revocation_id: &RevocationId, recipient: &DeviceId, now_ms: i64) -> Result<GroupRevocationResult, KeyEpochError>;
            async fn apply_group_epoch_update(&self, payload: &[u8]) -> Result<GroupEpoch, KeyEpochError>;
            async fn pending_group_updates(&self, revocation_id: &RevocationId) -> Result<Vec<PendingGroupUpdate>, KeyEpochError>;
            async fn query_group_revocation(&self, revocation_id: &RevocationId) -> Result<Option<GroupRevocationResult>, KeyEpochError>;
            async fn resume_group_revocations(&self, now_ms: i64) -> Result<Vec<GroupRevocationResult>, KeyEpochError>;
            async fn pending_space_group_updates(&self) -> Result<Vec<PendingGroupUpdate>, KeyEpochError>;
            async fn acknowledge_space_group_update(&self, update_id: &str, now_ms: i64) -> Result<bool, KeyEpochError>;
        }
    }

    struct NoPeerAddresses;

    struct FixedPeerScope(Vec<DeviceId>);

    #[async_trait]
    impl CurrentWorkspacePeerScopePort for FixedPeerScope {
        async fn snapshot(
            &self,
        ) -> Result<CurrentWorkspacePeerSnapshot, CurrentWorkspacePeerScopeError> {
            Ok(CurrentWorkspacePeerSnapshot {
                revision: 1,
                source: CurrentWorkspacePeerScopeSource::CurrentHistory,
                local_membership: CurrentWorkspaceLocalMembership::Active,
                peer_device_ids: self.0.clone(),
            })
        }
    }

    #[async_trait]
    impl PeerAddressRepositoryPort for NoPeerAddresses {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(None)
        }
        async fn upsert(&self, _record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            Ok(())
        }
        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(Vec::new())
        }
        async fn remove(&self, _device: &DeviceId) -> Result<(), PeerAddressError> {
            Ok(())
        }
    }

    mock! {
        Members {}

        #[async_trait]
        impl MemberRepositoryPort for Members {
            async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError>;
            async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError>;
            async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError>;
            async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError>;
        }
    }

    async fn endpoint(seed: [u8; 32]) -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(SecretKey::from_bytes(&seed))
                .alpns(vec![GROUP_UPDATE_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .expect("bind endpoint"),
        )
    }

    async fn wait_for_direct_addrs(endpoint: &Endpoint) {
        for _ in 0..100 {
            if !endpoint.addr().addrs.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("endpoint never published direct addresses");
    }

    fn member_for(seed: [u8; 32], device_id: &str) -> SpaceMember {
        let key = SecretKey::from_bytes(&seed);
        let identity_fingerprint = Sha256IdentityFingerprintFactory
            .from_public_key(key.public().as_bytes())
            .expect("derive fingerprint");
        SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: "Test Device".to_owned(),
            identity_fingerprint,
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        }
    }

    #[tokio::test]
    async fn outbound_io_timeout_maps_to_transport_failure() {
        let error = run_outbound_io_phase(
            Duration::from_millis(1),
            pending::<Result<(), std::io::Error>>(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, GroupUpdateDispatchError::Transport);
    }

    #[tokio::test]
    async fn known_but_unadmitted_peer_is_rejected_before_epoch_update() {
        let sender_seed = [0x45u8; 32];
        let receiver_seed = [0x46u8; 32];
        let receiver = endpoint(receiver_seed).await;
        wait_for_direct_addrs(&receiver).await;
        let sender = endpoint(sender_seed).await;
        wait_for_direct_addrs(&sender).await;

        let mut members = MockMembers::new();
        members
            .expect_list()
            .times(1)
            .return_once(move || Ok(vec![member_for(sender_seed, "revoked-member")]));
        let mut group_revocation = MockGroupRevocation::new();
        group_revocation.expect_apply_group_epoch_update().times(0);
        let adapter = IrohGroupUpdateAdapter::new(
            Arc::clone(&receiver),
            Arc::new(NoPeerAddresses),
            Arc::new(members),
            Arc::new(crate::network::iroh::StaticPeerAdmission(false)),
            Arc::new(FixedPeerScope(Vec::new())),
            Arc::new(Sha256IdentityFingerprintFactory),
            Arc::new(group_revocation),
        );
        let router = iroh::protocol::Router::builder((*receiver).clone())
            .accept(GROUP_UPDATE_ALPN, adapter.handler())
            .spawn();

        let connection = sender
            .connect(receiver.addr(), GROUP_UPDATE_ALPN)
            .await
            .expect("dial receiver");
        let (mut send, mut recv) = connection.open_bi().await.expect("open stream");
        send.write_all(&(3u32).to_be_bytes())
            .await
            .expect("write length");
        send.write_all(b"MLS").await.expect("write payload");
        send.finish().expect("finish request");
        let mut ack = [0u8; 1];
        recv.read_exact(&mut ack).await.expect("read rejection ack");
        assert_eq!(ack[0], ACK_REJECTED);

        router.shutdown().await.ok();
        sender.close().await;
    }

    #[tokio::test]
    async fn current_history_member_may_deliver_a_valid_recovery_update() {
        let sender_seed = [0x47u8; 32];
        let receiver_seed = [0x48u8; 32];
        let receiver = endpoint(receiver_seed).await;
        wait_for_direct_addrs(&receiver).await;
        let sender = endpoint(sender_seed).await;
        wait_for_direct_addrs(&sender).await;

        let mut members = MockMembers::new();
        members
            .expect_list()
            .times(1)
            .return_once(move || Ok(vec![member_for(sender_seed, "relay-member")]));
        let mut group_revocation = MockGroupRevocation::new();
        group_revocation
            .expect_apply_group_epoch_update()
            .times(1)
            .withf(|payload| payload == b"MLS")
            .returning(|_| Ok(GroupEpoch::new(2)));
        let adapter = IrohGroupUpdateAdapter::new(
            Arc::clone(&receiver),
            Arc::new(NoPeerAddresses),
            Arc::new(members),
            Arc::new(crate::network::iroh::StaticPeerAdmission(false)),
            Arc::new(FixedPeerScope(vec![DeviceId::new("relay-member")])),
            Arc::new(Sha256IdentityFingerprintFactory),
            Arc::new(group_revocation),
        );
        let router = iroh::protocol::Router::builder((*receiver).clone())
            .accept(GROUP_UPDATE_ALPN, adapter.handler())
            .spawn();

        let connection = sender
            .connect(receiver.addr(), GROUP_UPDATE_ALPN)
            .await
            .expect("dial receiver");
        let (mut send, mut recv) = connection.open_bi().await.expect("open stream");
        send.write_all(&(3u32).to_be_bytes())
            .await
            .expect("write length");
        send.write_all(b"MLS").await.expect("write payload");
        send.finish().expect("finish request");
        let mut ack = [0u8; 1];
        recv.read_exact(&mut ack).await.expect("read accepted ack");
        assert_eq!(ack[0], ACK_ACCEPTED);

        router.shutdown().await.ok();
        sender.close().await;
    }
}
