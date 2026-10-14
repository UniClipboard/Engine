//! 原生 pull handler 只转换连接并调用完整服务，不编排准备步骤。
use super::super::connectivity::IrohPeerConnection;
use iroh::endpoint::Connection;
use iroh::protocol::{AcceptError, ProtocolHandler};
use std::sync::Arc;
use tokio::time::Instant;
use uc_application::deps::{ActivePullReceiverPort, ACTIVE_EXCHANGE_TIMEOUT};
use uc_observability_contract::diagnostics::{connectivity::NetworkRecorder, ObservationContext};
use uc_observability_contract::{error_source::io_error_kind, uc_warn};

#[derive(Clone)]
pub struct IrohActivePullHandler {
    receiver: Arc<dyn ActivePullReceiverPort>,
}
impl IrohActivePullHandler {
    pub fn new(receiver: Arc<dyn ActivePullReceiverPort>) -> Self {
        Self { receiver }
    }
}
impl std::fmt::Debug for IrohActivePullHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IrohActivePullHandler")
            .finish_non_exhaustive()
    }
}
impl ProtocolHandler for IrohActivePullHandler {
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
                        error_kind = "active_pull_receive",
                        io_error_kind = io_error_kind(&error),
                        "active pull receive failed"
                    );
                }
                Ok(())
            })
            .await
    }

    async fn shutdown(&self) {
        if let Err(error) = self.receiver.shutdown().await {
            uc_warn!(
                error_kind = "active_pull_shutdown",
                io_error_kind = io_error_kind(&error),
                "active pull connectivity shutdown failed"
            );
        }
    }
}
#[cfg(test)]
mod tests {
    use super::super::testing::{content_scope, manager, EmptyAddresses};
    use super::super::{ActiveClipboardWire, SessionActivePullLocalWork};
    use super::*;
    use crate::network::iroh::connectivity::ConnectivityProfile;
    use uc_application::deps::{
        ActivePullPreparationError, ActivePullPreparationPort, ActivePullPreparationStop,
        ActivePullReceiver,
    };
    use uc_core::ids::DeviceId;
    use uc_core::membership::MemberRepositoryPort;
    use uc_sync_protocol::active_clipboard_pull::ACTIVE_CLIPBOARD_PULL_ALPN;

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

    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;
    use uc_sync_protocol::active_clipboard_pull::{
        read_response, write_request, PullResponse, ACTIVE_PULL_MAGIC,
    };

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

    /// Serve port that records the requested hash and returns a canned result.
    struct StubServe {
        result: Mutex<Option<Result<Vec<u8>, ActivePullPreparationError>>>,
        seen_hash: Mutex<Option<String>>,
    }
    impl StubServe {
        fn new(result: Result<Vec<u8>, ActivePullPreparationError>) -> Arc<Self> {
            Arc::new(Self {
                result: Mutex::new(Some(result)),
                seen_hash: Mutex::new(None),
            })
        }
    }
    #[async_trait]
    impl ActivePullPreparationPort for StubServe {
        async fn prepare(
            &self,
            snapshot_hash: &str,
            _stop: &ActivePullPreparationStop,
        ) -> Result<Vec<u8>, ActivePullPreparationError> {
            *self.seen_hash.lock().await = Some(snapshot_hash.to_string());
            self.result
                .lock()
                .await
                .take()
                .expect("serve called more than once")
        }
    }

    /// Serve port that panics if `serve` is ever reached — proves an unknown
    /// peer never reaches the serve port.
    struct NeverServe;
    #[async_trait]
    impl ActivePullPreparationPort for NeverServe {
        async fn prepare(
            &self,
            _snapshot_hash: &str,
            _stop: &ActivePullPreparationStop,
        ) -> Result<Vec<u8>, ActivePullPreparationError> {
            panic!("serve reached past the admission gate");
        }
    }

    // ----- harness -----------------------------------------------------------

    async fn bind_endpoint_with(seed: [u8; 32]) -> Arc<Endpoint> {
        Arc::new(
            Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(SecretKey::from_bytes(&seed))
                .alpns(vec![ACTIVE_CLIPBOARD_PULL_ALPN.to_vec()])
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

    async fn spawn_serve(
        seed: [u8; 32],
        member_repo: Arc<dyn MemberRepositoryPort>,
        serve: Arc<dyn ActivePullPreparationPort>,
    ) -> (Arc<Endpoint>, Router) {
        spawn_serve_with_admission(seed, member_repo, serve, true, true).await
    }

    async fn spawn_serve_with_admission(
        seed: [u8; 32],
        member_repo: Arc<dyn MemberRepositoryPort>,
        serve: Arc<dyn ActivePullPreparationPort>,
        admitted: bool,
        content_allowed: bool,
    ) -> (Arc<Endpoint>, Router) {
        let endpoint = bind_endpoint_with(seed).await;
        wait_for_direct_addrs(&endpoint).await;
        let adapter = ActivePullReceiver::new(
            manager(
                endpoint.clone(),
                Arc::new(EmptyAddresses),
                crate::network::iroh::inbound_peer::member_table_directory(member_repo.clone()),
                admitted,
                ConnectivityProfile::ActiveClipboardPull,
            ),
            Arc::new(ActiveClipboardWire),
            serve,
            content_scope(member_repo, content_allowed),
            Arc::new(SessionActivePullLocalWork),
        );
        let router = Router::builder((*endpoint).clone())
            .accept(
                ACTIVE_CLIPBOARD_PULL_ALPN,
                IrohActivePullHandler::new(Arc::new(adapter)),
            )
            .spawn();
        (endpoint, router)
    }

    /// Open a bi-stream, send a request, return the decoded response.
    async fn pull_request(
        sender_seed: [u8; 32],
        receiver_addr: iroh::EndpointAddr,
        snapshot_hash: &str,
    ) -> Result<PullResponse, String> {
        let sender = bind_endpoint_with(sender_seed).await;
        wait_for_direct_addrs(&sender).await;
        let conn = sender
            .connect(receiver_addr, ACTIVE_CLIPBOARD_PULL_ALPN)
            .await
            .expect("dial serve");
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|err| format!("open stream: {err}"))?;
        write_request(&mut send, snapshot_hash)
            .await
            .map_err(|err| format!("write request: {err}"))?;
        send.finish()
            .map_err(|err| format!("finish request: {err}"))?;
        let resp = read_response(&mut recv)
            .await
            .map_err(|err| format!("read response: {err}"));
        let _ = conn.closed().await;
        resp
    }

    // ----- verdicts ----------------------------------------------------------

    /// Verdict 1 — a known member's pull is served: the requested hash reaches
    /// the serve port and the envelope round-trips back verbatim.
    #[tokio::test]
    async fn member_pull_is_served_with_envelope() {
        let sender_seed = [0x11u8; 32];
        let receiver_seed = [0x22u8; 32];

        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        member_repo
            .save(&make_member(sender_seed, "member-a"))
            .await
            .unwrap();

        let envelope = vec![0x55, 0x43, 0x33, 0x00, 0x01, 0x02, 0x03];
        let serve = StubServe::new(Ok(envelope.clone()));
        let (endpoint, router) = spawn_serve(
            receiver_seed,
            Arc::clone(&member_repo),
            Arc::clone(&serve) as _,
        )
        .await;

        let hash = format!("blake3v1:{}", "9".repeat(64));
        let resp = pull_request(sender_seed, endpoint.addr(), &hash)
            .await
            .expect("response decodes");

        assert_eq!(resp, PullResponse::Envelope(envelope));
        assert_eq!(serve.seen_hash.lock().await.as_deref(), Some(hash.as_str()));

        router.shutdown().await.ok();
    }

    /// 服务端内部失败只记固定分类与 io 类别，不带错误正文，并向对端回 Internal。
    #[tokio::test]
    async fn internal_serve_failure_is_recorded_without_its_message() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();
        let sender_seed = [0x51u8; 32];
        let receiver_seed = [0x52u8; 32];
        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        member_repo
            .save(&make_member(sender_seed, "member-internal"))
            .await
            .unwrap();
        let serve = StubServe::new(Err(ActivePullPreparationError::Internal(Box::new(
            std::io::Error::other("PRIVATE_SERVE_DETAIL"),
        ))));
        let (endpoint, router) = spawn_serve(
            receiver_seed,
            Arc::clone(&member_repo),
            Arc::clone(&serve) as _,
        )
        .await;

        let hash = format!("blake3v1:{}", "9".repeat(64));
        let resp = pull_request(sender_seed, endpoint.addr(), &hash)
            .await
            .expect("response decodes");

        assert_eq!(resp, PullResponse::Internal);
        assert_eq!(
            logs.count("active pull local work failed"),
            1,
            "{}",
            logs.output()
        );
        let output = logs.output();
        assert!(output.contains("error_kind=\"serve_internal\""), "{output}");
        assert!(output.contains("io_error_kind=Other"), "{output}");
        assert!(!output.contains("PRIVATE_SERVE_DETAIL"), "{output}");
        router.shutdown().await.ok();
    }

    /// Verdict 2 — an unknown peer (fingerprint not in member_repo) is dropped
    /// at the admission gate; the serve port is never reached and the dial
    /// surfaces as a closed connection / io error (no response frame).
    #[tokio::test]
    async fn unknown_peer_is_dropped_before_serve() {
        let sender_seed = [0x33u8; 32];
        let receiver_seed = [0x44u8; 32];

        // member_repo is empty — sender's fingerprint is unknown.
        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        let (endpoint, router) = spawn_serve(
            receiver_seed,
            Arc::clone(&member_repo),
            Arc::new(NeverServe),
        )
        .await;

        let hash = format!("blake3v1:{}", "9".repeat(64));
        let resp = pull_request(sender_seed, endpoint.addr(), &hash).await;
        // The handler returns Ok(()) immediately without writing a frame, so
        // the requester's read hits EOF → Io error.
        assert!(resp.is_err(), "unknown peer must not get a response frame");

        router.shutdown().await.ok();
    }

    /// Verdict 3 — a roster-resolved peer rejected by the current MLS group
    /// receives no pull response and cannot reach the content serve port.
    #[tokio::test]
    async fn known_but_unadmitted_peer_is_dropped_before_serve() {
        let sender_seed = [0x45u8; 32];
        let receiver_seed = [0x46u8; 32];
        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        member_repo
            .save(&make_member(sender_seed, "revoked-member"))
            .await
            .unwrap();
        let (endpoint, router) = spawn_serve_with_admission(
            receiver_seed,
            Arc::clone(&member_repo),
            Arc::new(NeverServe),
            false,
            true,
        )
        .await;

        let hash = format!("blake3v1:{}", "9".repeat(64));
        assert!(
            pull_request(sender_seed, endpoint.addr(), &hash)
                .await
                .is_err(),
            "an unadmitted peer must not receive a response frame"
        );
        router.shutdown().await.ok();
    }

    #[tokio::test]
    async fn upgrade_required_member_receives_no_content() {
        let sender_seed = [0x47u8; 32];
        let receiver_seed = [0x48u8; 32];
        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        member_repo
            .save(&make_member(sender_seed, "upgrade-required-member"))
            .await
            .unwrap();
        let (endpoint, router) = spawn_serve_with_admission(
            receiver_seed,
            Arc::clone(&member_repo),
            Arc::new(NeverServe),
            true,
            false,
        )
        .await;

        let hash = format!("blake3v1:{}", "9".repeat(64));
        let response = pull_request(sender_seed, endpoint.addr(), &hash)
            .await
            .expect("a blocked peer receives an explicit no-content response");
        assert_eq!(response, PullResponse::NotAvailable);
        router.shutdown().await.ok();
    }

    /// Verdict 4 — a locked holder responds `Locked` without panicking and
    /// without leaking any envelope bytes.
    #[tokio::test]
    async fn locked_holder_responds_locked() {
        let sender_seed = [0x55u8; 32];
        let receiver_seed = [0x66u8; 32];

        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        member_repo
            .save(&make_member(sender_seed, "member-a"))
            .await
            .unwrap();

        let serve = StubServe::new(Err(ActivePullPreparationError::NotUnlocked));
        let (endpoint, router) = spawn_serve(receiver_seed, Arc::clone(&member_repo), serve).await;

        let hash = format!("blake3v1:{}", "9".repeat(64));
        let resp = pull_request(sender_seed, endpoint.addr(), &hash)
            .await
            .expect("response decodes");
        assert_eq!(resp, PullResponse::Locked);

        router.shutdown().await.ok();
    }

    /// Verdict 4 — a member asking for content the holder does not hold gets
    /// `NotAvailable`.
    #[tokio::test]
    async fn missing_content_responds_not_available() {
        let sender_seed = [0x77u8; 32];
        let receiver_seed = [0x88u8; 32];

        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        member_repo
            .save(&make_member(sender_seed, "member-a"))
            .await
            .unwrap();

        let serve = StubServe::new(Err(ActivePullPreparationError::NotAvailable));
        let (endpoint, router) = spawn_serve(receiver_seed, Arc::clone(&member_repo), serve).await;

        let hash = format!("blake3v1:{}", "9".repeat(64));
        let resp = pull_request(sender_seed, endpoint.addr(), &hash)
            .await
            .expect("response decodes");
        assert_eq!(resp, PullResponse::NotAvailable);

        router.shutdown().await.ok();
    }
    /// 完整服务入口的预算覆盖不开流和只写半个帧头的对端。
    #[tokio::test]
    async fn silent_and_partial_request_close_within_entry_deadline() {
        for partial in [false, true] {
            let sender_seed = [0x61; 32];
            let members: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
            members
                .save(&make_member(sender_seed, "deadline-member"))
                .await
                .unwrap();
            let serve = StubServe::new(Ok(vec![1]));
            let (endpoint, router) = spawn_serve([0x62; 32], members, serve.clone()).await;
            let sender = bind_endpoint_with(sender_seed).await;
            let connection = sender
                .connect(endpoint.addr(), ACTIVE_CLIPBOARD_PULL_ALPN)
                .await
                .unwrap();
            let held = if partial {
                let (mut send, receive) = connection.open_bi().await.unwrap();
                send.write_all(&[ACTIVE_PULL_MAGIC]).await.unwrap();
                Some((send, receive))
            } else {
                None
            };
            let closed = tokio::time::timeout(Duration::from_secs(11), connection.closed()).await;
            drop(held);
            connection.close(0u32.into(), b"test complete");
            router.shutdown().await.unwrap();
            sender.close().await;
            assert!(
                closed.is_ok(),
                "silent or partial request cannot outlive the entry budget"
            );
            assert!(
                serve.seen_hash.lock().await.is_none(),
                "incomplete frame cannot begin preparation"
            );
        }
    }
}
