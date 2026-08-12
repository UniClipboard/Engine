//! Bounded membership-history exchange on authenticated Iroh connections.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryMessage,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

use super::connect_with_staggered_retry;

pub const MEMBERSHIP_HISTORY_EXCHANGE_ALPN: &[u8] = b"uniclipboard/membership-history/1";

const MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;
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
        let payload =
            postcard::to_stdvec(&message).map_err(|_| MembershipHistoryExchangeError::Transport)?;
        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(MembershipHistoryExchangeError::Transport);
        }
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
        postcard::from_bytes(&response).map_err(|_| MembershipHistoryExchangeError::Transport)
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
        let message: MembershipHistoryMessage = match postcard::from_bytes(&message) {
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
        let payload = match postcard::to_stdvec(&response) {
            Ok(payload) if payload.len() <= MAX_MESSAGE_SIZE => payload,
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
    let MembershipHistoryMessage::EventsResponse(response) = message else {
        return None;
    };
    response
        .events
        .iter()
        .find_map(|event| match &event.operation {
            uc_core::membership::MembershipOperation::AddDevice { admission }
                if admission.identity_fingerprint == *fingerprint =>
            {
                Some(admission.device_id.clone())
            }
            _ => None,
        })
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
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_MESSAGE_SIZE {
        return Err(MembershipHistoryExchangeError::Transport);
    }
    let mut payload = vec![0; length];
    tokio::time::timeout(IO_TIMEOUT, receive.read_exact(&mut payload))
        .await
        .map_err(|_| MembershipHistoryExchangeError::Transport)?
        .map_err(|_| MembershipHistoryExchangeError::Transport)?;
    Ok(payload)
}

async fn reject(send: &mut iroh::endpoint::SendStream) {
    let _ = send.write_all(&[REJECTED]).await;
    let _ = send.finish();
}

#[cfg(test)]
mod tests {
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        AdmissionChangeFacts, MemberInstanceId, MembershipEvent, MembershipEventsResponse,
        MembershipHistoryHello, MembershipHistoryMessage, MembershipOperation,
    };
    use uc_core::security::IdentityFingerprint;

    use super::introduced_device;

    fn fingerprint() -> IdentityFingerprint {
        IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
            .unwrap_or_else(|_| panic!("test fingerprint must be valid"))
    }

    fn introduction(device_id: &str, identity_fingerprint: IdentityFingerprint) -> MembershipEvent {
        let instance = MemberInstanceId::from_bytes([7; 32]);
        MembershipEvent::new(
            "space-a".to_owned(),
            None,
            0,
            [1; 16],
            instance,
            MembershipOperation::AddDevice {
                admission: AdmissionChangeFacts {
                    member_instance: instance,
                    device_id: DeviceId::new(device_id),
                    device_name: "device".to_owned(),
                    identity_fingerprint,
                    transport_public_key: vec![1; 32],
                    transport_address_blob: vec![2],
                    identity_signature: vec![3],
                },
            },
            [4; 32],
            [5; 32],
            Vec::new(),
            None,
            vec![6],
        )
    }

    #[test]
    fn unknown_member_introduction_must_bind_to_the_connection_identity() {
        let connected = fingerprint();
        let message = MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
            lineage_id: "space-a".to_owned(),
            after_event_id: None,
            events: vec![introduction("device-c", connected.clone())],
        });

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
        let message = MembershipHistoryMessage::Hello(MembershipHistoryHello {
            lineage_id: "space-a".to_owned(),
            member_instance_id: MemberInstanceId::from_bytes([7; 32]),
            admission: AdmissionChangeFacts {
                member_instance: MemberInstanceId::from_bytes([7; 32]),
                device_id: DeviceId::new("device-c"),
                device_name: "device".to_owned(),
                identity_fingerprint: fingerprint(),
                transport_public_key: vec![1; 32],
                transport_address_blob: vec![2],
                identity_signature: vec![3],
            },
            known_head: None,
            applied_head: None,
            applied_members_digest: None,
        });

        assert_eq!(introduced_device(&message, &fingerprint()), None);
    }
}
