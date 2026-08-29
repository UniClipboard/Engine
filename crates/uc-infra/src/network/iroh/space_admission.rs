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
            Ok(Err(HandlerError::Authentication)) => {
                connection.close(CLOSE_AUTHENTICATION.into(), b"authentication_rejected");
            }
            _ => connection.close(CLOSE_PROTOCOL.into(), b"protocol_rejected"),
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
}
