//! 拉取客户端真实 Iroh 合同测试；生产完整流程位于 Application。

#[cfg(test)]
mod tests {
    use super::super::testing::{content_scope, manager};
    use super::super::{ActiveClipboardWire, IrohActivePullHandler, SessionActivePullLocalWork};
    use crate::network::iroh::connectivity::ConnectivityProfile;
    use iroh::{Endpoint, EndpointAddr};
    use uc_application::deps::{
        ActivePullClient, ActivePullPreparationError, ActivePullPreparationPort,
        ActivePullPreparationStop, ActivePullReceiver,
    };
    use uc_core::ids::DeviceId;
    use uc_core::ports::{
        ActiveClipboardPullClientError, ActiveClipboardPullClientPort, PeerAddressRepositoryPort,
    };
    use uc_sync_protocol::active_clipboard_pull::ACTIVE_CLIPBOARD_PULL_ALPN;

    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use iroh::protocol::Router;
    use iroh::{RelayMode, SecretKey};
    use tokio::sync::Mutex;

    use uc_core::membership::{MembershipError, SpaceMember};
    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_core::ports::{PeerAddressError, PeerAddressRecord};
    use uc_core::{MemberRepositoryPort, MemberSyncPreferences};

    use uc_infra_crypto::identity_fingerprint::Sha256IdentityFingerprintFactory;

    // ----- in-memory peer_addr_repo -----------------------------------------

    #[derive(Default)]
    struct MemRepo {
        inner: Mutex<HashMap<String, PeerAddressRecord>>,
    }
    #[async_trait]
    impl PeerAddressRepositoryPort for MemRepo {
        async fn get(
            &self,
            device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(self.inner.lock().await.get(device.as_str()).cloned())
        }
        async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            self.inner
                .lock()
                .await
                .insert(record.device_id.as_str().to_string(), record.clone());
            Ok(())
        }
        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(self.inner.lock().await.values().cloned().collect())
        }
        async fn remove(&self, device: &DeviceId) -> Result<(), PeerAddressError> {
            self.inner.lock().await.remove(device.as_str());
            Ok(())
        }
    }

    // ----- in-memory member_repo (serve-side admission) ----------------------

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

    /// Serve port returning a canned result.
    struct StubServe(Mutex<Option<Result<Vec<u8>, ActivePullPreparationError>>>);
    impl StubServe {
        fn new(result: Result<Vec<u8>, ActivePullPreparationError>) -> Arc<Self> {
            Arc::new(Self(Mutex::new(Some(result))))
        }
    }
    #[async_trait]
    impl ActivePullPreparationPort for StubServe {
        async fn prepare(
            &self,
            _snapshot_hash: &str,
            _stop: &ActivePullPreparationStop,
        ) -> Result<Vec<u8>, ActivePullPreparationError> {
            self.0
                .lock()
                .await
                .take()
                .expect("serve called more than once")
        }
    }

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

    async fn seed_addr(repo: &MemRepo, device: &DeviceId, addr: &EndpointAddr) {
        let blob = postcard::to_stdvec(addr).expect("postcard encode EndpointAddr");
        repo.upsert(&PeerAddressRecord {
            device_id: device.clone(),
            addr_blob: blob,
            observed_at: Utc::now(),
        })
        .await
        .expect("upsert");
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

    /// Spawn a serve router for a member, returning its addr + router handle.
    async fn spawn_serve_for(
        serve_seed: [u8; 32],
        requester_seed: [u8; 32],
        requester_device: &str,
        serve_result: Result<Vec<u8>, ActivePullPreparationError>,
    ) -> (EndpointAddr, Router) {
        let member_repo: Arc<dyn MemberRepositoryPort> = Arc::new(MemMemberRepo::default());
        member_repo
            .save(&make_member(requester_seed, requester_device))
            .await
            .unwrap();
        let endpoint = bind_endpoint_with(serve_seed).await;
        wait_for_direct_addrs(&endpoint).await;
        let adapter = ActivePullReceiver::new(
            manager(
                endpoint.clone(),
                Arc::new(MemRepo::default()),
                crate::network::iroh::inbound_peer::member_table_directory(member_repo.clone()),
                true,
                ConnectivityProfile::ActiveClipboardPull,
            ),
            Arc::new(ActiveClipboardWire),
            StubServe::new(serve_result),
            content_scope(member_repo, true),
            Arc::new(SessionActivePullLocalWork),
        );
        let router = Router::builder((*endpoint).clone())
            .accept(
                ACTIVE_CLIPBOARD_PULL_ALPN,
                IrohActivePullHandler::new(Arc::new(adapter)),
            )
            .spawn();
        (endpoint.addr(), router)
    }

    async fn client_for(endpoint: Arc<Endpoint>, addresses: Arc<MemRepo>) -> ActivePullClient {
        let members = Arc::new(MemMemberRepo::default());
        for record in addresses.list().await.unwrap() {
            let address: EndpointAddr = postcard::from_bytes(&record.addr_blob).unwrap();
            let fingerprint = Sha256IdentityFingerprintFactory
                .from_public_key(address.id.as_bytes())
                .unwrap();
            members
                .save(&SpaceMember {
                    device_id: record.device_id,
                    device_name: "holder".into(),
                    identity_fingerprint: fingerprint,
                    joined_at: Utc::now(),
                    sync_preferences: MemberSyncPreferences::default(),
                })
                .await
                .unwrap();
        }
        ActivePullClient::new(
            manager(
                endpoint,
                addresses,
                crate::network::iroh::inbound_peer::member_table_directory(members),
                true,
                ConnectivityProfile::ActiveClipboardPull,
            ),
            Arc::new(ActiveClipboardWire),
        )
    }

    // ----- verdicts ----------------------------------------------------------

    /// Verdict 1 — end-to-end happy path: the client dials a live serve
    /// handler and returns the served envelope bytes verbatim.
    #[tokio::test]
    async fn pull_returns_served_envelope() {
        let requester_seed = [0x21u8; 32];
        let serve_seed = [0x22u8; 32];

        let envelope = vec![0x55, 0x43, 0x33, 0x00, 0xAB, 0xCD];
        let (serve_addr, router) = spawn_serve_for(
            serve_seed,
            requester_seed,
            "requester-x",
            Ok(envelope.clone()),
        )
        .await;

        let requester_endpoint = bind_endpoint_with(requester_seed).await;
        wait_for_direct_addrs(&requester_endpoint).await;
        let repo = Arc::new(MemRepo::default());
        let target = DeviceId::new("holder");
        seed_addr(&repo, &target, &serve_addr).await;

        let client = client_for(requester_endpoint, repo).await;
        let hash = format!("blake3v1:{}", "7".repeat(64));
        let got = client.pull(&target, &hash).await.expect("pull succeeds");
        assert_eq!(got, envelope);

        router.shutdown().await.ok();
    }

    /// Verdict 1b — the pull returns promptly, not after the serve side's 3s
    /// close-barrier timeout. Both ends used to wait on `connection.closed()`,
    /// so a successful pull against the real serve handler always burned the
    /// full `CLOSE_BARRIER_TIMEOUT` (3s) before returning — the dominant cost of
    /// the #1017 restore lag. The client now actively closes once the response
    /// is read, resolving the serve barrier immediately. A 2s bound clears the
    /// real exchange (low ms on loopback) by a wide margin while still failing
    /// hard if the 3s standoff is reintroduced.
    #[tokio::test]
    async fn pull_returns_before_close_barrier_timeout() {
        use std::time::Instant;

        let requester_seed = [0x23u8; 32];
        let serve_seed = [0x24u8; 32];

        let envelope = vec![0x55, 0x43, 0x33, 0x00, 0x99];
        let (serve_addr, router) = spawn_serve_for(
            serve_seed,
            requester_seed,
            "requester-fast",
            Ok(envelope.clone()),
        )
        .await;

        let requester_endpoint = bind_endpoint_with(requester_seed).await;
        wait_for_direct_addrs(&requester_endpoint).await;
        let repo = Arc::new(MemRepo::default());
        let target = DeviceId::new("holder");
        seed_addr(&repo, &target, &serve_addr).await;

        let client = client_for(requester_endpoint, repo).await;
        let hash = format!("blake3v1:{}", "5".repeat(64));

        let started = Instant::now();
        let got = client.pull(&target, &hash).await.expect("pull succeeds");
        let elapsed = started.elapsed();

        assert_eq!(got, envelope);
        assert!(
            elapsed < Duration::from_secs(2),
            "pull took {elapsed:?}; the close-barrier standoff (~3s) appears to be back",
        );

        router.shutdown().await.ok();
    }

    /// Verdict 2 — the holder responds `Locked`; the client maps it to
    /// `NotAvailable` (a locked holder cannot serve).
    #[tokio::test]
    async fn pull_maps_locked_holder_to_not_available() {
        let requester_seed = [0x31u8; 32];
        let serve_seed = [0x32u8; 32];

        let (serve_addr, router) = spawn_serve_for(
            serve_seed,
            requester_seed,
            "requester-x",
            Err(ActivePullPreparationError::NotUnlocked),
        )
        .await;

        let requester_endpoint = bind_endpoint_with(requester_seed).await;
        wait_for_direct_addrs(&requester_endpoint).await;
        let repo = Arc::new(MemRepo::default());
        let target = DeviceId::new("holder");
        seed_addr(&repo, &target, &serve_addr).await;

        let client = client_for(requester_endpoint, repo).await;
        let hash = format!("blake3v1:{}", "7".repeat(64));
        let err = client
            .pull(&target, &hash)
            .await
            .expect_err("locked holder must surface as error");
        assert!(matches!(err, ActiveClipboardPullClientError::NotAvailable));

        router.shutdown().await.ok();
    }

    /// Verdict 3 — no stored address for the peer → `Unreachable` without
    /// touching the network.
    #[tokio::test]
    async fn pull_returns_unreachable_when_addr_missing() {
        let requester_endpoint = bind_endpoint_with([0x41u8; 32]).await;
        let repo = Arc::new(MemRepo::default());
        let members = Arc::new(MemMemberRepo::default());
        let fingerprint = Sha256IdentityFingerprintFactory
            .from_public_key(SecretKey::from_bytes(&[0x42; 32]).public().as_bytes())
            .unwrap();
        members
            .save(&SpaceMember {
                device_id: DeviceId::new("known-without-address"),
                device_name: "holder".into(),
                identity_fingerprint: fingerprint,
                joined_at: Utc::now(),
                sync_preferences: MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let client = ActivePullClient::new(
            manager(
                requester_endpoint,
                repo,
                crate::network::iroh::inbound_peer::member_table_directory(members),
                true,
                ConnectivityProfile::ActiveClipboardPull,
            ),
            Arc::new(ActiveClipboardWire),
        );

        let err = client
            .pull(
                &DeviceId::new("known-without-address"),
                &format!("blake3v1:{}", "7".repeat(64)),
            )
            .await
            .expect_err("missing addr must surface as error");
        assert!(matches!(err, ActiveClipboardPullClientError::Unreachable));
    }
}
