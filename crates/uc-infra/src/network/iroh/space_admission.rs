use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use uc_observability_contract::diagnostics::connectivity::{
    complete_admission_authentication_failure, complete_admission_connection_failure,
    AuthenticationFailure, AuthenticationStage, CredentialFailure, DialFailure, IdentityCheck,
    ProofFailure, ReadFailure,
};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use tokio::sync::Semaphore;
use tracing::instrument::WithSubscriber;
use tracing::{debug, Instrument};
use uc_application::deps::{
    AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply,
    AuthenticatedSpaceAdmissionMessage, HandleAuthenticatedSpaceAdmissionMessagePort,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
use uc_core::membership::{
    AdmissionChannelPeerId, AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent,
    AdmissionPeerBinding, InvitationId, SpaceAdmissionEnvelopeV1, SpaceAdmissionId,
    SpaceAdmissionProtocolVersion, SpaceAdmissionRoute,
};
use uc_observability_contract::diagnostics::{
    complete_operation, operation_span, DiagnosticDomain, DiagnosticErrorType, DiagnosticOperation,
    DiagnosticRole, DiagnosticSpanKind, OperationCompletion, OperationContext,
};

use crate::security::{
    SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionContinuationCredential,
    SpaceAdmissionKe1, SpaceAdmissionKe2, SpaceAdmissionKe3, SpaceAdmissionRegistration,
    SpaceAdmissionServerSetup,
};

#[cfg(test)]
use super::space_admission_wire::LARGE_MESSAGE_LIMIT;
use super::space_admission_wire::{
    read_envelope, read_raw_with_limit, read_typed, write_envelope, write_typed,
    AuthenticatedEnvelopeV1, ContinuationHelloV1, FrameKind, InitialHelloV1, OpaqueFinishV1,
    OpaqueResponseV1, WireError, AUTH_FRAME_LIMIT, IO_DEADLINE,
};
use super::trace_context::{inject_current, set_remote_parent, WireTraceContext};

pub const SPACE_ADMISSION_ALPN: &[u8] = b"/uniclipboard/space-admission/1";
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(120);
const MAX_INBOUND_EXCHANGES: usize = 8;
const LEGACY_CLOSE_PROTOCOL: u32 = 0x51;
const CLOSE_AUTHENTICATION: u32 = 0x52;
const CLOSE_BUSY: u32 = 0x53;
const CLOSE_PEER_UPGRADE_REQUIRED: u32 = 0x54;
const CLOSE_PROTOCOL: u32 = 0x55;
const DIAL_ROUTE_FORMAT_V1: u16 = 1;

type HmacSha512 = Hmac<Sha512>;

pub struct SponsorOpaqueMaterial {
    server_setup: SpaceAdmissionServerSetup,
    registration: SpaceAdmissionRegistration,
}

impl SponsorOpaqueMaterial {
    pub fn new(
        server_setup: SpaceAdmissionServerSetup,
        registration: SpaceAdmissionRegistration,
    ) -> Self {
        Self {
            server_setup,
            registration,
        }
    }

    #[cfg(test)]
    pub(crate) fn into_parts(self) -> (SpaceAdmissionServerSetup, SpaceAdmissionRegistration) {
        (self.server_setup, self.registration)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceAdmissionChannelCredentialError {
    #[error("space admission channel credential is unavailable")]
    Unavailable {
        #[source]
        source: anyhow::Error,
    },
    #[error("space admission channel credential was rejected")]
    Rejected {
        #[source]
        source: anyhow::Error,
    },
}

#[async_trait]
pub trait SpaceAdmissionChannelCredentialPort: Send + Sync {
    async fn resolve_initial(
        &self,
        invitation_id: InvitationId,
        admission_id: SpaceAdmissionId,
    ) -> Result<SponsorOpaqueMaterial, SpaceAdmissionChannelCredentialError>;

    async fn load_continuation(
        &self,
        admission_id: SpaceAdmissionId,
    ) -> Result<AdmissionContinuationCredential, SpaceAdmissionChannelCredentialError>;
}

#[derive(Serialize, Deserialize)]
struct AdmissionDialRouteV1 {
    format_version: u16,
    invitation_id: Option<[u8; 32]>,
    endpoint_addr: Vec<u8>,
}

pub fn encode_space_admission_route(
    endpoint_addr: &EndpointAddr,
    invitation_id: Option<InvitationId>,
) -> Result<Vec<u8>, SpaceAdmissionTransportError> {
    let endpoint_addr = postcard::to_stdvec(endpoint_addr)
        .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
    encode_space_admission_route_bytes(&endpoint_addr, invitation_id)
}

pub(crate) fn encode_space_admission_route_bytes(
    endpoint_addr: &[u8],
    invitation_id: Option<InvitationId>,
) -> Result<Vec<u8>, SpaceAdmissionTransportError> {
    postcard::to_stdvec(&AdmissionDialRouteV1 {
        format_version: DIAL_ROUTE_FORMAT_V1,
        invitation_id: invitation_id.map(|id| *id.as_bytes()),
        endpoint_addr: endpoint_addr.to_vec(),
    })
    .map_err(|_| SpaceAdmissionTransportError::Unavailable)
}

pub struct IrohSpaceAdmissionTransport {
    endpoint: Arc<Endpoint>,
}

impl IrohSpaceAdmissionTransport {
    pub fn new(endpoint: Arc<Endpoint>) -> Self {
        Self { endpoint }
    }
}

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
    fn with_exchange_deadline(mut self, deadline: Duration) -> Self {
        self.exchange_deadline = deadline;
        self
    }

    async fn run(&self, connection: &Connection) -> Result<(), HandlerError> {
        let connection_started = std::time::Instant::now();
        let deadline = tokio::time::Instant::now() + self.exchange_deadline;
        let mut diagnostic_stage = AuthenticationStep::ReceiveHello;
        let authenticated = tokio::time::timeout_at(deadline, async {
            let remote_peer_id = peer_id(connection.remote_id().as_bytes()).map_err(|source| {
                HandlerError::AuthenticationProof {
                    source: anyhow::Error::new(source),
                }
            })?;
            let (mut send, mut receive) = tokio::time::timeout(IO_DEADLINE, connection.accept_bi())
                .await
                .map_err(|_| HandlerError::Timeout)?
                .map_err(|source| HandlerError::Transport {
                    source: anyhow::Error::new(source),
                })?;
            let (kind, payload) = read_raw_with_limit(&mut receive, AUTH_FRAME_LIMIT)
                .await
                .map_err(map_server_wire_error)?;
            let (admission_id, credential, is_initial) = match kind {
                FrameKind::InitialHello => {
                    let hello: InitialHelloV1 =
                        postcard::from_bytes(&payload).map_err(|_| HandlerError::Protocol)?;
                    let admission_id = SpaceAdmissionId::from_bytes(hello.admission_id)
                        .ok_or(HandlerError::Protocol)?;
                    let invitation_id = InvitationId::from_bytes(hello.invitation_id)
                        .ok_or(HandlerError::Protocol)?;
                    diagnostic_stage = AuthenticationStep::InitialVersion;
                    if hello.protocol_version != SpaceAdmissionProtocolVersion::V1.as_u16() {
                        return Err(HandlerError::Authentication);
                    }
                    diagnostic_stage = AuthenticationStep::InitialIdentity;
                    if hello.joiner_peer_id != *remote_peer_id.as_bytes() {
                        return Err(HandlerError::Authentication);
                    }
                    diagnostic_stage = AuthenticationStep::InitialCredential;
                    let material = self
                        .credentials
                        .resolve_initial(invitation_id, admission_id)
                        .await
                        .map_err(HandlerError::Credential)?;
                    diagnostic_stage = AuthenticationStep::InitialProof;
                    let context = SpaceAdmissionAuthContext::new(
                        SpaceAdmissionProtocolVersion::V1,
                        admission_id,
                        invitation_id,
                        remote_peer_id,
                        self.local_peer_id,
                    );
                    let ke1 =
                        SpaceAdmissionKe1::decode_from_transport(&hello.ke1).map_err(|source| {
                            HandlerError::AuthenticationProof {
                                source: anyhow::Error::new(source),
                            }
                        })?;
                    let (server, ke2) = SpaceAdmissionAuth::start_server(
                        &material.server_setup,
                        &material.registration,
                        &context,
                        ke1,
                    )
                    .map_err(|source| HandlerError::AuthenticationProof {
                        source: anyhow::Error::new(source),
                    })?;
                    write_typed(
                        &mut send,
                        FrameKind::OpaqueResponse,
                        &OpaqueResponseV1 {
                            sponsor_peer_id: *self.local_peer_id.as_bytes(),
                            ke2: ke2.encode_for_transport(),
                        },
                        AUTH_FRAME_LIMIT,
                    )
                    .await
                    .map_err(map_server_wire_error)?;
                    let finish: OpaqueFinishV1 =
                        read_typed(&mut receive, FrameKind::OpaqueFinish, AUTH_FRAME_LIMIT)
                            .await
                            .map_err(map_server_wire_error)?;
                    let ke3 = SpaceAdmissionKe3::decode_from_transport(&finish.ke3).map_err(
                        |source| HandlerError::AuthenticationProof {
                            source: anyhow::Error::new(source),
                        },
                    )?;
                    let credential = server
                        .finish(&context, ke3)
                        .and_then(SpaceAdmissionContinuationCredential::into_core)
                        .map_err(|source| HandlerError::AuthenticationProof {
                            source: anyhow::Error::new(source),
                        })?;
                    (admission_id, credential, true)
                }
                FrameKind::ContinuationHello => {
                    let hello: ContinuationHelloV1 =
                        postcard::from_bytes(&payload).map_err(|_| HandlerError::Protocol)?;
                    let admission_id = SpaceAdmissionId::from_bytes(hello.admission_id)
                        .ok_or(HandlerError::Protocol)?;
                    diagnostic_stage = AuthenticationStep::ContinuationIdentity;
                    if hello.local_peer_id != *remote_peer_id.as_bytes()
                        || hello.remote_peer_id != *self.local_peer_id.as_bytes()
                    {
                        return Err(HandlerError::Authentication);
                    }
                    diagnostic_stage = AuthenticationStep::ContinuationCredential;
                    let credential = self
                        .credentials
                        .load_continuation(admission_id)
                        .await
                        .map_err(HandlerError::Credential)?;
                    diagnostic_stage = AuthenticationStep::ContinuationProof;
                    verify_mac(
                        &credential,
                        b"resume",
                        admission_id,
                        remote_peer_id,
                        self.local_peer_id,
                        &hello.nonce,
                        &hello.request_digest,
                        None,
                        &hello.mac,
                    )?;
                    (admission_id, credential, false)
                }
                _ => return Err(HandlerError::Protocol),
            };

            diagnostic_stage = AuthenticationStep::ReceiveRequest;
            let (wire, envelope, canonical_digest) =
                read_envelope(&mut receive, FrameKind::Request)
                    .await
                    .map_err(map_request_wire_error)?;
            diagnostic_stage = AuthenticationStep::RequestIdentity;
            if envelope.header().admission_id() != admission_id {
                return Err(HandlerError::Authentication);
            }
            diagnostic_stage = AuthenticationStep::RequestProof;
            verify_mac(
                &credential,
                b"request",
                admission_id,
                remote_peer_id,
                self.local_peer_id,
                &wire.nonce,
                &canonical_digest,
                wire.trace_context.as_ref(),
                &wire.mac,
            )?;
            Ok::<_, HandlerError>((
                send,
                receive,
                remote_peer_id,
                admission_id,
                credential,
                is_initial,
                wire,
                envelope,
                canonical_digest,
            ))
        })
        .await;
        let authenticated = match authenticated {
            Ok(Ok(authenticated)) => authenticated,
            Ok(Err(error)) => {
                complete_admission_authentication_failure(
                    diagnostic_stage.failure(&error),
                    connection_started.elapsed(),
                );
                return Err(error);
            }
            Err(_) => {
                let error = HandlerError::Timeout;
                complete_admission_authentication_failure(
                    diagnostic_stage.failure(&error),
                    connection_started.elapsed(),
                );
                return Err(error);
            }
        };
        let (
            mut send,
            mut receive,
            remote_peer_id,
            admission_id,
            credential,
            is_initial,
            wire,
            envelope,
            canonical_digest,
        ) = authenticated;
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

async fn read_peer_acknowledgement<R>(receive: &mut R) -> Result<(), HandlerError>
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

struct EstablishedExchange {
    connection: Connection,
    send: SendStream,
    receive: RecvStream,
    admission_id: SpaceAdmissionId,
    binding: AdmissionPeerBinding,
    credential: AdmissionContinuationCredential,
    newly_established: Option<AdmissionContinuationCredential>,
}

#[async_trait]
impl AuthenticatedAdmissionExchangePort for EstablishedExchange {
    fn peer_binding(&self) -> AdmissionPeerBinding {
        self.binding
    }

    fn take_newly_established_continuation(&mut self) -> Option<AdmissionContinuationCredential> {
        self.newly_established.take()
    }

    async fn exchange(
        mut self: Box<Self>,
        request: &SpaceAdmissionEnvelopeV1,
    ) -> Result<AuthenticatedAdmissionReply, SpaceAdmissionTransportError> {
        let canonical = request
            .encode_canonical_v1()
            .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
        let digest: [u8; 32] = Sha256::digest(&canonical).into();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::NetworkTransport,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        let started = std::time::Instant::now();
        let result = async {
            let trace_context = inject_current();
            let nonce = random_nonce();
            let mac = calculate_mac(
                &self.credential,
                b"request",
                self.admission_id,
                self.binding.local_peer_id(),
                self.binding.remote_peer_id(),
                &nonce,
                &digest,
                trace_context.as_ref(),
            )
            .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            write_envelope(
                &mut self.send,
                FrameKind::Request,
                &AuthenticatedEnvelopeV1 {
                    nonce,
                    canonical_envelope: canonical,
                    trace_context,
                    mac,
                },
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            let (wire, reply, reply_digest) =
                read_authenticated_reply(&mut self.receive, &self.connection).await?;
            verify_mac(
                &self.credential,
                b"reply",
                self.admission_id,
                self.binding.remote_peer_id(),
                self.binding.local_peer_id(),
                &wire.nonce,
                &reply_digest,
                wire.trace_context.as_ref(),
                &wire.mac,
            )
            .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            write_typed(&mut self.send, FrameKind::Ack, &1u8, AUTH_FRAME_LIMIT)
                .await
                .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            self.send
                .finish()
                .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            let mut trailing = [0_u8; 1];
            let trailing_length = tokio::time::timeout(
                IO_DEADLINE,
                tokio::io::AsyncReadExt::read(&mut self.receive, &mut trailing),
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            if trailing_length != 0 {
                return Err(SpaceAdmissionTransportError::ProtocolRejected);
            }
            AuthenticatedAdmissionReply::new(reply, reply_digest)
                .ok_or(SpaceAdmissionTransportError::ProtocolRejected)
        }
        .instrument(span.clone())
        .await;
        span.in_scope(|| {
            record_client_completion(
                DiagnosticOperation::NetworkTransport,
                started.elapsed(),
                result.as_ref().err(),
            )
        });
        result
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for IrohSpaceAdmissionTransport {
    async fn establish_initial(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        password: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::SpaceAdmission,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        let mut connection_failure = None;
        let result = async {
            let route = decode_route(route, true)?;
            let invitation_id = route
                .invitation_id
                .and_then(InvitationId::from_bytes)
                .ok_or(SpaceAdmissionTransportError::InvitationUnavailable)?;
            let local = peer_id(self.endpoint.id().as_bytes())?;
            let remote = peer_id(route.endpoint_addr.id.as_bytes())?;
            let binding = AdmissionPeerBinding::new(local, remote)
                .ok_or(SpaceAdmissionTransportError::AuthenticationRejected)?;
            let context = SpaceAdmissionAuthContext::new(
                SpaceAdmissionProtocolVersion::V1,
                admission_id,
                invitation_id,
                local,
                remote,
            );
            let (client, ke1) =
                SpaceAdmissionAuth::start_client_with_password_equivalent(password, &context)
                    .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            let connection =
                connect(&self.endpoint, route.endpoint_addr)
                    .await
                    .map_err(|error| {
                        connection_failure = Some(error.category());
                        SpaceAdmissionTransportError::Deferred
                    })?;
            let (mut send, mut receive) = open_stream(&connection).await?;
            write_typed(
                &mut send,
                FrameKind::InitialHello,
                &InitialHelloV1 {
                    protocol_version: SpaceAdmissionProtocolVersion::V1.as_u16(),
                    admission_id: *admission_id.as_bytes(),
                    invitation_id: *invitation_id.as_bytes(),
                    joiner_peer_id: *local.as_bytes(),
                    ke1: ke1.encode_for_transport(),
                },
                AUTH_FRAME_LIMIT,
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            let response: OpaqueResponseV1 =
                read_typed(&mut receive, FrameKind::OpaqueResponse, AUTH_FRAME_LIMIT)
                    .await
                    .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            if response.sponsor_peer_id != *remote.as_bytes() {
                return Err(SpaceAdmissionTransportError::AuthenticationRejected);
            }
            let ke2 = SpaceAdmissionKe2::decode_from_transport(&response.ke2)
                .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            let (credential, ke3) = client
                .finish(&context, ke2)
                .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            let credential = credential
                .into_core()
                .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            write_typed(
                &mut send,
                FrameKind::OpaqueFinish,
                &OpaqueFinishV1 {
                    ke3: ke3.encode_for_transport(),
                },
                AUTH_FRAME_LIMIT,
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            let newly_established = copy_credential(&credential)?;
            Ok(Box::new(EstablishedExchange {
                connection,
                send,
                receive,
                admission_id,
                binding,
                credential,
                newly_established: Some(newly_established),
            })
                as Box<dyn AuthenticatedAdmissionExchangePort>)
        }
        .instrument(span.clone())
        .await;
        span.in_scope(|| {
            if let Some(failure) = connection_failure {
                complete_admission_connection_failure(failure, started.elapsed());
                return;
            }
            record_client_completion(
                DiagnosticOperation::SpaceAdmission,
                started.elapsed(),
                result.as_ref().err(),
            )
        });
        result
    }

    async fn resume(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        binding: AdmissionPeerBinding,
        credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let started = Instant::now();
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::SpaceAdmission,
            operation: DiagnosticOperation::SpaceAdmission,
            role: DiagnosticRole::Joiner,
            kind: DiagnosticSpanKind::Client,
        });
        uc_observability_contract::diagnostics::describe_admission_connection(&span, true);
        let mut connection_failure = None;
        let result = async {
            let route = decode_route(route, false)?;
            let local = peer_id(self.endpoint.id().as_bytes())?;
            let remote = peer_id(route.endpoint_addr.id.as_bytes())?;
            if binding.local_peer_id() != local || binding.remote_peer_id() != remote {
                return Err(SpaceAdmissionTransportError::AuthenticationRejected);
            }
            let connection =
                connect(&self.endpoint, route.endpoint_addr)
                    .await
                    .map_err(|error| {
                        connection_failure = Some(error.category());
                        SpaceAdmissionTransportError::Deferred
                    })?;
            let (mut send, receive) = open_stream(&connection).await?;
            let nonce = random_nonce();
            let request_digest = [0u8; 32];
            let mac = calculate_mac(
                credential,
                b"resume",
                admission_id,
                local,
                remote,
                &nonce,
                &request_digest,
                None,
            )
            .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
            write_typed(
                &mut send,
                FrameKind::ContinuationHello,
                &ContinuationHelloV1 {
                    admission_id: *admission_id.as_bytes(),
                    local_peer_id: *local.as_bytes(),
                    remote_peer_id: *remote.as_bytes(),
                    nonce,
                    request_digest,
                    mac,
                },
                AUTH_FRAME_LIMIT,
            )
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
            Ok(Box::new(EstablishedExchange {
                connection,
                send,
                receive,
                admission_id,
                binding,
                credential: copy_credential(credential)?,
                newly_established: None,
            })
                as Box<dyn AuthenticatedAdmissionExchangePort>)
        }
        .instrument(span.clone())
        .await;
        span.in_scope(|| {
            if let Some(failure) = connection_failure {
                complete_admission_connection_failure(failure, started.elapsed());
                return;
            }
            record_client_completion(
                DiagnosticOperation::SpaceAdmission,
                started.elapsed(),
                result.as_ref().err(),
            )
        });
        result
    }
}

struct DecodedRoute {
    invitation_id: Option<[u8; 32]>,
    endpoint_addr: EndpointAddr,
}

fn decode_route(
    route: &SpaceAdmissionRoute,
    initial: bool,
) -> Result<DecodedRoute, SpaceAdmissionTransportError> {
    let wire: AdmissionDialRouteV1 = postcard::from_bytes(route.as_bytes())
        .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
    if wire.format_version != DIAL_ROUTE_FORMAT_V1 || (initial && wire.invitation_id.is_none()) {
        return Err(SpaceAdmissionTransportError::ProtocolRejected);
    }
    let endpoint_addr = postcard::from_bytes(&wire.endpoint_addr)
        .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
    Ok(DecodedRoute {
        invitation_id: wire.invitation_id,
        endpoint_addr,
    })
}

pub(crate) fn decode_space_admission_route(
    route: &[u8],
) -> Result<(EndpointAddr, Option<InvitationId>), SpaceAdmissionTransportError> {
    let route = SpaceAdmissionRoute::from_bytes(route.to_vec())
        .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
    let decoded = decode_route(&route, true)?;
    Ok((
        decoded.endpoint_addr,
        decoded.invitation_id.and_then(InvitationId::from_bytes),
    ))
}

#[derive(Debug, thiserror::Error)]
enum AdmissionConnectError {
    #[error("admission connection timed out")]
    TimedOut(#[source] tokio::time::error::Elapsed),
    #[error("admission connection failed")]
    Transport(#[source] iroh::endpoint::ConnectError),
}
impl AdmissionConnectError {
    fn category(&self) -> DialFailure {
        match self {
            Self::TimedOut(_) => DialFailure::TimedOut,
            Self::Transport(_) => DialFailure::TransportFailed,
        }
    }
}
async fn connect(
    endpoint: &Endpoint,
    addr: EndpointAddr,
) -> Result<Connection, AdmissionConnectError> {
    // 底层连接驱动不能延长业务 span，完整调用的结算仍由外层负责人执行。
    let connection = endpoint
        .connect(addr, SPACE_ADMISSION_ALPN)
        .with_subscriber(tracing::Dispatch::new(
            tracing::subscriber::NoSubscriber::default(),
        ));
    tokio::time::timeout(IO_DEADLINE, connection)
        .await
        .map_err(AdmissionConnectError::TimedOut)?
        .map_err(AdmissionConnectError::Transport)
}

async fn open_stream(
    connection: &Connection,
) -> Result<(SendStream, RecvStream), SpaceAdmissionTransportError> {
    tokio::time::timeout(IO_DEADLINE, connection.open_bi())
        .await
        .map_err(|_| SpaceAdmissionTransportError::Deferred)?
        .map_err(|_| SpaceAdmissionTransportError::Deferred)
}

fn peer_id(bytes: &[u8; 32]) -> Result<AdmissionChannelPeerId, SpaceAdmissionTransportError> {
    AdmissionChannelPeerId::from_bytes(*bytes)
        .ok_or(SpaceAdmissionTransportError::AuthenticationRejected)
}

fn copy_credential(
    credential: &AdmissionContinuationCredential,
) -> Result<AdmissionContinuationCredential, SpaceAdmissionTransportError> {
    AdmissionContinuationCredential::from_bytes(credential.as_bytes().to_vec())
        .map_err(|_| SpaceAdmissionTransportError::Unavailable)
}

fn random_nonce() -> [u8; 32] {
    let mut nonce = [0u8; 32];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

fn calculate_mac(
    credential: &AdmissionContinuationCredential,
    direction: &[u8],
    admission_id: SpaceAdmissionId,
    sender: AdmissionChannelPeerId,
    receiver: AdmissionChannelPeerId,
    nonce: &[u8; 32],
    digest: &[u8; 32],
    trace_context: Option<&WireTraceContext>,
) -> Result<Vec<u8>, HandlerError> {
    let mut mac = HmacSha512::new_from_slice(credential.as_bytes())
        .map_err(|_| HandlerError::Authentication)?;
    update_mac(
        &mut mac,
        direction,
        admission_id,
        sender,
        receiver,
        nonce,
        digest,
        trace_context,
    );
    Ok(mac.finalize().into_bytes().to_vec())
}

fn verify_mac(
    credential: &AdmissionContinuationCredential,
    direction: &[u8],
    admission_id: SpaceAdmissionId,
    sender: AdmissionChannelPeerId,
    receiver: AdmissionChannelPeerId,
    nonce: &[u8; 32],
    digest: &[u8; 32],
    trace_context: Option<&WireTraceContext>,
    provided: &[u8],
) -> Result<(), HandlerError> {
    if provided.len() != 64 {
        return Err(HandlerError::Authentication);
    }
    let mut mac = HmacSha512::new_from_slice(credential.as_bytes())
        .map_err(|_| HandlerError::Authentication)?;
    update_mac(
        &mut mac,
        direction,
        admission_id,
        sender,
        receiver,
        nonce,
        digest,
        trace_context,
    );
    mac.verify_slice(provided)
        .map_err(|_| HandlerError::Authentication)
}

fn update_mac(
    mac: &mut HmacSha512,
    direction: &[u8],
    admission_id: SpaceAdmissionId,
    sender: AdmissionChannelPeerId,
    receiver: AdmissionChannelPeerId,
    nonce: &[u8; 32],
    digest: &[u8; 32],
    trace_context: Option<&WireTraceContext>,
) {
    mac.update(b"uc-space-admission-channel-v1");
    mac.update(direction);
    mac.update(admission_id.as_bytes());
    mac.update(sender.as_bytes());
    mac.update(receiver.as_bytes());
    mac.update(nonce);
    mac.update(digest);
    let trace_context_digest = trace_context_digest(trace_context);
    mac.update(&trace_context_digest);
}

fn trace_context_digest(trace_context: Option<&WireTraceContext>) -> [u8; 32] {
    let encoded = trace_context
        .and_then(|context| postcard::to_stdvec(context).ok())
        .unwrap_or_default();
    Sha256::digest(encoded).into()
}

fn record_client_completion(
    operation: DiagnosticOperation,
    elapsed: Duration,
    error: Option<&SpaceAdmissionTransportError>,
) {
    let completion = match error {
        None => OperationCompletion::succeeded(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            elapsed,
        ),
        Some(SpaceAdmissionTransportError::AuthenticationRejected) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            DiagnosticErrorType::AuthenticationFailed,
            elapsed,
        ),
        Some(SpaceAdmissionTransportError::PeerUpgradeRequired) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            DiagnosticErrorType::PeerIncompatible,
            elapsed,
        ),
        Some(SpaceAdmissionTransportError::ProtocolRejected) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            DiagnosticErrorType::DecodeFailed,
            elapsed,
        ),
        Some(SpaceAdmissionTransportError::Deferred) => OperationCompletion::deferred(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            elapsed,
        ),
        Some(
            SpaceAdmissionTransportError::InvitationUnavailable
            | SpaceAdmissionTransportError::Unavailable,
        ) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            operation,
            DiagnosticRole::Joiner,
            DiagnosticErrorType::Unavailable,
            elapsed,
        ),
    };
    complete_operation(completion);
}

fn record_server_completion(elapsed: Duration, error: Option<&HandlerError>) {
    let completion = match error {
        None => OperationCompletion::succeeded(
            DiagnosticDomain::SpaceAdmission,
            DiagnosticOperation::NetworkTransport,
            DiagnosticRole::Sponsor,
            elapsed,
        ),
        Some(error) => OperationCompletion::failed(
            DiagnosticDomain::SpaceAdmission,
            DiagnosticOperation::NetworkTransport,
            DiagnosticRole::Sponsor,
            server_error_type(error),
            elapsed,
        ),
    };
    complete_operation(completion);
}

fn server_operation_span() -> tracing::Span {
    operation_span(OperationContext {
        domain: DiagnosticDomain::SpaceAdmission,
        operation: DiagnosticOperation::NetworkTransport,
        role: DiagnosticRole::Sponsor,
        kind: DiagnosticSpanKind::Server,
    })
}

// 等待位置只用于本次认证的外层截止时间，不参与协议状态或业务决策。
#[derive(Clone, Copy)]
enum AuthenticationStep {
    ReceiveHello,
    InitialVersion,
    InitialIdentity,
    InitialCredential,
    InitialProof,
    ContinuationIdentity,
    ContinuationCredential,
    ContinuationProof,
    ReceiveRequest,
    RequestIdentity,
    RequestProof,
}
impl AuthenticationStep {
    fn failure(self, error: &HandlerError) -> AuthenticationFailure {
        let credential = match error {
            HandlerError::Credential(source) => source.diagnostic_failure(),
            _ => CredentialFailure::Unavailable,
        };
        let read = match error {
            HandlerError::Transport { .. } => ReadFailure::TransportFailed,
            HandlerError::Timeout => ReadFailure::TimedOut,
            HandlerError::PeerUpgradeRequired => ReadFailure::PeerUpgradeRequired,
            _ => ReadFailure::InvalidMessage,
        };
        let proof = if matches!(
            error,
            HandlerError::Authentication | HandlerError::AuthenticationProof { .. }
        ) {
            ProofFailure::Rejected
        } else {
            ProofFailure::Exchange(read)
        };
        match self {
            Self::ReceiveHello => AuthenticationFailure::ReadHello(read),
            Self::ReceiveRequest => AuthenticationFailure::ReadRequest(read),
            Self::InitialVersion => {
                AuthenticationFailure::IdentityMismatch(IdentityCheck::InitialVersion)
            }
            Self::InitialIdentity => {
                AuthenticationFailure::IdentityMismatch(IdentityCheck::InitialPeer)
            }
            Self::ContinuationIdentity => {
                AuthenticationFailure::IdentityMismatch(IdentityCheck::ContinuationPeer)
            }
            Self::RequestIdentity => {
                AuthenticationFailure::IdentityMismatch(IdentityCheck::RequestBinding)
            }
            Self::InitialCredential if matches!(error, HandlerError::Timeout) => {
                AuthenticationFailure::DeadlineExceeded(AuthenticationStage::InitialCredential)
            }
            Self::ContinuationCredential if matches!(error, HandlerError::Timeout) => {
                AuthenticationFailure::DeadlineExceeded(AuthenticationStage::ContinuationCredential)
            }
            Self::InitialCredential => AuthenticationFailure::InitialCredential(credential),
            Self::ContinuationCredential => {
                AuthenticationFailure::ContinuationCredential(credential)
            }
            Self::InitialProof => AuthenticationFailure::InitialProof(proof),
            Self::ContinuationProof => AuthenticationFailure::ContinuationProof(proof),
            Self::RequestProof => AuthenticationFailure::RequestProof(proof),
        }
    }
}

fn server_error_type(error: &HandlerError) -> DiagnosticErrorType {
    match error {
        HandlerError::Authentication
        | HandlerError::Credential(_)
        | HandlerError::AuthenticationProof { .. } => DiagnosticErrorType::AuthenticationFailed,
        HandlerError::Protocol | HandlerError::Transport { .. } => {
            DiagnosticErrorType::DecodeFailed
        }
        HandlerError::PeerUpgradeRequired => DiagnosticErrorType::PeerIncompatible,
        HandlerError::Application => DiagnosticErrorType::Internal,
        HandlerError::Acknowledgement => DiagnosticErrorType::ChannelClosed,
        HandlerError::Timeout => DiagnosticErrorType::Timeout,
    }
}

#[derive(Debug, thiserror::Error)]
enum HandlerError {
    #[error("admission message is invalid")]
    Protocol,
    #[error("admission authentication was rejected")]
    Authentication,
    #[error("admission credential could not be used")]
    Credential(#[source] SpaceAdmissionChannelCredentialError),
    #[error("admission proof was rejected")]
    AuthenticationProof {
        #[source]
        source: anyhow::Error,
    },
    #[error("admission transport failed")]
    Transport {
        #[source]
        source: anyhow::Error,
    },
    #[error("admission peer upgrade required")]
    PeerUpgradeRequired,
    #[error("admission handling failed")]
    Application,
    #[error("admission acknowledgement missing")]
    Acknowledgement,
    #[error("admission deadline elapsed")]
    Timeout,
}

fn map_request_wire_error(error: WireError) -> HandlerError {
    match error {
        WireError::UnsupportedLayout => HandlerError::PeerUpgradeRequired,
        other => map_server_wire_error(other),
    }
}

fn map_server_wire_error(error: WireError) -> HandlerError {
    match error {
        WireError::Timeout => HandlerError::Timeout,
        WireError::Io(source) => HandlerError::Transport {
            source: anyhow::Error::new(source),
        },
        _ => HandlerError::Protocol,
    }
}

fn map_reply_wire_error(error: WireError) -> SpaceAdmissionTransportError {
    match error {
        WireError::UnsupportedLayout => SpaceAdmissionTransportError::PeerUpgradeRequired,
        _ => SpaceAdmissionTransportError::ProtocolRejected,
    }
}

async fn read_authenticated_reply(
    receive: &mut RecvStream,
    connection: &Connection,
) -> Result<
    (AuthenticatedEnvelopeV1, SpaceAdmissionEnvelopeV1, [u8; 32]),
    SpaceAdmissionTransportError,
> {
    match read_envelope(receive, FrameKind::Reply).await {
        Ok(reply) => Ok(reply),
        Err(error @ WireError::UnsupportedLayout) => Err(map_reply_wire_error(error)),
        Err(error) => {
            let close_reason = match connection.close_reason() {
                Some(reason) => Some(reason),
                None => tokio::time::timeout(Duration::from_millis(100), connection.closed())
                    .await
                    .ok(),
            };
            match close_reason {
                Some(iroh::endpoint::ConnectionError::ApplicationClosed(close)) => {
                    match map_application_close_code(close.error_code.into_inner()) {
                        Some(mapped) => Err(mapped),
                        None => Err(map_reply_wire_error(error)),
                    }
                }
                _ => Err(map_reply_wire_error(error)),
            }
        }
    }
}

fn map_application_close_code(code: u64) -> Option<SpaceAdmissionTransportError> {
    match code {
        code if code == u64::from(CLOSE_PEER_UPGRADE_REQUIRED)
            || code == u64::from(LEGACY_CLOSE_PROTOCOL) =>
        {
            Some(SpaceAdmissionTransportError::PeerUpgradeRequired)
        }
        code if code == u64::from(CLOSE_AUTHENTICATION) => {
            Some(SpaceAdmissionTransportError::AuthenticationRejected)
        }
        code if code == u64::from(CLOSE_BUSY) => Some(SpaceAdmissionTransportError::Deferred),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use iroh::RelayMode;
    use opentelemetry::logs::AnyValue;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
    use tokio::sync::{Mutex, Notify};
    use tracing_subscriber::filter::filter_fn;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Layer;
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        AdmissionBaseSnapshot, AdmissionCandidateV1, AdmissionChangeFacts, AdmissionCommitV1,
        AdmissionContinuationRoute, AdmissionIdentitySignature, AdmissionInvitationClaim,
        AdmissionJoinRequestV1, AdmissionKeyPackage, AdmissionMessageId, AdmissionMlsCommit,
        AdmissionMlsWelcome, AdmissionPreparedV1, AdmissionRecoveryPublicKey,
        AdmissionSealedRecoveryMaterial, AdmissionSealedSecurityState,
        AdmissionSecurityCommitmentV1, AdmissionSignedMembershipHistory,
        AdmissionStagedSecurityState, BaseMembershipHistoryPosition, MemberInstanceId,
        MembershipAdmissionV2, MembershipCredential, MembershipEventV2, MembershipOperationV2,
        PreparedAdmissionProofV1, SponsorAdmission, UnreadableHistoryPolicy,
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
        MEMBERSHIP_EVENT_FORMAT_V2,
    };
    use uc_core::membership::{AdmissionRecordPersistence, AdmissionRole, SpaceAdmissionBodyV1};
    use uc_core::security::IdentityFingerprint;

    use super::*;

    fn credential() -> AdmissionContinuationCredential {
        AdmissionContinuationCredential::from_bytes(vec![0x31; 64])
            .expect("bounded continuation credential")
    }

    fn admission_id() -> SpaceAdmissionId {
        SpaceAdmissionId::from_bytes([0x32; 32]).expect("non-zero admission id")
    }

    fn peer(byte: u8) -> AdmissionChannelPeerId {
        AdmissionChannelPeerId::from_bytes([byte; 32]).expect("non-zero peer id")
    }

    #[test]
    fn continuation_mac_binds_direction_identity_nonce_and_digest() {
        let credential = credential();
        let admission = admission_id();
        let sender = peer(0x33);
        let receiver = peer(0x34);
        let nonce = [0x35; 32];
        let digest = [0x36; 32];
        let mac = calculate_mac(
            &credential,
            b"request",
            admission,
            sender,
            receiver,
            &nonce,
            &digest,
            None,
        )
        .expect("valid MAC inputs");

        assert!(verify_mac(
            &credential,
            b"request",
            admission,
            sender,
            receiver,
            &nonce,
            &digest,
            None,
            &mac,
        )
        .is_ok());
        assert!(verify_mac(
            &credential,
            b"reply",
            admission,
            sender,
            receiver,
            &nonce,
            &digest,
            None,
            &mac,
        )
        .is_err());
        assert!(verify_mac(
            &credential,
            b"request",
            admission,
            receiver,
            sender,
            &nonce,
            &digest,
            None,
            &mac,
        )
        .is_err());
        let mut changed_nonce = nonce;
        changed_nonce[0] ^= 1;
        assert!(verify_mac(
            &credential,
            b"request",
            admission,
            sender,
            receiver,
            &changed_nonce,
            &digest,
            None,
            &mac,
        )
        .is_err());
        let mut changed_digest = digest;
        changed_digest[0] ^= 1;
        assert!(verify_mac(
            &credential,
            b"request",
            admission,
            sender,
            receiver,
            &nonce,
            &changed_digest,
            None,
            &mac,
        )
        .is_err());

        let trace_context = WireTraceContext {
            traceparent: "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_owned(),
        };
        assert!(verify_mac(
            &credential,
            b"request",
            admission,
            sender,
            receiver,
            &nonce,
            &digest,
            Some(&trace_context),
            &mac,
        )
        .is_err());
    }

    #[test]
    fn old_layout_and_close_codes_keep_upgrade_authentication_and_protocol_distinct() {
        assert!(matches!(
            map_request_wire_error(WireError::UnsupportedLayout),
            HandlerError::PeerUpgradeRequired
        ));
        assert!(matches!(
            map_reply_wire_error(WireError::UnsupportedLayout),
            SpaceAdmissionTransportError::PeerUpgradeRequired
        ));
        assert!(matches!(
            map_application_close_code(u64::from(CLOSE_PEER_UPGRADE_REQUIRED)),
            Some(SpaceAdmissionTransportError::PeerUpgradeRequired)
        ));
        assert!(matches!(
            map_application_close_code(u64::from(CLOSE_AUTHENTICATION)),
            Some(SpaceAdmissionTransportError::AuthenticationRejected)
        ));
        assert!(matches!(
            map_application_close_code(u64::from(CLOSE_BUSY)),
            Some(SpaceAdmissionTransportError::Deferred)
        ));
        assert!(map_application_close_code(u64::from(CLOSE_PROTOCOL)).is_none());
        assert!(matches!(
            map_application_close_code(u64::from(LEGACY_CLOSE_PROTOCOL)),
            Some(SpaceAdmissionTransportError::PeerUpgradeRequired)
        ));
        assert!(matches!(
            map_server_wire_error(WireError::Timeout),
            HandlerError::Timeout
        ));
    }

    #[test]
    fn deferred_client_completion_is_not_reported_as_a_failure() {
        let log_exporter = InMemoryLogExporter::default();
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(log_exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(
            OpenTelemetryTracingBridge::new(&logs)
                .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry")),
        );

        tracing::subscriber::with_default(subscriber, || {
            record_client_completion(
                DiagnosticOperation::NetworkTransport,
                Duration::from_millis(7),
                Some(&SpaceAdmissionTransportError::Deferred),
            );
        });
        logs.force_flush().expect("log flush");

        let emitted = log_exporter.get_emitted_logs().expect("emitted logs");
        assert_eq!(emitted.len(), 1);
        let record = &emitted[0].record;
        assert!(record.attributes_iter().any(|(key, value)| {
            key.as_str() == "uc.outcome"
                && matches!(value, AnyValue::String(value) if value.as_str() == "deferred")
        }));
        assert!(!record
            .attributes_iter()
            .any(|(key, _)| key.as_str() == "error.type"));
    }

    #[test]
    fn pre_authentication_failure_is_one_unassociated_log_without_a_span() {
        let span_exporter = InMemorySpanExporter::default();
        let log_exporter = InMemoryLogExporter::default();
        let traces = SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .build();
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(log_exporter.clone())
            .build();
        let trace_layer = tracing_opentelemetry::layer()
            .with_tracer(traces.tracer("space-admission-pre-auth-test"))
            .with_context_activation(true)
            .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
        let log_layer = OpenTelemetryTracingBridge::new(&logs)
            .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
        let subscriber = tracing_subscriber::registry()
            .with(trace_layer)
            .with(log_layer);

        tracing::subscriber::with_default(subscriber, || {
            complete_admission_authentication_failure(
                AuthenticationFailure::InitialProof(ProofFailure::Rejected),
                Duration::from_secs(30),
            );
        });
        traces.force_flush().expect("trace flush");
        logs.force_flush().expect("log flush");

        assert!(span_exporter
            .get_finished_spans()
            .expect("finished spans")
            .is_empty());
        let emitted = log_exporter.get_emitted_logs().expect("emitted logs");
        assert_eq!(emitted.len(), 1);
        let record = &emitted[0].record;
        assert!(record.trace_context().is_none());
        assert!(record.body().is_none());
        assert!(record.attributes_iter().any(|(key, value)| {
            key.as_str() == "error.type"
                && matches!(value, AnyValue::String(value) if value.as_str() == "authentication_failed")
        }));
        assert!(record.attributes_iter().any(|(key, value)| {
            key.as_str() == "duration_ms" && matches!(value, AnyValue::Int(30_000))
        }));
    }

    #[tokio::test]
    async fn stalled_pre_authentication_records_one_unassociated_timeout() {
        let span_exporter = InMemorySpanExporter::default();
        let log_exporter = InMemoryLogExporter::default();
        let traces = SdkTracerProvider::builder()
            .with_simple_exporter(span_exporter.clone())
            .build();
        let logs = SdkLoggerProvider::builder()
            .with_simple_exporter(log_exporter.clone())
            .build();
        let trace_layer = tracing_opentelemetry::layer()
            .with_tracer(traces.tracer("space-admission-stalled-auth-test"))
            .with_context_activation(true)
            .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
        let log_layer = OpenTelemetryTracingBridge::new(&logs).with_filter(filter_fn(|metadata| {
            matches!(metadata.target(), "uc.telemetry" | "uc.connectivity")
        }));
        let subscriber = tracing_subscriber::registry()
            .with(trace_layer)
            .with(log_layer);
        let _subscriber = tracing::subscriber::set_default(subscriber);

        let sponsor = bound_endpoint().await;
        wait_for_direct_addrs(&sponsor).await;
        let joiner = bound_endpoint().await;
        wait_for_direct_addrs(&joiner).await;
        let endpoint = Arc::new(HangingLoopbackEndpoint {
            calls: AtomicUsize::new(0),
            entered: Notify::new(),
        });
        let credentials = Arc::new(LoopbackCredentials {
            initial: Mutex::new(None),
            continuation: Mutex::new(None),
        });
        let handler = Arc::new(
            IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials)
                .expect("handler")
                .with_exchange_deadline(Duration::from_millis(100)),
        );
        let router = Router::builder((*sponsor).clone())
            .accept(SPACE_ADMISSION_ALPN, handler)
            .spawn();
        let connection = connect(&joiner, sponsor.addr())
            .await
            .expect("connect stalled peer");
        let (_send, _receive) = open_stream(&connection).await.expect("open stalled stream");
        tokio::time::timeout(Duration::from_secs(2), connection.closed())
            .await
            .expect("server deadline must close the stalled peer");
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);

        router.shutdown().await.expect("router shutdown");
        joiner.close().await;
        sponsor.close().await;
        traces.force_flush().expect("trace flush");
        logs.force_flush().expect("log flush");

        assert!(span_exporter
            .get_finished_spans()
            .expect("finished spans")
            .is_empty());
        let emitted = log_exporter.get_emitted_logs().expect("emitted logs");
        let completed: Vec<_> = emitted.iter().filter(|entry| entry.record.attributes_iter().any(|(key, value)| {
            key.as_str() == "event.name" && matches!(value, AnyValue::String(value) if value.as_str() == "uc.operation.completed")
        })).collect();
        assert_eq!(completed.len(), 1);
        let record = &completed[0].record;
        assert!(record.trace_context().is_none());
        assert!(record.attributes_iter().any(|(key, value)| {
            key.as_str() == "error.type"
                && matches!(value, AnyValue::String(value) if value.as_str() == "timeout")
        }));
        assert!(record.attributes_iter().any(|(key, value)| {
            key.as_str() == "duration_ms"
                && matches!(value, AnyValue::Int(duration) if *duration >= 100)
        }));
    }

    #[tokio::test]
    async fn rejected_continuation_diagnostics_distinguish_identity_credentials_and_proof() {
        #[derive(Debug)]
        struct DetailProbe(Arc<std::sync::Mutex<Vec<(&'static str, &'static str)>>>);
        impl opentelemetry_sdk::logs::LogProcessor for DetailProbe {
            fn emit(
                &self,
                data: &mut opentelemetry_sdk::logs::SdkLogRecord,
                _: &opentelemetry::InstrumentationScope,
            ) {
                let field = |name: &str| {
                    data.attributes_iter()
                        .find_map(|(key, value)| {
                            if key.as_str() == name {
                                if let AnyValue::String(value) = value {
                                    return Some(value.as_str());
                                }
                            }
                            None
                        })
                        .unwrap_or_default()
                };
                if let Some(detail) = uc_observability_contract::diagnostics::connectivity::take_local_completion_detail(
                    field("uc.domain"), field("uc.operation"), field("uc.role"), field("uc.outcome"),
                ) { self.0.lock().expect("details").push(detail.local_fields()); }
            }
            fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
                Ok(())
            }
            fn shutdown_with_timeout(
                &self,
                _: Duration,
            ) -> opentelemetry_sdk::error::OTelSdkResult {
                Ok(())
            }
        }
        for (wrong_identity, stored_credential, expected_stage, expected_reason) in [
            (true, None, "continuation_identity", "identity_mismatch"),
            (
                false,
                None,
                "continuation_credential",
                "storage_unavailable",
            ),
            (
                false,
                Some(vec![7; 64]),
                "continuation_proof",
                "proof_rejected",
            ),
        ] {
            let exporter = InMemoryLogExporter::default();
            let details = Arc::new(std::sync::Mutex::new(Vec::new()));
            let logs = SdkLoggerProvider::builder()
                .with_simple_exporter(exporter.clone())
                .with_log_processor(DetailProbe(details.clone()))
                .build();
            let subscriber = tracing_subscriber::registry().with(
                OpenTelemetryTracingBridge::new(&logs)
                    .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry")),
            );
            let _subscriber = tracing::subscriber::set_default(subscriber);
            let sponsor = bound_endpoint().await;
            wait_for_direct_addrs(&sponsor).await;
            let joiner = bound_endpoint().await;
            wait_for_direct_addrs(&joiner).await;
            let endpoint = Arc::new(HangingLoopbackEndpoint {
                calls: AtomicUsize::new(0),
                entered: Notify::new(),
            });
            let credentials = Arc::new(LoopbackCredentials {
                initial: Mutex::new(None),
                continuation: Mutex::new(stored_credential),
            });
            let handler = Arc::new(
                IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials)
                    .expect("handler"),
            );
            let router = Router::builder((*sponsor).clone())
                .accept(SPACE_ADMISSION_ALPN, handler)
                .spawn();
            let connection = connect(&joiner, sponsor.addr()).await.expect("connection");
            let (mut send, _receive) = open_stream(&connection).await.expect("stream");
            write_typed(
                &mut send,
                FrameKind::ContinuationHello,
                &ContinuationHelloV1 {
                    admission_id: [3; 32],
                    local_peer_id: if wrong_identity {
                        [9; 32]
                    } else {
                        *joiner.id().as_bytes()
                    },
                    remote_peer_id: *sponsor.id().as_bytes(),
                    nonce: [4; 32],
                    request_digest: [0; 32],
                    mac: vec![0; 64],
                },
                AUTH_FRAME_LIMIT,
            )
            .await
            .expect("hello");
            let closed = tokio::time::timeout(Duration::from_secs(2), connection.closed())
                .await
                .expect("rejected");
            assert!(
                matches!(closed, iroh::endpoint::ConnectionError::ApplicationClosed(ref close) if close.error_code == CLOSE_AUTHENTICATION.into())
            );
            assert_eq!(endpoint.calls.load(Ordering::SeqCst), 0);
            router.shutdown().await.expect("router");
            joiner.close().await;
            sponsor.close().await;
            logs.force_flush().expect("flush");
            let records = exporter.get_emitted_logs().expect("diagnostics");
            assert_eq!(records.len(), 1);
            assert_eq!(
                *details.lock().expect("details"),
                vec![(expected_stage, expected_reason)]
            );
            let record = &records[0].record;
            assert!(record.attributes_iter().any(|(key, value)| key.as_str() == "error.type" && matches!(value, AnyValue::String(value) if value.as_str() == "authentication_failed")));
            assert!(record.body().is_none());
            assert!(record.trace_context().is_none());
        }
    }

    #[tokio::test]
    async fn server_rejects_missing_and_invalid_peer_acknowledgements() {
        let (writer, mut reader) = tokio::io::duplex(128);
        drop(writer);
        assert!(matches!(
            read_peer_acknowledgement(&mut reader).await,
            Err(HandlerError::Acknowledgement)
        ));

        let (mut writer, mut reader) = tokio::io::duplex(128);
        write_typed(&mut writer, FrameKind::Ack, &0_u8, AUTH_FRAME_LIMIT)
            .await
            .expect("invalid acknowledgement frame");
        assert!(matches!(
            read_peer_acknowledgement(&mut reader).await,
            Err(HandlerError::Protocol)
        ));
        assert_eq!(
            server_error_type(&HandlerError::Acknowledgement),
            DiagnosticErrorType::ChannelClosed
        );
        assert_eq!(
            server_error_type(&HandlerError::Timeout),
            DiagnosticErrorType::Timeout
        );
    }

    #[tokio::test]
    async fn server_classifies_a_stalled_peer_acknowledgement_as_timeout() {
        let (_writer, mut reader) = tokio::io::duplex(128);
        tokio::time::pause();
        let acknowledgement = read_peer_acknowledgement(&mut reader);
        tokio::pin!(acknowledgement);
        tokio::select! {
            biased;
            _ = &mut acknowledgement => panic!("stalled acknowledgement completed early"),
            () = tokio::task::yield_now() => {}
        }
        tokio::time::advance(IO_DEADLINE + Duration::from_millis(1)).await;
        tokio::task::yield_now().await;

        assert!(matches!(acknowledgement.await, Err(HandlerError::Timeout)));
    }

    #[test]
    fn authenticated_context_builds_a_real_client_server_parent() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let subscriber = tracing_subscriber::registry().with(
            tracing_opentelemetry::layer()
                .with_tracer(provider.tracer("space-admission-trace-test"))
                .with_context_activation(true),
        );
        let admission = admission_id();

        tracing::subscriber::with_default(subscriber, || {
            let client = operation_span(OperationContext {
                domain: DiagnosticDomain::SpaceAdmission,
                operation: DiagnosticOperation::NetworkTransport,
                role: DiagnosticRole::Joiner,
                kind: DiagnosticSpanKind::Client,
            });
            let _client_entered = client.enter();
            let wire_context = inject_current().expect("client W3C context");
            let credential = credential();
            let sender = peer(0x33);
            let receiver = peer(0x34);
            let nonce = [0x35; 32];
            let digest = [0x36; 32];
            let mac = calculate_mac(
                &credential,
                b"request",
                admission,
                sender,
                receiver,
                &nonce,
                &digest,
                Some(&wire_context),
            )
            .expect("context-bound MAC");
            verify_mac(
                &credential,
                b"request",
                admission,
                sender,
                receiver,
                &nonce,
                &digest,
                Some(&wire_context),
                &mac,
            )
            .expect("authenticated context");
            let server = operation_span(OperationContext {
                domain: DiagnosticDomain::SpaceAdmission,
                operation: DiagnosticOperation::NetworkTransport,
                role: DiagnosticRole::Sponsor,
                kind: DiagnosticSpanKind::Server,
            });
            assert!(set_remote_parent(&server, Some(&wire_context)));
            let _server_entered = server.enter();
        });
        provider.force_flush().expect("trace flush");

        let spans = exporter.get_finished_spans().expect("finished spans");
        let client = spans
            .iter()
            .find(|span| {
                span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "uc.role" && attribute.value.as_str() == "joiner"
                })
            })
            .expect("client span");
        let server = spans
            .iter()
            .find(|span| {
                span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "uc.role" && attribute.value.as_str() == "sponsor"
                })
            })
            .expect("server span");
        assert!(!server
            .attributes
            .iter()
            .any(|attribute| attribute.key.as_str() == "uc.flow.id"));
        assert_eq!(
            server.span_context.trace_id(),
            client.span_context.trace_id()
        );
        assert_eq!(server.parent_span_id, client.span_context.span_id());
    }

    struct LoopbackCredentials {
        initial: Mutex<Option<SponsorOpaqueMaterial>>,
        continuation: Mutex<Option<Vec<u8>>>,
    }

    #[async_trait]
    impl SpaceAdmissionChannelCredentialPort for LoopbackCredentials {
        async fn resolve_initial(
            &self,
            _invitation_id: InvitationId,
            _admission_id: SpaceAdmissionId,
        ) -> Result<SponsorOpaqueMaterial, SpaceAdmissionChannelCredentialError> {
            self.initial.lock().await.take().ok_or_else(|| {
                SpaceAdmissionChannelCredentialError::Rejected {
                    source: anyhow::anyhow!("initial credential already consumed"),
                }
            })
        }

        async fn load_continuation(
            &self,
            _admission_id: SpaceAdmissionId,
        ) -> Result<AdmissionContinuationCredential, SpaceAdmissionChannelCredentialError> {
            let bytes = self.continuation.lock().await.clone().ok_or_else(|| {
                SpaceAdmissionChannelCredentialError::Unavailable {
                    source: anyhow::anyhow!("continuation is not committed"),
                }
            })?;
            AdmissionContinuationCredential::from_bytes(bytes).map_err(|source| {
                SpaceAdmissionChannelCredentialError::Rejected {
                    source: anyhow::Error::new(source),
                }
            })
        }
    }

    struct PersistingLoopbackEndpoint {
        credentials: Arc<LoopbackCredentials>,
        candidate_state: Mutex<Option<Vec<u8>>>,
        calls: AtomicUsize,
        completed: AtomicUsize,
        continuation_route: Vec<u8>,
    }

    struct HangingLoopbackEndpoint {
        calls: AtomicUsize,
        entered: Notify,
    }

    #[async_trait]
    impl HandleAuthenticatedSpaceAdmissionMessagePort for HangingLoopbackEndpoint {
        async fn handle(
            &self,
            _message: AuthenticatedSpaceAdmissionMessage,
        ) -> Result<
            uc_application::deps::SpaceAdmissionMessageReply,
            uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            std::future::pending().await
        }
    }

    struct LegacyLayoutHandler {
        local_peer_id: AdmissionChannelPeerId,
        credentials: Arc<LoopbackCredentials>,
    }

    impl std::fmt::Debug for LegacyLayoutHandler {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("LegacyLayoutHandler(REDACTED)")
        }
    }

    impl ProtocolHandler for LegacyLayoutHandler {
        async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
            let remote_peer_id =
                peer_id(connection.remote_id().as_bytes()).expect("legacy remote peer");
            let (mut send, mut receive) = connection.accept_bi().await.expect("legacy stream");
            let (kind, payload) = read_raw_with_limit(&mut receive, AUTH_FRAME_LIMIT)
                .await
                .expect("legacy initial hello");
            assert_eq!(kind, FrameKind::InitialHello);
            let hello: InitialHelloV1 =
                postcard::from_bytes(&payload).expect("legacy hello layout");
            let admission_id =
                SpaceAdmissionId::from_bytes(hello.admission_id).expect("legacy admission id");
            let invitation_id =
                InvitationId::from_bytes(hello.invitation_id).expect("legacy invitation id");
            assert_eq!(
                hello.protocol_version,
                SpaceAdmissionProtocolVersion::V1.as_u16()
            );
            assert_eq!(hello.joiner_peer_id, *remote_peer_id.as_bytes());
            let material = self
                .credentials
                .resolve_initial(invitation_id, admission_id)
                .await
                .expect("legacy credential");
            let context = SpaceAdmissionAuthContext::new(
                SpaceAdmissionProtocolVersion::V1,
                admission_id,
                invitation_id,
                remote_peer_id,
                self.local_peer_id,
            );
            let ke1 = SpaceAdmissionKe1::decode_from_transport(&hello.ke1)
                .expect("legacy client authentication start");
            let (server, ke2) = SpaceAdmissionAuth::start_server(
                &material.server_setup,
                &material.registration,
                &context,
                ke1,
            )
            .expect("legacy server authentication start");
            write_typed(
                &mut send,
                FrameKind::OpaqueResponse,
                &OpaqueResponseV1 {
                    sponsor_peer_id: *self.local_peer_id.as_bytes(),
                    ke2: ke2.encode_for_transport(),
                },
                AUTH_FRAME_LIMIT,
            )
            .await
            .expect("legacy authentication response");
            let finish: OpaqueFinishV1 =
                read_typed(&mut receive, FrameKind::OpaqueFinish, AUTH_FRAME_LIMIT)
                    .await
                    .expect("legacy authentication finish");
            let ke3 = SpaceAdmissionKe3::decode_from_transport(&finish.ke3)
                .expect("legacy authentication finish payload");
            let _credential = server
                .finish(&context, ke3)
                .expect("legacy authenticated peer");
            let (kind, payload) = read_raw_with_limit(&mut receive, LARGE_MESSAGE_LIMIT)
                .await
                .expect("legacy authenticated request");
            assert_eq!(kind, FrameKind::Request);
            assert!(!payload.is_empty());
            connection.close(LEGACY_CLOSE_PROTOCOL.into(), b"protocol_rejected");
            Ok(())
        }
    }

    #[async_trait]
    impl HandleAuthenticatedSpaceAdmissionMessagePort for PersistingLoopbackEndpoint {
        async fn handle(
            &self,
            message: AuthenticatedSpaceAdmissionMessage,
        ) -> Result<
            uc_application::deps::SpaceAdmissionMessageReply,
            uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError,
        > {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (binding, envelope, digest, continuation) = message.into_parts();
            match envelope.body() {
                SpaceAdmissionBodyV1::JoinRequest(_) => {
                    let continuation = continuation.ok_or_else(|| {
                        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                            anyhow::anyhow!("fresh request missing continuation"),
                        )
                    })?;
                    *self.credentials.continuation.lock().await =
                        Some(continuation.as_bytes().to_vec());
                    let admission_id = envelope.header().admission_id();
                    let predecessor = envelope.header().message_id();
                    let accepted = SponsorAdmission::accept_join_request(
                        admission_id,
                        AdmissionInvitationClaim::from_bytes(vec![0x41; 32]).map_err(
                            uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                        )?,
                        envelope,
                        uc_core::membership::AdmissionMessageEvidence::new(
                            AdmissionRole::Joiner,
                            0,
                            predecessor,
                            None,
                            digest,
                        )
                        .ok_or_else(|| uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!("invalid evidence")))?,
                        AdmissionBaseSnapshot::from_bytes(vec![0x42; 64]).map_err(
                            uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                        )?,
                        binding,
                        continuation,
                    )
                    .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?
                    .into_replacement();
                    let candidate = SpaceAdmissionEnvelopeV1::new(
                        admission_id,
                        AdmissionRole::Sponsor,
                        0,
                        message_id(0x43),
                        Some(predecessor),
                        SpaceAdmissionBodyV1::Candidate(candidate_body(
                            admission_id,
                            self.continuation_route.clone(),
                        )),
                    )
                    .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?;
                    let candidate = accepted
                        .fix_candidate(
                            candidate,
                            AdmissionStagedSecurityState::from_bytes(vec![0x44; 64]).map_err(
                                uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                            )?,
                        )
                        .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?
                        .into_replacement();
                    let encoded = candidate.encode_persisted().map_err(
                        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                    )?;
                    *self.candidate_state.lock().await = Some(encoded.clone());
                    let reply = SponsorAdmission::decode_persisted(&encoded).map_err(
                        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                    )?;
                    let reply = uc_application::deps::SpaceAdmissionMessageReply::new(reply).ok_or_else(|| {
                        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(
                            anyhow::anyhow!("candidate reply was not saved"),
                        )
                    })?;
                    self.completed.store(1, Ordering::SeqCst);
                    Ok(reply)
                }
                SpaceAdmissionBodyV1::Prepared(_) => {
                    if continuation.is_some() {
                        return Err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!("resume created a new continuation")));
                    }
                    let encoded = self.candidate_state.lock().await.clone().ok_or_else(|| {
                        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!("candidate state missing"))
                    })?;
                    let candidate = SponsorAdmission::decode_persisted(&encoded).map_err(
                        uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid,
                    )?;
                    let fixed_bytes = candidate
                        .sponsor_commit_preparation()
                        .ok_or_else(|| uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!("fixed candidate missing")))?
                        .candidate_reply()
                        .encode_canonical_v1()
                        .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?;
                    let fixed = match SpaceAdmissionEnvelopeV1::decode_canonical_v1(&fixed_bytes)
                        .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?
                        .into_body()
                    {
                        SpaceAdmissionBodyV1::Candidate(body) => body,
                        _ => return Err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::recovery_required(anyhow::anyhow!("fixed candidate body missing"))),
                    };
                    let history = AdmissionSignedMembershipHistory::from_bytes(vec![0x45; 64])
                        .map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?;
                    let commit = SpaceAdmissionEnvelopeV1::new(
                        envelope.header().admission_id(),
                        AdmissionRole::Sponsor,
                        1,
                        message_id(0x46),
                        Some(envelope.header().message_id()),
                        SpaceAdmissionBodyV1::Commit(AdmissionCommitV1::new(
                            fixed,
                            AdmissionSignedMembershipHistory::from_bytes(history.as_bytes().to_vec()).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?,
                            AdmissionSealedRecoveryMaterial::from_bytes(vec![0x47; 64]).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?,
                        )),
                    ).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?;
                    let committed = candidate.commit_prepared(
                        envelope,
                        digest,
                        history,
                        AdmissionSealedSecurityState::from_bytes(vec![0x48; 64]).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?,
                        commit,
                    ).map_err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid)?.into_replacement();
                    uc_application::deps::SpaceAdmissionMessageReply::new(committed).ok_or_else(|| uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::invalid(anyhow::anyhow!("commit reply was not saved")))
                }
                _ => Err(uc_application::deps::HandleAuthenticatedSpaceAdmissionMessageError::out_of_order(anyhow::anyhow!("unexpected loopback message"))),
            }
        }
    }

    #[tokio::test]
    async fn new_client_maps_a_real_legacy_layout_server_to_peer_upgrade_required() {
        let sponsor = bound_endpoint().await;
        wait_for_direct_addrs(&sponsor).await;
        let joiner = bound_endpoint().await;
        wait_for_direct_addrs(&joiner).await;
        let invitation = InvitationId::from_bytes([0x61; 32]).expect("invitation id");
        let admission = SpaceAdmissionId::from_bytes([0x62; 32]).expect("admission id");
        let derived = SpaceAdmissionAuth::derive_password_equivalent(b"legacy-pass", invitation);
        let setup = SpaceAdmissionAuth::generate_server_setup();
        let registration = SpaceAdmissionAuth::register_password_equivalent(&setup, &derived)
            .expect("legacy registration");
        let credentials = Arc::new(LoopbackCredentials {
            initial: Mutex::new(Some(SponsorOpaqueMaterial::new(setup, registration))),
            continuation: Mutex::new(None),
        });
        let legacy = LegacyLayoutHandler {
            local_peer_id: peer_id(sponsor.id().as_bytes()).expect("legacy sponsor peer"),
            credentials,
        };
        let router = Router::builder((*sponsor).clone())
            .accept(SPACE_ADMISSION_ALPN, legacy)
            .spawn();
        let password =
            AdmissionEncryptedPasswordEquivalent::from_bytes(derived.as_bytes().to_vec())
                .expect("password equivalent");
        let route = SpaceAdmissionRoute::from_bytes(
            encode_space_admission_route(&sponsor.addr(), Some(invitation))
                .expect("route encoding"),
        )
        .expect("route");

        let mut exchange = IrohSpaceAdmissionTransport::new(joiner.clone())
            .establish_initial(admission, &route, &password)
            .await
            .expect("legacy peer authenticates before layout detection");
        let _ = exchange.take_newly_established_continuation();
        let result = exchange
            .exchange(&join_request(admission, invitation))
            .await;

        assert!(matches!(
            result,
            Err(SpaceAdmissionTransportError::PeerUpgradeRequired)
        ));
        router.shutdown().await.expect("router shutdown");
        joiner.close().await;
        sponsor.close().await;
    }

    #[tokio::test]
    async fn stalled_authenticated_endpoint_records_one_server_timeout() {
        let exporter = InMemorySpanExporter::default();
        let log_exporter = InMemoryLogExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let log_provider = SdkLoggerProvider::builder()
            .with_simple_exporter(log_exporter.clone())
            .build();
        let trace_layer = tracing_opentelemetry::layer()
            .with_tracer(provider.tracer("space-admission-timeout-test"))
            .with_context_activation(true)
            .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
        let log_layer = OpenTelemetryTracingBridge::new(&log_provider)
            .with_filter(filter_fn(|metadata| metadata.target() == "uc.telemetry"));
        let subscriber = tracing_subscriber::registry()
            .with(trace_layer)
            .with(log_layer);
        let _subscriber = tracing::subscriber::set_default(subscriber);

        let sponsor = bound_endpoint().await;
        wait_for_direct_addrs(&sponsor).await;
        let joiner = bound_endpoint().await;
        wait_for_direct_addrs(&joiner).await;
        let invitation = InvitationId::from_bytes([0x71; 32]).expect("invitation id");
        let admission = SpaceAdmissionId::from_bytes([0x72; 32]).expect("admission id");
        let derived = SpaceAdmissionAuth::derive_password_equivalent(b"timeout-pass", invitation);
        let setup = SpaceAdmissionAuth::generate_server_setup();
        let registration = SpaceAdmissionAuth::register_password_equivalent(&setup, &derived)
            .expect("registration");
        let credentials = Arc::new(LoopbackCredentials {
            initial: Mutex::new(Some(SponsorOpaqueMaterial::new(setup, registration))),
            continuation: Mutex::new(None),
        });
        let endpoint = Arc::new(HangingLoopbackEndpoint {
            calls: AtomicUsize::new(0),
            entered: Notify::new(),
        });
        let handler = Arc::new(
            IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials)
                .expect("handler")
                .with_exchange_deadline(Duration::from_secs(30)),
        );
        let router = Router::builder((*sponsor).clone())
            .accept(SPACE_ADMISSION_ALPN, handler)
            .spawn();
        let password =
            AdmissionEncryptedPasswordEquivalent::from_bytes(derived.as_bytes().to_vec())
                .expect("password equivalent");
        let route = SpaceAdmissionRoute::from_bytes(
            encode_space_admission_route(&sponsor.addr(), Some(invitation))
                .expect("route encoding"),
        )
        .expect("route");
        let exchange = IrohSpaceAdmissionTransport::new(joiner.clone())
            .establish_initial(admission, &route, &password)
            .await
            .expect("initial authentication");

        let entered = endpoint.entered.notified();
        tokio::pin!(entered);
        let request = join_request(admission, invitation);
        let exchange = exchange.exchange(&request);
        tokio::pin!(exchange);
        tokio::select! {
            () = &mut entered => {}
            _ = &mut exchange => panic!("endpoint must stall before completion"),
        }
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(31)).await;
        tokio::task::yield_now().await;
        let result = tokio::time::timeout(Duration::from_secs(1), &mut exchange)
            .await
            .expect("server deadline must close the exchange");
        assert!(result.is_err());
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 1);

        router.shutdown().await.expect("router shutdown");
        joiner.close().await;
        sponsor.close().await;
        provider.force_flush().expect("trace flush");
        log_provider.force_flush().expect("log flush");
        let server_spans = exporter
            .get_finished_spans()
            .expect("finished spans")
            .into_iter()
            .filter(|span| {
                span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "uc.role" && attribute.value.as_str() == "sponsor"
                }) && span.attributes.iter().any(|attribute| {
                    attribute.key.as_str() == "uc.operation"
                        && attribute.value.as_str() == "network_transport"
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(server_spans.len(), 1);
        assert_eq!(
            server_spans[0].span_kind,
            opentelemetry::trace::SpanKind::Server
        );
        assert!(server_spans[0].attributes.iter().any(|attribute| {
            attribute.key.as_str() == "uc.domain" && attribute.value.as_str() == "space_admission"
        }));
        assert!(matches!(
            server_spans[0].status,
            opentelemetry::trace::Status::Error { .. }
        ));
        let timeout_logs = log_exporter
            .get_emitted_logs()
            .expect("emitted logs")
            .into_iter()
            .filter(|log| {
                let matches_span = log.record.trace_context().is_some_and(|context| {
                    context.trace_id == server_spans[0].span_context.trace_id()
                        && context.span_id == server_spans[0].span_context.span_id()
                });
                let is_timeout = log.record.attributes_iter().any(|(key, value)| {
                    key.as_str() == "error.type"
                        && matches!(value, AnyValue::String(value) if value.as_str() == "timeout")
                });
                matches_span && is_timeout
            })
            .collect::<Vec<_>>();
        assert_eq!(timeout_logs.len(), 1);
        assert!(timeout_logs[0].record.body().is_none());
        assert!(!timeout_logs[0]
            .record
            .attributes_iter()
            .any(|(key, _)| key.as_str() == "uc.flow.id"));
    }

    #[tokio::test]
    async fn real_iroh_loopback_runs_initial_and_continuation_typed_exchanges() {
        let sponsor = bound_endpoint().await;
        wait_for_direct_addrs(&sponsor).await;
        let joiner = bound_endpoint().await;
        wait_for_direct_addrs(&joiner).await;
        let invitation = InvitationId::from_bytes([0x51; 32]).expect("invitation id");
        let admission = SpaceAdmissionId::from_bytes([0x52; 32]).expect("admission id");
        let derived = SpaceAdmissionAuth::derive_password_equivalent(b"loopback-pass", invitation);
        let setup = SpaceAdmissionAuth::generate_server_setup();
        let registration = SpaceAdmissionAuth::register_password_equivalent(&setup, &derived)
            .expect("registration");
        let credentials = Arc::new(LoopbackCredentials {
            initial: Mutex::new(Some(SponsorOpaqueMaterial::new(setup, registration))),
            continuation: Mutex::new(None),
        });
        let route_bytes = encode_space_admission_route(&sponsor.addr(), Some(invitation))
            .expect("route encoding");
        let route = SpaceAdmissionRoute::from_bytes(route_bytes.clone()).expect("route");
        let endpoint = Arc::new(PersistingLoopbackEndpoint {
            credentials: Arc::clone(&credentials),
            candidate_state: Mutex::new(None),
            calls: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            continuation_route: route_bytes,
        });
        let handler = Arc::new(
            IrohSpaceAdmissionHandler::new(&sponsor, endpoint.clone(), credentials)
                .expect("handler"),
        );
        let router = Router::builder((*sponsor).clone())
            .accept(SPACE_ADMISSION_ALPN, Arc::clone(&handler))
            .spawn();
        let transport = IrohSpaceAdmissionTransport::new(joiner.clone());
        let password =
            AdmissionEncryptedPasswordEquivalent::from_bytes(derived.as_bytes().to_vec())
                .expect("password equivalent");
        let mut initial = transport
            .establish_initial(admission, &route, &password)
            .await
            .expect("initial OPAQUE");
        let binding = initial.peer_binding();
        let continuation = initial
            .take_newly_established_continuation()
            .expect("new continuation");
        let join_request = join_request(admission, invitation);
        let candidate_result = initial.exchange(&join_request).await;
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 1);
        assert_eq!(endpoint.completed.load(Ordering::SeqCst), 1);
        let saved = endpoint
            .candidate_state
            .lock()
            .await
            .clone()
            .expect("candidate state persisted");
        let saved = SponsorAdmission::decode_persisted(&saved).expect("candidate state decodes");
        let saved_reply = saved.current_exact_reply().expect("candidate reply saved");
        let canonical = saved_reply
            .encode_canonical_v1()
            .expect("candidate encodes");
        SpaceAdmissionEnvelopeV1::decode_canonical_v1(&canonical)
            .expect("candidate canonical reply decodes");
        let candidate = candidate_result.expect("Candidate reply");
        let (candidate, _) = candidate.into_parts();
        assert_eq!(
            candidate.kind(),
            uc_core::membership::SpaceAdmissionMessageKind::Candidate
        );

        let prepared = prepared_request(admission, &candidate);
        let resumed = transport
            .resume(admission, &route, binding, &continuation)
            .await
            .expect("continuation authentication");
        let commit = resumed.exchange(&prepared).await.expect("Commit reply");
        let (commit, _) = commit.into_parts();
        assert_eq!(
            commit.kind(),
            uc_core::membership::SpaceAdmissionMessageKind::Commit
        );
        assert_eq!(endpoint.calls.load(Ordering::SeqCst), 2);

        router.shutdown().await.expect("router shutdown");
        joiner.close().await;
        sponsor.close().await;
    }

    async fn bound_endpoint() -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(presets::N0)
                .alpns(vec![SPACE_ADMISSION_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .clear_address_lookup()
                .bind()
                .await
                .expect("bind loopback endpoint"),
        )
    }

    async fn wait_for_direct_addrs(endpoint: &Endpoint) {
        for _ in 0..100 {
            if !endpoint.addr().addrs.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("endpoint did not publish a direct address");
    }

    fn message_id(byte: u8) -> AdmissionMessageId {
        AdmissionMessageId::from_bytes([byte; 32]).expect("non-zero message id")
    }

    fn join_request(
        admission_id: SpaceAdmissionId,
        invitation_id: InvitationId,
    ) -> SpaceAdmissionEnvelopeV1 {
        let device = DeviceId::new("loopback-joiner");
        let credential = MembershipCredential::new(1, vec![0x61; 32]);
        let signature = vec![0x62; 64];
        let facts = AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&device),
            device_id: device.clone(),
            device_name: "Loopback joiner".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .expect("fingerprint"),
            transport_public_key: vec![0x63; 32],
            transport_address_blob: vec![0x64; 32],
            identity_signature: signature.clone(),
        };
        let body = AdmissionJoinRequestV1::new(
            invitation_id,
            device,
            facts,
            credential,
            AdmissionKeyPackage::from_bytes(vec![0x65; 48]).expect("key package"),
            AdmissionRecoveryPublicKey::from_bytes([0x66; 32]).expect("recovery key"),
            AdmissionIdentitySignature::from_bytes(signature).expect("identity signature"),
            UnreadableHistoryPolicy::Discard,
        )
        .expect("JoinRequest");
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            0,
            message_id(0x67),
            None,
            SpaceAdmissionBodyV1::JoinRequest(body),
        )
        .expect("JoinRequest envelope")
    }

    fn candidate_body(
        admission_id: SpaceAdmissionId,
        continuation_route: Vec<u8>,
    ) -> AdmissionCandidateV1 {
        let sponsor_credential =
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x71; 32]);
        let joiner_credential =
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x72; 32]);
        let joiner_device = DeviceId::new("candidate-joiner");
        let admission = MembershipAdmissionV2 {
            facts: AdmissionChangeFacts {
                member_instance: joiner_credential.member_instance_id(&joiner_device),
                device_id: joiner_device,
                device_name: "candidate-joiner".to_owned(),
                identity_fingerprint: IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .expect("fingerprint"),
                transport_public_key: vec![0x73; 32],
                transport_address_blob: vec![0x74; 16],
                identity_signature: vec![0x75; 64],
            },
            membership_credential: joiner_credential,
            resume_public_key_digest: [0x76; 32],
            security_commitment_id: [0x77; 32],
        };
        let event = MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            "lineage".to_owned(),
            None,
            0,
            [0x78; 16],
            MemberInstanceId::from_bytes([0x79; 32]),
            sponsor_credential.credential_id,
            ED25519_SIGNATURE_ALGORITHM_V1,
            MembershipOperationV2::AddDevice { admission },
            [0x7a; 32],
            [0x7b; 32],
            vec![0x7c],
            Some([0x7d; 32]),
            vec![0x7e; 64],
        );
        let base = BaseMembershipHistoryPosition {
            event_id: None,
            depth: 0,
            history_digest: [0x7f; 32],
        };
        let commitment = AdmissionSecurityCommitmentV1::new(
            ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
            "lineage".to_owned(),
            vec![0x80; 16],
            *admission_id.as_bytes(),
            base,
            [0x81; 32],
            1,
            0,
            1,
            [0x82; 32],
            [0x83; 32],
            [0x84; 32],
            [0x85; 32],
            [0x86; 32],
        )
        .expect("security commitment");
        AdmissionCandidateV1::new(
            AdmissionSignedMembershipHistory::from_bytes(vec![0x87; 64]).expect("history"),
            event,
            commitment,
            AdmissionMlsCommit::from_bytes(vec![0x88; 64]).expect("MLS commit"),
            AdmissionMlsWelcome::from_bytes(vec![0x89; 64]).expect("MLS welcome"),
            AdmissionContinuationRoute::from_bytes(continuation_route).expect("continuation route"),
        )
        .expect("Candidate")
    }

    fn prepared_request(
        admission_id: SpaceAdmissionId,
        candidate: &SpaceAdmissionEnvelopeV1,
    ) -> SpaceAdmissionEnvelopeV1 {
        let SpaceAdmissionBodyV1::Candidate(body) = candidate.body() else {
            panic!("candidate fixture kind");
        };
        let operation = &body.candidate_event().operation;
        let MembershipOperationV2::AddDevice { admission } = operation else {
            panic!("candidate fixture operation");
        };
        let proof = PreparedAdmissionProofV1::new(
            *admission_id.as_bytes(),
            body.security_commitment().lineage_id.clone(),
            body.security_commitment().base_history_position.clone(),
            body.candidate_event().event_id(),
            body.candidate_event().resulting_members_digest,
            body.security_commitment().security_commitment_id,
            admission.facts.member_instance,
            admission.membership_credential.credential_id,
            vec![0x91; 64],
        );
        SpaceAdmissionEnvelopeV1::new(
            admission_id,
            AdmissionRole::Joiner,
            1,
            message_id(0x92),
            Some(candidate.header().message_id()),
            SpaceAdmissionBodyV1::Prepared(AdmissionPreparedV1::new(proof)),
        )
        .expect("Prepared envelope")
    }
}
