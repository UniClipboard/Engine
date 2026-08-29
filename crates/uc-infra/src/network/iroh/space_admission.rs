use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use tokio::sync::Semaphore;
use tracing::debug;
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

use crate::security::{
    SpaceAdmissionAuth, SpaceAdmissionAuthContext, SpaceAdmissionContinuationCredential,
    SpaceAdmissionKe1, SpaceAdmissionKe2, SpaceAdmissionKe3, SpaceAdmissionRegistration,
    SpaceAdmissionServerSetup,
};

use super::space_admission_wire::{
    read_envelope, read_raw_with_limit, read_typed, write_envelope, write_typed,
    AuthenticatedEnvelopeV1, ContinuationHelloV1, FrameKind, InitialHelloV1, OpaqueFinishV1,
    OpaqueResponseV1, AUTH_FRAME_LIMIT, IO_DEADLINE,
};

pub const SPACE_ADMISSION_ALPN: &[u8] = b"/uniclipboard/space-admission/1";
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(120);
const MAX_INBOUND_EXCHANGES: usize = 8;
const CLOSE_PROTOCOL: u32 = 0x51;
const CLOSE_AUTHENTICATION: u32 = 0x52;
const CLOSE_BUSY: u32 = 0x53;
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
        })
    }

    async fn run(&self, connection: &Connection) -> Result<(), HandlerError> {
        let remote_peer_id =
            peer_id(connection.remote_id().as_bytes()).map_err(|_| HandlerError::Authentication)?;
        let (mut send, mut receive) = tokio::time::timeout(IO_DEADLINE, connection.accept_bi())
            .await
            .map_err(|_| HandlerError::Protocol)?
            .map_err(|_| HandlerError::Protocol)?;
        let (kind, payload) = read_raw_with_limit(&mut receive, AUTH_FRAME_LIMIT)
            .await
            .map_err(|_| HandlerError::Protocol)?;
        let (admission_id, credential, is_initial) = match kind {
            FrameKind::InitialHello => {
                let hello: InitialHelloV1 =
                    postcard::from_bytes(&payload).map_err(|_| HandlerError::Protocol)?;
                let admission_id = SpaceAdmissionId::from_bytes(hello.admission_id)
                    .ok_or(HandlerError::Protocol)?;
                let invitation_id =
                    InvitationId::from_bytes(hello.invitation_id).ok_or(HandlerError::Protocol)?;
                if hello.protocol_version != SpaceAdmissionProtocolVersion::V1.as_u16()
                    || hello.joiner_peer_id != *remote_peer_id.as_bytes()
                {
                    return Err(HandlerError::Authentication);
                }
                let material = self
                    .credentials
                    .resolve_initial(invitation_id, admission_id)
                    .await
                    .map_err(|_| HandlerError::Authentication)?;
                let context = SpaceAdmissionAuthContext::new(
                    SpaceAdmissionProtocolVersion::V1,
                    admission_id,
                    invitation_id,
                    remote_peer_id,
                    self.local_peer_id,
                );
                let ke1 = SpaceAdmissionKe1::decode_from_transport(&hello.ke1)
                    .map_err(|_| HandlerError::Authentication)?;
                let (server, ke2) = SpaceAdmissionAuth::start_server(
                    &material.server_setup,
                    &material.registration,
                    &context,
                    ke1,
                )
                .map_err(|_| HandlerError::Authentication)?;
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
                .map_err(|_| HandlerError::Protocol)?;
                let finish: OpaqueFinishV1 =
                    read_typed(&mut receive, FrameKind::OpaqueFinish, AUTH_FRAME_LIMIT)
                        .await
                        .map_err(|_| HandlerError::Protocol)?;
                let ke3 = SpaceAdmissionKe3::decode_from_transport(&finish.ke3)
                    .map_err(|_| HandlerError::Authentication)?;
                let credential = server
                    .finish(&context, ke3)
                    .and_then(SpaceAdmissionContinuationCredential::into_core)
                    .map_err(|_| HandlerError::Authentication)?;
                (admission_id, credential, true)
            }
            FrameKind::ContinuationHello => {
                let hello: ContinuationHelloV1 =
                    postcard::from_bytes(&payload).map_err(|_| HandlerError::Protocol)?;
                let admission_id = SpaceAdmissionId::from_bytes(hello.admission_id)
                    .ok_or(HandlerError::Protocol)?;
                if hello.local_peer_id != *remote_peer_id.as_bytes()
                    || hello.remote_peer_id != *self.local_peer_id.as_bytes()
                {
                    return Err(HandlerError::Authentication);
                }
                let credential = self
                    .credentials
                    .load_continuation(admission_id)
                    .await
                    .map_err(|_| HandlerError::Authentication)?;
                verify_mac(
                    &credential,
                    b"resume",
                    admission_id,
                    remote_peer_id,
                    self.local_peer_id,
                    &hello.nonce,
                    &hello.request_digest,
                    &hello.mac,
                )?;
                (admission_id, credential, false)
            }
            _ => return Err(HandlerError::Protocol),
        };

        let (wire, envelope, canonical_digest) = read_envelope(&mut receive, FrameKind::Request)
            .await
            .map_err(|_| HandlerError::Protocol)?;
        if envelope.header().admission_id() != admission_id {
            return Err(HandlerError::Authentication);
        }
        verify_mac(
            &credential,
            b"request",
            admission_id,
            remote_peer_id,
            self.local_peer_id,
            &wire.nonce,
            &canonical_digest,
            &wire.mac,
        )?;
        let endpoint_credential = if is_initial {
            Some(copy_credential(&credential).map_err(|_| HandlerError::Authentication)?)
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
        )?;
        write_envelope(
            &mut send,
            FrameKind::Reply,
            &AuthenticatedEnvelopeV1 {
                nonce,
                canonical_envelope: canonical,
                mac,
            },
        )
        .await
        .map_err(|_| HandlerError::Protocol)?;
        send.finish().map_err(|_| HandlerError::Protocol)?;
        let ack: u8 = read_typed(&mut receive, FrameKind::Ack, AUTH_FRAME_LIMIT)
            .await
            .map_err(|_| HandlerError::Protocol)?;
        if ack != 1 {
            return Err(HandlerError::Protocol);
        }
        Ok(())
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
        match tokio::time::timeout(EXCHANGE_DEADLINE, self.run(&connection)).await {
            Ok(Ok(())) => {}
            Ok(Err(error @ HandlerError::Authentication)) => {
                debug!(?error, "Space admission exchange rejected");
                connection.close(CLOSE_AUTHENTICATION.into(), b"authentication_rejected");
            }
            Ok(Err(error)) => {
                debug!(?error, "Space admission exchange rejected");
                connection.close(CLOSE_PROTOCOL.into(), b"protocol_rejected");
            }
            Err(_) => connection.close(CLOSE_PROTOCOL.into(), b"protocol_timeout"),
        }
        Ok(())
    }

    async fn shutdown(&self) {
        self.accepting.store(false, Ordering::Release);
    }
}

struct EstablishedExchange {
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
        let nonce = random_nonce();
        let mac = calculate_mac(
            &self.credential,
            b"request",
            self.admission_id,
            self.binding.local_peer_id(),
            self.binding.remote_peer_id(),
            &nonce,
            &digest,
        )
        .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
        write_envelope(
            &mut self.send,
            FrameKind::Request,
            &AuthenticatedEnvelopeV1 {
                nonce,
                canonical_envelope: canonical,
                mac,
            },
        )
        .await
        .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
        let (wire, reply, reply_digest) = read_envelope(&mut self.receive, FrameKind::Reply)
            .await
            .map_err(|_| SpaceAdmissionTransportError::ProtocolRejected)?;
        verify_mac(
            &self.credential,
            b"reply",
            self.admission_id,
            self.binding.remote_peer_id(),
            self.binding.local_peer_id(),
            &wire.nonce,
            &reply_digest,
            &wire.mac,
        )
        .map_err(|_| SpaceAdmissionTransportError::AuthenticationRejected)?;
        write_typed(&mut self.send, FrameKind::Ack, &1u8, AUTH_FRAME_LIMIT)
            .await
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
        self.send
            .finish()
            .map_err(|_| SpaceAdmissionTransportError::Unavailable)?;
        AuthenticatedAdmissionReply::new(reply, reply_digest)
            .ok_or(SpaceAdmissionTransportError::ProtocolRejected)
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
        let connection = connect(&self.endpoint, route.endpoint_addr).await?;
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
            send,
            receive,
            admission_id,
            binding,
            credential,
            newly_established: Some(newly_established),
        }))
    }

    async fn resume(
        &self,
        admission_id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        binding: AdmissionPeerBinding,
        credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        let route = decode_route(route, false)?;
        let local = peer_id(self.endpoint.id().as_bytes())?;
        let remote = peer_id(route.endpoint_addr.id.as_bytes())?;
        if binding.local_peer_id() != local || binding.remote_peer_id() != remote {
            return Err(SpaceAdmissionTransportError::AuthenticationRejected);
        }
        let connection = connect(&self.endpoint, route.endpoint_addr).await?;
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
            send,
            receive,
            admission_id,
            binding,
            credential: copy_credential(credential)?,
            newly_established: None,
        }))
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

#[cfg(test)]
pub(crate) fn decode_space_admission_route_for_test(
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

async fn connect(
    endpoint: &Endpoint,
    addr: EndpointAddr,
) -> Result<Connection, SpaceAdmissionTransportError> {
    tokio::time::timeout(IO_DEADLINE, endpoint.connect(addr, SPACE_ADMISSION_ALPN))
        .await
        .map_err(|_| SpaceAdmissionTransportError::Deferred)?
        .map_err(|_| SpaceAdmissionTransportError::Deferred)
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
) {
    mac.update(b"uc-space-admission-channel-v1");
    mac.update(direction);
    mac.update(admission_id.as_bytes());
    mac.update(sender.as_bytes());
    mac.update(receiver.as_bytes());
    mac.update(nonce);
    mac.update(digest);
}

#[derive(Debug)]
enum HandlerError {
    Protocol,
    Authentication,
    Application,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use iroh::endpoint::presets;
    use iroh::protocol::Router;
    use iroh::RelayMode;
    use tokio::sync::Mutex;
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
            &mac,
        )
        .is_err());
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
