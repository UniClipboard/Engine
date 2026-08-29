//! Bounded membership-history exchange on authenticated Iroh connections.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use uc_application::deps::{
    RestrictedMembershipDelivery, RestrictedMembershipDeliveryError,
    RestrictedMembershipDeliveryPort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage, MembershipHistoryV2Ack,
    MAX_MEMBERSHIP_HISTORY_FRAME_SIZE,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

use super::connect_with_staggered_retry;

pub const MEMBERSHIP_HISTORY_EXCHANGE_ALPN: &[u8] = b"uniclipboard/membership-history/2";

const WIRE_VERSION: u8 = 2;
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPTED: u8 = 1;
const REJECTED: u8 = 2;

pub struct IrohMembershipHistoryExchangeAdapter {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
}

impl IrohMembershipHistoryExchangeAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
        }
    }

    pub(crate) fn handler(
        &self,
        member_repo: Arc<dyn MemberRepositoryPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        endpoint: Arc<dyn MembershipHistoryExchangeEndpointPort>,
    ) -> IrohMembershipHistoryExchangeHandler {
        IrohMembershipHistoryExchangeHandler {
            state: Arc::new(HandlerState {
                member_repo,
                fingerprint_factory,
                endpoint,
            }),
        }
    }

    async fn resolve_addr(&self, recipient: &DeviceId) -> Option<EndpointAddr> {
        self.peer_addr_repo
            .get(recipient)
            .await
            .ok()
            .flatten()
            .and_then(|record| postcard::from_bytes(&record.addr_blob).ok())
    }
}

#[async_trait]
impl MembershipHistoryExchangePort for IrohMembershipHistoryExchangeAdapter {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        let payload = encode_message(&message)?;
        let address = self
            .resolve_addr(recipient)
            .await
            .ok_or(MembershipHistoryExchangeError::Offline)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            address,
            MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
            "membership-history",
        )
        .await
        .map_err(|_| MembershipHistoryExchangeError::Offline)?;
        let (mut send, mut receive) = tokio::time::timeout(IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|_| MembershipHistoryExchangeError::Transport)?
            .map_err(|_| MembershipHistoryExchangeError::Transport)?;
        write_message(&mut send, &payload).await?;
        let accepted = read_byte(&mut receive).await?;
        if accepted == REJECTED {
            return Err(MembershipHistoryExchangeError::Rejected);
        }
        if accepted != ACCEPTED {
            return Err(MembershipHistoryExchangeError::Transport);
        }
        let response = read_message(&mut receive).await?;
        decode_message(&response)
    }
}

#[async_trait]
impl RestrictedMembershipDeliveryPort for IrohMembershipHistoryExchangeAdapter {
    async fn deliver_restricted_membership(
        &self,
        peer: &DeviceId,
        delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        let message = match delivery {
            RestrictedMembershipDelivery::Event(event) => {
                MembershipHistoryMessage::RestrictedEventV2(event.clone())
            }
            RestrictedMembershipDelivery::Decision(decision) => {
                MembershipHistoryMessage::RestrictedDecisionV2(decision.clone())
            }
        };
        match self.exchange_membership_history(peer, message).await {
            Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Consistent | MembershipHistoryV2Ack::UpdatesApplied,
            )) => Ok(()),
            Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid | MembershipHistoryV2Ack::Diverged,
            ))
            | Ok(MembershipHistoryMessage::HistoryPageV2(_))
            | Ok(MembershipHistoryMessage::RestrictedEventV2(_))
            | Ok(MembershipHistoryMessage::RestrictedDecisionV2(_)) => {
                Err(RestrictedMembershipDeliveryError::Rejected)
            }
            Ok(MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Continue { .. }))
            | Err(MembershipHistoryExchangeError::Offline)
            | Err(MembershipHistoryExchangeError::Transport) => {
                Err(RestrictedMembershipDeliveryError::Deferred)
            }
            Err(MembershipHistoryExchangeError::Rejected) => {
                Err(RestrictedMembershipDeliveryError::Rejected)
            }
        }
    }
}

#[derive(Clone)]
pub struct IrohMembershipHistoryExchangeHandler {
    state: Arc<HandlerState>,
}

impl std::fmt::Debug for IrohMembershipHistoryExchangeHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohMembershipHistoryExchangeHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohMembershipHistoryExchangeHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut receive) =
            match tokio::time::timeout(IO_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                _ => return Ok(()),
            };
        let message = match read_message(&mut receive).await {
            Ok(message) => message,
            Err(_) => {
                reject(&mut send).await;
                return Ok(());
            }
        };
        let message: MembershipHistoryMessage = match decode_message(&message) {
            Ok(message) => message,
            Err(_) => {
                reject(&mut send).await;
                return Ok(());
            }
        };
        let Some(source_device) = self
            .resolve_source_device(connection.remote_id().as_bytes(), &message)
            .await
        else {
            reject(&mut send).await;
            return Ok(());
        };
        let response = match self
            .state
            .endpoint
            .handle_membership_history_exchange(&source_device, message)
            .await
        {
            Ok(response) => response,
            Err(_) => {
                reject(&mut send).await;
                return Ok(());
            }
        };
        let payload = match encode_message(&response) {
            Ok(payload) => payload,
            _ => {
                reject(&mut send).await;
                return Ok(());
            }
        };
        let _ = send.write_all(&[ACCEPTED]).await;
        let _ = write_message(&mut send, &payload).await;
        let _ = connection.closed().await;
        Ok(())
    }
}

fn encode_message(
    message: &MembershipHistoryMessage,
) -> Result<Vec<u8>, MembershipHistoryExchangeError> {
    let mut payload = vec![WIRE_VERSION];
    payload.extend(
        postcard::to_stdvec(message).map_err(|_| MembershipHistoryExchangeError::Transport)?,
    );
    if payload.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    Ok(payload)
}

fn decode_message(
    payload: &[u8],
) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
    let Some((&version, body)) = payload.split_first() else {
        return Err(MembershipHistoryExchangeError::Transport);
    };
    if version != WIRE_VERSION
        || body.is_empty()
        || payload.len() > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE
    {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    let message: MembershipHistoryMessage =
        postcard::from_bytes(body).map_err(|_| MembershipHistoryExchangeError::Transport)?;
    if matches!(
        message,
        MembershipHistoryMessage::HistoryPageV2(_)
            | MembershipHistoryMessage::AckV2(_)
            | MembershipHistoryMessage::RestrictedEventV2(_)
            | MembershipHistoryMessage::RestrictedDecisionV2(_)
    ) {
        Ok(message)
    } else {
        Err(MembershipHistoryExchangeError::Transport)
    }
}

struct HandlerState {
    member_repo: Arc<dyn MemberRepositoryPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    endpoint: Arc<dyn MembershipHistoryExchangeEndpointPort>,
}

impl IrohMembershipHistoryExchangeHandler {
    async fn resolve_source_device(
        &self,
        public_key: &[u8; 32],
        message: &MembershipHistoryMessage,
    ) -> Option<DeviceId> {
        let fingerprint = self
            .state
            .fingerprint_factory
            .from_public_key(public_key)
            .ok()?;
        let known = self
            .state
            .member_repo
            .list()
            .await
            .ok()?
            .into_iter()
            .find(|member| member.identity_fingerprint == fingerprint)
            .map(|member| member.device_id);
        if known.is_some() {
            return known;
        }

        // A device admitted while this peer was offline can introduce itself
        // only through the signed admission record that binds its transport
        // identity. The application verifies and persists that record before
        // the device is treated as a normal member on later exchanges.
        introduced_device(message, &fingerprint)
    }
}

fn introduced_device(
    message: &MembershipHistoryMessage,
    fingerprint: &uc_core::security::IdentityFingerprint,
) -> Option<DeviceId> {
    match message {
        MembershipHistoryMessage::HistoryPageV2(page) => {
            (page.sender_admission().identity_fingerprint == *fingerprint)
                .then(|| page.sender_admission().device_id.clone())
        }
        MembershipHistoryMessage::AckV2(_)
        | MembershipHistoryMessage::RestrictedEventV2(_)
        | MembershipHistoryMessage::RestrictedDecisionV2(_) => None,
    }
}

async fn write_message(
    send: &mut iroh::endpoint::SendStream,
    payload: &[u8],
) -> Result<(), MembershipHistoryExchangeError> {
    let length =
        u32::try_from(payload.len()).map_err(|_| MembershipHistoryExchangeError::Transport)?;
    send.write_all(&length.to_be_bytes())
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    send.write_all(payload)
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    send.finish()
        .map_err(|_| MembershipHistoryExchangeError::Transport)
}

async fn read_byte(
    receive: &mut iroh::endpoint::RecvStream,
) -> Result<u8, MembershipHistoryExchangeError> {
    let mut value = [0; 1];
    tokio::time::timeout(IO_TIMEOUT, receive.read_exact(&mut value))
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    Ok(value[0])
}

async fn read_message(
    receive: &mut iroh::endpoint::RecvStream,
) -> Result<Vec<u8>, MembershipHistoryExchangeError> {
    let mut length = [0; 4];
    tokio::time::timeout(IO_TIMEOUT, receive.read_exact(&mut length))
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    let length = checked_message_length(u32::from_be_bytes(length) as usize)?;
    let mut payload = vec![0; length];
    tokio::time::timeout(IO_TIMEOUT, receive.read_exact(&mut payload))
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    Ok(payload)
}

fn checked_message_length(length: usize) -> Result<usize, MembershipHistoryExchangeError> {
    if length == 0 || length > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    Ok(length)
}

async fn reject(send: &mut iroh::endpoint::SendStream) {
    let _ = send.write_all(&[REJECTED]).await;
    let _ = send.finish();
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        AdmissionChangeFacts, HistoricalMembershipSignatureError,
        HistoricalMembershipSignatureVerifier, MembershipAdmissionV2, MembershipCredential,
        MembershipEventV2, MembershipHistoryMessage, MembershipHistoryV2Ack, MembershipOperationV2,
        VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
        MAX_MEMBERSHIP_HISTORY_FRAME_SIZE, MEMBERSHIP_EVENT_FORMAT_V2,
    };
    use uc_core::security::IdentityFingerprint;

    use super::{
        checked_message_length, decode_message, encode_message, introduced_device,
        MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
    };

    #[test]
    fn history_frame_length_accepts_the_boundary_and_rejects_oversize_before_allocation() {
        assert_eq!(checked_message_length(1), Ok(1));
        assert_eq!(
            checked_message_length(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE),
            Ok(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE)
        );
        assert!(checked_message_length(0).is_err());
        assert!(checked_message_length(MAX_MEMBERSHIP_HISTORY_FRAME_SIZE + 1).is_err());
    }

    #[test]
    fn history_v2_wire_checks_version_before_decoding_the_body() {
        assert_eq!(
            MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
            b"uniclipboard/membership-history/2"
        );
        let message = MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Consistent);
        let encoded = encode_message(&message).unwrap();
        assert_eq!(encoded[0], 2);
        assert_eq!(decode_message(&encoded).unwrap(), message);

        let mut old_version_with_invalid_body = vec![1];
        old_version_with_invalid_body.extend([0xff; 32]);
        assert!(decode_message(&old_version_with_invalid_body).is_err());
    }

    fn fingerprint() -> IdentityFingerprint {
        IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
            .unwrap_or_else(|_| panic!("test fingerprint must be valid"))
    }

    struct TestVerifier;

    impl TestVerifier {
        fn sign(&self, credential: &MembershipCredential, payload: &[u8]) -> Vec<u8> {
            let mut hasher = Sha256::new();
            hasher.update(b"membership-history-v2-infra-test\0");
            hasher.update(&credential.public_key);
            hasher.update(payload);
            hasher.finalize().to_vec()
        }
    }

    impl HistoricalMembershipSignatureVerifier for TestVerifier {
        fn verify(
            &self,
            signature_algorithm_version: u16,
            public_key: &[u8],
            payload: &[u8],
            signature: &[u8],
        ) -> Result<bool, HistoricalMembershipSignatureError> {
            if signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
                return Err(HistoricalMembershipSignatureError::UnsupportedAlgorithm);
            }
            let credential =
                MembershipCredential::new(signature_algorithm_version, public_key.to_vec());
            Ok(self.sign(&credential, payload) == signature)
        }
    }

    fn introduction(
        device_id: &str,
        identity_fingerprint: IdentityFingerprint,
    ) -> MembershipHistoryMessage {
        let verifier = TestVerifier;
        let device_id = DeviceId::new(device_id);
        let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![7; 32]);
        let instance = credential.member_instance_id(&device_id);
        let admission = MembershipAdmissionV2 {
            facts: AdmissionChangeFacts {
                member_instance: instance,
                device_id,
                device_name: "device".to_owned(),
                identity_fingerprint,
                transport_public_key: vec![1; 32],
                transport_address_blob: vec![2],
                identity_signature: vec![3],
            },
            membership_credential: credential.clone(),
            resume_public_key_digest: [8; 32],
            security_commitment_id: [9; 32],
        };
        let mut history = VersionedMembershipHistory::new("space-a".to_owned());
        let operation = MembershipOperationV2::AddDevice {
            admission: admission.clone(),
        };
        let mut event = MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            "space-a".to_owned(),
            None,
            0,
            [1; 16],
            instance,
            credential.credential_id,
            credential.signature_algorithm_version,
            operation.clone(),
            history
                .expected_resulting_members_digest(None, &operation)
                .expect("genesis digest"),
            [5; 32],
            vec![6],
            Some([7; 32]),
            Vec::new(),
        );
        event.signature = verifier.sign(&credential, &event.signing_payload());
        history
            .verify_and_receive_event(event, &verifier)
            .expect("genesis verifies");
        MembershipHistoryMessage::HistoryPageV2(
            history
                .export_reconciliation_pages_v2(admission.facts)
                .expect("history exports")
                .remove(0),
        )
    }

    #[test]
    fn unknown_member_introduction_must_bind_to_the_connection_identity() {
        let connected = fingerprint();
        let message = introduction("device-c", connected.clone());

        assert_eq!(
            introduced_device(&message, &connected),
            Some(DeviceId::new("device-c"))
        );
        let other = IdentityFingerprint::from_display_string("QRST-UVWX-YZ23-4567")
            .unwrap_or_else(|_| panic!("test fingerprint must be valid"));
        assert_eq!(introduced_device(&message, &other), None);
    }

    #[test]
    fn unknown_member_cannot_introduce_itself_with_a_regular_history_message() {
        let message = MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Consistent);

        assert_eq!(introduced_device(&message, &fingerprint()), None);
    }
}
