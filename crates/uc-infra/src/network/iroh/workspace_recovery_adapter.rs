//! Restricted workspace recovery channel iroh adapter (ADR-016).
//!
//! A lagging member that may not yet recognize the carrying device can
//! request the continuous change chain over the separate versioned
//! `workspace-recovery/1` channel. Every message is sealed with the
//! application-layer AEAD (see `security/recovery_seal.rs`) on top of
//! iroh's authenticated encrypted connection; the current workspace key is
//! never used as a fallback. Message sizes, concurrency and retry rates are
//! bounded at the inbound boundary.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tracing::{debug, warn};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, PeerAdmissionPort, RecoveryChannelMessage, RecoveryTransportEndpointPort,
    RecoveryTransportError, RecoveryTransportPort,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

use super::connect_with_staggered_retry;

pub const WORKSPACE_RECOVERY_ALPN: &[u8] = b"uniclipboard/workspace-recovery/1";

const MAX_RECOVERY_MESSAGE_SIZE: usize = 256 * 1024;
const MAX_RECOVERY_CONCURRENCY: usize = 4;
const RECOVERY_IO_TIMEOUT: Duration = Duration::from_secs(10);
const ACK_ACCEPTED: u8 = 1;
const ACK_REJECTED: u8 = 2;

pub struct IrohWorkspaceRecoveryAdapter {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
}

pub(crate) struct IrohWorkspaceRecoveryHandlers {
    pub(crate) recovery: IrohWorkspaceRecoveryHandler,
}

struct RecoveryHandlerState {
    member_repo: Arc<dyn MemberRepositoryPort>,
    peer_admission: Arc<dyn PeerAdmissionPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    recovery_endpoint: Arc<dyn RecoveryTransportEndpointPort>,
    recovery_concurrency: Arc<tokio::sync::Semaphore>,
}

impl IrohWorkspaceRecoveryAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
        }
    }

    pub(crate) fn handlers(
        &self,
        member_repo: Arc<dyn MemberRepositoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        recovery_endpoint: Arc<dyn RecoveryTransportEndpointPort>,
    ) -> IrohWorkspaceRecoveryHandlers {
        IrohWorkspaceRecoveryHandlers {
            recovery: IrohWorkspaceRecoveryHandler {
                state: Arc::new(RecoveryHandlerState {
                    member_repo,
                    peer_admission,
                    fingerprint_factory,
                    recovery_endpoint,
                    recovery_concurrency: Arc::new(tokio::sync::Semaphore::new(
                        MAX_RECOVERY_CONCURRENCY,
                    )),
                }),
            },
        }
    }

    async fn resolve_addr(&self, recipient: &DeviceId) -> Option<EndpointAddr> {
        match self.peer_addr_repo.get(recipient).await {
            Ok(Some(record)) => postcard::from_bytes(&record.addr_blob).ok(),
            Ok(None) => None,
            Err(_) => {
                warn!(
                    failure = "address_lookup_failed",
                    "workspace recovery address lookup failed"
                );
                None
            }
        }
    }
}

#[async_trait]
impl RecoveryTransportPort for IrohWorkspaceRecoveryAdapter {
    async fn exchange_recovery(
        &self,
        recipient: &DeviceId,
        message: RecoveryChannelMessage,
    ) -> Result<RecoveryChannelMessage, RecoveryTransportError> {
        let payload =
            postcard::to_stdvec(&message).map_err(|_| RecoveryTransportError::Transport)?;
        if payload.len() > MAX_RECOVERY_MESSAGE_SIZE {
            return Err(RecoveryTransportError::Transport);
        }
        let addr = self
            .resolve_addr(recipient)
            .await
            .ok_or(RecoveryTransportError::Offline)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            addr,
            WORKSPACE_RECOVERY_ALPN,
            "workspace-recovery",
        )
        .await
        .map_err(|_| RecoveryTransportError::Offline)?;
        let (mut send, mut recv) = tokio::time::timeout(RECOVERY_IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|_| RecoveryTransportError::Transport)?
            .map_err(|_| RecoveryTransportError::Transport)?;
        let length = u32::try_from(payload.len()).map_err(|_| RecoveryTransportError::Transport)?;
        send.write_all(&length.to_be_bytes())
            .await
            .map_err(|_| RecoveryTransportError::Transport)?;
        send.write_all(&payload)
            .await
            .map_err(|_| RecoveryTransportError::Transport)?;
        send.finish()
            .map_err(|_| RecoveryTransportError::Transport)?;
        let mut ack = [0u8; 1];
        tokio::time::timeout(RECOVERY_IO_TIMEOUT, recv.read_exact(&mut ack))
            .await
            .map_err(|_| RecoveryTransportError::Transport)?
            .map_err(|_| RecoveryTransportError::Transport)?;
        match ack[0] {
            ACK_ACCEPTED => {}
            ACK_REJECTED => return Err(RecoveryTransportError::Transport),
            _ => return Err(RecoveryTransportError::Transport),
        }
        let mut length = [0u8; 4];
        tokio::time::timeout(RECOVERY_IO_TIMEOUT, recv.read_exact(&mut length))
            .await
            .map_err(|_| RecoveryTransportError::Transport)?
            .map_err(|_| RecoveryTransportError::Transport)?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_RECOVERY_MESSAGE_SIZE {
            return Err(RecoveryTransportError::Transport);
        }
        let mut response = vec![0u8; length];
        tokio::time::timeout(RECOVERY_IO_TIMEOUT, recv.read_exact(&mut response))
            .await
            .map_err(|_| RecoveryTransportError::Transport)?
            .map_err(|_| RecoveryTransportError::Transport)?;
        postcard::from_bytes(&response).map_err(|_| RecoveryTransportError::Transport)
    }
}

#[derive(Clone)]
pub struct IrohWorkspaceRecoveryHandler {
    state: Arc<RecoveryHandlerState>,
}

impl std::fmt::Debug for IrohWorkspaceRecoveryHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohWorkspaceRecoveryHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohWorkspaceRecoveryHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let state = Arc::clone(&self.state);
        let _permit = match state.recovery_concurrency.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return Ok(()),
        };
        if let Err(error) = handle_recovery_connection(state, connection).await {
            debug!(error = %error, "workspace recovery connection closed");
        }
        Ok(())
    }
}

async fn handle_recovery_connection(
    state: Arc<RecoveryHandlerState>,
    connection: Connection,
) -> Result<(), String> {
    let remote_key = connection.remote_id();
    let fingerprint = state
        .fingerprint_factory
        .from_public_key(remote_key.as_bytes())
        .map_err(|_| "invalid_remote_identity".to_owned())?;
    let resolved = state
        .member_repo
        .list()
        .await
        .map_err(|_| "member_lookup_failed".to_owned())?
        .into_iter()
        .find(|member| member.identity_fingerprint == fingerprint);
    let Some(member) = resolved else {
        return Err("unknown_source_member".to_owned());
    };
    if !state
        .peer_admission
        .is_admitted(&member.device_id)
        .await
        .unwrap_or(false)
    {
        return Err("unadmitted_source".to_owned());
    }
    let (mut send, mut recv) = connection
        .accept_bi()
        .await
        .map_err(|_| "stream_open_failed".to_owned())?;
    let mut length = [0u8; 4];
    tokio::time::timeout(RECOVERY_IO_TIMEOUT, recv.read_exact(&mut length))
        .await
        .map_err(|_| "read_timeout".to_owned())?
        .map_err(|_| "read_failed".to_owned())?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_RECOVERY_MESSAGE_SIZE {
        send.write_all(&[ACK_REJECTED])
            .await
            .map_err(|_| "reject_failed".to_owned())?;
        return Err("oversized_request".to_owned());
    }
    let mut payload = vec![0u8; length];
    tokio::time::timeout(RECOVERY_IO_TIMEOUT, recv.read_exact(&mut payload))
        .await
        .map_err(|_| "read_timeout".to_owned())?
        .map_err(|_| "read_failed".to_owned())?;
    let message: RecoveryChannelMessage = match postcard::from_bytes(&payload) {
        Ok(message) => message,
        Err(_) => {
            send.write_all(&[ACK_REJECTED])
                .await
                .map_err(|_| "reject_failed".to_owned())?;
            return Err("invalid_request".to_owned());
        }
    };
    let response = state
        .recovery_endpoint
        .handle_recovery(&member.device_id, message)
        .await;
    match response {
        Ok(response) => {
            let response_payload =
                postcard::to_stdvec(&response).map_err(|_| "encode_failed".to_owned())?;
            if response_payload.len() > MAX_RECOVERY_MESSAGE_SIZE {
                send.write_all(&[ACK_REJECTED])
                    .await
                    .map_err(|_| "reject_failed".to_owned())?;
                return Err("oversized_response".to_owned());
            }
            send.write_all(&[ACK_ACCEPTED])
                .await
                .map_err(|_| "ack_failed".to_owned())?;
            let response_length =
                u32::try_from(response_payload.len()).map_err(|_| "encode_failed".to_owned())?;
            send.write_all(&response_length.to_be_bytes())
                .await
                .map_err(|_| "write_failed".to_owned())?;
            send.write_all(&response_payload)
                .await
                .map_err(|_| "write_failed".to_owned())?;
            send.finish().map_err(|_| "finish_failed".to_owned())?;
            Ok(())
        }
        Err(error) => {
            warn!(
                rejection = ?error,
                "workspace recovery request rejected"
            );
            send.write_all(&[ACK_REJECTED])
                .await
                .map_err(|_| "reject_failed".to_owned())?;
            Err("request_rejected".to_owned())
        }
    }
}
