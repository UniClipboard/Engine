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
    ContentKeyId, MemberRepositoryPort, RecoveryBinding, RecoveryChannelMessage,
    RecoveryEnvelopeHeader, RecoveryTransportEndpointPort, RecoveryTransportError,
    RecoveryTransportPort, MIN_HISTORY_KEY_NUMBER,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

use super::connect_with_staggered_retry;
use crate::security::{open_recovery_message, seal_recovery_message, InMemorySession};

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub const WORKSPACE_RECOVERY_ALPN: &[u8] = b"uniclipboard/workspace-recovery/1";

const MAX_RECOVERY_MESSAGE_SIZE: usize = 256 * 1024;
const MAX_RECOVERY_CONCURRENCY: usize = 4;
const RECOVERY_IO_TIMEOUT: Duration = Duration::from_secs(10);
const ACK_ACCEPTED: u8 = 1;
const ACK_REJECTED: u8 = 2;

pub struct IrohWorkspaceRecoveryAdapter {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    session: Arc<InMemorySession>,
}

pub(crate) struct IrohWorkspaceRecoveryHandlers {
    pub(crate) recovery: IrohWorkspaceRecoveryHandler,
}

struct RecoveryHandlerState {
    member_repo: Arc<dyn MemberRepositoryPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    recovery_endpoint: Arc<dyn RecoveryTransportEndpointPort>,
    recovery_concurrency: Arc<tokio::sync::Semaphore>,
    session: Arc<InMemorySession>,
    local_public_key: Vec<u8>,
}

impl IrohWorkspaceRecoveryAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        session: Arc<InMemorySession>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
            session,
        }
    }

    pub(crate) fn handlers(
        &self,
        member_repo: Arc<dyn MemberRepositoryPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        recovery_endpoint: Arc<dyn RecoveryTransportEndpointPort>,
    ) -> IrohWorkspaceRecoveryHandlers {
        IrohWorkspaceRecoveryHandlers {
            recovery: IrohWorkspaceRecoveryHandler {
                state: Arc::new(RecoveryHandlerState {
                    member_repo,
                    fingerprint_factory,
                    recovery_endpoint,
                    recovery_concurrency: Arc::new(tokio::sync::Semaphore::new(
                        MAX_RECOVERY_CONCURRENCY,
                    )),
                    session: Arc::clone(&self.session),
                    local_public_key: self.endpoint.secret_key().public().as_bytes().to_vec(),
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

/// Map a declared history key number to the local session content key id.
/// The legacy transport key is the only historical key today.
fn history_key_id(history_key_number: u64) -> Option<ContentKeyId> {
    (history_key_number == MIN_HISTORY_KEY_NUMBER).then(ContentKeyId::legacy_v1)
}

/// Seal a message for `binding` with the shared historical transport key.
/// `history_key_number` must resolve to a key the local session holds.
fn seal_for(
    session: &InMemorySession,
    binding: &RecoveryBinding,
    message: &RecoveryChannelMessage,
) -> Result<Vec<u8>, RecoveryTransportError> {
    let key_id =
        history_key_id(binding.history_key_number).ok_or(RecoveryTransportError::Rejected(
            uc_core::membership::RecoveryRejection::VersionIncompatible,
        ))?;
    let space_id = session
        .current_space_id()
        .map_err(|_| RecoveryTransportError::Offline)?;
    seal_recovery_message(session, &space_id, &key_id, binding, message).map_err(
        |error| match error {
            crate::security::RecoverySealError::Unavailable => RecoveryTransportError::Offline,
            _ => RecoveryTransportError::Transport,
        },
    )
}

/// Open a sealed envelope from the authenticated peer. `peer_public_key` is
/// the sender on this connection.
fn open_from(
    session: &InMemorySession,
    local_public_key: &[u8],
    peer_public_key: &[u8],
    envelope: &[u8],
) -> Result<RecoveryChannelMessage, RecoveryTransportError> {
    let header = RecoveryEnvelopeHeader::decode(
        &envelope[..envelope
            .len()
            .min(uc_core::membership::RECOVERY_ENVELOPE_HEADER_BYTES)],
    )
    .map_err(|_| RecoveryTransportError::Transport)?;
    let key_id =
        history_key_id(header.history_key_number).ok_or(RecoveryTransportError::Rejected(
            uc_core::membership::RecoveryRejection::VersionIncompatible,
        ))?;
    let space_id = session
        .current_space_id()
        .map_err(|_| RecoveryTransportError::Offline)?;
    let binding = header.to_binding(peer_public_key.to_vec(), local_public_key.to_vec());
    open_recovery_message(session, &space_id, &key_id, &binding, envelope).map_err(|error| {
        match error {
            crate::security::RecoverySealError::Unavailable => RecoveryTransportError::Offline,
            _ => RecoveryTransportError::Rejected(
                uc_core::membership::RecoveryRejection::IdentityMismatch,
            ),
        }
    })
}

/// Build the reply envelope header for `response` to an inbound request
/// whose clear header is `request`. Instances are swapped (the local side
/// now sends); numbers, range and target digest are taken from the reply.
fn reply_header(
    request: &RecoveryEnvelopeHeader,
    response: &RecoveryChannelMessage,
) -> RecoveryEnvelopeHeader {
    match response {
        RecoveryChannelMessage::Offer(offer) => RecoveryEnvelopeHeader {
            space_lineage_fingerprint: request.space_lineage_fingerprint,
            history_key_number: request.history_key_number,
            request_number: offer.request_number,
            reply_number: offer.reply_number,
            from_epoch: offer.from_epoch,
            to_epoch: offer.to_epoch,
            target_digest: offer.target_digest,
            sender_instance: request.receiver_instance,
            receiver_instance: request.sender_instance,
        },
        RecoveryChannelMessage::Ack(ack) => RecoveryEnvelopeHeader {
            space_lineage_fingerprint: request.space_lineage_fingerprint,
            history_key_number: request.history_key_number,
            request_number: ack.request_number,
            reply_number: ack.reply_number,
            from_epoch: 0,
            to_epoch: ack.confirmed_epoch,
            target_digest: ack.target_digest,
            sender_instance: request.receiver_instance,
            receiver_instance: request.sender_instance,
        },
        RecoveryChannelMessage::Reject(reject) => RecoveryEnvelopeHeader {
            space_lineage_fingerprint: request.space_lineage_fingerprint,
            history_key_number: request.history_key_number,
            request_number: reject.request_number,
            reply_number: reject.reply_number,
            from_epoch: request.from_epoch,
            to_epoch: request.to_epoch,
            target_digest: [0; 32],
            sender_instance: request.receiver_instance,
            receiver_instance: request.sender_instance,
        },
        RecoveryChannelMessage::Request(_) => request.clone(),
    }
}

#[async_trait]
impl RecoveryTransportPort for IrohWorkspaceRecoveryAdapter {
    async fn exchange_recovery(
        &self,
        recipient: &DeviceId,
        binding: &RecoveryBinding,
        message: RecoveryChannelMessage,
    ) -> Result<RecoveryChannelMessage, RecoveryTransportError> {
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
        let local_public_key = self.endpoint.secret_key().public().as_bytes().to_vec();
        let remote_public_key = connection.remote_id().as_bytes().to_vec();
        let mut sealed_binding = binding.clone();
        sealed_binding.sender_transport_public_key = local_public_key.clone();
        sealed_binding.receiver_transport_public_key = remote_public_key.clone();
        let payload = seal_for(&self.session, &sealed_binding, &message)?;
        if payload.len() > MAX_RECOVERY_MESSAGE_SIZE {
            return Err(RecoveryTransportError::Transport);
        }
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
        open_from(
            &self.session,
            &local_public_key,
            &remote_public_key,
            &response,
        )
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
    let local_public_key = state.local_public_key.clone();
    let remote_public_key = remote_key.as_bytes().to_vec();
    let fingerprint = state
        .fingerprint_factory
        .from_public_key(&remote_public_key)
        .map_err(|_| "invalid_remote_identity".to_owned())?;
    // The recovery channel deliberately accepts sources that are not yet
    // known or not admitted: the application-layer seal, the space lineage
    // and the change-chain verification decide what such a source may
    // obtain. Known members resolve to their device id; unknown sources get
    // a stable derived id so the endpoint can reject requests and replay
    // keys consistently without ever disclosing anything.
    let resolved = state
        .member_repo
        .list()
        .await
        .map_err(|_| "member_lookup_failed".to_owned())?
        .into_iter()
        .find(|member| member.identity_fingerprint == fingerprint);
    let source_device = match resolved {
        Some(member) => member.device_id,
        None => {
            let mut derived = [0u8; 16];
            derived.copy_from_slice(&fingerprint.as_display().as_bytes()[..16]);
            DeviceId::new(&format!("recovery-unknown-{}", hex_encode(&derived)))
        }
    };
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
    let header = match RecoveryEnvelopeHeader::decode(
        &payload[..payload
            .len()
            .min(uc_core::membership::RECOVERY_ENVELOPE_HEADER_BYTES)],
    ) {
        Ok(header) => header,
        Err(_) => {
            send.write_all(&[ACK_REJECTED])
                .await
                .map_err(|_| "reject_failed".to_owned())?;
            return Err("invalid_envelope_header".to_owned());
        }
    };
    let key_id = match history_key_id(header.history_key_number) {
        Some(key_id) => key_id,
        None => {
            send.write_all(&[ACK_REJECTED])
                .await
                .map_err(|_| "reject_failed".to_owned())?;
            return Err("unknown_history_key".to_owned());
        }
    };
    let space_id = match state.session.current_space_id() {
        Ok(space_id) => space_id,
        Err(_) => {
            send.write_all(&[ACK_REJECTED])
                .await
                .map_err(|_| "reject_failed".to_owned())?;
            return Err("session_locked".to_owned());
        }
    };
    let binding = header.to_binding(remote_public_key.clone(), local_public_key.clone());
    let message =
        match open_recovery_message(&state.session, &space_id, &key_id, &binding, &payload) {
            Ok(message) => message,
            Err(_) => {
                send.write_all(&[ACK_REJECTED])
                    .await
                    .map_err(|_| "reject_failed".to_owned())?;
                return Err("seal_open_failed".to_owned());
            }
        };
    let response = state
        .recovery_endpoint
        .handle_recovery(&source_device, message)
        .await;
    match response {
        Ok(response) => {
            let response_header = reply_header(&header, &response);
            let mut response_binding =
                response_header.to_binding(local_public_key.clone(), remote_public_key.clone());
            response_binding.space_lineage_fingerprint = header.space_lineage_fingerprint;
            let response_payload = match seal_for(&state.session, &response_binding, &response) {
                Ok(payload) => payload,
                Err(_) => {
                    send.write_all(&[ACK_REJECTED])
                        .await
                        .map_err(|_| "reject_failed".to_owned())?;
                    return Err("response_seal_failed".to_owned());
                }
            };
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
