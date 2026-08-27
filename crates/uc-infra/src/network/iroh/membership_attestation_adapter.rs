use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr, Watcher};
use serde::{Deserialize, Serialize};
use tracing::{debug, instrument, warn};
use uc_application::deps::CurrentMemberSignaturePort;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMembershipAnnouncementMaterial, CurrentMembershipAnnouncementPort,
    CurrentMembershipIdentity, CurrentMembershipIdentityError, CurrentMembershipIdentityPort,
    MemberRepositoryPort, MembershipAttestationEndpointPort, MembershipAttestationError,
    MembershipAttestationPort, MembershipGossipEndpointError, MembershipGossipEndpointPort,
    MembershipGossipMessage, MembershipGossipTransportError, MembershipGossipTransportPort,
    PeerAdmissionPort, RelayedSecurityUpdate, SpaceMembershipCandidate, VerifiedMembershipPeer,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{DeviceIdentityPort, PeerAddressRepositoryPort, SettingsPort};
use uc_core::security::IdentityFingerprint;

use crate::pairing::session::legacy_pairing_protocol_is_reachable;
use crate::security::InMemorySession;

use super::connect_with_staggered_retry;
use super::persistable_addr::to_persistable_addr;

const WIRE_VERSION: u8 = 1;
pub const MEMBERSHIP_ATTESTATION_ALPN: &[u8] = b"uniclipboard/membership-gossip/1";
const MAX_MESSAGE_SIZE: usize = 256 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const ATTESTATION_DOMAIN: &[u8] = b"uniclipboard/membership-gossip/1\0";

pub(crate) struct IrohMembershipIdentityAdapter {
    endpoint: Arc<Endpoint>,
    session: Arc<InMemorySession>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    settings: Arc<dyn SettingsPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
}

impl IrohMembershipIdentityAdapter {
    pub(crate) fn new(
        endpoint: Arc<Endpoint>,
        session: Arc<InMemorySession>,
        device_identity: Arc<dyn DeviceIdentityPort>,
        settings: Arc<dyn SettingsPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            endpoint,
            session,
            device_identity,
            settings,
            fingerprint_factory,
        }
    }
}

#[async_trait]
impl CurrentMembershipIdentityPort for IrohMembershipIdentityAdapter {
    async fn current_membership_identity(
        &self,
    ) -> Result<CurrentMembershipIdentity, uc_core::membership::CurrentMembershipIdentityError>
    {
        use uc_core::membership::CurrentMembershipIdentityError;

        let space_id = self
            .session
            .current_space_id()
            .map_err(|_| CurrentMembershipIdentityError::Unavailable)?;
        let settings = self
            .settings
            .load()
            .await
            .map_err(|_| CurrentMembershipIdentityError::LoadFailed)?;
        let device_name = settings
            .general
            .device_name
            .filter(|name| !name.trim().is_empty())
            .ok_or(CurrentMembershipIdentityError::Unavailable)?;
        let identity_fingerprint = self
            .fingerprint_factory
            .from_public_key(self.endpoint.id().as_bytes())
            .map_err(|_| CurrentMembershipIdentityError::LoadFailed)?;

        Ok(CurrentMembershipIdentity {
            space_id,
            device_id: self.device_identity.current_device_id(),
            device_name,
            identity_fingerprint,
        })
    }
}

#[derive(Serialize, Deserialize, Clone)]
struct WireEnvelope {
    version: u8,
    message: WireMessage,
}

#[derive(Serialize, Deserialize, Clone)]
enum WireMessage {
    Hello(WireHello),
    Challenge(WireChallenge),
    Proof(Vec<u8>),
    Ack,
    GossipRequest(WireGossipRequest),
    GossipResponse(MembershipGossipMessage),
    Reject(WireReject),
}

#[derive(Serialize, Deserialize, Clone)]
struct WireGossipRequest {
    source_device_id: String,
    message: MembershipGossipMessage,
}

#[derive(Serialize, Deserialize, Clone)]
struct WireHello {
    space_id: String,
    group_epoch: u64,
    source_device_id: String,
    target_device_id: String,
    source_device_name: String,
    source_identity_fingerprint: String,
    source_transport_key: [u8; 32],
    source_address: Vec<u8>,
    source_nonce: [u8; 32],
    security_updates: Vec<RelayedSecurityUpdate>,
}

#[derive(Serialize, Deserialize, Clone)]
struct WireChallenge {
    responder_device_id: String,
    responder_device_name: String,
    responder_identity_fingerprint: String,
    responder_transport_key: [u8; 32],
    responder_address: Vec<u8>,
    responder_nonce: [u8; 32],
    signature: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
enum WireReject {
    Invalid,
    WrongSpace,
    EpochMismatch,
    Version,
    Persistence,
}

pub struct IrohMembershipAttestationAdapter {
    endpoint: Arc<Endpoint>,
    identity: Arc<dyn CurrentMembershipIdentityPort>,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
}

pub struct IrohMembershipGossipTransportAdapter {
    endpoint: Arc<Endpoint>,
    identity: Arc<dyn CurrentMembershipIdentityPort>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    handler_state: Arc<MembershipGossipHandlerState>,
}

struct MembershipGossipHandlerState {
    session: Arc<InMemorySession>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    peer_admission: Arc<dyn PeerAdmissionPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
}

impl IrohMembershipGossipTransportAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        session: Arc<InMemorySession>,
        identity: Arc<dyn CurrentMembershipIdentityPort>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            endpoint,
            identity,
            peer_addr_repo,
            handler_state: Arc::new(MembershipGossipHandlerState {
                session,
                member_repo,
                peer_admission,
                fingerprint_factory,
            }),
        }
    }

    async fn resolve_addr(
        &self,
        recipient: &DeviceId,
    ) -> Result<EndpointAddr, MembershipGossipTransportError> {
        let record = self
            .peer_addr_repo
            .get(recipient)
            .await
            .map_err(|_| MembershipGossipTransportError::Transport)?
            .ok_or(MembershipGossipTransportError::Offline)?;
        postcard::from_bytes(&record.addr_blob)
            .map_err(|_| MembershipGossipTransportError::Transport)
    }
}

#[async_trait]
impl CurrentMembershipAnnouncementPort for IrohMembershipGossipTransportAdapter {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        let identity = self.identity.current_membership_identity().await?;
        let transport_address_blob =
            postcard::to_stdvec(&to_persistable_addr(self.endpoint.addr()))
                .map_err(|_| CurrentMembershipIdentityError::LoadFailed)?;
        Ok(CurrentMembershipAnnouncementMaterial {
            space_id: identity.space_id,
            device_id: identity.device_id,
            device_name: identity.device_name,
            identity_fingerprint: identity.identity_fingerprint,
            transport_public_key: self.endpoint.id().as_bytes().to_vec(),
            transport_address_blob,
        })
    }

    async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
        self.endpoint
            .watch_addr()
            .updated()
            .await
            .map(|_| ())
            .map_err(|_| CurrentMembershipIdentityError::LoadFailed)
    }
}

#[async_trait]
impl MembershipGossipTransportPort for IrohMembershipGossipTransportAdapter {
    #[instrument(name = "membership_gossip.exchange", level = "debug", skip_all)]
    async fn exchange(
        &self,
        recipient: &DeviceId,
        message: MembershipGossipMessage,
    ) -> Result<MembershipGossipMessage, MembershipGossipTransportError> {
        message
            .validate_transfer_bounds()
            .map_err(|_| MembershipGossipTransportError::Rejected)?;
        let identity = self
            .identity
            .current_membership_identity()
            .await
            .map_err(|_| MembershipGossipTransportError::Rejected)?;
        if &identity.device_id == recipient {
            return Err(MembershipGossipTransportError::Rejected);
        }
        let remote_addr = self.resolve_addr(recipient).await?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            remote_addr.clone(),
            MEMBERSHIP_ATTESTATION_ALPN,
            "membership-gossip",
        )
        .await;
        let connection = match connection {
            Ok(connection) => connection,
            Err(_) => {
                if legacy_pairing_protocol_is_reachable(Arc::clone(&self.endpoint), remote_addr)
                    .await
                {
                    return Err(MembershipGossipTransportError::VersionIncompatible);
                }
                return Err(MembershipGossipTransportError::Offline);
            }
        };
        let (mut send, mut recv) = tokio::time::timeout(IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|_| MembershipGossipTransportError::Transport)?
            .map_err(|_| MembershipGossipTransportError::Transport)?;
        write_message(
            &mut send,
            &WireMessage::GossipRequest(WireGossipRequest {
                source_device_id: identity.device_id.as_str().to_owned(),
                message,
            }),
        )
        .await
        .map_err(map_attestation_transport_error)?;
        send.finish()
            .map_err(|_| MembershipGossipTransportError::Transport)?;
        match read_message(&mut recv)
            .await
            .map_err(map_attestation_transport_error)?
        {
            WireMessage::GossipResponse(response) => {
                response
                    .validate_transfer_bounds()
                    .map_err(|_| MembershipGossipTransportError::Rejected)?;
                Ok(response)
            }
            WireMessage::Reject(WireReject::Version) => {
                Err(MembershipGossipTransportError::VersionIncompatible)
            }
            WireMessage::Reject(_) => Err(MembershipGossipTransportError::Rejected),
            _ => Err(MembershipGossipTransportError::Transport),
        }
    }
}

fn map_attestation_transport_error(
    error: MembershipAttestationError,
) -> MembershipGossipTransportError {
    match error {
        MembershipAttestationError::VersionIncompatible => {
            MembershipGossipTransportError::VersionIncompatible
        }
        MembershipAttestationError::Rejected
        | MembershipAttestationError::MissingSecurityUpdate => {
            MembershipGossipTransportError::Rejected
        }
        MembershipAttestationError::Offline | MembershipAttestationError::Transport => {
            MembershipGossipTransportError::Transport
        }
    }
}

impl IrohMembershipAttestationAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        identity: Arc<dyn CurrentMembershipIdentityPort>,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            endpoint,
            identity,
            signatures,
            fingerprint_factory,
        }
    }

    pub fn handler(
        &self,
        application_endpoint: Arc<dyn MembershipAttestationEndpointPort>,
    ) -> IrohMembershipAttestationHandler {
        IrohMembershipAttestationHandler {
            endpoint: Arc::clone(&self.endpoint),
            identity: Arc::clone(&self.identity),
            signatures: Arc::clone(&self.signatures),
            fingerprint_factory: Arc::clone(&self.fingerprint_factory),
            application_endpoint,
            gossip_state: None,
            gossip_endpoint: None,
        }
    }

    pub fn handler_with_gossip(
        &self,
        application_endpoint: Arc<dyn MembershipAttestationEndpointPort>,
        transport: &IrohMembershipGossipTransportAdapter,
        gossip_endpoint: Arc<dyn MembershipGossipEndpointPort>,
    ) -> IrohMembershipAttestationHandler {
        IrohMembershipAttestationHandler {
            endpoint: Arc::clone(&self.endpoint),
            identity: Arc::clone(&self.identity),
            signatures: Arc::clone(&self.signatures),
            fingerprint_factory: Arc::clone(&self.fingerprint_factory),
            application_endpoint,
            gossip_state: Some(Arc::clone(&transport.handler_state)),
            gossip_endpoint: Some(gossip_endpoint),
        }
    }

    async fn local_address(&self) -> Result<Vec<u8>, MembershipAttestationError> {
        postcard::to_stdvec(&to_persistable_addr(self.endpoint.addr()))
            .map_err(|_| MembershipAttestationError::Transport)
    }
}

#[async_trait]
impl MembershipAttestationPort for IrohMembershipAttestationAdapter {
    #[instrument(name = "membership_attestation.exchange", level = "debug", skip_all)]
    async fn attest_candidate(
        &self,
        candidate: &SpaceMembershipCandidate,
    ) -> Result<VerifiedMembershipPeer, MembershipAttestationError> {
        let remote_addr: EndpointAddr = postcard::from_bytes(candidate.transport_address_blob())
            .map_err(|_| MembershipAttestationError::Transport)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            remote_addr.clone(),
            MEMBERSHIP_ATTESTATION_ALPN,
            "membership-attestation",
        )
        .await;
        let connection = match connection {
            Ok(connection) => connection,
            Err(_) => {
                if legacy_pairing_protocol_is_reachable(Arc::clone(&self.endpoint), remote_addr)
                    .await
                {
                    return Err(MembershipAttestationError::VersionIncompatible);
                }
                return Err(MembershipAttestationError::Offline);
            }
        };
        let remote_key = *connection.remote_id().as_bytes();
        let local = self
            .identity
            .current_membership_identity()
            .await
            .map_err(|_| MembershipAttestationError::Rejected)?;
        if &local.space_id != candidate.space_id() || local.device_id == *candidate.device_id() {
            return Err(MembershipAttestationError::Rejected);
        }
        let group_epoch = self
            .signatures
            .current_member_epoch()
            .await
            .map_err(|_| MembershipAttestationError::Rejected)?;
        let initiator_nonce = rand::random::<[u8; 32]>();
        let hello = WireHello {
            space_id: local.space_id.as_ref().to_owned(),
            group_epoch,
            source_device_id: local.device_id.as_str().to_owned(),
            target_device_id: candidate.device_id().as_str().to_owned(),
            source_device_name: local.device_name.clone(),
            source_identity_fingerprint: local.identity_fingerprint.as_display().to_owned(),
            source_transport_key: *self.endpoint.id().as_bytes(),
            source_address: self.local_address().await?,
            source_nonce: initiator_nonce,
            security_updates: candidate.security_updates().to_vec(),
        };
        let (mut send, mut recv) = run_io(connection.open_bi()).await?;
        write_message(&mut send, &WireMessage::Hello(hello.clone())).await?;
        let challenge = match read_message(&mut recv).await? {
            WireMessage::Challenge(challenge) => challenge,
            WireMessage::Reject(reason) => return Err(map_rejection(reason)),
            _ => return Err(MembershipAttestationError::Transport),
        };
        let responder_id = DeviceId::try_new(&challenge.responder_device_id)
            .ok_or(MembershipAttestationError::Rejected)?;
        if responder_id != *candidate.device_id() || challenge.responder_transport_key != remote_key
        {
            return Err(MembershipAttestationError::Rejected);
        }
        let responder_fingerprint =
            IdentityFingerprint::from_display_string(&challenge.responder_identity_fingerprint)
                .map_err(|_| MembershipAttestationError::Rejected)?;
        let connected_fingerprint = self
            .fingerprint_factory
            .from_public_key(&remote_key)
            .map_err(|_| MembershipAttestationError::Rejected)?;
        if responder_fingerprint != connected_fingerprint
            || &responder_fingerprint != candidate.identity_fingerprint_hint()
        {
            return Err(MembershipAttestationError::Rejected);
        }
        let transcript = build_transcript(
            &local,
            candidate.device_id(),
            group_epoch,
            *self.endpoint.id().as_bytes(),
            remote_key,
            initiator_nonce,
            challenge.responder_nonce,
            &identity_digest(&local, &hello.source_address, self.endpoint.id().as_bytes()),
            &wire_identity_digest(
                &challenge.responder_device_id,
                &challenge.responder_device_name,
                &challenge.responder_identity_fingerprint,
                &challenge.responder_transport_key,
                &challenge.responder_address,
            ),
            &hello.security_updates,
        );
        let transcript_bytes = attestation_transcript(&transcript);
        if !self
            .signatures
            .verify_current_member_payload(&responder_id, &transcript_bytes, &challenge.signature)
            .await
            .map_err(|_| MembershipAttestationError::Rejected)?
        {
            return Err(MembershipAttestationError::Rejected);
        }
        let proof = self
            .signatures
            .sign_current_member_payload(&transcript_bytes)
            .await
            .map_err(|_| MembershipAttestationError::Rejected)?;
        write_message(&mut send, &WireMessage::Proof(proof)).await?;
        send.finish()
            .map_err(|_| MembershipAttestationError::Transport)?;
        match read_message(&mut recv).await? {
            WireMessage::Ack => Ok(VerifiedMembershipPeer {
                space_id: local.space_id,
                device_id: responder_id,
                device_name: challenge.responder_device_name,
                identity_fingerprint: responder_fingerprint,
                transport_public_key: remote_key.to_vec(),
                transport_address_blob: challenge.responder_address,
            }),
            WireMessage::Reject(reason) => Err(map_rejection(reason)),
            _ => Err(MembershipAttestationError::Transport),
        }
    }
}

#[derive(Clone)]
pub struct IrohMembershipAttestationHandler {
    endpoint: Arc<Endpoint>,
    identity: Arc<dyn CurrentMembershipIdentityPort>,
    signatures: Arc<dyn CurrentMemberSignaturePort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    application_endpoint: Arc<dyn MembershipAttestationEndpointPort>,
    gossip_state: Option<Arc<MembershipGossipHandlerState>>,
    gossip_endpoint: Option<Arc<dyn MembershipGossipEndpointPort>>,
}

impl std::fmt::Debug for IrohMembershipAttestationHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohMembershipAttestationHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohMembershipAttestationHandler {
    #[instrument(name = "membership_attestation.accept", level = "debug", skip_all)]
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let close_barrier = connection.clone();
        if let Err(reason) = self.handle(connection).await {
            debug!(reason, "membership attestation connection rejected");
        }
        let _ = tokio::time::timeout(IO_TIMEOUT, close_barrier.closed()).await;
        Ok(())
    }
}

impl IrohMembershipAttestationHandler {
    async fn handle(&self, connection: Connection) -> Result<(), &'static str> {
        let remote_key = *connection.remote_id().as_bytes();
        let (mut send, mut recv) = tokio::time::timeout(IO_TIMEOUT, connection.accept_bi())
            .await
            .map_err(|_| "accept_timeout")?
            .map_err(|_| "accept_failed")?;
        let hello = match read_message(&mut recv).await {
            Ok(WireMessage::Hello(hello)) => hello,
            Ok(WireMessage::GossipRequest(request)) => {
                return self
                    .handle_gossip(connection, &mut send, remote_key, request)
                    .await;
            }
            Err(MembershipAttestationError::VersionIncompatible) => {
                reject(&mut send, WireReject::Version).await;
                return Err("version_incompatible");
            }
            Ok(_) | Err(_) => {
                reject(&mut send, WireReject::Invalid).await;
                return Err("invalid_hello");
            }
        };
        let local = self
            .identity
            .current_membership_identity()
            .await
            .map_err(|_| "identity_unavailable")?;
        let source_id = match DeviceId::try_new(&hello.source_device_id) {
            Some(device_id) => device_id,
            None => {
                reject(&mut send, WireReject::Invalid).await;
                return Err("invalid_source");
            }
        };
        if hello.target_device_id != local.device_id.as_str()
            || hello.source_transport_key != remote_key
            || source_id == local.device_id
        {
            reject(&mut send, WireReject::Invalid).await;
            return Err("identity_binding");
        }
        if hello.space_id != local.space_id.as_ref() {
            reject(&mut send, WireReject::WrongSpace).await;
            return Err("wrong_space");
        }
        let applied_epoch = match self
            .application_endpoint
            .apply_relayed_security_updates(&local.space_id, &hello.security_updates)
            .await
        {
            Ok(epoch) => epoch,
            Err(uc_core::membership::MembershipAttestationEndpointError::MissingSecurityUpdate) => {
                reject(&mut send, WireReject::EpochMismatch).await;
                return Err("missing_security_update");
            }
            Err(uc_core::membership::MembershipAttestationEndpointError::Rejected) => {
                reject(&mut send, WireReject::Invalid).await;
                return Err("invalid_security_update");
            }
            Err(uc_core::membership::MembershipAttestationEndpointError::Persistence) => {
                reject(&mut send, WireReject::Persistence).await;
                return Err("security_update_persistence");
            }
        };
        let epoch = self
            .signatures
            .current_member_epoch()
            .await
            .map_err(|_| "epoch_unavailable")?;
        if epoch != hello.group_epoch || applied_epoch != hello.group_epoch {
            warn!(
                peer = %source_id.as_str(),
                local_epoch = epoch,
                peer_epoch = hello.group_epoch,
                applied_epoch,
                error_kind = "membership_attestation_epoch_mismatch",
                "membership attestation rejected because the group epochs do not match"
            );
            reject(&mut send, WireReject::EpochMismatch).await;
            return Err("epoch_mismatch");
        }
        let source_fingerprint =
            match IdentityFingerprint::from_display_string(&hello.source_identity_fingerprint) {
                Ok(fingerprint) => fingerprint,
                Err(_) => {
                    reject(&mut send, WireReject::Invalid).await;
                    return Err("invalid_fingerprint");
                }
            };
        let connected_fingerprint = self
            .fingerprint_factory
            .from_public_key(&remote_key)
            .map_err(|_| "fingerprint_failed")?;
        if source_fingerprint != connected_fingerprint {
            reject(&mut send, WireReject::Invalid).await;
            return Err("fingerprint_binding");
        }
        let responder_address = postcard::to_stdvec(&to_persistable_addr(self.endpoint.addr()))
            .map_err(|_| "address_encode")?;
        let responder_nonce = rand::random::<[u8; 32]>();
        let source_digest = wire_identity_digest(
            &hello.source_device_id,
            &hello.source_device_name,
            &hello.source_identity_fingerprint,
            &hello.source_transport_key,
            &hello.source_address,
        );
        let responder_digest =
            identity_digest(&local, &responder_address, self.endpoint.id().as_bytes());
        let transcript = build_transcript(
            &CurrentMembershipIdentity {
                space_id: local.space_id.clone(),
                device_id: source_id,
                device_name: hello.source_device_name.clone(),
                identity_fingerprint: source_fingerprint.clone(),
            },
            &local.device_id,
            epoch,
            remote_key,
            *self.endpoint.id().as_bytes(),
            hello.source_nonce,
            responder_nonce,
            &source_digest,
            &responder_digest,
            &hello.security_updates,
        );
        let transcript_bytes = attestation_transcript(&transcript);
        let responder_signature = self
            .signatures
            .sign_current_member_payload(&transcript_bytes)
            .await
            .map_err(|_| "sign_failed")?;
        let challenge = WireChallenge {
            responder_device_id: local.device_id.as_str().to_owned(),
            responder_device_name: local.device_name,
            responder_identity_fingerprint: local.identity_fingerprint.as_display().to_owned(),
            responder_transport_key: *self.endpoint.id().as_bytes(),
            responder_address,
            responder_nonce,
            signature: responder_signature,
        };
        write_message(&mut send, &WireMessage::Challenge(challenge))
            .await
            .map_err(|_| "challenge_send")?;
        let proof = match read_message(&mut recv).await {
            Ok(WireMessage::Proof(proof)) => proof,
            Ok(_) | Err(_) => {
                reject(&mut send, WireReject::Invalid).await;
                return Err("invalid_proof");
            }
        };
        let valid = self
            .signatures
            .verify_current_member_payload(&source_id, &transcript_bytes, &proof)
            .await
            .map_err(|_| "verify_failed")?;
        if !valid {
            reject(&mut send, WireReject::Invalid).await;
            return Err("invalid_signature");
        }
        let verified = VerifiedMembershipPeer {
            space_id: local.space_id,
            device_id: source_id,
            device_name: hello.source_device_name,
            identity_fingerprint: source_fingerprint,
            transport_public_key: remote_key.to_vec(),
            transport_address_blob: hello.source_address,
        };
        if self
            .application_endpoint
            .accept_verified_peer(verified)
            .await
            .is_err()
        {
            reject(&mut send, WireReject::Persistence).await;
            return Err("persistence");
        }
        write_message(&mut send, &WireMessage::Ack)
            .await
            .map_err(|_| "ack_send")?;
        send.finish().map_err(|_| "ack_finish")?;
        let _ = connection.closed().await;
        Ok(())
    }

    async fn handle_gossip(
        &self,
        connection: Connection,
        send: &mut iroh::endpoint::SendStream,
        remote_key: [u8; 32],
        request: WireGossipRequest,
    ) -> Result<(), &'static str> {
        let Some(state) = &self.gossip_state else {
            reject(send, WireReject::Invalid).await;
            return Err("gossip_unavailable");
        };
        let Some(endpoint) = &self.gossip_endpoint else {
            reject(send, WireReject::Invalid).await;
            return Err("gossip_unavailable");
        };
        let source_device_id = match DeviceId::try_new(&request.source_device_id) {
            Some(source_device_id) => source_device_id,
            None => {
                reject(send, WireReject::Invalid).await;
                return Err("invalid_gossip_source");
            }
        };
        if tokio::time::timeout(IO_TIMEOUT, state.session.wait_until_ready())
            .await
            .is_err()
        {
            reject(send, WireReject::Persistence).await;
            return Err("gossip_session_not_ready");
        }
        let resolved = match state.resolve_device(&remote_key).await {
            Some(resolved) => resolved,
            None => {
                reject(send, WireReject::Invalid).await;
                return Err("unknown_gossip_source");
            }
        };
        if resolved != source_device_id || !state.is_admitted(&source_device_id).await {
            reject(send, WireReject::Invalid).await;
            return Err("gossip_source_rejected");
        }
        if request.message.validate_transfer_bounds().is_err() {
            reject(send, WireReject::Invalid).await;
            return Err("invalid_gossip_message");
        }
        let local = match self.identity.current_membership_identity().await {
            Ok(local) => local,
            Err(_) => {
                reject(send, WireReject::Persistence).await;
                return Err("gossip_identity_unavailable");
            }
        };
        let message_space_id = match &request.message {
            MembershipGossipMessage::Digest(message) => &message.space_id,
            MembershipGossipMessage::RequestMissing(message) => &message.space_id,
            MembershipGossipMessage::RequestSharedDevicePage(message) => &message.space_id,
            MembershipGossipMessage::SharedDevicePage(message) => &message.space_id,
            MembershipGossipMessage::EventBatch(message) => &message.space_id,
            MembershipGossipMessage::Ack(message) => &message.space_id,
        };
        if message_space_id != &local.space_id {
            reject(send, WireReject::WrongSpace).await;
            return Err("wrong_gossip_space");
        }
        let response = match endpoint
            .handle_message(&source_device_id, request.message)
            .await
        {
            Ok(response) => response,
            Err(MembershipGossipEndpointError::Rejected) => {
                reject(send, WireReject::Invalid).await;
                return Err("gossip_application_rejected");
            }
            Err(MembershipGossipEndpointError::Persistence) => {
                reject(send, WireReject::Persistence).await;
                return Err("gossip_application_persistence");
            }
        };
        if response.validate_transfer_bounds().is_err() {
            reject(send, WireReject::Invalid).await;
            return Err("invalid_gossip_response");
        }
        write_message(send, &WireMessage::GossipResponse(response))
            .await
            .map_err(|_| "gossip_response_send")?;
        send.finish().map_err(|_| "gossip_response_finish")?;
        let _ = connection.closed().await;
        Ok(())
    }
}

impl MembershipGossipHandlerState {
    async fn resolve_device(&self, public_key: &[u8; 32]) -> Option<DeviceId> {
        let fingerprint = self.fingerprint_factory.from_public_key(public_key).ok()?;
        let members = match self.member_repo.list().await {
            Ok(members) => members,
            Err(error) => {
                warn!(error = %error, "membership gossip member lookup failed");
                return None;
            }
        };
        members
            .into_iter()
            .find(|member| member.identity_fingerprint == fingerprint)
            .map(|member| member.device_id)
    }

    async fn is_admitted(&self, device_id: &DeviceId) -> bool {
        match self.peer_admission.is_admitted(device_id).await {
            Ok(admitted) => admitted,
            Err(error) => {
                warn!(error = %error, "membership gossip admission check failed");
                false
            }
        }
    }
}

struct AttestationTranscript {
    space_id: String,
    group_epoch: u64,
    initiator_device_id: String,
    responder_device_id: String,
    initiator_transport_key: [u8; 32],
    responder_transport_key: [u8; 32],
    initiator_nonce: [u8; 32],
    responder_nonce: [u8; 32],
    initiator_identity_digest: [u8; 32],
    responder_identity_digest: [u8; 32],
    security_updates_digest: [u8; 32],
}

fn attestation_transcript(transcript: &AttestationTranscript) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(256);
    bytes.extend_from_slice(ATTESTATION_DOMAIN);
    append_field(&mut bytes, transcript.space_id.as_bytes());
    bytes.extend_from_slice(&transcript.group_epoch.to_be_bytes());
    append_field(&mut bytes, transcript.initiator_device_id.as_bytes());
    append_field(&mut bytes, transcript.responder_device_id.as_bytes());
    bytes.extend_from_slice(&transcript.initiator_transport_key);
    bytes.extend_from_slice(&transcript.responder_transport_key);
    bytes.extend_from_slice(&transcript.initiator_nonce);
    bytes.extend_from_slice(&transcript.responder_nonce);
    bytes.extend_from_slice(&transcript.initiator_identity_digest);
    bytes.extend_from_slice(&transcript.responder_identity_digest);
    bytes.extend_from_slice(&transcript.security_updates_digest);
    bytes
}

fn append_field(output: &mut Vec<u8>, field: &[u8]) {
    let length = u32::try_from(field.len()).unwrap_or(u32::MAX);
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(field);
}

fn identity_digest(
    identity: &CurrentMembershipIdentity,
    address: &[u8],
    transport_key: &[u8; 32],
) -> [u8; 32] {
    wire_identity_digest(
        identity.device_id.as_str(),
        &identity.device_name,
        identity.identity_fingerprint.as_display(),
        transport_key,
        address,
    )
}

fn wire_identity_digest(
    device_id: &str,
    device_name: &str,
    fingerprint: &str,
    transport_key: &[u8; 32],
    address: &[u8],
) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(device_id.len() + device_name.len() + address.len() + 96);
    append_field(&mut bytes, device_id.as_bytes());
    append_field(&mut bytes, device_name.as_bytes());
    append_field(&mut bytes, fingerprint.as_bytes());
    bytes.extend_from_slice(transport_key);
    append_field(&mut bytes, address);
    *blake3::hash(&bytes).as_bytes()
}

#[allow(clippy::too_many_arguments)]
fn build_transcript(
    initiator: &CurrentMembershipIdentity,
    responder_device_id: &DeviceId,
    group_epoch: u64,
    initiator_transport_key: [u8; 32],
    responder_transport_key: [u8; 32],
    initiator_nonce: [u8; 32],
    responder_nonce: [u8; 32],
    initiator_identity_digest: &[u8; 32],
    responder_identity_digest: &[u8; 32],
    security_updates: &[RelayedSecurityUpdate],
) -> AttestationTranscript {
    AttestationTranscript {
        space_id: initiator.space_id.as_ref().to_owned(),
        group_epoch,
        initiator_device_id: initiator.device_id.as_str().to_owned(),
        responder_device_id: responder_device_id.as_str().to_owned(),
        initiator_transport_key,
        responder_transport_key,
        initiator_nonce,
        responder_nonce,
        initiator_identity_digest: *initiator_identity_digest,
        responder_identity_digest: *responder_identity_digest,
        security_updates_digest: security_updates_digest(security_updates),
    }
}

fn security_updates_digest(updates: &[RelayedSecurityUpdate]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(updates.len().saturating_mul(80));
    for update in updates {
        bytes.extend_from_slice(&update.previous_epoch.to_be_bytes());
        bytes.extend_from_slice(&update.next_epoch.to_be_bytes());
        bytes.extend_from_slice(&update.digest);
    }
    *blake3::hash(&bytes).as_bytes()
}

async fn run_io<T, E>(
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, MembershipAttestationError> {
    tokio::time::timeout(IO_TIMEOUT, future)
        .await
        .map_err(|_| MembershipAttestationError::Transport)?
        .map_err(|_| MembershipAttestationError::Transport)
}

async fn write_message(
    send: &mut iroh::endpoint::SendStream,
    message: &WireMessage,
) -> Result<(), MembershipAttestationError> {
    let payload = postcard::to_stdvec(&WireEnvelope {
        version: WIRE_VERSION,
        message: message.clone(),
    })
    .map_err(|_| MembershipAttestationError::Transport)?;
    if payload.is_empty() || payload.len() > MAX_MESSAGE_SIZE {
        return Err(MembershipAttestationError::Transport);
    }
    let length = u32::try_from(payload.len()).map_err(|_| MembershipAttestationError::Transport)?;
    run_io(send.write_all(&length.to_be_bytes())).await?;
    run_io(send.write_all(&payload)).await
}

async fn read_message(
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<WireMessage, MembershipAttestationError> {
    let mut length = [0u8; 4];
    run_io(recv.read_exact(&mut length)).await?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_MESSAGE_SIZE {
        return Err(MembershipAttestationError::Transport);
    }
    let mut payload = vec![0u8; length];
    run_io(recv.read_exact(&mut payload)).await?;
    let envelope: WireEnvelope =
        postcard::from_bytes(&payload).map_err(|_| MembershipAttestationError::Transport)?;
    if envelope.version != WIRE_VERSION {
        return Err(MembershipAttestationError::VersionIncompatible);
    }
    Ok(envelope.message)
}

async fn reject(send: &mut iroh::endpoint::SendStream, reason: WireReject) {
    if write_message(send, &WireMessage::Reject(reason))
        .await
        .is_ok()
    {
        let _ = send.finish();
    }
}

fn map_rejection(reason: WireReject) -> MembershipAttestationError {
    match reason {
        WireReject::EpochMismatch => MembershipAttestationError::MissingSecurityUpdate,
        WireReject::Version => MembershipAttestationError::VersionIncompatible,
        WireReject::Invalid | WireReject::WrongSpace | WireReject::Persistence => {
            MembershipAttestationError::Rejected
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use iroh::{Endpoint, RelayMode, SecretKey};
    use uc_application::deps::{CurrentMemberSignatureError, CurrentMemberSignaturePort};
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        CurrentMembershipIdentity, CurrentMembershipIdentityError, CurrentMembershipIdentityPort,
        MemberRepositoryPort, MembershipAttestationEndpointError,
        MembershipAttestationEndpointPort, MembershipAttestationError, MembershipAttestationPort,
        MembershipError, MembershipEventBatch, MembershipGossipEndpointError,
        MembershipGossipEndpointPort, MembershipGossipMessage, MembershipGossipTransportError,
        MembershipGossipTransportPort, MembershipSharedDevicePage,
        MembershipSharedDevicePageRequest, PeerAdmissionError, PeerAdmissionPort,
        RelayedSecurityUpdate, SpaceMember, SpaceMembershipCandidate, SponsorCandidateSeed,
        VerifiedMembershipPeer,
    };
    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_core::ports::{
        DeviceIdentityPort, PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort,
        SettingsPort,
    };
    use uc_core::settings::model::Settings;

    use super::{
        attestation_transcript, build_transcript, identity_digest, read_message, run_io,
        wire_identity_digest, write_message, AttestationTranscript,
        IrohMembershipAttestationAdapter, IrohMembershipGossipTransportAdapter,
        IrohMembershipIdentityAdapter, WireChallenge, WireEnvelope, WireGossipRequest, WireHello,
        WireMessage, WireReject, MAX_MESSAGE_SIZE, MEMBERSHIP_ATTESTATION_ALPN, WIRE_VERSION,
    };
    use crate::pairing::session::LEGACY_PAIRING_ALPN;
    use crate::security::{InMemorySession, MasterKey, Sha256IdentityFingerprintFactory};

    struct FixedDeviceIdentity(DeviceId);

    impl DeviceIdentityPort for FixedDeviceIdentity {
        fn current_device_id(&self) -> DeviceId {
            self.0
        }
    }

    struct FixedSettings(Settings);

    #[async_trait]
    impl SettingsPort for FixedSettings {
        async fn load(&self) -> anyhow::Result<Settings> {
            Ok(self.0.clone())
        }

        async fn save(&self, _settings: &Settings) -> anyhow::Result<()> {
            Ok(())
        }
    }

    fn transcript() -> AttestationTranscript {
        AttestationTranscript {
            space_id: "space-a".to_owned(),
            group_epoch: 7,
            initiator_device_id: "device-a".to_owned(),
            responder_device_id: "device-c".to_owned(),
            initiator_transport_key: [1; 32],
            responder_transport_key: [2; 32],
            initiator_nonce: [3; 32],
            responder_nonce: [4; 32],
            initiator_identity_digest: [5; 32],
            responder_identity_digest: [6; 32],
            security_updates_digest: *blake3::hash(&[]).as_bytes(),
        }
    }

    struct FixedIdentity(CurrentMembershipIdentity);

    #[async_trait]
    impl CurrentMembershipIdentityPort for FixedIdentity {
        async fn current_membership_identity(
            &self,
        ) -> Result<CurrentMembershipIdentity, CurrentMembershipIdentityError> {
            Ok(self.0.clone())
        }
    }

    struct TestSignatures {
        device_id: DeviceId,
        secrets: Arc<HashMap<DeviceId, [u8; 32]>>,
        epoch: Arc<AtomicU64>,
    }

    #[async_trait]
    impl CurrentMemberSignaturePort for TestSignatures {
        async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
            Ok(self.epoch.load(Ordering::SeqCst))
        }

        async fn current_member_instance(
            &self,
            device_id: &DeviceId,
        ) -> Result<uc_core::membership::MemberInstanceId, CurrentMemberSignatureError> {
            let secret = self
                .secrets
                .get(device_id)
                .ok_or(CurrentMemberSignatureError::Unavailable)?;
            Ok(uc_core::membership::MemberInstanceId::derive(
                device_id.as_str(),
                secret,
            ))
        }

        async fn sign_current_member_payload(
            &self,
            payload: &[u8],
        ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
            let secret = self
                .secrets
                .get(&self.device_id)
                .ok_or(CurrentMemberSignatureError::Unavailable)?;
            Ok(blake3::keyed_hash(secret, payload).as_bytes().to_vec())
        }

        async fn verify_current_member_payload(
            &self,
            member: &DeviceId,
            payload: &[u8],
            signature: &[u8],
        ) -> Result<bool, CurrentMemberSignatureError> {
            let Some(secret) = self.secrets.get(member) else {
                return Ok(false);
            };
            Ok(blake3::keyed_hash(secret, payload).as_bytes() == signature)
        }
    }

    #[derive(Default)]
    struct RecordingEndpoint(Mutex<Vec<VerifiedMembershipPeer>>);

    #[async_trait]
    impl MembershipAttestationEndpointPort for RecordingEndpoint {
        async fn apply_relayed_security_updates(
            &self,
            _space_id: &SpaceId,
            _updates: &[RelayedSecurityUpdate],
        ) -> Result<u64, MembershipAttestationEndpointError> {
            Ok(7)
        }

        async fn accept_verified_peer(
            &self,
            peer: VerifiedMembershipPeer,
        ) -> Result<(), MembershipAttestationEndpointError> {
            self.0.lock().unwrap().push(peer);
            Ok(())
        }
    }

    async fn endpoint(seed: [u8; 32]) -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(SecretKey::from_bytes(&seed))
                .alpns(vec![MEMBERSHIP_ATTESTATION_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .unwrap(),
        )
    }

    async fn legacy_pairing_only_endpoint(
        seed: [u8; 32],
    ) -> (
        Arc<Endpoint>,
        Arc<AtomicU64>,
        Arc<AtomicBool>,
        Arc<AtomicU64>,
        tokio::task::JoinHandle<()>,
    ) {
        let endpoint = Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(SecretKey::from_bytes(&seed))
                .alpns(vec![LEGACY_PAIRING_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .unwrap(),
        );
        wait_for_direct_addrs(&endpoint).await;
        let accepted = Arc::new(AtomicU64::new(0));
        let stream_opened = Arc::new(AtomicBool::new(false));
        let completed = Arc::new(AtomicU64::new(0));
        let task = {
            let endpoint = Arc::clone(&endpoint);
            let accepted = Arc::clone(&accepted);
            let stream_opened = Arc::clone(&stream_opened);
            let completed = Arc::clone(&completed);
            tokio::spawn(async move {
                while let Some(incoming) = endpoint.accept().await {
                    let Ok(connection) = incoming.await else {
                        continue;
                    };
                    accepted.fetch_add(1, Ordering::SeqCst);
                    if matches!(
                        tokio::time::timeout(Duration::from_millis(250), connection.accept_bi())
                            .await,
                        Ok(Ok(_))
                    ) {
                        stream_opened.store(true, Ordering::SeqCst);
                    }
                    completed.fetch_add(1, Ordering::SeqCst);
                }
            })
        };
        (endpoint, accepted, stream_opened, completed, task)
    }

    async fn wait_for_counter(counter: &AtomicU64, expected: u64) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while counter.load(Ordering::SeqCst) != expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    fn membership_identity_adapter(
        endpoint: Arc<Endpoint>,
        session: Arc<InMemorySession>,
        device_name: Option<&str>,
    ) -> IrohMembershipIdentityAdapter {
        let mut settings = Settings::default();
        settings.general.device_name = device_name.map(str::to_owned);
        IrohMembershipIdentityAdapter::new(
            endpoint,
            session,
            Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            Arc::new(FixedSettings(settings)),
            Arc::new(Sha256IdentityFingerprintFactory),
        )
    }

    fn ready_session() -> Arc<InMemorySession> {
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            SpaceId::from("space-a"),
            MasterKey::from_bytes(&[0x61; 32]).unwrap(),
        );
        session
    }

    #[tokio::test]
    async fn current_membership_identity_is_unavailable_while_space_is_locked() {
        let endpoint = endpoint([0x41; 32]).await;
        let adapter = membership_identity_adapter(
            Arc::clone(&endpoint),
            Arc::new(InMemorySession::new()),
            Some("Device A"),
        );

        let result = adapter.current_membership_identity().await;

        assert_eq!(result, Err(CurrentMembershipIdentityError::Unavailable));
        endpoint.close().await;
    }

    #[tokio::test]
    async fn current_membership_identity_requires_a_device_name() {
        let endpoint = endpoint([0x42; 32]).await;
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            SpaceId::from("space-a"),
            MasterKey::from_bytes(&[0x51; 32]).unwrap(),
        );
        let adapter = membership_identity_adapter(Arc::clone(&endpoint), session, None);

        let result = adapter.current_membership_identity().await;

        assert_eq!(result, Err(CurrentMembershipIdentityError::Unavailable));
        endpoint.close().await;
    }

    #[tokio::test]
    async fn current_membership_identity_uses_the_active_space_and_endpoint_key() {
        let endpoint = endpoint([0x43; 32]).await;
        let session = Arc::new(InMemorySession::new());
        session.set_master_key_for_space(
            SpaceId::from("space-a"),
            MasterKey::from_bytes(&[0x52; 32]).unwrap(),
        );
        let adapter = membership_identity_adapter(Arc::clone(&endpoint), session, Some("Device A"));

        let identity = adapter.current_membership_identity().await.unwrap();

        assert_eq!(identity.space_id, SpaceId::from("space-a"));
        assert_eq!(identity.device_id, DeviceId::new("device-a"));
        assert_eq!(identity.device_name, "Device A");
        assert_eq!(
            identity.identity_fingerprint,
            Sha256IdentityFingerprintFactory
                .from_public_key(endpoint.id().as_bytes())
                .unwrap()
        );
        endpoint.close().await;
    }

    async fn wait_for_direct_addrs(endpoint: &Endpoint) {
        for _ in 0..100 {
            if !endpoint.addr().addrs.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("endpoint never published direct addresses")
    }

    fn identity(seed: [u8; 32], device_id: &str) -> CurrentMembershipIdentity {
        let public_key = SecretKey::from_bytes(&seed).public();
        let fingerprint = Sha256IdentityFingerprintFactory
            .from_public_key(public_key.as_bytes())
            .unwrap();
        CurrentMembershipIdentity {
            space_id: SpaceId::from("space-a"),
            device_id: DeviceId::new(device_id),
            device_name: format!("{device_id} name"),
            identity_fingerprint: fingerprint,
        }
    }

    fn signatures(
        device_id: &str,
        secrets: Arc<HashMap<DeviceId, [u8; 32]>>,
    ) -> Arc<dyn CurrentMemberSignaturePort> {
        signatures_at(device_id, secrets, Arc::new(AtomicU64::new(7)))
    }

    fn signatures_at(
        device_id: &str,
        secrets: Arc<HashMap<DeviceId, [u8; 32]>>,
        epoch: Arc<AtomicU64>,
    ) -> Arc<dyn CurrentMemberSignaturePort> {
        Arc::new(TestSignatures {
            device_id: DeviceId::new(device_id),
            secrets,
            epoch,
        })
    }

    fn candidate(target: &CurrentMembershipIdentity, address: Vec<u8>) -> SpaceMembershipCandidate {
        candidate_with_updates(target, address, Vec::new())
    }

    fn candidate_with_updates(
        target: &CurrentMembershipIdentity,
        address: Vec<u8>,
        security_updates: Vec<RelayedSecurityUpdate>,
    ) -> SpaceMembershipCandidate {
        SpaceMembershipCandidate::from_sponsor_seed(
            SponsorCandidateSeed {
                space_id: target.space_id.clone(),
                device_id: target.device_id,
                device_name_hint: target.device_name.clone(),
                identity_fingerprint_hint: target.identity_fingerprint.clone(),
                transport_address_blob: address,
                address_observed_at_ms: 1_000,
                source_device_id: DeviceId::new("sponsor"),
                security_updates,
                expires_at_ms: 50_000,
            },
            2_000,
        )
        .unwrap()
    }

    struct ApplyingEndpoint {
        epoch: Arc<AtomicU64>,
        applied: Mutex<Vec<RelayedSecurityUpdate>>,
        accepted: Mutex<Vec<VerifiedMembershipPeer>>,
    }

    struct RecordingGossipEndpoint(Mutex<Vec<(DeviceId, MembershipGossipMessage)>>);

    #[async_trait]
    impl MembershipGossipEndpointPort for RecordingGossipEndpoint {
        async fn handle_message(
            &self,
            source_device_id: &DeviceId,
            message: MembershipGossipMessage,
        ) -> Result<MembershipGossipMessage, MembershipGossipEndpointError> {
            let response = match &message {
                MembershipGossipMessage::EventBatch(batch) => {
                    MembershipGossipMessage::Ack(uc_core::membership::MembershipAck {
                        space_id: batch.space_id.clone(),
                        batch_id: batch.batch_id,
                    })
                }
                MembershipGossipMessage::RequestSharedDevicePage(request) => {
                    MembershipGossipMessage::SharedDevicePage(MembershipSharedDevicePage {
                        space_id: request.space_id.clone(),
                        seeds: Vec::new(),
                        next_after_device_id: None,
                    })
                }
                _ => return Err(MembershipGossipEndpointError::Rejected),
            };
            self.0.lock().unwrap().push((*source_device_id, message));
            Ok(response)
        }
    }

    struct StaticMemberRepository(Vec<SpaceMember>);

    #[async_trait]
    impl MemberRepositoryPort for StaticMemberRepository {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self
                .0
                .iter()
                .find(|member| &member.device_id == device_id)
                .cloned())
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(self.0.clone())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Ok(())
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(false)
        }
    }

    struct StaticPeerAddressRepository(Mutex<HashMap<DeviceId, PeerAddressRecord>>);

    #[async_trait]
    impl PeerAddressRepositoryPort for StaticPeerAddressRepository {
        async fn get(
            &self,
            device_id: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(self.0.lock().unwrap().get(device_id).cloned())
        }

        async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            self.0
                .lock()
                .unwrap()
                .insert(record.device_id, record.clone());
            Ok(())
        }

        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }

        async fn remove(&self, device_id: &DeviceId) -> Result<(), PeerAddressError> {
            self.0.lock().unwrap().remove(device_id);
            Ok(())
        }
    }

    struct AdmitAll;

    #[async_trait]
    impl PeerAdmissionPort for AdmitAll {
        async fn is_admitted(&self, _device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
            Ok(true)
        }
    }

    struct ToggleAdmission(AtomicBool);

    #[async_trait]
    impl PeerAdmissionPort for ToggleAdmission {
        async fn is_admitted(&self, _device_id: &DeviceId) -> Result<bool, PeerAdmissionError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }

    #[async_trait]
    impl MembershipAttestationEndpointPort for ApplyingEndpoint {
        async fn apply_relayed_security_updates(
            &self,
            space_id: &SpaceId,
            updates: &[RelayedSecurityUpdate],
        ) -> Result<u64, MembershipAttestationEndpointError> {
            if space_id != &SpaceId::from("space-a") {
                return Err(MembershipAttestationEndpointError::Rejected);
            }
            let mut epoch = self.epoch.load(Ordering::SeqCst);
            for update in updates {
                if update.previous_epoch != epoch || update.next_epoch != epoch + 1 {
                    return Err(MembershipAttestationEndpointError::Rejected);
                }
                epoch = update.next_epoch;
                self.applied.lock().unwrap().push(update.clone());
            }
            self.epoch.store(epoch, Ordering::SeqCst);
            Ok(epoch)
        }

        async fn accept_verified_peer(
            &self,
            peer: VerifiedMembershipPeer,
        ) -> Result<(), MembershipAttestationEndpointError> {
            self.accepted.lock().unwrap().push(peer);
            Ok(())
        }
    }

    async fn wire_hello(
        endpoint: &Endpoint,
        source: &CurrentMembershipIdentity,
        target: &CurrentMembershipIdentity,
    ) -> WireHello {
        WireHello {
            space_id: source.space_id.as_ref().to_owned(),
            group_epoch: 7,
            source_device_id: source.device_id.as_str().to_owned(),
            target_device_id: target.device_id.as_str().to_owned(),
            source_device_name: source.device_name.clone(),
            source_identity_fingerprint: source.identity_fingerprint.as_display().to_owned(),
            source_transport_key: *endpoint.id().as_bytes(),
            source_address: postcard::to_stdvec(&endpoint.addr()).unwrap(),
            source_nonce: [0x51; 32],
            security_updates: Vec::new(),
        }
    }

    async fn raw_connection(
        source: &Endpoint,
        target: &Endpoint,
    ) -> (
        iroh::endpoint::Connection,
        iroh::endpoint::SendStream,
        iroh::endpoint::RecvStream,
    ) {
        let connection = source
            .connect(target.addr(), MEMBERSHIP_ATTESTATION_ALPN)
            .await
            .unwrap();
        let (send, recv) = connection.open_bi().await.unwrap();
        (connection, send, recv)
    }

    async fn raw_gossip_response(
        source: &Endpoint,
        target: &Endpoint,
        source_device_id: &str,
        message: MembershipGossipMessage,
    ) -> Result<WireMessage, MembershipAttestationError> {
        let (connection, mut send, mut recv) = raw_connection(source, target).await;
        let payload = postcard::to_stdvec(&WireEnvelope {
            version: WIRE_VERSION,
            message: WireMessage::GossipRequest(WireGossipRequest {
                source_device_id: source_device_id.to_owned(),
                message,
            }),
        })
        .unwrap();
        let length = u32::try_from(payload.len()).unwrap().to_be_bytes();
        run_io(send.write_all(&length)).await.unwrap();
        run_io(send.write_all(&payload)).await.unwrap();
        send.finish().unwrap();
        let response = read_message(&mut recv).await;
        drop(connection);
        response
    }

    fn proof_for_challenge(
        hello: &WireHello,
        challenge: &WireChallenge,
        source: &CurrentMembershipIdentity,
        source_secret: &[u8; 32],
    ) -> Vec<u8> {
        let responder_id = DeviceId::new(&challenge.responder_device_id);
        let transcript = build_transcript(
            source,
            &responder_id,
            hello.group_epoch,
            hello.source_transport_key,
            challenge.responder_transport_key,
            hello.source_nonce,
            challenge.responder_nonce,
            &identity_digest(source, &hello.source_address, &hello.source_transport_key),
            &wire_identity_digest(
                &challenge.responder_device_id,
                &challenge.responder_device_name,
                &challenge.responder_identity_fingerprint,
                &challenge.responder_transport_key,
                &challenge.responder_address,
            ),
            &hello.security_updates,
        );
        blake3::keyed_hash(source_secret, &attestation_transcript(&transcript))
            .as_bytes()
            .to_vec()
    }

    #[test]
    fn transcript_is_deterministic_and_domain_separated() {
        let bytes = attestation_transcript(&transcript());

        assert_eq!(bytes, attestation_transcript(&transcript()));
        assert!(bytes.starts_with(b"uniclipboard/membership-gossip/1\0"));
    }

    #[test]
    fn transcript_changes_when_any_connection_binding_changes() {
        let original = attestation_transcript(&transcript());
        let mut changed = transcript();
        changed.responder_nonce[0] ^= 1;
        assert_ne!(original, attestation_transcript(&changed));

        let mut changed = transcript();
        changed.initiator_transport_key[0] ^= 1;
        assert_ne!(original, attestation_transcript(&changed));

        let mut changed = transcript();
        changed.space_id = "space-b".to_owned();
        assert_ne!(original, attestation_transcript(&changed));
    }

    #[tokio::test]
    async fn attestation_identifies_a_peer_that_only_supports_legacy_pairing() {
        let local_seed = [0x41; 32];
        let legacy_seed = [0x43; 32];
        let local_endpoint = endpoint(local_seed).await;
        wait_for_direct_addrs(&local_endpoint).await;
        let (legacy_endpoint, accepted, stream_opened, completed, legacy_task) =
            legacy_pairing_only_endpoint(legacy_seed).await;
        let local_identity = identity(local_seed, "device-a");
        let legacy_identity = identity(legacy_seed, "device-c");
        let signatures = Arc::new(HashMap::from([(local_identity.device_id, [0xa1; 32])]));
        let adapter = IrohMembershipAttestationAdapter::new(
            Arc::clone(&local_endpoint),
            Arc::new(FixedIdentity(local_identity)),
            self::signatures("device-a", signatures),
            Arc::new(Sha256IdentityFingerprintFactory),
        );

        let result = adapter
            .attest_candidate(&candidate(
                &legacy_identity,
                postcard::to_stdvec(&legacy_endpoint.addr()).unwrap(),
            ))
            .await;

        assert_eq!(result, Err(MembershipAttestationError::VersionIncompatible));
        wait_for_counter(&completed, 1).await;
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
        assert!(!stream_opened.load(Ordering::SeqCst));
        legacy_task.abort();
        legacy_endpoint.close().await;
        local_endpoint.close().await;
    }

    #[tokio::test]
    async fn gossip_identifies_a_recipient_that_only_supports_legacy_pairing() {
        let local_seed = [0x42; 32];
        let legacy_seed = [0x41; 32];
        let local_endpoint = endpoint(local_seed).await;
        wait_for_direct_addrs(&local_endpoint).await;
        let (legacy_endpoint, accepted, stream_opened, completed, legacy_task) =
            legacy_pairing_only_endpoint(legacy_seed).await;
        let local_identity = identity(local_seed, "device-b");
        let legacy_identity = identity(legacy_seed, "device-a");
        let addresses = Arc::new(StaticPeerAddressRepository(Mutex::new(HashMap::from([(
            legacy_identity.device_id,
            PeerAddressRecord {
                device_id: legacy_identity.device_id,
                addr_blob: postcard::to_stdvec(&legacy_endpoint.addr()).unwrap(),
                observed_at: chrono::Utc::now(),
            },
        )]))));
        let transport = IrohMembershipGossipTransportAdapter::new(
            Arc::clone(&local_endpoint),
            ready_session(),
            Arc::new(FixedIdentity(local_identity)),
            addresses,
            Arc::new(StaticMemberRepository(Vec::new())),
            Arc::new(AdmitAll),
            Arc::new(Sha256IdentityFingerprintFactory),
        );

        let result = transport
            .exchange(
                &legacy_identity.device_id,
                MembershipGossipMessage::EventBatch(MembershipEventBatch {
                    space_id: SpaceId::from("space-a"),
                    batch_id: [8; 32],
                    events: Vec::new(),
                }),
            )
            .await;

        assert_eq!(
            result,
            Err(MembershipGossipTransportError::VersionIncompatible)
        );
        wait_for_counter(&completed, 1).await;
        assert_eq!(accepted.load(Ordering::SeqCst), 1);
        assert!(!stream_opened.load(Ordering::SeqCst));
        legacy_task.abort();
        legacy_endpoint.close().await;
        local_endpoint.close().await;
    }

    #[tokio::test]
    async fn two_unknown_peers_complete_mutual_attestation_over_the_dedicated_channel() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let a_adapter = Arc::new(IrohMembershipAttestationAdapter::new(
            a_endpoint.clone(),
            Arc::new(FixedIdentity(a_identity.clone())),
            signatures("device-a", secrets.clone()),
            Arc::new(Sha256IdentityFingerprintFactory),
        ));
        let c_adapter = Arc::new(IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity.clone())),
            signatures("device-c", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        ));
        let a_inbound = Arc::new(RecordingEndpoint::default());
        let c_inbound = Arc::new(RecordingEndpoint::default());
        let a_router = iroh::protocol::Router::builder((*a_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                a_adapter.handler(a_inbound.clone()),
            )
            .spawn();
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(c_inbound.clone()),
            )
            .spawn();
        let c_address = postcard::to_stdvec(&c_endpoint.addr()).unwrap();

        let verified = a_adapter
            .attest_candidate(&candidate(&c_identity, c_address))
            .await
            .unwrap();

        assert_eq!(verified.device_id, c_identity.device_id);
        let accepted_by_c = c_inbound.0.lock().unwrap();
        assert_eq!(accepted_by_c.len(), 1);
        assert_eq!(accepted_by_c[0].device_id, a_identity.device_id);
        drop(accepted_by_c);
        a_router.shutdown().await.ok();
        c_router.shutdown().await.ok();
    }

    #[tokio::test]
    async fn lagging_responder_applies_relayed_update_before_mutual_attestation() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let a_epoch = Arc::new(AtomicU64::new(6));
        let a_adapter = Arc::new(IrohMembershipAttestationAdapter::new(
            a_endpoint.clone(),
            Arc::new(FixedIdentity(a_identity.clone())),
            signatures_at("device-a", secrets.clone(), a_epoch.clone()),
            Arc::new(Sha256IdentityFingerprintFactory),
        ));
        let c_adapter = Arc::new(IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity.clone())),
            signatures("device-c", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        ));
        let a_inbound = Arc::new(ApplyingEndpoint {
            epoch: a_epoch.clone(),
            applied: Mutex::new(Vec::new()),
            accepted: Mutex::new(Vec::new()),
        });
        let a_router = iroh::protocol::Router::builder((*a_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                a_adapter.handler(a_inbound.clone()),
            )
            .spawn();
        let update = RelayedSecurityUpdate {
            previous_epoch: 6,
            next_epoch: 7,
            payload: b"epoch-6-to-7".to_vec(),
            digest: [7; 32],
        };

        let verified = c_adapter
            .attest_candidate(&candidate_with_updates(
                &a_identity,
                postcard::to_stdvec(&a_endpoint.addr()).unwrap(),
                vec![update.clone()],
            ))
            .await
            .unwrap();

        assert_eq!(verified.device_id, a_identity.device_id);
        assert_eq!(a_epoch.load(Ordering::SeqCst), 7);
        assert_eq!(*a_inbound.applied.lock().unwrap(), vec![update]);
        assert_eq!(a_inbound.accepted.lock().unwrap().len(), 1);
        a_router.shutdown().await.ok();
        c_endpoint.close().await;
    }

    #[tokio::test]
    async fn known_member_event_batch_and_shared_device_page_round_trip_over_membership_channel() {
        let a_seed = [0x41; 32];
        let b_seed = [0x42; 32];
        let a_endpoint = endpoint(a_seed).await;
        let b_endpoint = endpoint(b_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&b_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let b_identity = identity(b_seed, "device-b");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (b_identity.device_id, [0xb1; 32]),
        ]));
        let a_attestation = Arc::new(IrohMembershipAttestationAdapter::new(
            a_endpoint.clone(),
            Arc::new(FixedIdentity(a_identity.clone())),
            signatures("device-a", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        ));
        let member = |identity: &CurrentMembershipIdentity| SpaceMember {
            device_id: identity.device_id,
            device_name: identity.device_name.clone(),
            identity_fingerprint: identity.identity_fingerprint.clone(),
            joined_at: chrono::Utc::now(),
            sync_preferences: uc_core::MemberSyncPreferences::default(),
        };
        let a_transport = IrohMembershipGossipTransportAdapter::new(
            a_endpoint.clone(),
            ready_session(),
            Arc::new(FixedIdentity(a_identity.clone())),
            Arc::new(StaticPeerAddressRepository(Mutex::new(HashMap::new()))),
            Arc::new(StaticMemberRepository(vec![member(&b_identity)])),
            Arc::new(AdmitAll),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let received = Arc::new(RecordingGossipEndpoint(Mutex::new(Vec::new())));
        let a_router = iroh::protocol::Router::builder((*a_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                a_attestation.handler_with_gossip(
                    Arc::new(RecordingEndpoint::default()),
                    &a_transport,
                    received.clone(),
                ),
            )
            .spawn();
        let b_addresses = Arc::new(StaticPeerAddressRepository(Mutex::new(HashMap::from([(
            a_identity.device_id,
            PeerAddressRecord {
                device_id: a_identity.device_id,
                addr_blob: postcard::to_stdvec(&a_endpoint.addr()).unwrap(),
                observed_at: chrono::Utc::now(),
            },
        )]))));
        let b_transport = IrohMembershipGossipTransportAdapter::new(
            b_endpoint.clone(),
            ready_session(),
            Arc::new(FixedIdentity(b_identity.clone())),
            b_addresses,
            Arc::new(StaticMemberRepository(Vec::new())),
            Arc::new(AdmitAll),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let batch = MembershipEventBatch {
            space_id: SpaceId::from("space-a"),
            batch_id: [8; 32],
            events: Vec::new(),
        };

        let response = b_transport
            .exchange(
                &a_identity.device_id,
                MembershipGossipMessage::EventBatch(batch.clone()),
            )
            .await
            .unwrap();

        assert_eq!(
            response,
            MembershipGossipMessage::Ack(uc_core::membership::MembershipAck {
                space_id: batch.space_id,
                batch_id: batch.batch_id,
            })
        );
        let received_events = received.0.lock().unwrap();
        assert_eq!(received_events.len(), 1);
        assert_eq!(received_events[0].0, b_identity.device_id);
        drop(received_events);

        let shared_device_page = b_transport
            .exchange(
                &a_identity.device_id,
                MembershipGossipMessage::RequestSharedDevicePage(
                    MembershipSharedDevicePageRequest {
                        space_id: SpaceId::from("space-a"),
                        after_device_id: None,
                    },
                ),
            )
            .await
            .unwrap();

        assert_eq!(
            shared_device_page,
            MembershipGossipMessage::SharedDevicePage(MembershipSharedDevicePage {
                space_id: SpaceId::from("space-a"),
                seeds: Vec::new(),
                next_after_device_id: None,
            })
        );
        let received_events = received.0.lock().unwrap();
        assert_eq!(received_events.len(), 2);
        assert!(matches!(
            received_events[1].1,
            MembershipGossipMessage::RequestSharedDevicePage(_)
        ));
        drop(received_events);
        a_router.shutdown().await.ok();
        b_endpoint.close().await;
    }

    #[tokio::test]
    async fn gossip_request_waits_for_the_receiver_session_on_the_same_connection() {
        let a_seed = [0x51; 32];
        let b_seed = [0x52; 32];
        let a_endpoint = endpoint(a_seed).await;
        let b_endpoint = endpoint(b_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&b_endpoint).await;

        let a_identity = identity(a_seed, "device-a");
        let b_identity = identity(b_seed, "device-b");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (b_identity.device_id, [0xb1; 32]),
        ]));
        let a_session = Arc::new(InMemorySession::new());
        let a_attestation = Arc::new(IrohMembershipAttestationAdapter::new(
            a_endpoint.clone(),
            Arc::new(FixedIdentity(a_identity.clone())),
            signatures("device-a", secrets.clone()),
            Arc::new(Sha256IdentityFingerprintFactory),
        ));
        let member = |identity: &CurrentMembershipIdentity| SpaceMember {
            device_id: identity.device_id,
            device_name: identity.device_name.clone(),
            identity_fingerprint: identity.identity_fingerprint.clone(),
            joined_at: chrono::Utc::now(),
            sync_preferences: uc_core::MemberSyncPreferences::default(),
        };
        let a_transport = IrohMembershipGossipTransportAdapter::new(
            a_endpoint.clone(),
            Arc::clone(&a_session),
            Arc::new(FixedIdentity(a_identity.clone())),
            Arc::new(StaticPeerAddressRepository(Mutex::new(HashMap::new()))),
            Arc::new(StaticMemberRepository(vec![member(&b_identity)])),
            Arc::new(AdmitAll),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let received = Arc::new(RecordingGossipEndpoint(Mutex::new(Vec::new())));
        let a_router = iroh::protocol::Router::builder((*a_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                a_attestation.handler_with_gossip(
                    Arc::new(RecordingEndpoint::default()),
                    &a_transport,
                    received.clone(),
                ),
            )
            .spawn();
        let b_addresses = Arc::new(StaticPeerAddressRepository(Mutex::new(HashMap::from([(
            a_identity.device_id,
            PeerAddressRecord {
                device_id: a_identity.device_id,
                addr_blob: postcard::to_stdvec(&a_endpoint.addr()).unwrap(),
                observed_at: chrono::Utc::now(),
            },
        )]))));
        let b_transport = Arc::new(IrohMembershipGossipTransportAdapter::new(
            b_endpoint.clone(),
            ready_session(),
            Arc::new(FixedIdentity(b_identity.clone())),
            b_addresses,
            Arc::new(StaticMemberRepository(Vec::new())),
            Arc::new(AdmitAll),
            Arc::new(Sha256IdentityFingerprintFactory),
        ));
        let batch = MembershipEventBatch {
            space_id: SpaceId::from("space-a"),
            batch_id: [9; 32],
            events: Vec::new(),
        };
        let exchange = tokio::spawn({
            let b_transport = Arc::clone(&b_transport);
            let recipient = a_identity.device_id;
            async move {
                b_transport
                    .exchange(
                        &recipient,
                        MembershipGossipMessage::EventBatch(batch.clone()),
                    )
                    .await
            }
        });
        tokio::pin!(exchange);

        assert!(
            tokio::time::timeout(Duration::from_millis(50), &mut exchange)
                .await
                .is_err()
        );

        a_session.set_master_key_for_space(
            SpaceId::from("space-a"),
            MasterKey::from_bytes(&[0x51; 32]).unwrap(),
        );

        let response = tokio::time::timeout(Duration::from_secs(2), exchange)
            .await
            .expect("the original gossip request must complete after session readiness")
            .expect("gossip task must not panic")
            .expect("the original gossip request must not be rejected");
        assert!(matches!(response, MembershipGossipMessage::Ack(_)));
        assert_eq!(received.0.lock().unwrap().len(), 1);
        a_router.shutdown().await.ok();
        a_endpoint.close().await;
        b_endpoint.close().await;
    }

    #[tokio::test]
    async fn gossip_explicitly_rejects_spoofed_untrusted_wrong_space_and_invalid_messages() {
        let a_seed = [0x41; 32];
        let b_seed = [0x42; 32];
        let unknown_seed = [0x55; 32];
        let a_endpoint = endpoint(a_seed).await;
        let b_endpoint = endpoint(b_seed).await;
        let unknown_endpoint = endpoint(unknown_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&b_endpoint).await;
        wait_for_direct_addrs(&unknown_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let b_identity = identity(b_seed, "device-b");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (b_identity.device_id, [0xb1; 32]),
        ]));
        let admission = Arc::new(ToggleAdmission(AtomicBool::new(true)));
        let a_attestation = IrohMembershipAttestationAdapter::new(
            a_endpoint.clone(),
            Arc::new(FixedIdentity(a_identity.clone())),
            signatures("device-a", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let a_transport = IrohMembershipGossipTransportAdapter::new(
            a_endpoint.clone(),
            ready_session(),
            Arc::new(FixedIdentity(a_identity.clone())),
            Arc::new(StaticPeerAddressRepository(Mutex::new(HashMap::new()))),
            Arc::new(StaticMemberRepository(vec![SpaceMember {
                device_id: b_identity.device_id,
                device_name: b_identity.device_name.clone(),
                identity_fingerprint: b_identity.identity_fingerprint.clone(),
                joined_at: chrono::Utc::now(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            }])),
            admission.clone(),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let received = Arc::new(RecordingGossipEndpoint(Mutex::new(Vec::new())));
        let a_router = iroh::protocol::Router::builder((*a_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                a_attestation.handler_with_gossip(
                    Arc::new(RecordingEndpoint::default()),
                    &a_transport,
                    received.clone(),
                ),
            )
            .spawn();
        let batch = |space: &str| {
            MembershipGossipMessage::EventBatch(MembershipEventBatch {
                space_id: SpaceId::from(space),
                batch_id: [8; 32],
                events: Vec::new(),
            })
        };

        let spoofed = raw_gossip_response(&b_endpoint, &a_endpoint, "device-c", batch("space-a"))
            .await
            .unwrap();
        assert!(matches!(spoofed, WireMessage::Reject(WireReject::Invalid)));

        admission.0.store(false, Ordering::SeqCst);
        let untrusted = raw_gossip_response(&b_endpoint, &a_endpoint, "device-b", batch("space-a"))
            .await
            .unwrap();
        assert!(matches!(
            untrusted,
            WireMessage::Reject(WireReject::Invalid)
        ));

        admission.0.store(true, Ordering::SeqCst);
        let wrong_space =
            raw_gossip_response(&b_endpoint, &a_endpoint, "device-b", batch("space-b"))
                .await
                .unwrap();
        assert!(matches!(
            wrong_space,
            WireMessage::Reject(WireReject::WrongSpace)
        ));

        let invalid_message =
            MembershipGossipMessage::Digest(uc_core::membership::MembershipDigest {
                space_id: SpaceId::from("space-a"),
                group_epoch: 7,
                group_update_head_digest: None,
                announcements: (0..65)
                    .map(|index| uc_core::membership::MembershipAnnouncementVersion {
                        device_id: DeviceId::new(format!("device-{index}")),
                        sequence: 1,
                        content_digest: [index as u8; 32],
                    })
                    .collect(),
            });
        let invalid = raw_gossip_response(&b_endpoint, &a_endpoint, "device-b", invalid_message)
            .await
            .unwrap();
        assert!(matches!(invalid, WireMessage::Reject(WireReject::Invalid)));

        let unknown = raw_gossip_response(
            &unknown_endpoint,
            &a_endpoint,
            "device-unknown",
            batch("space-a"),
        )
        .await
        .unwrap();
        assert!(matches!(unknown, WireMessage::Reject(WireReject::Invalid)));
        assert!(received.0.lock().unwrap().is_empty());

        a_router.shutdown().await.ok();
        b_endpoint.close().await;
        unknown_endpoint.close().await;
    }

    #[tokio::test]
    async fn unknown_wire_version_is_explicitly_rejected() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let c_adapter = IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity.clone())),
            signatures("device-c", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(Arc::new(RecordingEndpoint::default())),
            )
            .spawn();
        let connection = a_endpoint
            .connect(c_endpoint.addr(), MEMBERSHIP_ATTESTATION_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        let payload = postcard::to_stdvec(&WireEnvelope {
            version: WIRE_VERSION + 1,
            message: WireMessage::Hello(wire_hello(&a_endpoint, &a_identity, &c_identity).await),
        })
        .unwrap();
        let length = u32::try_from(payload.len()).unwrap().to_be_bytes();
        run_io(send.write_all(&length)).await.unwrap();
        run_io(send.write_all(&payload)).await.unwrap();

        let response = read_message(&mut recv).await.unwrap();

        assert!(matches!(response, WireMessage::Reject(WireReject::Version)));
        drop(connection);
        c_router.shutdown().await.ok();
        a_endpoint.close().await;
    }

    #[tokio::test]
    async fn wrong_space_is_rejected_without_persisting_the_peer() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let mut a_identity = identity(a_seed, "device-a");
        a_identity.space_id = SpaceId::from("space-b");
        let c_identity = identity(c_seed, "device-c");
        let mut candidate_identity = c_identity.clone();
        candidate_identity.space_id = a_identity.space_id.clone();
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let a_adapter = IrohMembershipAttestationAdapter::new(
            a_endpoint.clone(),
            Arc::new(FixedIdentity(a_identity)),
            signatures("device-a", secrets.clone()),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_adapter = IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity)),
            signatures("device-c", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_inbound = Arc::new(RecordingEndpoint::default());
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(c_inbound.clone()),
            )
            .spawn();

        let result = a_adapter
            .attest_candidate(&candidate(
                &candidate_identity,
                postcard::to_stdvec(&c_endpoint.addr()).unwrap(),
            ))
            .await;

        assert_eq!(result, Err(MembershipAttestationError::Rejected));
        assert!(c_inbound.0.lock().unwrap().is_empty());
        c_router.shutdown().await.ok();
        a_endpoint.close().await;
    }

    #[tokio::test]
    async fn invalid_member_signature_is_rejected_without_persisting_the_peer() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let correct_secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let invalid_a_secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xff; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let a_adapter = IrohMembershipAttestationAdapter::new(
            a_endpoint.clone(),
            Arc::new(FixedIdentity(a_identity)),
            signatures("device-a", invalid_a_secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_adapter = IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity.clone())),
            signatures("device-c", correct_secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_inbound = Arc::new(RecordingEndpoint::default());
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(c_inbound.clone()),
            )
            .spawn();

        let result = a_adapter
            .attest_candidate(&candidate(
                &c_identity,
                postcard::to_stdvec(&c_endpoint.addr()).unwrap(),
            ))
            .await;

        assert_eq!(result, Err(MembershipAttestationError::Rejected));
        assert!(c_inbound.0.lock().unwrap().is_empty());
        c_router.shutdown().await.ok();
        a_endpoint.close().await;
    }

    #[tokio::test]
    async fn transport_key_mismatch_is_rejected_before_the_challenge() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let c_adapter = IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity.clone())),
            signatures("device-c", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_inbound = Arc::new(RecordingEndpoint::default());
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(c_inbound.clone()),
            )
            .spawn();
        let (connection, mut send, mut recv) = raw_connection(&a_endpoint, &c_endpoint).await;
        let mut hello = wire_hello(&a_endpoint, &a_identity, &c_identity).await;
        hello.source_transport_key = [0xff; 32];
        write_message(&mut send, &WireMessage::Hello(hello))
            .await
            .unwrap();

        let response = read_message(&mut recv).await.unwrap();

        assert!(matches!(response, WireMessage::Reject(WireReject::Invalid)));
        assert!(c_inbound.0.lock().unwrap().is_empty());
        drop(connection);
        c_router.shutdown().await.ok();
        a_endpoint.close().await;
    }

    #[tokio::test]
    async fn oversized_message_is_rejected_before_allocating_the_payload() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let c_adapter = IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity)),
            signatures("device-c", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_inbound = Arc::new(RecordingEndpoint::default());
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(c_inbound.clone()),
            )
            .spawn();
        let (connection, mut send, mut recv) = raw_connection(&a_endpoint, &c_endpoint).await;
        let oversized = u32::try_from(MAX_MESSAGE_SIZE + 1).unwrap().to_be_bytes();
        run_io(send.write_all(&oversized)).await.unwrap();

        let response = read_message(&mut recv).await.unwrap();

        assert!(matches!(response, WireMessage::Reject(WireReject::Invalid)));
        assert!(c_inbound.0.lock().unwrap().is_empty());
        drop(connection);
        c_router.shutdown().await.ok();
        a_endpoint.close().await;
    }

    #[tokio::test]
    async fn replayed_proof_is_rejected_on_a_new_connection() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let c_adapter = IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity.clone())),
            signatures("device-c", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_inbound = Arc::new(RecordingEndpoint::default());
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(c_inbound.clone()),
            )
            .spawn();
        let first_hello = wire_hello(&a_endpoint, &a_identity, &c_identity).await;
        let (first_connection, mut first_send, mut first_recv) =
            raw_connection(&a_endpoint, &c_endpoint).await;
        write_message(&mut first_send, &WireMessage::Hello(first_hello.clone()))
            .await
            .unwrap();
        let first_challenge = match read_message(&mut first_recv).await.unwrap() {
            WireMessage::Challenge(challenge) => challenge,
            _ => panic!("expected membership challenge"),
        };
        let replayed_proof =
            proof_for_challenge(&first_hello, &first_challenge, &a_identity, &[0xa1; 32]);
        write_message(&mut first_send, &WireMessage::Proof(replayed_proof.clone()))
            .await
            .unwrap();
        assert!(matches!(
            read_message(&mut first_recv).await.unwrap(),
            WireMessage::Ack
        ));
        drop(first_connection);

        let (replay_connection, mut replay_send, mut replay_recv) =
            raw_connection(&a_endpoint, &c_endpoint).await;
        write_message(&mut replay_send, &WireMessage::Hello(first_hello))
            .await
            .unwrap();
        assert!(matches!(
            read_message(&mut replay_recv).await.unwrap(),
            WireMessage::Challenge(_)
        ));
        write_message(&mut replay_send, &WireMessage::Proof(replayed_proof))
            .await
            .unwrap();

        let response = read_message(&mut replay_recv).await.unwrap();

        assert!(matches!(response, WireMessage::Reject(WireReject::Invalid)));
        assert_eq!(c_inbound.0.lock().unwrap().len(), 1);
        drop(replay_connection);
        c_router.shutdown().await.ok();
        a_endpoint.close().await;
    }

    #[tokio::test]
    async fn removed_member_is_rejected_by_the_current_member_set() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let a_view = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let c_view_after_removal = Arc::new(HashMap::from([(c_identity.device_id, [0xc1; 32])]));
        let a_adapter = IrohMembershipAttestationAdapter::new(
            a_endpoint.clone(),
            Arc::new(FixedIdentity(a_identity)),
            signatures("device-a", a_view),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_adapter = IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity.clone())),
            signatures("device-c", c_view_after_removal),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_inbound = Arc::new(RecordingEndpoint::default());
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(c_inbound.clone()),
            )
            .spawn();

        let result = a_adapter
            .attest_candidate(&candidate(
                &c_identity,
                postcard::to_stdvec(&c_endpoint.addr()).unwrap(),
            ))
            .await;

        assert_eq!(result, Err(MembershipAttestationError::Rejected));
        assert!(c_inbound.0.lock().unwrap().is_empty());
        c_router.shutdown().await.ok();
        a_endpoint.close().await;
    }

    #[tokio::test]
    async fn device_id_impersonation_is_rejected_without_persisting_the_peer() {
        let a_seed = [0x41; 32];
        let c_seed = [0x43; 32];
        let a_endpoint = endpoint(a_seed).await;
        let c_endpoint = endpoint(c_seed).await;
        wait_for_direct_addrs(&a_endpoint).await;
        wait_for_direct_addrs(&c_endpoint).await;
        let a_identity = identity(a_seed, "device-a");
        let c_identity = identity(c_seed, "device-c");
        let secrets = Arc::new(HashMap::from([
            (a_identity.device_id, [0xa1; 32]),
            (DeviceId::new("device-b"), [0xb1; 32]),
            (c_identity.device_id, [0xc1; 32]),
        ]));
        let c_adapter = IrohMembershipAttestationAdapter::new(
            c_endpoint.clone(),
            Arc::new(FixedIdentity(c_identity.clone())),
            signatures("device-c", secrets),
            Arc::new(Sha256IdentityFingerprintFactory),
        );
        let c_inbound = Arc::new(RecordingEndpoint::default());
        let c_router = iroh::protocol::Router::builder((*c_endpoint).clone())
            .accept(
                MEMBERSHIP_ATTESTATION_ALPN,
                c_adapter.handler(c_inbound.clone()),
            )
            .spawn();
        let (connection, mut send, mut recv) = raw_connection(&a_endpoint, &c_endpoint).await;
        let mut hello = wire_hello(&a_endpoint, &a_identity, &c_identity).await;
        hello.source_device_id = "device-b".to_owned();
        write_message(&mut send, &WireMessage::Hello(hello.clone()))
            .await
            .unwrap();
        let challenge = match read_message(&mut recv).await.unwrap() {
            WireMessage::Challenge(challenge) => challenge,
            _ => panic!("expected membership challenge"),
        };
        let mut claimed_identity = a_identity.clone();
        claimed_identity.device_id = DeviceId::new("device-b");
        let forged_proof = proof_for_challenge(&hello, &challenge, &claimed_identity, &[0xa1; 32]);
        write_message(&mut send, &WireMessage::Proof(forged_proof))
            .await
            .unwrap();

        let response = read_message(&mut recv).await.unwrap();

        assert!(matches!(response, WireMessage::Reject(WireReject::Invalid)));
        assert!(c_inbound.0.lock().unwrap().is_empty());
        drop(connection);
        c_router.shutdown().await.ok();
        a_endpoint.close().await;
    }
}
