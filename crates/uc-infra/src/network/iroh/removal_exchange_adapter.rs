//! 移除意图交换与受限迟交入口的 iroh 适配器。
//!
//! 普通成员之间通过 `removal-exchange/1` 通道同步交换意图、备用 key package
//! 与恢复资料;已被移除的设备通过 `removal-late/1` 受限入口提交历史意图。
//! 受限入口只返回有界接收结果,不返回当前成员列表、在线状态、收敛摘要、
//! 安全代次、密钥或剪贴板状态;请求大小、单次数量、并发数与重试速率都有
//! 固定上限。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::{Endpoint, EndpointAddr};
use tracing::{debug, warn};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberRepositoryPort, PeerAdmissionPort, RemovalExchangeEndpointPort, RemovalExchangeError,
    RemovalExchangeMessage, RemovalExchangePort, RemovalLateAcceptance, RemovalLateSubmission,
    RemovalLateSubmissionEndpointPort, RemovalLateSubmissionError, RemovalLateSubmissionPort,
    RemovalLateSubmissionTransportError,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::PeerAddressRepositoryPort;

use super::connect_with_staggered_retry;

pub const REMOVAL_EXCHANGE_ALPN: &[u8] = b"uniclipboard/removal-exchange/1";
pub const REMOVAL_LATE_ALPN: &[u8] = b"uniclipboard/removal-late/1";

const MAX_EXCHANGE_MESSAGE_SIZE: usize = 4 * 1024 * 1024;
const MAX_LATE_SUBMISSION_SIZE: usize = 1024 * 1024;
const MAX_LATE_SUBMISSION_CONCURRENCY: usize = 4;
const MAX_LATE_IDENTITIES: usize = 256;
const LATE_SUBMISSION_MIN_INTERVAL: Duration = Duration::from_secs(10);
const EXCHANGE_IO_TIMEOUT: Duration = Duration::from_secs(10);
const ACK_ACCEPTED: u8 = 1;
const ACK_REJECTED: u8 = 2;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use async_trait::async_trait;
    use chrono::Utc;
    use iroh::endpoint::presets::N0;
    use iroh::protocol::Router;
    use iroh::SecretKey;
    use iroh::{Endpoint, RelayMode};
    use uc_core::ids::DeviceId;
    use uc_core::membership::{
        MemberInstanceId, MemberRepositoryPort, MemberSyncPreferences, MembershipError,
        PeerAdmissionError, PeerAdmissionPort, RemovalCausalProof, RemovalCausalProofMember,
        RemovalExchangeEndpointPort, RemovalExchangeError, RemovalExchangeMessage,
        RemovalExchangePort, RemovalIntentContent, RemovalLateAcceptance, RemovalLateSubmission,
        RemovalLateSubmissionEndpointPort, RemovalLateSubmissionError, RemovalLateSubmissionPort,
        SignedRemovalIntent, SpaceMember,
    };
    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_core::ports::{PeerAddressError, PeerAddressRecord, PeerAddressRepositoryPort};

    use super::{
        IrohRemovalExchangeAdapter, LateSubmissionLimiter, REMOVAL_EXCHANGE_ALPN, REMOVAL_LATE_ALPN,
    };
    use crate::security::Sha256IdentityFingerprintFactory;

    #[test]
    fn late_submission_limiter_rejects_a_second_request_in_the_same_window() {
        let limiter = LateSubmissionLimiter::default();
        let start = Instant::now();

        assert!(limiter.allow(b"peer-a", start));
        assert!(!limiter.allow(b"peer-a", start + Duration::from_secs(9)));
        assert!(limiter.allow(b"peer-a", start + Duration::from_secs(10)));
    }

    #[derive(Default)]
    struct StaticMembers(Vec<SpaceMember>);

    #[async_trait]
    impl MemberRepositoryPort for StaticMembers {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self
                .0
                .iter()
                .find(|member| member.device_id == *device_id)
                .cloned())
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(self.0.clone())
        }

        async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
            Err(MembershipError::Repository(
                "test repository is read-only".to_owned(),
            ))
        }

        async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
            Err(MembershipError::Repository(
                "test repository is read-only".to_owned(),
            ))
        }
    }

    #[derive(Default)]
    struct MemoryPeerAddresses(Mutex<std::collections::BTreeMap<DeviceId, PeerAddressRecord>>);

    #[async_trait]
    impl PeerAddressRepositoryPort for MemoryPeerAddresses {
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
                .insert(record.device_id.clone(), record.clone());
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

    #[derive(Default)]
    struct RecordingExchangeEndpoint(Mutex<Vec<RemovalExchangeMessage>>);

    #[async_trait]
    impl RemovalExchangeEndpointPort for RecordingExchangeEndpoint {
        async fn handle_exchange(
            &self,
            _source_device_id: &DeviceId,
            message: RemovalExchangeMessage,
        ) -> Result<RemovalExchangeMessage, RemovalExchangeError> {
            self.0.lock().unwrap().push(message);
            Ok(RemovalExchangeMessage::IntentAck(
                uc_core::membership::RemovalIntentId::from_bytes([9; 32]),
            ))
        }
    }

    #[derive(Default)]
    struct RecordingLateEndpoint(Mutex<Vec<RemovalLateSubmission>>);

    #[async_trait]
    impl RemovalLateSubmissionEndpointPort for RecordingLateEndpoint {
        async fn handle_late_submission(
            &self,
            submission: RemovalLateSubmission,
        ) -> Result<RemovalLateAcceptance, RemovalLateSubmissionError> {
            let intent_id = match &submission {
                RemovalLateSubmission::Intent(intent) => intent.intent_id,
            };
            self.0.lock().unwrap().push(submission);
            Ok(RemovalLateAcceptance::Accepted { intent_id })
        }
    }

    fn member(seed: [u8; 32], device_id: &str) -> SpaceMember {
        let factory = Sha256IdentityFingerprintFactory;
        let identity_fingerprint = factory
            .from_public_key(SecretKey::from_bytes(&seed).public().as_bytes())
            .unwrap();
        SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: "test member".to_owned(),
            identity_fingerprint,
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        }
    }

    async fn endpoint(seed: [u8; 32]) -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(N0)
                .secret_key(SecretKey::from_bytes(&seed))
                .alpns(vec![
                    REMOVAL_EXCHANGE_ALPN.to_vec(),
                    REMOVAL_LATE_ALPN.to_vec(),
                ])
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .unwrap(),
        )
    }

    async fn wait_for_direct_address(endpoint: &Endpoint) {
        for _ in 0..100 {
            if !endpoint.addr().addrs.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("endpoint never published a direct address");
    }

    fn historical_intent() -> SignedRemovalIntent {
        let alice = MemberInstanceId::derive("alice", &[1; 32]);
        let bob = MemberInstanceId::derive("bob", &[2; 32]);
        let proof = RemovalCausalProof::new(
            1,
            vec![
                RemovalCausalProofMember {
                    device_id: DeviceId::new("alice"),
                    instance: alice,
                    signing_public_key: vec![1; 32],
                },
                RemovalCausalProofMember {
                    device_id: DeviceId::new("bob"),
                    instance: bob,
                    signing_public_key: vec![2; 32],
                },
            ],
        );
        SignedRemovalIntent::new(
            RemovalIntentContent {
                space_lineage: "space-a".to_owned(),
                view_epoch: 1,
                view_members: vec![alice, bob],
                initiator: alice,
                target: bob,
            },
            vec![1],
            proof,
        )
    }

    #[tokio::test]
    async fn actual_iroh_exchange_and_late_submission_reach_their_separate_handlers() {
        let sender_seed = [0x11; 32];
        let receiver_seed = [0x22; 32];
        let sender = endpoint(sender_seed).await;
        let receiver = endpoint(receiver_seed).await;
        wait_for_direct_address(&sender).await;
        wait_for_direct_address(&receiver).await;

        let exchange_endpoint = Arc::new(RecordingExchangeEndpoint::default());
        let late_endpoint = Arc::new(RecordingLateEndpoint::default());
        let receiver_adapter = IrohRemovalExchangeAdapter::new(
            Arc::clone(&receiver),
            Arc::new(MemoryPeerAddresses::default()),
        );
        let handlers = receiver_adapter.handlers(
            Arc::new(StaticMembers(vec![member(sender_seed, "alice")])),
            Arc::new(AdmitAll),
            Arc::new(Sha256IdentityFingerprintFactory),
            exchange_endpoint.clone(),
            late_endpoint.clone(),
        );
        let router = Router::builder((*receiver).clone())
            .accept(REMOVAL_EXCHANGE_ALPN, handlers.exchange)
            .accept(REMOVAL_LATE_ALPN, handlers.late)
            .spawn();

        let addresses: Arc<dyn PeerAddressRepositoryPort> =
            Arc::new(MemoryPeerAddresses::default());
        addresses
            .upsert(&PeerAddressRecord {
                device_id: DeviceId::new("bob"),
                addr_blob: postcard::to_stdvec(&receiver.addr()).unwrap(),
                observed_at: Utc::now(),
            })
            .await
            .unwrap();
        let sender_adapter = IrohRemovalExchangeAdapter::new(sender.clone(), addresses);
        let intent = historical_intent();

        let response = sender_adapter
            .exchange(
                &DeviceId::new("bob"),
                RemovalExchangeMessage::Intent(Box::new(intent.clone())),
            )
            .await
            .unwrap();
        assert!(matches!(response, RemovalExchangeMessage::IntentAck(_)));
        let acceptance = sender_adapter
            .submit_late(
                &DeviceId::new("bob"),
                RemovalLateSubmission::Intent(Box::new(intent)),
            )
            .await
            .unwrap();
        assert!(matches!(acceptance, RemovalLateAcceptance::Accepted { .. }));
        let limited = sender_adapter
            .submit_late(
                &DeviceId::new("bob"),
                RemovalLateSubmission::Intent(Box::new(historical_intent())),
            )
            .await
            .unwrap();
        assert!(matches!(
            limited,
            RemovalLateAcceptance::Rejected {
                reason: uc_core::membership::RemovalLateRejectionReason::LimitExceeded
            }
        ));
        assert_eq!(exchange_endpoint.0.lock().unwrap().len(), 1);
        assert_eq!(late_endpoint.0.lock().unwrap().len(), 1);

        router.shutdown().await.ok();
        sender.close().await;
        receiver.close().await;
    }
}

#[derive(Default)]
struct LateSubmissionLimiter {
    last_seen: Mutex<HashMap<Vec<u8>, Instant>>,
}

impl LateSubmissionLimiter {
    fn allow(&self, identity: &[u8], now: Instant) -> bool {
        let mut last_seen = match self.last_seen.lock() {
            Ok(last_seen) => last_seen,
            Err(_) => return false,
        };
        last_seen.retain(|_, previous| {
            now.checked_duration_since(*previous)
                .is_some_and(|elapsed| elapsed < LATE_SUBMISSION_MIN_INTERVAL)
        });
        if last_seen.get(identity).is_some_and(|previous| {
            now.checked_duration_since(*previous)
                .is_some_and(|elapsed| elapsed < LATE_SUBMISSION_MIN_INTERVAL)
        }) {
            return false;
        }
        if last_seen.len() >= MAX_LATE_IDENTITIES {
            if let Some(oldest) = last_seen
                .iter()
                .min_by_key(|(_, previous)| **previous)
                .map(|(identity, _)| identity.clone())
            {
                last_seen.remove(&oldest);
            }
        }
        last_seen.insert(identity.to_vec(), now);
        true
    }
}

/// 普通成员交换入口所需的状态。
struct ExchangeHandlerState {
    member_repo: Arc<dyn MemberRepositoryPort>,
    peer_admission: Arc<dyn PeerAdmissionPort>,
    fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    exchange_endpoint: Arc<dyn RemovalExchangeEndpointPort>,
}

/// 受限迟交入口所需的状态。
struct LateHandlerState {
    late_submission: Arc<dyn RemovalLateSubmissionEndpointPort>,
    late_concurrency: Arc<tokio::sync::Semaphore>,
    late_limiter: LateSubmissionLimiter,
}

/// 普通成员之间的移除意图交换。
pub struct IrohRemovalExchangeAdapter {
    endpoint: Arc<Endpoint>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
}

pub(crate) struct IrohRemovalHandlers {
    pub(crate) exchange: IrohRemovalExchangeHandler,
    pub(crate) late: IrohRemovalLateHandler,
}

impl IrohRemovalExchangeAdapter {
    pub fn new(
        endpoint: Arc<Endpoint>,
        peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    ) -> Self {
        Self {
            endpoint,
            peer_addr_repo,
        }
    }

    pub(crate) fn handlers(
        &self,
        member_repo: Arc<dyn MemberRepositoryPort>,
        peer_admission: Arc<dyn PeerAdmissionPort>,
        fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
        exchange_endpoint: Arc<dyn RemovalExchangeEndpointPort>,
        late_submission: Arc<dyn RemovalLateSubmissionEndpointPort>,
    ) -> IrohRemovalHandlers {
        IrohRemovalHandlers {
            exchange: IrohRemovalExchangeHandler {
                state: Arc::new(ExchangeHandlerState {
                    member_repo,
                    peer_admission,
                    fingerprint_factory,
                    exchange_endpoint,
                }),
            },
            late: IrohRemovalLateHandler {
                state: Arc::new(LateHandlerState {
                    late_submission,
                    late_concurrency: Arc::new(tokio::sync::Semaphore::new(
                        MAX_LATE_SUBMISSION_CONCURRENCY,
                    )),
                    late_limiter: LateSubmissionLimiter::default(),
                }),
            },
        }
    }

    async fn resolve_addr(&self, recipient: &DeviceId) -> Option<EndpointAddr> {
        match self.peer_addr_repo.get(recipient).await {
            Ok(Some(record)) => postcard::from_bytes(&record.addr_blob).ok(),
            Ok(None) => None,
            Err(_) => {
                warn!(
                    failure = "address_lookup_failed",
                    "removal exchange address lookup failed"
                );
                None
            }
        }
    }
}

#[async_trait]
impl RemovalExchangePort for IrohRemovalExchangeAdapter {
    async fn exchange(
        &self,
        recipient: &DeviceId,
        message: RemovalExchangeMessage,
    ) -> Result<RemovalExchangeMessage, RemovalExchangeError> {
        let payload = postcard::to_stdvec(&message).map_err(|_| RemovalExchangeError::Transport)?;
        if payload.len() > MAX_EXCHANGE_MESSAGE_SIZE {
            return Err(RemovalExchangeError::Transport);
        }
        let addr = self
            .resolve_addr(recipient)
            .await
            .ok_or(RemovalExchangeError::Offline)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            addr,
            REMOVAL_EXCHANGE_ALPN,
            "removal-exchange",
        )
        .await
        .map_err(|_| RemovalExchangeError::Offline)?;
        let (mut send, mut recv) = tokio::time::timeout(EXCHANGE_IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|_| RemovalExchangeError::Transport)?
            .map_err(|_| RemovalExchangeError::Transport)?;
        let length = u32::try_from(payload.len()).map_err(|_| RemovalExchangeError::Transport)?;
        send.write_all(&length.to_be_bytes())
            .await
            .map_err(|_| RemovalExchangeError::Transport)?;
        send.write_all(&payload)
            .await
            .map_err(|_| RemovalExchangeError::Transport)?;
        send.finish().map_err(|_| RemovalExchangeError::Transport)?;
        let mut ack = [0u8; 1];
        tokio::time::timeout(EXCHANGE_IO_TIMEOUT, recv.read_exact(&mut ack))
            .await
            .map_err(|_| RemovalExchangeError::Transport)?
            .map_err(|_| RemovalExchangeError::Transport)?;
        match ack[0] {
            ACK_ACCEPTED => {}
            ACK_REJECTED => return Err(RemovalExchangeError::Rejected),
            _ => return Err(RemovalExchangeError::Transport),
        }
        let mut length = [0u8; 4];
        tokio::time::timeout(EXCHANGE_IO_TIMEOUT, recv.read_exact(&mut length))
            .await
            .map_err(|_| RemovalExchangeError::Transport)?
            .map_err(|_| RemovalExchangeError::Transport)?;
        let length = u32::from_be_bytes(length) as usize;
        if length == 0 || length > MAX_EXCHANGE_MESSAGE_SIZE {
            return Err(RemovalExchangeError::Transport);
        }
        let mut response = vec![0u8; length];
        tokio::time::timeout(EXCHANGE_IO_TIMEOUT, recv.read_exact(&mut response))
            .await
            .map_err(|_| RemovalExchangeError::Transport)?
            .map_err(|_| RemovalExchangeError::Transport)?;
        postcard::from_bytes(&response).map_err(|_| RemovalExchangeError::Transport)
    }
}

#[async_trait]
impl RemovalLateSubmissionPort for IrohRemovalExchangeAdapter {
    async fn submit_late(
        &self,
        recipient: &DeviceId,
        submission: RemovalLateSubmission,
    ) -> Result<RemovalLateAcceptance, RemovalLateSubmissionTransportError> {
        let payload = postcard::to_stdvec(&submission)
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        if payload.len() > MAX_LATE_SUBMISSION_SIZE {
            return Err(RemovalLateSubmissionTransportError::Transport);
        }
        let addr = self
            .resolve_addr(recipient)
            .await
            .ok_or(RemovalLateSubmissionTransportError::Offline)?;
        let connection = connect_with_staggered_retry(
            Arc::clone(&self.endpoint),
            addr,
            REMOVAL_LATE_ALPN,
            "removal-late",
        )
        .await
        .map_err(|_| RemovalLateSubmissionTransportError::Offline)?;
        let (mut send, mut recv) = tokio::time::timeout(EXCHANGE_IO_TIMEOUT, connection.open_bi())
            .await
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        let length = u32::try_from(payload.len())
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        send.write_all(&length.to_be_bytes())
            .await
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        send.write_all(&payload)
            .await
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        send.finish()
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        let mut ack = [0u8; 1];
        tokio::time::timeout(EXCHANGE_IO_TIMEOUT, recv.read_exact(&mut ack))
            .await
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        if ack[0] != ACK_ACCEPTED {
            return Err(RemovalLateSubmissionTransportError::Transport);
        }
        let mut response_length = [0u8; 4];
        tokio::time::timeout(EXCHANGE_IO_TIMEOUT, recv.read_exact(&mut response_length))
            .await
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        let response_length = u32::from_be_bytes(response_length) as usize;
        if response_length == 0 || response_length > MAX_LATE_SUBMISSION_SIZE {
            return Err(RemovalLateSubmissionTransportError::Transport);
        }
        let mut response = vec![0u8; response_length];
        tokio::time::timeout(EXCHANGE_IO_TIMEOUT, recv.read_exact(&mut response))
            .await
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?
            .map_err(|_| RemovalLateSubmissionTransportError::Transport)?;
        postcard::from_bytes(&response).map_err(|_| RemovalLateSubmissionTransportError::Transport)
    }
}

#[derive(Clone)]
pub struct IrohRemovalExchangeHandler {
    state: Arc<ExchangeHandlerState>,
}

impl std::fmt::Debug for IrohRemovalExchangeHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohRemovalExchangeHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohRemovalExchangeHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer_device_id = self
            .state
            .resolve_device(connection.remote_id().as_bytes())
            .await;
        let (mut send, mut recv) =
            match tokio::time::timeout(EXCHANGE_IO_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                _ => {
                    debug!("removal exchange stream accept failed");
                    return Ok(());
                }
            };
        let Some(peer_device_id) = peer_device_id else {
            emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
            let _ = connection.closed().await;
            return Ok(());
        };
        if !self.state.is_admitted(&peer_device_id).await {
            warn!("removal exchange: peer is not admitted");
            emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
            let _ = connection.closed().await;
            return Ok(());
        }
        let Some(message) = read_length_prefixed(&mut recv, MAX_EXCHANGE_MESSAGE_SIZE).await else {
            emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
            let _ = connection.closed().await;
            return Ok(());
        };
        let message: RemovalExchangeMessage = match postcard::from_bytes(&message) {
            Ok(message) => message,
            Err(_) => {
                emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        let response = match self
            .state
            .exchange_endpoint
            .handle_exchange(&peer_device_id, message)
            .await
        {
            Ok(response) => response,
            Err(_) => {
                warn!(
                    failure = "endpoint_rejected",
                    "removal exchange handling failed"
                );
                emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        let response_payload = match postcard::to_stdvec(&response) {
            Ok(payload) => payload,
            Err(_) => {
                emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        emit_exchange_reply(&mut send, ACK_ACCEPTED, Some(&response_payload)).await;
        let _ = connection.closed().await;
        Ok(())
    }
}

/// 受限迟交入口:不检查成员资格,只接受有界的历史意图提交。
#[derive(Clone)]
pub struct IrohRemovalLateHandler {
    state: Arc<LateHandlerState>,
}

impl std::fmt::Debug for IrohRemovalLateHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IrohRemovalLateHandler")
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for IrohRemovalLateHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let (mut send, mut recv) =
            match tokio::time::timeout(EXCHANGE_IO_TIMEOUT, connection.accept_bi()).await {
                Ok(Ok(streams)) => streams,
                _ => {
                    debug!("removal late stream accept failed");
                    return Ok(());
                }
            };
        let Ok(_permit) = self.state.late_concurrency.try_acquire() else {
            emit_late_limit_rejection(&mut send).await;
            let _ = connection.closed().await;
            return Ok(());
        };
        if !self
            .state
            .late_limiter
            .allow(connection.remote_id().as_bytes(), Instant::now())
        {
            emit_late_limit_rejection(&mut send).await;
            let _ = connection.closed().await;
            return Ok(());
        }
        let Some(submission) = read_length_prefixed(&mut recv, MAX_LATE_SUBMISSION_SIZE).await
        else {
            emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
            let _ = connection.closed().await;
            return Ok(());
        };
        let submission: RemovalLateSubmission = match postcard::from_bytes(&submission) {
            Ok(submission) => submission,
            Err(_) => {
                emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        let acceptance = match self
            .state
            .late_submission
            .handle_late_submission(submission)
            .await
        {
            Ok(acceptance) => acceptance,
            Err(error) => {
                warn!(
                    failure = "endpoint_rejected",
                    "removal late submission handling failed"
                );
                let bounded = RemovalLateAcceptance::Rejected {
                    reason: match error {
                        RemovalLateSubmissionError::LimitExceeded => {
                            uc_core::membership::RemovalLateRejectionReason::LimitExceeded
                        }
                        _ => uc_core::membership::RemovalLateRejectionReason::Unavailable,
                    },
                };
                let payload = match postcard::to_stdvec(&bounded) {
                    Ok(payload) => payload,
                    Err(_) => {
                        emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
                        let _ = connection.closed().await;
                        return Ok(());
                    }
                };
                emit_exchange_reply(&mut send, ACK_ACCEPTED, Some(&payload)).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        let payload = match postcard::to_stdvec(&acceptance) {
            Ok(payload) => payload,
            Err(_) => {
                emit_exchange_reply(&mut send, ACK_REJECTED, None).await;
                let _ = connection.closed().await;
                return Ok(());
            }
        };
        emit_exchange_reply(&mut send, ACK_ACCEPTED, Some(&payload)).await;
        let _ = connection.closed().await;
        Ok(())
    }
}

async fn emit_late_limit_rejection(send: &mut iroh::endpoint::SendStream) {
    let rejection = RemovalLateAcceptance::Rejected {
        reason: uc_core::membership::RemovalLateRejectionReason::LimitExceeded,
    };
    let payload = match postcard::to_stdvec(&rejection) {
        Ok(payload) => payload,
        Err(_) => {
            emit_exchange_reply(send, ACK_REJECTED, None).await;
            return;
        }
    };
    emit_exchange_reply(send, ACK_ACCEPTED, Some(&payload)).await;
}

impl ExchangeHandlerState {
    async fn resolve_device(&self, public_key: &[u8; 32]) -> Option<DeviceId> {
        let fingerprint = self.fingerprint_factory.from_public_key(public_key).ok()?;
        let members = match self.member_repo.list().await {
            Ok(members) => members,
            Err(_) => {
                warn!(
                    failure = "member_lookup_failed",
                    "removal exchange member lookup failed"
                );
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
            Err(_) => {
                warn!(
                    failure = "admission_check_failed",
                    "removal exchange admission check failed"
                );
                false
            }
        }
    }
}

/// 读取长度前缀 + 载荷的请求体;超出上限或超时返回 `None`。
async fn read_length_prefixed(
    recv: &mut iroh::endpoint::RecvStream,
    max_size: usize,
) -> Option<Vec<u8>> {
    let mut length = [0u8; 4];
    tokio::time::timeout(EXCHANGE_IO_TIMEOUT, recv.read_exact(&mut length))
        .await
        .ok()?
        .ok()?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > max_size {
        return None;
    }
    let mut payload = vec![0u8; length];
    tokio::time::timeout(EXCHANGE_IO_TIMEOUT, recv.read_exact(&mut payload))
        .await
        .ok()?
        .ok()?;
    Some(payload)
}

async fn emit_exchange_reply(
    send: &mut iroh::endpoint::SendStream,
    ack: u8,
    response: Option<&[u8]>,
) {
    let mut buffer = vec![ack];
    let response = response.unwrap_or_default();
    buffer.extend_from_slice(&(response.len() as u32).to_be_bytes());
    buffer.extend_from_slice(response);
    if matches!(
        tokio::time::timeout(EXCHANGE_IO_TIMEOUT, send.write_all(&buffer)).await,
        Ok(Ok(()))
    ) {
        let _ = send.finish();
    }
}
