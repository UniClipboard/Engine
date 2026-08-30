use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tracing::{debug, warn};
use uc_core::membership::{
    GroupRevocationPort, GroupUpdateDispatchError, GroupUpdateDispatchPort, PendingGroupUpdate,
};
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
    group_revocation: Arc<dyn GroupRevocationPort>,
}

impl IrohGroupUpdateAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        group_revocation: Arc<dyn GroupRevocationPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
            handler_state: Arc::new(HandlerState { group_revocation }),
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
    use iroh::{RelayMode, SecretKey};
    use mockall::mock;
    use uc_core::ids::DeviceId;
    use uc_core::membership::{GroupEpoch, GroupRevocationResult, KeyEpochError, RevocationId};
    use uc_core::ports::{PeerAddressError, PeerAddressRecord};

    use super::*;

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
    async fn cryptographically_invalid_recovery_update_is_rejected() {
        let sender_seed = [0x45u8; 32];
        let receiver_seed = [0x46u8; 32];
        let receiver = endpoint(receiver_seed).await;
        wait_for_direct_addrs(&receiver).await;
        let sender = endpoint(sender_seed).await;
        wait_for_direct_addrs(&sender).await;

        let mut group_revocation = MockGroupRevocation::new();
        group_revocation
            .expect_apply_group_epoch_update()
            .times(1)
            .withf(|payload| payload == b"MLS")
            .returning(|_| Err(KeyEpochError::Repository("invalid update".to_owned())));
        let adapter = IrohGroupUpdateAdapter::new(
            Arc::clone(&receiver),
            Arc::new(NoPeerAddresses),
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

        let mut group_revocation = MockGroupRevocation::new();
        group_revocation
            .expect_apply_group_epoch_update()
            .times(1)
            .withf(|payload| payload == b"MLS")
            .returning(|_| Ok(GroupEpoch::new(2)));
        let adapter = IrohGroupUpdateAdapter::new(
            Arc::clone(&receiver),
            Arc::new(NoPeerAddresses),
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

    #[tokio::test]
    async fn unknown_relay_may_deliver_a_cryptographically_valid_recovery_update() {
        let sender_seed = [0x49u8; 32];
        let receiver_seed = [0x4au8; 32];
        let receiver = endpoint(receiver_seed).await;
        wait_for_direct_addrs(&receiver).await;
        let sender = endpoint(sender_seed).await;
        wait_for_direct_addrs(&sender).await;

        let mut group_revocation = MockGroupRevocation::new();
        group_revocation
            .expect_apply_group_epoch_update()
            .times(1)
            .withf(|payload| payload == b"MLS")
            .returning(|_| Ok(GroupEpoch::new(2)));
        let adapter = IrohGroupUpdateAdapter::new(
            Arc::clone(&receiver),
            Arc::new(NoPeerAddresses),
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
