//! 原生 Router 只转换连接并调用一次完整接收能力。
use super::super::connectivity::IrohPeerConnection;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::sync::Arc;
use tokio::time::Instant;
#[cfg(test)]
use tokio_util::sync::CancellationToken;
use uc_application::deps::{ActiveStateReceiverPort, ACTIVE_EXCHANGE_TIMEOUT};
use uc_observability_contract::diagnostics::{connectivity::NetworkRecorder, ObservationContext};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

#[derive(Clone)]
pub struct IrohActiveStateHandler {
    receiver: Arc<dyn ActiveStateReceiverPort>,
}
impl IrohActiveStateHandler {
    pub fn new(receiver: Arc<dyn ActiveStateReceiverPort>) -> Self {
        Self { receiver }
    }
}
impl std::fmt::Debug for IrohActiveStateHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohActiveStateHandler")
            .finish_non_exhaustive()
    }
}
impl ProtocolHandler for IrohActiveStateHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let deadline = Instant::now() + ACTIVE_EXCHANGE_TIMEOUT;
        // 已有不透明物理上下文只关联本次完整接收，不传出原生编号或业务阶段。
        let context = NetworkRecorder::current().in_connection_scope(
            *connection.remote_id().as_bytes(),
            connection.stable_id() as u64,
            ObservationContext::capture,
        );
        context
            .scope(async {
                if let Err(error) = self
                    .receiver
                    .receive(IrohPeerConnection::wrap(connection), deadline)
                    .await
                {
                    uc_warn!(
                        error_kind = "active_state_receive",
                        io_error_kind = io_error_kind(&error),
                        "active clipboard receive failed"
                    );
                }
                Ok(())
            })
            .await
    }

    async fn shutdown(&self) {
        if let Err(error) = self.receiver.shutdown().await {
            uc_warn!(
                error_kind = "active_state_shutdown",
                io_error_kind = io_error_kind(&error),
                "active clipboard connectivity shutdown failed"
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::iroh::admitted_connections::AdmittedConnections;
    use crate::network::iroh::connectivity::{ConnectivityProfile, IrohConnectionAdmission};
    use tokio::sync::broadcast;
    use uc_application::deps::{
        ActiveStateReceiver, ConnectivityBackend, ConnectivityError, ConnectivityManager,
        ConnectivityPolicy, PeerConnection,
    };
    use uc_core::ids::DeviceId;
    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_core::ports::{ActiveClipboardReceiverPort, InboundActiveClipboardState};
    use uc_sync_protocol::active_clipboard_state::ACTIVE_CLIPBOARD_ALPN;

    struct InboundOnlyBackend;
    #[async_trait]
    impl ConnectivityBackend for InboundOnlyBackend {
        async fn connect(
            &self,
            _: DeviceId,
            _address_hint: Option<Vec<u8>>,
            _cancel: CancellationToken,
        ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
            Err(ConnectivityError::Offline)
        }
    }
    fn test_manager(
        endpoint: Arc<Endpoint>,
        members: Arc<dyn MemberRepositoryPort>,
        admitted: bool,
    ) -> ConnectivityManager {
        // 该夹具仅接收入站；身份与准入仍走真实 Iroh bridge，出站能力显式拒绝。
        let _endpoint = endpoint;
        ConnectivityManager::new(
            Arc::new(InboundOnlyBackend),
            Arc::new(IrohConnectionAdmission::new(
                crate::network::iroh::inbound_peer::member_table_directory(members),
                Arc::new(crate::network::iroh::StaticPeerAdmission(admitted)),
                Arc::new(Sha256IdentityFingerprintFactory),
                AdmittedConnections::default(),
                ConnectivityProfile::ActiveClipboardState,
            )),
            ConnectivityPolicy {
                inbound_per_connection: 1,
                ..ConnectivityPolicy::default()
            },
        )
    }

    use uc_core::membership::MemberRepositoryPort;

    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use iroh::protocol::Router;
    use iroh::{Endpoint, RelayMode, SecretKey};
    use tokio::sync::Mutex;

    use uc_core::membership::{MembershipError, SpaceMember};
    use uc_core::MemberSyncPreferences;

    use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;
    use uc_sync_protocol::active_clipboard_state::{write_frame, ActiveClipboardWireMessage};

    // ----- test doubles ------------------------------------------------------

    #[derive(Default)]
    struct MemMemberRepo {
        inner: Mutex<HashMap<String, SpaceMember>>,
    }
    crate::network::iroh::inbound_peer::member_table_identity_directory!(MemMemberRepo);
    #[async_trait]
    impl MemberRepositoryPort for MemMemberRepo {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self.inner.lock().await.get(device_id.as_str()).cloned())
        }
        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(self.inner.lock().await.values().cloned().collect())
        }
        async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
            self.inner
                .lock()
                .await
                .insert(member.device_id.as_str().to_string(), member.clone());
            Ok(())
        }
        async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(self.inner.lock().await.remove(device_id.as_str()).is_some())
        }
    }

    // ----- harness -----------------------------------------------------------

    async fn bind_endpoint_with(seed: [u8; 32]) -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(SecretKey::from_bytes(&seed))
                .alpns(vec![ACTIVE_CLIPBOARD_ALPN.to_vec()])
                .relay_mode(RelayMode::Disabled)
                .bind()
                .await
                .expect("bind endpoint"),
        )
    }

    async fn wait_for_direct_addrs(endpoint: &Endpoint) {
        for _ in 0..100 {
            if !endpoint.addr().addrs.is_empty() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("endpoint never published direct addresses");
    }

    fn make_member(seed: [u8; 32], device_id: &str) -> SpaceMember {
        let factory = Sha256IdentityFingerprintFactory;
        let sk = SecretKey::from_bytes(&seed);
        let fp = factory
            .from_public_key(sk.public().as_bytes())
            .expect("derive fingerprint for test member");
        SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: "Test Device".to_string(),
            identity_fingerprint: fp,
            joined_at: Utc::now(),
            sync_preferences: MemberSyncPreferences::default(),
        }
    }

    fn sample_message() -> ActiveClipboardWireMessage {
        ActiveClipboardWireMessage {
            snapshot_hash: format!("blake3v1:{}", "9".repeat(64)),
            entry_id: "01941b00-0000-7000-8000-000000000001".to_string(),
            activated_at_ms: 1_700_000_000_000,
            activated_by: "sender-001".to_string(),
        }
    }

    struct ReceiverHarness {
        receiver_endpoint: Arc<Endpoint>,
        receiver_router: Router,
        inbound_rx: broadcast::Receiver<InboundActiveClipboardState>,
    }

    async fn spawn_receiver(
        seed: [u8; 32],
        member_repo: Arc<dyn MemberRepositoryPort>,
    ) -> ReceiverHarness {
        spawn_receiver_with_admission(seed, member_repo, true).await
    }

    async fn spawn_receiver_with_admission(
        seed: [u8; 32],
        member_repo: Arc<dyn MemberRepositoryPort>,
        admitted: bool,
    ) -> ReceiverHarness {
        let receiver_endpoint = bind_endpoint_with(seed).await;
        wait_for_direct_addrs(&receiver_endpoint).await;

        let adapter = ActiveStateReceiver::new(
            test_manager(receiver_endpoint.clone(), member_repo, admitted),
            Arc::new(super::super::wire::ActiveClipboardWire),
        );
        let inbound_rx = adapter.subscribe();
        let router = Router::builder((*receiver_endpoint).clone())
            .accept(
                ACTIVE_CLIPBOARD_ALPN,
                IrohActiveStateHandler::new(Arc::new(adapter)),
            )
            .spawn();

        ReceiverHarness {
            receiver_endpoint,
            receiver_router: router,
            inbound_rx,
        }
    }

    /// Drive one raw frame at the receiver from a known sender endpoint.
    async fn send_one_frame(
        sender_seed: [u8; 32],
        receiver_addr: iroh::EndpointAddr,
        msg: &ActiveClipboardWireMessage,
    ) {
        let sender_endpoint = bind_endpoint_with(sender_seed).await;
        wait_for_direct_addrs(&sender_endpoint).await;
        let conn = sender_endpoint
            .connect(receiver_addr, ACTIVE_CLIPBOARD_ALPN)
            .await
            .expect("dial receiver");
        let (mut send, _recv) = conn.open_bi().await.expect("open_bi");
        write_frame(&mut send, msg).await.expect("write frame");
        send.finish().expect("finish");
        let _ = conn.closed().await;
    }

    // ----- verdicts ----------------------------------------------------------

    /// Verdict 1 — a known peer's state observation reaches the broadcast
    /// stream with every field intact, and the peer is attributed correctly.
    #[tokio::test]
    async fn accepts_state_from_known_peer() {
        let sender_seed = [0x11u8; 32];
        let receiver_seed = [0x22u8; 32];

        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        let sender_member = make_member(sender_seed, "sender-a");
        member_repo.save(&sender_member).await.unwrap();

        let harness = spawn_receiver(receiver_seed, Arc::clone(&member_repo)).await;
        let receiver_addr = harness.receiver_endpoint.addr();

        let msg = sample_message();
        send_one_frame(sender_seed, receiver_addr, &msg).await;

        let mut inbound_rx = harness.inbound_rx;
        let inbound = tokio::time::timeout(Duration::from_secs(3), inbound_rx.recv())
            .await
            .expect("broadcast arrives within timeout")
            .expect("subscriber sees the observation");

        assert_eq!(inbound.peer_device_id.as_str(), "sender-a");
        assert_eq!(inbound.snapshot_hash, msg.snapshot_hash);
        assert_eq!(inbound.sender_entry_id, msg.entry_id);
        assert_eq!(inbound.activated_at_ms, msg.activated_at_ms);
        assert_eq!(inbound.activated_by.as_str(), msg.activated_by);

        harness.receiver_router.shutdown().await.ok();
    }

    /// Verdict 2 — an unknown peer (fingerprint not in member_repo) never
    /// publishes to the broadcast stream.
    #[tokio::test]
    async fn rejects_unknown_peer_without_publishing() {
        let sender_seed = [0x33u8; 32];
        let receiver_seed = [0x44u8; 32];

        // member_repo is empty — sender's fingerprint is unknown.
        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        let harness = spawn_receiver(receiver_seed, Arc::clone(&member_repo)).await;
        let receiver_addr = harness.receiver_endpoint.addr();

        send_one_frame(sender_seed, receiver_addr, &sample_message()).await;

        let mut inbound_rx = harness.inbound_rx;
        let fast_poll = tokio::time::timeout(Duration::from_millis(300), inbound_rx.recv()).await;
        assert!(fast_poll.is_err(), "unknown peer must not publish");

        harness.receiver_router.shutdown().await.ok();
    }

    /// Verdict 3 — a roster-resolved peer rejected by the current MLS group
    /// cannot publish an active clipboard observation.
    #[tokio::test]
    async fn rejects_known_but_unadmitted_peer_without_publishing() {
        let sender_seed = [0x45u8; 32];
        let receiver_seed = [0x46u8; 32];
        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        member_repo
            .save(&make_member(sender_seed, "revoked-sender"))
            .await
            .unwrap();
        let harness =
            spawn_receiver_with_admission(receiver_seed, Arc::clone(&member_repo), false).await;
        let receiver_addr = harness.receiver_endpoint.addr();

        send_one_frame(sender_seed, receiver_addr, &sample_message()).await;

        let mut inbound_rx = harness.inbound_rx;
        assert!(
            tokio::time::timeout(Duration::from_millis(300), inbound_rx.recv())
                .await
                .is_err(),
            "an unadmitted peer must not publish"
        );
        harness.receiver_router.shutdown().await.ok();
    }

    /// Verdict 4 — a known peer that writes a bad-magic frame is dropped
    /// without touching the broadcast stream.
    #[tokio::test]
    async fn drops_bad_magic_without_publishing() {
        let sender_seed = [0x55u8; 32];
        let receiver_seed = [0x66u8; 32];

        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        let sender_member = make_member(sender_seed, "known-bad-actor");
        member_repo.save(&sender_member).await.unwrap();

        let harness = spawn_receiver(receiver_seed, Arc::clone(&member_repo)).await;
        let receiver_addr = harness.receiver_endpoint.addr();

        let sender_endpoint = bind_endpoint_with(sender_seed).await;
        wait_for_direct_addrs(&sender_endpoint).await;
        let conn = sender_endpoint
            .connect(receiver_addr, ACTIVE_CLIPBOARD_ALPN)
            .await
            .expect("dial receiver");
        let (mut send, _recv) = conn.open_bi().await.expect("open_bi");
        // Wrong magic (0xC1) + garbage; receiver bails at the magic check.
        send.write_all(&[0xC1, 0x00, 0x00, 0x00, 0x05])
            .await
            .expect("write garbage");
        send.finish().expect("finish");
        let _ = conn.closed().await;

        let mut inbound_rx = harness.inbound_rx;
        let fast_poll = tokio::time::timeout(Duration::from_millis(300), inbound_rx.recv()).await;
        assert!(fast_poll.is_err(), "bad magic must not publish");

        harness.receiver_router.shutdown().await.ok();
    }

    /// Verdict 4 — a well-formed frame whose `activated_by` exceeds the
    /// device-id bound is dropped at the trust boundary instead of panicking
    /// the accept task, and never reaches the broadcast stream.
    #[tokio::test]
    async fn drops_overlong_activated_by_without_publishing() {
        use uc_core::ids::device_id::DEVICE_ID_MAX_BYTES;

        let sender_seed = [0x77u8; 32];
        let receiver_seed = [0x88u8; 32];

        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        let sender_member = make_member(sender_seed, "sender-a");
        member_repo.save(&sender_member).await.unwrap();

        let harness = spawn_receiver(receiver_seed, Arc::clone(&member_repo)).await;
        let receiver_addr = harness.receiver_endpoint.addr();

        let mut msg = sample_message();
        msg.activated_by = "x".repeat(DEVICE_ID_MAX_BYTES + 1);
        send_one_frame(sender_seed, receiver_addr, &msg).await;

        let mut inbound_rx = harness.inbound_rx;
        let fast_poll = tokio::time::timeout(Duration::from_millis(300), inbound_rx.recv()).await;
        assert!(
            fast_poll.is_err(),
            "over-long activated_by must not publish"
        );

        harness.receiver_router.shutdown().await.ok();
    }
    /// 协议入口起算：已认证成员不开流，也必须在总期限内关闭。
    #[tokio::test]
    async fn known_peer_without_stream_is_closed_within_exchange_deadline() {
        let sender_seed = [0x31u8; 32];
        let members: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        members
            .save(&make_member(sender_seed, "deadline-sender"))
            .await
            .unwrap();
        let harness = spawn_receiver([0x32; 32], members).await;
        let sender = bind_endpoint_with(sender_seed).await;
        let connection = sender
            .connect(harness.receiver_endpoint.addr(), ACTIVE_CLIPBOARD_ALPN)
            .await
            .unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(11), connection.closed()).await;
        connection.close(0u32.into(), b"test complete");
        harness.receiver_router.shutdown().await.unwrap();
        sender.close().await;
        assert!(
            closed.is_ok(),
            "no-stream input must not leave an unbounded handler"
        );
    }
}
