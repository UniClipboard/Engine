use std::sync::Arc;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tracing::{debug, warn};
use uc_core::membership::{
    GroupRevocationPort, GroupUpdateDispatchError, GroupUpdateDispatchPort, MemberRepositoryPort,
    PendingGroupUpdate,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

use super::connect_with_staggered_retry;

pub const GROUP_UPDATE_ALPN: &[u8] = b"uniclipboard/group-update/1";
const MAX_UPDATE_SIZE: usize = 4 * 1024 * 1024;
const ACK_ACCEPTED: u8 = 1;
const ACK_REJECTED: u8 = 2;

pub struct IrohGroupUpdateAdapter {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    handler_state: Arc<HandlerState>,
}

struct HandlerState {
    member_repo: Arc<dyn MemberRepositoryPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    group_revocation: Arc<dyn GroupRevocationPort>,
}

impl IrohGroupUpdateAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        group_revocation: Arc<dyn GroupRevocationPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
            handler_state: Arc::new(HandlerState {
                member_repo,
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
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|_| GroupUpdateDispatchError::Transport)?;
        let length = u32::try_from(update.payload().len())
            .map_err(|_| GroupUpdateDispatchError::Transport)?;
        send.write_all(&length.to_be_bytes())
            .await
            .map_err(|_| GroupUpdateDispatchError::Transport)?;
        send.write_all(update.payload())
            .await
            .map_err(|_| GroupUpdateDispatchError::Transport)?;
        send.finish()
            .map_err(|_| GroupUpdateDispatchError::Transport)?;
        let mut ack = [0u8; 1];
        recv.read_exact(&mut ack)
            .await
            .map_err(|_| GroupUpdateDispatchError::Transport)?;
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
        let known_peer = self
            .state
            .is_known_peer(connection.remote_id().as_bytes())
            .await;
        let (mut send, mut recv) = match connection.accept_bi().await {
            Ok(streams) => streams,
            Err(error) => {
                debug!(error = %error, "group update stream accept failed");
                return Ok(());
            }
        };
        if !known_peer {
            emit_ack(&mut send, ACK_REJECTED).await;
            return Ok(());
        }
        let mut length = [0u8; 4];
        if recv.read_exact(&mut length).await.is_err() {
            emit_ack(&mut send, ACK_REJECTED).await;
            return Ok(());
        }
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_UPDATE_SIZE {
            emit_ack(&mut send, ACK_REJECTED).await;
            return Ok(());
        }
        let mut payload = vec![0u8; length];
        if recv.read_exact(&mut payload).await.is_err() {
            emit_ack(&mut send, ACK_REJECTED).await;
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
        Ok(())
    }
}

impl HandlerState {
    async fn is_known_peer(&self, public_key: &[u8; 32]) -> bool {
        let fingerprint = match self.fingerprint_factory.from_public_key(public_key) {
            Ok(fingerprint) => fingerprint,
            Err(_) => return false,
        };
        match self.member_repo.list().await {
            Ok(members) => members
                .into_iter()
                .any(|member| member.identity_fingerprint == fingerprint),
            Err(error) => {
                warn!(error = %error, "group update member lookup failed");
                false
            }
        }
    }
}

async fn emit_ack(send: &mut iroh::endpoint::SendStream, ack: u8) {
    if send.write_all(&[ack]).await.is_ok() {
        let _ = send.finish();
    }
}
