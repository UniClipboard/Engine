//! Sponsor 拥有一次入站交换、并发限制、截止时间和关闭责任。
use super::super::space_admission_wire::{
    read_typed, write_envelope, AuthenticatedEnvelopeV1, FrameKind, WireError, AUTH_FRAME_LIMIT,
};
use super::super::trace_context::set_remote_parent;
use super::credential::SpaceAdmissionChannelCredentialPort;
use super::crypto::{calculate_mac, copy_credential, peer_id, random_nonce};
use super::diagnostics::{record_server_completion, server_error_type, server_operation_span};
use super::errors::{
    map_server_wire_error, HandlerError, CLOSE_AUTHENTICATION, CLOSE_BUSY,
    CLOSE_PEER_UPGRADE_REQUIRED, CLOSE_PROTOCOL,
};
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::Endpoint;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::{debug, Instrument};
use uc_application::deps::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessagePort,
    SpaceAdmissionTransportError,
};
use uc_core::membership::{AdmissionChannelPeerId, AdmissionPeerBinding};
mod authentication;
use authentication::AuthenticatedRequest;
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(120);
const MAX_INBOUND_EXCHANGES: usize = 8;

pub struct IrohSpaceAdmissionHandler {
    local_peer_id: AdmissionChannelPeerId,
    endpoint: Arc<dyn HandleAuthenticatedSpaceAdmissionMessagePort>,
    credentials: Arc<dyn SpaceAdmissionChannelCredentialPort>,
    permits: Arc<Semaphore>,
    accepting: AtomicBool,
    exchange_deadline: Duration,
}

impl IrohSpaceAdmissionHandler {
    pub fn new(
        local_endpoint: &Endpoint,
        endpoint: Arc<dyn HandleAuthenticatedSpaceAdmissionMessagePort>,
        credentials: Arc<dyn SpaceAdmissionChannelCredentialPort>,
    ) -> Result<Self, SpaceAdmissionTransportError> {
        Ok(Self {
            local_peer_id: peer_id(local_endpoint.id().as_bytes())?,
            endpoint,
            credentials,
            permits: Arc::new(Semaphore::new(MAX_INBOUND_EXCHANGES)),
            accepting: AtomicBool::new(true),
            exchange_deadline: EXCHANGE_DEADLINE,
        })
    }

    #[cfg(test)]
    pub(super) fn with_exchange_deadline(mut self, deadline: Duration) -> Self {
        self.exchange_deadline = deadline;
        self
    }

    async fn run(&self, connection: &Connection) -> Result<(), HandlerError> {
        let connection_started = std::time::Instant::now();
        let deadline = tokio::time::Instant::now() + self.exchange_deadline;
        let AuthenticatedRequest {
            mut send,
            mut receive,
            remote_peer_id,
            admission_id,
            credential,
            is_initial,
            wire,
            envelope,
            canonical_digest,
        } = self
            .authenticate(connection, deadline, connection_started)
            .await?;
        let span = server_operation_span();
        let _ = set_remote_parent(&span, wire.trace_context.as_ref());
        let started = std::time::Instant::now();
        let result = tokio::time::timeout_at(deadline, async {
            let endpoint_credential = if is_initial {
                Some(copy_credential(&credential).map_err(|source| {
                    HandlerError::AuthenticationProof {
                        source: anyhow::Error::new(source),
                    }
                })?)
            } else {
                None
            };
            let binding = AdmissionPeerBinding::new(self.local_peer_id, remote_peer_id)
                .ok_or(HandlerError::Authentication)?;
            let message = AuthenticatedSpaceAdmissionMessage::new(
                binding,
                envelope,
                canonical_digest,
                endpoint_credential,
            )
            .ok_or(HandlerError::Protocol)?;
            let reply = self
                .endpoint
                .handle(message)
                .await
                .map_err(|_| HandlerError::Application)?;
            let reply = reply.envelope().ok_or(HandlerError::Application)?;
            let canonical = reply
                .encode_canonical_v1()
                .map_err(|_| HandlerError::Application)?;
            let digest: [u8; 32] = Sha256::digest(&canonical).into();
            let nonce = random_nonce();
            let mac = calculate_mac(
                &credential,
                b"reply",
                admission_id,
                self.local_peer_id,
                remote_peer_id,
                &nonce,
                &digest,
                None,
            )?;
            write_envelope(
                &mut send,
                FrameKind::Reply,
                &AuthenticatedEnvelopeV1 {
                    nonce,
                    canonical_envelope: canonical,
                    trace_context: None,
                    mac,
                },
            )
            .await
            .map_err(map_server_wire_error)?;
            read_peer_acknowledgement(&mut receive).await?;
            send.finish().map_err(|source| HandlerError::Transport {
                source: anyhow::Error::new(source),
            })?;
            Ok(())
        })
        .instrument(span.clone())
        .await
        .map_err(|_| HandlerError::Timeout)
        .and_then(|result| result);
        span.in_scope(|| record_server_completion(started.elapsed(), result.as_ref().err()));
        drop(span);
        if result.is_ok() {
            let _ = tokio::time::timeout_at(deadline, send.stopped()).await;
        }
        result
    }
}

pub(super) async fn read_peer_acknowledgement<R>(receive: &mut R) -> Result<(), HandlerError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let ack: u8 = read_typed(receive, FrameKind::Ack, AUTH_FRAME_LIMIT)
        .await
        .map_err(map_ack_wire_error)?;
    if ack != 1 {
        return Err(HandlerError::Protocol);
    }
    Ok(())
}

fn map_ack_wire_error(error: WireError) -> HandlerError {
    match error {
        WireError::Timeout => HandlerError::Timeout,
        WireError::Io(_) => HandlerError::Acknowledgement,
        WireError::InvalidHeader
        | WireError::UnknownFrame
        | WireError::InvalidLength
        | WireError::InvalidPayload
        | WireError::UnsupportedLayout => HandlerError::Protocol,
    }
}

impl std::fmt::Debug for IrohSpaceAdmissionHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohSpaceAdmissionHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohSpaceAdmissionHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if !self.accepting.load(Ordering::Acquire) {
            connection.close(CLOSE_BUSY.into(), b"admission_stopping");
            return Ok(());
        }
        let Ok(_permit) = Arc::clone(&self.permits).try_acquire_owned() else {
            connection.close(CLOSE_BUSY.into(), b"admission_busy");
            return Ok(());
        };
        match self.run(&connection).await {
            Ok(()) => {}
            Err(
                error @ (HandlerError::Authentication
                | HandlerError::Credential(_)
                | HandlerError::AuthenticationProof { .. }),
            ) => {
                debug!(error_type = ?server_error_type(&error), "Space admission exchange rejected");
                connection.close(CLOSE_AUTHENTICATION.into(), b"authentication_rejected");
            }
            Err(error @ HandlerError::PeerUpgradeRequired) => {
                debug!(error_type = ?server_error_type(&error), "Space admission peer upgrade required");
                connection.close(CLOSE_PEER_UPGRADE_REQUIRED.into(), b"peer_upgrade_required");
            }
            Err(error @ HandlerError::Acknowledgement) => {
                debug!(
                    error_type = ?server_error_type(&error),
                    "Space admission reply completed without peer acknowledgement"
                );
            }
            Err(error @ HandlerError::Timeout) => {
                debug!(error_type = ?server_error_type(&error), "Space admission exchange timed out");
                connection.close(CLOSE_PROTOCOL.into(), b"protocol_timeout");
            }
            Err(error) => {
                debug!(error_type = ?server_error_type(&error), "Space admission exchange rejected");
                connection.close(CLOSE_PROTOCOL.into(), b"protocol_rejected");
            }
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.accepting.store(false, Ordering::Release);
    }
}
