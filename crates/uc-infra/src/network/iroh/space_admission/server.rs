//! Sponsor 拥有一次入站交换、并发限制、截止时间和关闭责任。
use super::super::space_admission_wire::{
    read_typed, write_envelope, AuthenticatedEnvelopeV1, FrameKind, WireError, AUTH_FRAME_LIMIT,
};
use super::super::trace_context::set_remote_parent;
use super::credential::SpaceAdmissionChannelCredentialPort;
use super::crypto::{calculate_mac, copy_credential, peer_id, random_nonce};
use super::diagnostics::{
    handler_failure, record_network_snapshot, server_completion, server_error_type,
    server_operation_span, wire_failure,
};
use super::errors::{
    map_server_wire_error, HandlerError, CLOSE_AUTHENTICATION, CLOSE_BUSY,
    CLOSE_PEER_UPGRADE_REQUIRED, CLOSE_PROTOCOL,
};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::Endpoint;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tracing::Instrument;
use uc_application::deps::{
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessagePort,
    SpaceAdmissionTransportError,
};
use uc_core::membership::{AdmissionChannelPeerId, AdmissionPeerBinding};
use uc_observability_contract::diagnostics::connectivity::{
    AdmissionExchangeFailure, AdmissionExchangeObservation, AdmissionExchangeSide,
    AdmissionExchangeStep, AdmissionNetworkPoint,
};
mod authentication;
use authentication::AuthenticatedRequest;
use uc_observability_contract::{log_fields::log_vocab_debug, uc_debug, uc_warn};
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(120);
/// 一次交换成功后同一连接上等待下一次交换的时长；加入方每轮之间的本机处理通常数秒内完成。
const CONNECTION_REUSE_IDLE: Duration = Duration::from_secs(30);
const MAX_INBOUND_EXCHANGES: usize = 8;

pub struct IrohSpaceAdmissionHandler {
    local_peer_id: AdmissionChannelPeerId,
    endpoint: Arc<dyn HandleAuthenticatedSpaceAdmissionMessagePort>,
    credentials: Arc<dyn SpaceAdmissionChannelCredentialPort>,
    permits: Arc<Semaphore>,
    accepting: AtomicBool,
    exchange_deadline: Duration,
    reuse_connections: bool,
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
            reuse_connections: true,
        })
    }

    #[cfg(test)]
    pub(super) fn with_exchange_deadline(mut self, deadline: Duration) -> Self {
        self.exchange_deadline = deadline;
        self
    }

    /// 关闭复用等价于旧版邀请端：每条连接只处理一次交换。
    #[cfg(test)]
    pub(super) fn without_connection_reuse(mut self) -> Self {
        self.reuse_connections = false;
        self
    }

    #[cfg(test)]
    pub(super) fn with_capacity(mut self, permits: usize) -> Self {
        self.permits = Arc::new(Semaphore::new(permits));
        self
    }

    /// 在同一条连接上依次处理加入方的各轮交换：第一次交换按原有期限与错误分类处理；之后加入方可以复用连接，
    /// 对端关闭或空闲超时只是正常结束，不记为失败。旧加入方每条连接只有一次交换，行为不变。
    async fn serve(&self, connection: &Connection) -> Result<(), HandlerError> {
        let mut streams = None;
        loop {
            // 第一次交换的流在认证阶段接受，期限与错误分类保持原样；之后加入方复用连接时，空闲等待不计入期限。
            let deadline = tokio::time::Instant::now() + self.exchange_deadline;
            self.run(connection, streams.take(), deadline).await?;
            if !self.reuse_connections {
                return Ok(());
            }
            streams =
                match tokio::time::timeout(CONNECTION_REUSE_IDLE, connection.accept_bi()).await {
                    Ok(Ok(next)) => Some(next),
                    Ok(Err(_)) | Err(_) => return Ok(()),
                };
        }
    }

    async fn run(
        &self,
        connection: &Connection,
        streams: Option<(SendStream, RecvStream)>,
        deadline: tokio::time::Instant,
    ) -> Result<(), HandlerError> {
        let connection_started = std::time::Instant::now();
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
            attempt_contract,
        } = self
            .authenticate(connection, streams, deadline, connection_started)
            .await?;
        let span = server_operation_span();
        let _ = set_remote_parent(&span, wire.trace_context.as_ref());
        let started = std::time::Instant::now();
        let mut progress =
            span.in_scope(|| AdmissionExchangeObservation::begin(AdmissionExchangeSide::Sponsor));
        let result = tokio::time::timeout_at(deadline, async {
            record_network_snapshot(
                connection,
                AdmissionExchangeSide::Sponsor,
                AdmissionNetworkPoint::ExchangeStarted,
            );
            progress.start_step(AdmissionExchangeStep::HandleRequest);
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
                .ok_or_else(HandlerError::authentication)?;
            let message = AuthenticatedSpaceAdmissionMessage::new(
                binding,
                envelope,
                canonical_digest,
                endpoint_credential,
                attempt_contract,
            )
            .ok_or_else(HandlerError::protocol)?;
            let reply = self
                .endpoint
                .handle(message)
                .await
                .map_err(HandlerError::application_from)?;
            progress.start_step(AdmissionExchangeStep::PrepareReply);
            let reply = reply.envelope().ok_or_else(HandlerError::application)?;
            let canonical = reply
                .encode_canonical_v1()
                .map_err(HandlerError::application_from)?;
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
            progress.start_step(AdmissionExchangeStep::SendReply);
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
            .inspect_err(|error| progress.fail(wire_failure(error)))
            .map_err(map_server_wire_error)?;
            progress.start_step(AdmissionExchangeStep::ReceiveAcknowledgement);
            read_peer_acknowledgement_observed(&mut receive, &mut progress).await?;
            progress.start_step(AdmissionExchangeStep::FinishSend);
            send.finish()
                .inspect_err(|_| progress.fail(AdmissionExchangeFailure::ConnectionClosed))
                .map_err(|source| HandlerError::Transport {
                    source: anyhow::Error::new(source),
                })?;
            Ok(())
        })
        .instrument(span.clone())
        .await
        // discarded-source[timeout]: `Elapsed`: the timeout itself is the classification
        .map_err(|_| HandlerError::Timeout)
        .and_then(|result| result);
        if let Err(error) = &result {
            progress.fail(handler_failure(error));
        }
        span.in_scope(|| {
            record_network_snapshot(
                connection,
                AdmissionExchangeSide::Sponsor,
                AdmissionNetworkPoint::ExchangeFinished,
            )
        });
        progress.finish(server_completion(started.elapsed(), result.as_ref().err()));
        drop(span);
        if result.is_ok() {
            let _ = tokio::time::timeout_at(deadline, send.stopped()).await;
        }
        result
    }
}

pub(super) async fn read_peer_acknowledgement_observed<R>(
    receive: &mut R,
    progress: &mut AdmissionExchangeObservation,
) -> Result<(), HandlerError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    read_ack(receive)
        .await
        .inspect_err(|error| progress.fail(wire_failure(error)))
        .map_err(map_ack_wire_error)
}

#[cfg(test)]
pub(super) async fn read_peer_acknowledgement<R>(receive: &mut R) -> Result<(), HandlerError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    read_ack(receive).await.map_err(map_ack_wire_error)
}

async fn read_ack<R>(receive: &mut R) -> Result<(), WireError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let ack: u8 = read_typed(receive, FrameKind::Ack, AUTH_FRAME_LIMIT).await?;
    if ack != 1 {
        return Err(WireError::invalid_payload());
    }
    Ok(())
}

fn map_ack_wire_error(error: WireError) -> HandlerError {
    match error {
        WireError::Timeout => HandlerError::Timeout,
        WireError::Io(_) => HandlerError::Acknowledgement,
        WireError::InvalidHeader { .. }
        | WireError::UnknownFrame
        | WireError::InvalidLength
        | WireError::InvalidPayload { .. }
        | WireError::UnsupportedLayout => HandlerError::protocol(),
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
            uc_warn!(
                reason = "busy",
                "Space admission connection rejected while sponsor is at capacity"
            );
            connection.close(CLOSE_BUSY.into(), b"admission_busy");
            return Ok(());
        };
        match self.serve(&connection).await {
            Ok(()) => {}
            Err(
                error @ (HandlerError::Authentication { .. }
                | HandlerError::Credential(_)
                | HandlerError::AuthenticationProof { .. }),
            ) => {
                uc_debug!(
                    error_type = log_vocab_debug(&server_error_type(&error)),
                    "Space admission exchange rejected"
                );
                connection.close(CLOSE_AUTHENTICATION.into(), b"authentication_rejected");
            }
            Err(error @ HandlerError::PeerUpgradeRequired) => {
                uc_debug!(
                    error_type = log_vocab_debug(&server_error_type(&error)),
                    "Space admission peer upgrade required"
                );
                connection.close(CLOSE_PEER_UPGRADE_REQUIRED.into(), b"peer_upgrade_required");
            }
            Err(error @ HandlerError::Acknowledgement) => {
                uc_debug!(
                    error_type = log_vocab_debug(&server_error_type(&error)),
                    "Space admission reply completed without peer acknowledgement"
                );
            }
            Err(error @ HandlerError::Timeout) => {
                uc_debug!(
                    error_type = log_vocab_debug(&server_error_type(&error)),
                    "Space admission exchange timed out"
                );
                connection.close(CLOSE_PROTOCOL.into(), b"protocol_timeout");
            }
            Err(error) => {
                uc_debug!(
                    error_type = log_vocab_debug(&server_error_type(&error)),
                    "Space admission exchange rejected"
                );
                connection.close(CLOSE_PROTOCOL.into(), b"protocol_rejected");
            }
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.accepting.store(false, Ordering::Release);
    }
}
