//! Restricted completion recovery for a committed workspace admission.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uc_application::deps::{
    AdmissionCompletionRecoveryEndpointPort, AdmissionCompletionRecoveryPort,
    AdmissionCompletionRecoveryTransportError,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionCompletionRecoveryChallenge, AdmissionCompletionRecoveryHello,
    AdmissionCompletionRecoveryResponseV1, AdmissionCompletionRecoveryTransportBinding,
};
use uc_core::pairing::DurableAdmissionFrame;
use uc_core::ports::PeerAddressRepositoryPort;

use super::connect_with_staggered_retry;

pub const ADMISSION_COMPLETION_RECOVERY_ALPN: &[u8] = b"uniclipboard/workspace-admission-resume/1";

const WIRE_VERSION: u8 = 1;
const MAX_FRAME_SIZE: usize = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Serialize, Deserialize)]
enum RecoveryRequest {
    Hello {
        hello: AdmissionCompletionRecoveryHello,
        joiner_last_message_id: [u8; 32],
    },
    Response {
        hello: AdmissionCompletionRecoveryHello,
        response: AdmissionCompletionRecoveryResponseV1,
    },
}

#[derive(Debug, Serialize, Deserialize)]
enum RecoveryReply {
    Challenge(AdmissionCompletionRecoveryChallenge),
    Complete(WireDurableAdmissionFrame),
    Rejected,
}

#[derive(Debug, Serialize, Deserialize)]
struct WireDurableAdmissionFrame {
    attempt_id: [u8; 32],
    message_id: [u8; 32],
    predecessor_message_id: Option<[u8; 32]>,
    payload: Vec<u8>,
}

impl From<DurableAdmissionFrame> for WireDurableAdmissionFrame {
    fn from(frame: DurableAdmissionFrame) -> Self {
        Self {
            attempt_id: frame.attempt_id,
            message_id: frame.message_id,
            predecessor_message_id: frame.predecessor_message_id,
            payload: frame.payload,
        }
    }
}

impl From<WireDurableAdmissionFrame> for DurableAdmissionFrame {
    fn from(frame: WireDurableAdmissionFrame) -> Self {
        Self {
            attempt_id: frame.attempt_id,
            kind: uc_core::pairing::DurableAdmissionMessageKind::Complete,
            message_id: frame.message_id,
            predecessor_message_id: frame.predecessor_message_id,
            payload: frame.payload,
        }
    }
}

pub struct IrohAdmissionCompletionRecoveryAdapter {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
}

impl IrohAdmissionCompletionRecoveryAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
        }
    }

    pub fn handler(
        &self,
        endpoint: Arc<dyn AdmissionCompletionRecoveryEndpointPort>,
    ) -> IrohAdmissionCompletionRecoveryHandler {
        IrohAdmissionCompletionRecoveryHandler {
            local_endpoint: Arc::clone(&self.endpoint),
            endpoint,
        }
    }

    async fn resolve_addr(&self, recipient: &DeviceId, route: &[u8]) -> Option<EndpointAddr> {
        if let Ok(address) = postcard::from_bytes(route) {
            return Some(address);
        }
        self.peer_addr_repo
            .get(recipient)
            .await
            .ok()
            .flatten()
            .and_then(|record| postcard::from_bytes(&record.addr_blob).ok())
    }

    async fn exchange(
        &self,
        recipient: &DeviceId,
        route: &[u8],
        request: RecoveryRequest,
    ) -> Result<RecoveryReply, AdmissionCompletionRecoveryTransportError> {
        let address = self
            .resolve_addr(recipient, route)
            .await
            .ok_or(AdmissionCompletionRecoveryTransportError::Offline)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            address,
            ADMISSION_COMPLETION_RECOVERY_ALPN,
            "admission-completion-recovery",
        )
        .await
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Offline)?;
        let expected_binding = AdmissionCompletionRecoveryTransportBinding {
            joiner_transport_identity_digest: Sha256::digest(self.endpoint.id().as_bytes()).into(),
            helper_transport_identity_digest: Sha256::digest(connection.remote_id().as_bytes())
                .into(),
        };
        let (mut send, mut receive) = tokio::time::timeout(IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)?
            .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)?;
        write_frame(&mut send, &request).await?;
        let reply: RecoveryReply = read_frame(&mut receive).await?;
        if let RecoveryReply::Challenge(challenge) = &reply {
            verify_challenge_transport_binding(challenge.transport_binding, expected_binding)?;
        }
        Ok(reply)
    }
}

#[async_trait]
impl AdmissionCompletionRecoveryPort for IrohAdmissionCompletionRecoveryAdapter {
    async fn request_completion_recovery_challenge(
        &self,
        helper: &DeviceId,
        route: &[u8],
        hello: AdmissionCompletionRecoveryHello,
        joiner_last_message_id: [u8; 32],
    ) -> Result<AdmissionCompletionRecoveryChallenge, AdmissionCompletionRecoveryTransportError>
    {
        match self
            .exchange(
                helper,
                route,
                RecoveryRequest::Hello {
                    hello,
                    joiner_last_message_id,
                },
            )
            .await?
        {
            RecoveryReply::Challenge(challenge) => Ok(challenge),
            RecoveryReply::Complete(_) | RecoveryReply::Rejected => {
                Err(AdmissionCompletionRecoveryTransportError::Rejected)
            }
        }
    }

    async fn submit_completion_recovery_response(
        &self,
        helper: &DeviceId,
        route: &[u8],
        hello: AdmissionCompletionRecoveryHello,
        response: AdmissionCompletionRecoveryResponseV1,
    ) -> Result<DurableAdmissionFrame, AdmissionCompletionRecoveryTransportError> {
        match self
            .exchange(helper, route, RecoveryRequest::Response { hello, response })
            .await?
        {
            RecoveryReply::Complete(frame) => Ok(frame.into()),
            RecoveryReply::Challenge(_) | RecoveryReply::Rejected => {
                Err(AdmissionCompletionRecoveryTransportError::Rejected)
            }
        }
    }
}

#[derive(Clone)]
pub struct IrohAdmissionCompletionRecoveryHandler {
    local_endpoint: Arc<Endpoint>,
    endpoint: Arc<dyn AdmissionCompletionRecoveryEndpointPort>,
}

impl std::fmt::Debug for IrohAdmissionCompletionRecoveryHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohAdmissionCompletionRecoveryHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohAdmissionCompletionRecoveryHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let binding = transport_binding(&self.local_endpoint, &connection);
        let (mut send, mut receive) =
            match tokio::time::timeout(IO_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                _ => return Ok(()),
            };
        let request: RecoveryRequest = match read_frame(&mut receive).await {
            Ok(request) => request,
            Err(_) => return Ok(()),
        };
        let reply = match request {
            RecoveryRequest::Hello {
                hello,
                joiner_last_message_id,
            } => {
                let helper_last_message_id = recovery_message_id(&hello, joiner_last_message_id);
                match self
                    .endpoint
                    .handle_completion_recovery_hello(
                        hello,
                        binding,
                        joiner_last_message_id,
                        helper_last_message_id,
                    )
                    .await
                {
                    Ok(challenge) => RecoveryReply::Challenge(challenge),
                    Err(_) => RecoveryReply::Rejected,
                }
            }
            RecoveryRequest::Response { hello, response } => match self
                .endpoint
                .handle_completion_recovery_response(hello, response, binding)
                .await
            {
                Ok(frame) => RecoveryReply::Complete(frame.into()),
                Err(_) => RecoveryReply::Rejected,
            },
        };
        let _ = write_frame(&mut send, &reply).await;
        Ok(())
    }
}

fn transport_binding(
    local_endpoint: &Endpoint,
    connection: &Connection,
) -> AdmissionCompletionRecoveryTransportBinding {
    AdmissionCompletionRecoveryTransportBinding {
        joiner_transport_identity_digest: Sha256::digest(connection.remote_id().as_bytes()).into(),
        helper_transport_identity_digest: Sha256::digest(local_endpoint.id().as_bytes()).into(),
    }
}

fn recovery_message_id(
    hello: &AdmissionCompletionRecoveryHello,
    joiner_last_message_id: [u8; 32],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-completion-recovery-message/v1\0");
    hasher.update(hello.digest());
    hasher.update(joiner_last_message_id);
    hasher.finalize().into()
}

async fn write_frame<T: Serialize>(
    send: &mut iroh::endpoint::SendStream,
    value: &T,
) -> Result<(), AdmissionCompletionRecoveryTransportError> {
    let payload = encode_frame_payload(value)?;
    let length = u32::try_from(payload.len())
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)?;
    send.write_all(&length.to_be_bytes())
        .await
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)?;
    send.write_all(&payload)
        .await
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)?;
    send.finish()
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)
}

fn encode_frame_payload<T: Serialize>(
    value: &T,
) -> Result<Vec<u8>, AdmissionCompletionRecoveryTransportError> {
    let body = postcard::to_stdvec(value)
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)?;
    let encoded_len = body
        .len()
        .checked_add(1)
        .ok_or(AdmissionCompletionRecoveryTransportError::Transport)?;
    if body.is_empty() || encoded_len > MAX_FRAME_SIZE {
        return Err(AdmissionCompletionRecoveryTransportError::Transport);
    }
    let mut payload = Vec::with_capacity(encoded_len);
    payload.push(WIRE_VERSION);
    payload.extend_from_slice(&body);
    Ok(payload)
}

fn decode_frame_payload<T: for<'de> Deserialize<'de>>(
    payload: &[u8],
) -> Result<T, AdmissionCompletionRecoveryTransportError> {
    if !(2..=MAX_FRAME_SIZE).contains(&payload.len()) {
        return Err(AdmissionCompletionRecoveryTransportError::Transport);
    }
    if payload[0] != WIRE_VERSION {
        return Err(AdmissionCompletionRecoveryTransportError::Rejected);
    }
    postcard::from_bytes(&payload[1..])
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Rejected)
}

fn verify_challenge_transport_binding(
    challenge_binding: AdmissionCompletionRecoveryTransportBinding,
    expected_binding: AdmissionCompletionRecoveryTransportBinding,
) -> Result<(), AdmissionCompletionRecoveryTransportError> {
    if challenge_binding == expected_binding {
        Ok(())
    } else {
        Err(AdmissionCompletionRecoveryTransportError::Rejected)
    }
}

async fn read_frame<T: for<'de> Deserialize<'de>>(
    receive: &mut iroh::endpoint::RecvStream,
) -> Result<T, AdmissionCompletionRecoveryTransportError> {
    let mut length = [0u8; 4];
    receive
        .read_exact(&mut length)
        .await
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)?;
    let length = u32::from_be_bytes(length) as usize;
    if !(2..=MAX_FRAME_SIZE).contains(&length) {
        return Err(AdmissionCompletionRecoveryTransportError::Transport);
    }
    let mut payload = vec![0u8; length];
    receive
        .read_exact(&mut payload)
        .await
        .map_err(|_| AdmissionCompletionRecoveryTransportError::Transport)?;
    decode_frame_payload(&payload)
}

#[cfg(test)]
mod tests {
    use super::{
        decode_frame_payload, encode_frame_payload, verify_challenge_transport_binding,
        AdmissionCompletionRecoveryTransportBinding, AdmissionCompletionRecoveryTransportError,
        MAX_FRAME_SIZE,
    };

    #[test]
    fn supported_wire_frame_round_trips() {
        let encoded = encode_frame_payload(&42u8).unwrap();

        assert_eq!(decode_frame_payload::<u8>(&encoded).unwrap(), 42);
    }

    #[test]
    fn wire_frame_larger_than_four_mib_is_rejected() {
        let oversized = vec![0u8; MAX_FRAME_SIZE];

        assert_eq!(
            encode_frame_payload(&oversized),
            Err(AdmissionCompletionRecoveryTransportError::Transport)
        );
    }

    #[test]
    fn unknown_wire_version_is_rejected() {
        let mut encoded = encode_frame_payload(&42u8).unwrap();
        encoded[0] = 2;

        assert_eq!(
            decode_frame_payload::<u8>(&encoded),
            Err(AdmissionCompletionRecoveryTransportError::Rejected)
        );
    }

    #[test]
    fn client_rejects_challenge_bound_to_other_transport_identities() {
        let actual_connection = AdmissionCompletionRecoveryTransportBinding {
            joiner_transport_identity_digest: [0x11; 32],
            helper_transport_identity_digest: [0x22; 32],
        };
        let challenge_binding = AdmissionCompletionRecoveryTransportBinding {
            joiner_transport_identity_digest: [0x11; 32],
            helper_transport_identity_digest: [0x33; 32],
        };

        assert_eq!(
            verify_challenge_transport_binding(challenge_binding, actual_connection),
            Err(AdmissionCompletionRecoveryTransportError::Rejected)
        );
    }
}
