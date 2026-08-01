use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, instrument, warn};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    LegacyUpgradeDescriptor, LegacyUpgradeDispatchError, LegacyUpgradeDispatchPort,
    LegacyUpgradeEndpointPort, LegacyUpgradeId, LegacyUpgradeRequest, LegacyUpgradeResponse,
    LegacyUpgradeResponseKind, MemberRepositoryPort, ProtectionGroupAdmission, ProtectionGroupId,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;
use uc_core::space_access::GroupAdmission;

use super::connect_with_staggered_retry;

const WIRE_VERSION: u8 = 1;
pub const LEGACY_UPGRADE_ALPN: &[u8] = b"uniclipboard/legacy-upgrade/1";
const MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const RESPONSE_ACCEPTED: u8 = 1;
const RESPONSE_REJECTED: u8 = 2;

#[derive(Serialize, Deserialize)]
struct WireEnvelope<T> {
    version: u8,
    body: T,
}

#[derive(Serialize, Deserialize)]
struct WireDescriptor {
    upgrade_id: [u8; 32],
    protection_group_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct WireRequest {
    source_device_id: String,
    target_device_id: String,
    descriptor: WireDescriptor,
    key_package: Vec<u8>,
    proof: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct WireResponse {
    descriptor: WireDescriptor,
    kind: WireResponseKind,
}

#[derive(Serialize, Deserialize)]
enum WireResponseKind {
    UpToDate,
    Retry,
    Admission {
        protection_group_id: String,
        welcome: Vec<u8>,
        encrypted_key_catalog: Vec<u8>,
        group_epoch: u64,
    },
    Rejected,
}

#[derive(Debug, Error)]
enum LegacyUpgradeWireError {
    #[error("legacy upgrade wire codec failed")]
    Codec,

    #[error("legacy upgrade wire version is unsupported")]
    Version,

    #[error("legacy upgrade wire value is invalid")]
    InvalidValue,
}

fn encode_request(request: &LegacyUpgradeRequest) -> Result<Vec<u8>, LegacyUpgradeWireError> {
    postcard::to_allocvec(&WireEnvelope {
        version: WIRE_VERSION,
        body: WireRequest {
            source_device_id: request.source_device_id().as_str().to_owned(),
            target_device_id: request.target_device_id().as_str().to_owned(),
            descriptor: descriptor_to_wire(request.descriptor()),
            key_package: request.key_package().to_vec(),
            proof: request.proof().to_vec(),
        },
    })
    .map_err(|_| LegacyUpgradeWireError::Codec)
}

fn decode_request(bytes: &[u8]) -> Result<LegacyUpgradeRequest, LegacyUpgradeWireError> {
    let envelope: WireEnvelope<WireRequest> =
        postcard::from_bytes(bytes).map_err(|_| LegacyUpgradeWireError::Codec)?;
    if envelope.version != WIRE_VERSION {
        return Err(LegacyUpgradeWireError::Version);
    }
    let body = envelope.body;
    Ok(LegacyUpgradeRequest::unsigned(
        DeviceId::try_new(body.source_device_id).ok_or(LegacyUpgradeWireError::InvalidValue)?,
        DeviceId::try_new(body.target_device_id).ok_or(LegacyUpgradeWireError::InvalidValue)?,
        descriptor_from_wire(body.descriptor)?,
        body.key_package,
    )
    .with_proof(body.proof))
}

fn encode_response(response: &LegacyUpgradeResponse) -> Result<Vec<u8>, LegacyUpgradeWireError> {
    let kind = match &response.kind {
        LegacyUpgradeResponseKind::UpToDate => WireResponseKind::UpToDate,
        LegacyUpgradeResponseKind::Retry => WireResponseKind::Retry,
        LegacyUpgradeResponseKind::Rejected => WireResponseKind::Rejected,
        LegacyUpgradeResponseKind::Admission(admission) => WireResponseKind::Admission {
            protection_group_id: admission.protection_group_id.as_str().to_owned(),
            welcome: admission.admission.welcome.clone(),
            encrypted_key_catalog: admission.admission.encrypted_key_catalog.clone(),
            group_epoch: admission.admission.group_epoch,
        },
    };
    postcard::to_allocvec(&WireEnvelope {
        version: WIRE_VERSION,
        body: WireResponse {
            descriptor: descriptor_to_wire(&response.descriptor),
            kind,
        },
    })
    .map_err(|_| LegacyUpgradeWireError::Codec)
}

fn decode_response(bytes: &[u8]) -> Result<LegacyUpgradeResponse, LegacyUpgradeWireError> {
    let envelope: WireEnvelope<WireResponse> =
        postcard::from_bytes(bytes).map_err(|_| LegacyUpgradeWireError::Codec)?;
    if envelope.version != WIRE_VERSION {
        return Err(LegacyUpgradeWireError::Version);
    }
    let body = envelope.body;
    let kind = match body.kind {
        WireResponseKind::UpToDate => LegacyUpgradeResponseKind::UpToDate,
        WireResponseKind::Retry => LegacyUpgradeResponseKind::Retry,
        WireResponseKind::Rejected => LegacyUpgradeResponseKind::Rejected,
        WireResponseKind::Admission {
            protection_group_id,
            welcome,
            encrypted_key_catalog,
            group_epoch,
        } => LegacyUpgradeResponseKind::Admission(ProtectionGroupAdmission {
            protection_group_id: ProtectionGroupId::from_string(protection_group_id)
                .map_err(|_| LegacyUpgradeWireError::InvalidValue)?,
            admission: GroupAdmission {
                welcome,
                encrypted_key_catalog,
                existing_member_updates: Vec::new(),
                group_epoch,
            },
        }),
    };
    Ok(LegacyUpgradeResponse {
        descriptor: descriptor_from_wire(body.descriptor)?,
        kind,
    })
}

fn descriptor_to_wire(descriptor: &LegacyUpgradeDescriptor) -> WireDescriptor {
    WireDescriptor {
        upgrade_id: *descriptor.upgrade_id().as_bytes(),
        protection_group_id: descriptor
            .protection_group_id()
            .map(|id| id.as_str().to_owned()),
    }
}

fn descriptor_from_wire(
    descriptor: WireDescriptor,
) -> Result<LegacyUpgradeDescriptor, LegacyUpgradeWireError> {
    let upgrade_id = LegacyUpgradeId::from_bytes(descriptor.upgrade_id);
    match descriptor.protection_group_id {
        Some(group_id) => Ok(LegacyUpgradeDescriptor::ready(
            upgrade_id,
            ProtectionGroupId::from_string(group_id)
                .map_err(|_| LegacyUpgradeWireError::InvalidValue)?,
        )),
        None => Ok(LegacyUpgradeDescriptor::legacy(upgrade_id)),
    }
}

async fn run_io<T, E>(
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, LegacyUpgradeDispatchError> {
    run_io_with_timeout(IO_TIMEOUT, future).await
}

async fn run_io_with_timeout<T, E>(
    timeout: Duration,
    future: impl Future<Output = Result<T, E>>,
) -> Result<T, LegacyUpgradeDispatchError> {
    tokio::time::timeout(timeout, future)
        .await
        .map_err(|_| LegacyUpgradeDispatchError::Transport)?
        .map_err(|_| LegacyUpgradeDispatchError::Transport)
}

pub struct IrohLegacyUpgradeAdapter {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    handler_state: Arc<HandlerState>,
}

struct HandlerState {
    member_repo: Arc<dyn MemberRepositoryPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
}

impl IrohLegacyUpgradeAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
        member_repo: Arc<dyn MemberRepositoryPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
            handler_state: Arc::new(HandlerState {
                member_repo,
                fingerprint_factory,
            }),
        }
    }

    pub fn handler(
        &self,
        upgrade_endpoint: Arc<dyn LegacyUpgradeEndpointPort>,
    ) -> IrohLegacyUpgradeHandler {
        IrohLegacyUpgradeHandler {
            state: Arc::clone(&self.handler_state),
            endpoint: upgrade_endpoint,
        }
    }

    async fn resolve_addr(&self, peer: &DeviceId) -> Option<EndpointAddr> {
        match self.peer_addr_repo.get(peer).await {
            Ok(Some(record)) => postcard::from_bytes(&record.addr_blob).ok(),
            Ok(None) => None,
            Err(error) => {
                warn!(error = %error, device_id = %peer, "legacy upgrade address lookup failed");
                None
            }
        }
    }
}

#[async_trait]
impl LegacyUpgradeDispatchPort for IrohLegacyUpgradeAdapter {
    #[instrument(
        name = "legacy_upgrade.exchange",
        level = "debug",
        skip_all,
        fields(device_id = %peer)
    )]
    async fn exchange_legacy_upgrade(
        &self,
        peer: &DeviceId,
        request: &LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeResponse, LegacyUpgradeDispatchError> {
        let payload = encode_request(request).map_err(|_| LegacyUpgradeDispatchError::Transport)?;
        if payload.is_empty() || payload.len() > MAX_MESSAGE_SIZE {
            return Err(LegacyUpgradeDispatchError::Transport);
        }
        let addr = self
            .resolve_addr(peer)
            .await
            .ok_or(LegacyUpgradeDispatchError::Offline)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            addr,
            LEGACY_UPGRADE_ALPN,
            "legacy-upgrade",
        )
        .await
        .map_err(|_| LegacyUpgradeDispatchError::Offline)?;
        let (mut send, mut recv) = run_io(connection.open_bi()).await?;
        let length =
            u32::try_from(payload.len()).map_err(|_| LegacyUpgradeDispatchError::Transport)?;
        run_io(send.write_all(&length.to_be_bytes())).await?;
        run_io(send.write_all(&payload)).await?;
        send.finish()
            .map_err(|_| LegacyUpgradeDispatchError::Transport)?;

        let mut status = [0u8; 1];
        run_io(recv.read_exact(&mut status)).await?;
        if status[0] == RESPONSE_REJECTED {
            debug!("legacy upgrade exchange was rejected by peer");
            return Err(LegacyUpgradeDispatchError::Rejected);
        }
        if status[0] != RESPONSE_ACCEPTED {
            return Err(LegacyUpgradeDispatchError::Transport);
        }
        let payload = read_frame(&mut recv).await?;
        let response =
            decode_response(&payload).map_err(|_| LegacyUpgradeDispatchError::Transport)?;
        debug!("legacy upgrade exchange completed");
        Ok(response)
    }
}

#[derive(Clone)]
pub struct IrohLegacyUpgradeHandler {
    state: Arc<HandlerState>,
    endpoint: Arc<dyn LegacyUpgradeEndpointPort>,
}

impl std::fmt::Debug for IrohLegacyUpgradeHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohLegacyUpgradeHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohLegacyUpgradeHandler {
    #[instrument(name = "legacy_upgrade.accept", level = "debug", skip_all)]
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = self
            .state
            .resolve_device(connection.remote_id().as_bytes())
            .await;
        let (mut send, mut recv) =
            match tokio::time::timeout(IO_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                Ok(Err(error)) => {
                    debug!(error = %error, "legacy upgrade stream accept failed");
                    return Ok(());
                }
                Err(_) => {
                    debug!("legacy upgrade stream accept timed out");
                    return Ok(());
                }
            };
        let Some(peer) = peer else {
            emit_rejected(&mut send).await;
            warn!(
                reason = "unknown_identity",
                "legacy upgrade connection rejected"
            );
            let _ = connection.closed().await;
            return Ok(());
        };
        let payload = match read_frame(&mut recv).await {
            Ok(payload) => payload,
            Err(_) => {
                emit_rejected(&mut send).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        let request = match decode_request(&payload) {
            Ok(request) => request,
            Err(_) => {
                emit_rejected(&mut send).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        let response = match self
            .endpoint
            .handle_legacy_upgrade_request(&peer, request)
            .await
        {
            Ok(response) => response,
            Err(error) => {
                warn!(error_kind = "upgrade_request_rejected", error = %error, retryable = false, device_id = %peer, "legacy upgrade request failed");
                emit_rejected(&mut send).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        let response = match encode_response(&response) {
            Ok(response) if response.len() <= MAX_MESSAGE_SIZE => response,
            _ => {
                emit_rejected(&mut send).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        if emit_response(&mut send, &response).await.is_ok() {
            debug!(device_id = %peer, "legacy upgrade response sent");
        }
        let _ = connection.closed().await;
        Ok(())
    }
}

impl HandlerState {
    async fn resolve_device(&self, public_key: &[u8; 32]) -> Option<DeviceId> {
        let fingerprint = self.fingerprint_factory.from_public_key(public_key).ok()?;
        let members = match self.member_repo.list().await {
            Ok(members) => members,
            Err(error) => {
                warn!(error = %error, "legacy upgrade member lookup failed");
                return None;
            }
        };
        members
            .into_iter()
            .find(|member| member.identity_fingerprint == fingerprint)
            .map(|member| member.device_id)
    }
}

async fn read_frame(
    recv: &mut iroh::endpoint::RecvStream,
) -> Result<Vec<u8>, LegacyUpgradeDispatchError> {
    let mut length = [0u8; 4];
    run_io(recv.read_exact(&mut length)).await?;
    let length = checked_frame_length(length)?;
    let mut payload = vec![0u8; length];
    run_io(recv.read_exact(&mut payload)).await?;
    Ok(payload)
}

fn checked_frame_length(length: [u8; 4]) -> Result<usize, LegacyUpgradeDispatchError> {
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_MESSAGE_SIZE {
        return Err(LegacyUpgradeDispatchError::Transport);
    }
    Ok(length)
}

async fn emit_response(
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), LegacyUpgradeDispatchError> {
    let length = u32::try_from(payload.len()).map_err(|_| LegacyUpgradeDispatchError::Transport)?;
    run_io(send.write_all(&[RESPONSE_ACCEPTED])).await?;
    run_io(send.write_all(&length.to_be_bytes())).await?;
    run_io(send.write_all(payload)).await?;
    send.finish()
        .map_err(|_| LegacyUpgradeDispatchError::Transport)
}

async fn emit_rejected(send: &mut iroh::endpoint::SendStream) {
    if matches!(run_io(send.write_all(&[RESPONSE_REJECTED])).await, Ok(())) {
        let _ = send.finish();
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::Utc;
    use iroh::{RelayMode, SecretKey};
    use uc_core::membership::{LegacyUpgradeError, MembershipError, SpaceMember};
    use uc_core::MemberSyncPreferences;

    use super::*;
    use crate::security::Sha256IdentityFingerprintFactory;

    struct StaticMembers(Vec<SpaceMember>);

    #[async_trait]
    impl MemberRepositoryPort for StaticMembers {
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

    #[derive(Default)]
    struct CountingApplicationEndpoint {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl LegacyUpgradeEndpointPort for CountingApplicationEndpoint {
        async fn handle_legacy_upgrade_request(
            &self,
            _authenticated_peer: &DeviceId,
            request: LegacyUpgradeRequest,
        ) -> Result<LegacyUpgradeResponse, LegacyUpgradeError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(LegacyUpgradeResponse {
                descriptor: request.descriptor().clone(),
                kind: LegacyUpgradeResponseKind::UpToDate,
            })
        }
    }

    async fn endpoint(seed: [u8; 32]) -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(SecretKey::from_bytes(&seed))
                .alpns(vec![LEGACY_UPGRADE_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .unwrap(),
        )
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

    fn member_for(seed: [u8; 32], device_id: &str) -> SpaceMember {
        let key = SecretKey::from_bytes(&seed);
        let identity_fingerprint = Sha256IdentityFingerprintFactory
            .from_public_key(key.public().as_bytes())
            .unwrap();
        SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: "Test Device".to_owned(),
            identity_fingerprint,
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        }
    }

    fn handler(
        members: Vec<SpaceMember>,
        endpoint: Arc<CountingApplicationEndpoint>,
    ) -> IrohLegacyUpgradeHandler {
        IrohLegacyUpgradeHandler {
            state: Arc::new(HandlerState {
                member_repo: Arc::new(StaticMembers(members)),
                fingerprint_factory: Arc::new(Sha256IdentityFingerprintFactory),
            }),
            endpoint,
        }
    }

    #[test]
    fn request_and_admission_response_round_trip_without_exposing_private_join_state() {
        let descriptor = LegacyUpgradeDescriptor::ready(
            LegacyUpgradeId::from_bytes([7; 32]),
            ProtectionGroupId::from_string("group-a").unwrap(),
        );
        let request = LegacyUpgradeRequest::unsigned(
            DeviceId::new("device-a"),
            DeviceId::new("device-b"),
            descriptor.clone(),
            vec![1, 2, 3],
        )
        .with_proof(vec![4, 5, 6]);
        let response = LegacyUpgradeResponse {
            descriptor,
            kind: LegacyUpgradeResponseKind::Admission(ProtectionGroupAdmission {
                protection_group_id: ProtectionGroupId::from_string("group-a").unwrap(),
                admission: GroupAdmission {
                    welcome: vec![7, 8],
                    encrypted_key_catalog: vec![9, 10],
                    existing_member_updates: Vec::new(),
                    group_epoch: 2,
                },
            }),
        };

        let request_bytes = encode_request(&request).unwrap();
        let response_bytes = encode_response(&response).unwrap();
        assert_eq!(
            hex::encode(&request_bytes),
            concat!(
                "01086465766963652d61086465766963652d62",
                "0707070707070707070707070707070707070707070707070707070707070707",
                "010767726f75702d610301020303040506"
            )
        );
        assert_eq!(
            hex::encode(&response_bytes),
            concat!(
                "010707070707070707070707070707070707070707070707070707070707070707",
                "010767726f75702d61020767726f75702d6102070802090a02"
            )
        );
        let decoded_request = decode_request(&request_bytes).unwrap();
        let decoded_response = decode_response(&response_bytes).unwrap();

        assert_eq!(decoded_request, request);
        assert_eq!(decoded_response, response);
    }

    #[test]
    fn unsupported_wire_version_is_rejected() {
        let request = LegacyUpgradeRequest::unsigned(
            DeviceId::new("device-a"),
            DeviceId::new("device-b"),
            LegacyUpgradeDescriptor::legacy(LegacyUpgradeId::from_bytes([7; 32])),
            vec![1, 2, 3],
        );
        let mut bytes = encode_request(&request).unwrap();
        bytes[0] = WIRE_VERSION + 1;

        assert!(matches!(
            decode_request(&bytes),
            Err(LegacyUpgradeWireError::Version)
        ));
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocation() {
        let oversized = u32::try_from(MAX_MESSAGE_SIZE + 1).unwrap();

        assert_eq!(
            checked_frame_length(oversized.to_be_bytes()),
            Err(LegacyUpgradeDispatchError::Transport)
        );
    }

    #[tokio::test]
    async fn stalled_io_maps_to_transport_failure() {
        let error = run_io_with_timeout(
            Duration::from_millis(1),
            pending::<Result<(), std::io::Error>>(),
        )
        .await
        .unwrap_err();

        assert_eq!(error, LegacyUpgradeDispatchError::Transport);
    }

    #[tokio::test]
    async fn known_identity_receives_the_complete_application_response() {
        let sender_seed = [0x29; 32];
        let sender = endpoint(sender_seed).await;
        let receiver = endpoint([0x30; 32]).await;
        wait_for_direct_addrs(&sender).await;
        wait_for_direct_addrs(&receiver).await;
        let application = Arc::new(CountingApplicationEndpoint::default());
        let router = iroh::protocol::Router::builder((*receiver).clone())
            .accept(
                LEGACY_UPGRADE_ALPN,
                handler(
                    vec![member_for(sender_seed, "device-a")],
                    Arc::clone(&application),
                ),
            )
            .spawn();
        let request = LegacyUpgradeRequest::unsigned(
            DeviceId::new("device-a"),
            DeviceId::new("device-b"),
            LegacyUpgradeDescriptor::legacy(LegacyUpgradeId::from_bytes([7; 32])),
            vec![1, 2, 3],
        );
        let payload = encode_request(&request).unwrap();

        let connection = sender
            .connect(receiver.addr(), LEGACY_UPGRADE_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .unwrap();
        send.write_all(&payload).await.unwrap();
        send.finish().unwrap();
        let mut status = [0u8; 1];
        recv.read_exact(&mut status).await.unwrap();
        let response = decode_response(&read_frame(&mut recv).await.unwrap()).unwrap();

        assert_eq!(status[0], RESPONSE_ACCEPTED);
        assert_eq!(response.descriptor, request.descriptor().clone());
        assert_eq!(response.kind, LegacyUpgradeResponseKind::UpToDate);
        assert_eq!(application.calls.load(Ordering::Acquire), 1);
        router.shutdown().await.ok();
        sender.close().await;
    }

    #[tokio::test]
    async fn unknown_identity_is_rejected_before_the_application_endpoint() {
        let sender = endpoint([0x31; 32]).await;
        let receiver = endpoint([0x32; 32]).await;
        wait_for_direct_addrs(&sender).await;
        wait_for_direct_addrs(&receiver).await;
        let application = Arc::new(CountingApplicationEndpoint::default());
        let router = iroh::protocol::Router::builder((*receiver).clone())
            .accept(
                LEGACY_UPGRADE_ALPN,
                handler(Vec::new(), Arc::clone(&application)),
            )
            .spawn();

        let connection = sender
            .connect(receiver.addr(), LEGACY_UPGRADE_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        let payload = encode_request(&LegacyUpgradeRequest::unsigned(
            DeviceId::new("unknown-device"),
            DeviceId::new("device-b"),
            LegacyUpgradeDescriptor::legacy(LegacyUpgradeId::from_bytes([7; 32])),
            vec![1, 2, 3],
        ))
        .unwrap();
        send.write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .unwrap();
        send.write_all(&payload).await.unwrap();
        send.finish().unwrap();
        let mut status = [0u8; 1];
        recv.read_exact(&mut status).await.unwrap();

        assert_eq!(status[0], RESPONSE_REJECTED);
        assert_eq!(application.calls.load(Ordering::Acquire), 0);
        router.shutdown().await.ok();
        sender.close().await;
    }

    #[tokio::test]
    async fn unsupported_wire_version_is_rejected_by_the_network_handler() {
        let sender_seed = [0x33; 32];
        let sender = endpoint(sender_seed).await;
        let receiver = endpoint([0x34; 32]).await;
        wait_for_direct_addrs(&sender).await;
        wait_for_direct_addrs(&receiver).await;
        let application = Arc::new(CountingApplicationEndpoint::default());
        let router = iroh::protocol::Router::builder((*receiver).clone())
            .accept(
                LEGACY_UPGRADE_ALPN,
                handler(
                    vec![member_for(sender_seed, "device-a")],
                    Arc::clone(&application),
                ),
            )
            .spawn();
        let request = LegacyUpgradeRequest::unsigned(
            DeviceId::new("device-a"),
            DeviceId::new("device-b"),
            LegacyUpgradeDescriptor::legacy(LegacyUpgradeId::from_bytes([7; 32])),
            vec![1, 2, 3],
        );
        let mut payload = encode_request(&request).unwrap();
        payload[0] = WIRE_VERSION + 1;

        let connection = sender
            .connect(receiver.addr(), LEGACY_UPGRADE_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(&(payload.len() as u32).to_be_bytes())
            .await
            .unwrap();
        send.write_all(&payload).await.unwrap();
        send.finish().unwrap();
        let mut status = [0u8; 1];
        recv.read_exact(&mut status).await.unwrap();

        assert_eq!(status[0], RESPONSE_REJECTED);
        assert_eq!(application.calls.load(Ordering::Acquire), 0);
        router.shutdown().await.ok();
        sender.close().await;
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_by_the_network_handler_before_allocation() {
        let sender_seed = [0x35; 32];
        let sender = endpoint(sender_seed).await;
        let receiver = endpoint([0x36; 32]).await;
        wait_for_direct_addrs(&sender).await;
        wait_for_direct_addrs(&receiver).await;
        let application = Arc::new(CountingApplicationEndpoint::default());
        let router = iroh::protocol::Router::builder((*receiver).clone())
            .accept(
                LEGACY_UPGRADE_ALPN,
                handler(
                    vec![member_for(sender_seed, "device-a")],
                    Arc::clone(&application),
                ),
            )
            .spawn();
        let oversized = u32::try_from(MAX_MESSAGE_SIZE + 1).unwrap();

        let connection = sender
            .connect(receiver.addr(), LEGACY_UPGRADE_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(&oversized.to_be_bytes()).await.unwrap();
        send.finish().unwrap();
        let mut status = [0u8; 1];
        recv.read_exact(&mut status).await.unwrap();

        assert_eq!(status[0], RESPONSE_REJECTED);
        assert_eq!(application.calls.load(Ordering::Acquire), 0);
        router.shutdown().await.ok();
        sender.close().await;
    }
}
