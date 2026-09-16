use super::*;

struct ControlledAdmission {
    blocked: AtomicBool,
    started: tokio::sync::Notify,
    proceed: tokio::sync::Semaphore,
}

#[tokio::test]
async fn stale_online_without_a_retained_connection_settles_before_redial() {
    let endpoint = bound_endpoint().await;
    let addresses = Arc::new(FakePeerAddressRepo::default());
    let gate = Arc::new(DelayedAdmission {
        checking: tokio::sync::Notify::new(),
        proceed: tokio::sync::Semaphore::new(0),
    });
    *addresses.delay.lock().unwrap() = Some(gate.clone());
    let adapter = Arc::new(build_adapter(endpoint.clone(), addresses));
    let device = DeviceId::new("stale-online");
    adapter
        .last_state
        .lock()
        .await
        .insert(device, ReachabilityState::Online);

    let verification = tokio::spawn({
        let adapter = adapter.clone();
        async move { adapter.verify_reachable(&device).await }
    });
    timeout(Duration::from_secs(1), gate.checking.notified())
        .await
        .unwrap();

    assert_eq!(
        adapter.current_state(&device).await,
        ReachabilityState::Offline,
        "an online cache without a retained connection must not survive a blocked redial"
    );

    gate.proceed.add_permits(1);
    assert!(matches!(
        verification.await.unwrap(),
        Err(PeerReachabilityError::NoAddress(id)) if id == device
    ));
    endpoint.close().await;
}

#[async_trait]
impl PeerAdmissionPort for ControlledAdmission {
    async fn is_admitted(
        &self,
        _: &DeviceId,
    ) -> Result<bool, uc_core::membership::PeerAdmissionError> {
        if self.blocked.load(Ordering::Acquire) {
            self.started.notify_one();
            self.proceed.acquire().await.unwrap().forget();
        }
        Ok(true)
    }
}

#[tokio::test]
async fn delayed_existing_check_preserves_revocation_and_new_connection_success() {
    for action in ["forget", "disconnect", "replace"] {
        let a = bound_endpoint().await;
        let b = bound_endpoint().await;
        wait_for_direct_addrs(&b).await;
        wait_for_direct_addrs(&a).await;
        let b_id = DeviceId::new("b");
        let addresses = Arc::new(FakePeerAddressRepo::default());
        addresses.seed(record(&b_id, postcard::to_stdvec(&b.addr()).unwrap()));
        let a_members = Arc::new(MemMemberRepo::default());
        a_members.seed(member_for_endpoint(&b, "b"));
        let a_adapter = Arc::new(build_adapter_with_member_repo(
            a.clone(),
            addresses,
            a_members,
        ));
        let a_router = Router::builder((*a).clone())
            .accept(PEER_REACHABILITY_ALPN, a_adapter.handler())
            .spawn();
        let members = Arc::new(MemMemberRepo::default());
        members.seed(member_for_endpoint(&a, "a"));
        let admission = Arc::new(ControlledAdmission {
            blocked: AtomicBool::new(false),
            started: tokio::sync::Notify::new(),
            proceed: tokio::sync::Semaphore::new(0),
        });
        let b_adapter = IrohPeerReachabilityAdapter::new(
            b.clone(),
            Arc::new(FakePeerAddressRepo::default()),
            members,
            admission.clone(),
            Arc::new(Sha256IdentityFingerprintFactory),
            Arc::new(FixedClock),
        );
        let router = Router::builder((*b).clone())
            .accept(PEER_REACHABILITY_ALPN, b_adapter.handler())
            .spawn();
        assert_eq!(
            a_adapter.ensure_reachable(&b_id).await.unwrap(),
            ReachabilityState::Online
        );
        // Finish an exchange before blocking subsequent liveness requests.
        assert_eq!(
            a_adapter.verify_reachable(&b_id).await.unwrap(),
            ReachabilityState::Online
        );
        admission.blocked.store(true, Ordering::Release);
        let check = tokio::spawn({
            let adapter = a_adapter.clone();
            async move { adapter.verify_reachable(&b_id).await }
        });
        timeout(Duration::from_secs(2), admission.started.notified())
            .await
            .unwrap();
        let fresh_connection = if action == "replace" {
            let fresh = b.connect(a.addr(), PEER_REACHABILITY_ALPN).await.unwrap();
            assert_eq!(
                request_admission_confirmation(&fresh).await,
                ADMISSION_ACCEPTED
            );
            timeout(Duration::from_secs(2), async {
                while a_adapter
                    .handler_state
                    .inbound_connections
                    .lock()
                    .await
                    .is_empty()
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            a_adapter.peers.lock().await[&b_id]
                .connection
                .close(0u32.into(), b"old connection failed");
            Some(fresh)
        } else {
            if action == "disconnect" {
                a_adapter.disconnect_all().await
            } else {
                a_adapter.forget(&b_id).await
            }
            None
        };
        admission.proceed.add_permits(1);
        let result = timeout(Duration::from_secs(2), check)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Some(fresh) = fresh_connection {
            assert_eq!(result, ReachabilityState::Online);
            assert_eq!(
                a_adapter.current_state(&b_id).await,
                ReachabilityState::Online
            );
            assert!(fresh.close_reason().is_none());
        } else {
            assert_ne!(result, ReachabilityState::Online);
            assert_ne!(
                a_adapter.current_state(&b_id).await,
                ReachabilityState::Online
            );
            assert!(a_adapter.peers.lock().await.is_empty());
            assert!(a_adapter.observations.lock().await.verified.is_empty());
        }
        a_adapter.disconnect_all().await;
        b_adapter.disconnect_all().await;
        router.shutdown().await.unwrap();
        a_router.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn one_connection_supports_checks_from_both_directions_without_redial() {
    let a = bound_endpoint().await;
    let b = bound_endpoint().await;
    wait_for_direct_addrs(&b).await;
    let a_id = DeviceId::new("a");
    let b_id = DeviceId::new("b");
    let addresses = Arc::new(FakePeerAddressRepo::default());
    addresses.seed(record(&b_id, postcard::to_stdvec(&b.addr()).unwrap()));
    let a_adapter = build_adapter(a.clone(), addresses.clone());
    let members = Arc::new(MemMemberRepo::default());
    members.seed(member_for_endpoint(&a, "a"));
    let b_adapter = build_adapter_with_member_repo(
        b.clone(),
        Arc::new(FakePeerAddressRepo::default()),
        members,
    );
    let router = Router::builder((*b).clone())
        .accept(PEER_REACHABILITY_ALPN, b_adapter.handler())
        .spawn();
    assert_eq!(
        a_adapter.ensure_reachable(&b_id).await.unwrap(),
        ReachabilityState::Online
    );
    addresses.remove(&b_id).await.unwrap();
    let original = a_adapter.peers.lock().await[&b_id].connection.stable_id();
    assert!(b_adapter.peers.lock().await.is_empty());
    for _ in 0..3 {
        let (outgoing, incoming) = tokio::join!(
            a_adapter.verify_reachable(&b_id),
            b_adapter.verify_reachable(&a_id)
        );
        assert_eq!(outgoing.unwrap(), ReachabilityState::Online);
        assert_eq!(incoming.unwrap(), ReachabilityState::Online);
        assert_eq!(
            a_adapter.peers.lock().await[&b_id].connection.stable_id(),
            original
        );
        assert_eq!(
            b_adapter
                .handler_state
                .inbound_connections
                .lock()
                .await
                .len(),
            1
        );
        assert!(b_adapter.peers.lock().await.is_empty());
    }
    a_adapter.disconnect_all().await;
    b_adapter.disconnect_all().await;
    router.shutdown().await.unwrap();
    a.close().await;
}

#[tokio::test]
async fn peer_rejection_wins_over_an_alive_parallel_connection() {
    let a = bound_endpoint().await;
    let b = bound_endpoint().await;
    wait_for_direct_addrs(&a).await;
    wait_for_direct_addrs(&b).await;
    let b_id = DeviceId::new("b");
    let members = Arc::new(MemMemberRepo::default());
    members.seed(member_for_endpoint(&b, "b"));
    let adapter = Arc::new(build_adapter_with_member_repo(
        a.clone(),
        Arc::new(FakePeerAddressRepo::default()),
        members,
    ));
    let router = Router::builder((*a).clone())
        .accept(PEER_REACHABILITY_ALPN, adapter.handler())
        .spawn();

    let alive = b.connect(a.addr(), PEER_REACHABILITY_ALPN).await.unwrap();
    assert_eq!(
        request_admission_confirmation(&alive).await,
        ADMISSION_ACCEPTED
    );
    let rejected = b.connect(a.addr(), PEER_REACHABILITY_ALPN).await.unwrap();
    assert_eq!(
        request_admission_confirmation(&rejected).await,
        ADMISSION_ACCEPTED
    );
    timeout(Duration::from_secs(2), async {
        while adapter.handler_state.inbound_connections.lock().await.len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let alive_replied = Arc::new(tokio::sync::Notify::new());
    let release_rejection = Arc::new(tokio::sync::Semaphore::new(0));
    let alive_response = tokio::spawn({
        let alive_replied = alive_replied.clone();
        async move {
            reply_to_liveness(&alive, true).await;
            alive_replied.notify_one();
        }
    });
    let rejected_response = tokio::spawn({
        let release_rejection = release_rejection.clone();
        async move {
            let (mut send, mut receive) = rejected.accept_bi().await.unwrap();
            let bytes = receive
                .read_to_end(peer_reachability_protocol::FRAME_SIZE)
                .await
                .unwrap();
            let challenge = peer_reachability_protocol::challenge(&bytes).unwrap();
            release_rejection.acquire().await.unwrap().forget();
            send.write_all(&peer_reachability_protocol::reply(&challenge, false))
                .await
                .unwrap();
            send.finish().unwrap();
        }
    });
    let check = tokio::spawn({
        let adapter = adapter.clone();
        async move { adapter.verify_reachable(&b_id).await }
    });

    timeout(Duration::from_secs(2), alive_replied.notified())
        .await
        .unwrap();
    release_rejection.add_permits(1);
    assert_eq!(
        timeout(Duration::from_secs(2), check)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        ReachabilityState::Offline
    );
    assert_eq!(
        adapter.current_state(&b_id).await,
        ReachabilityState::Offline
    );
    assert!(adapter
        .handler_state
        .inbound_connections
        .lock()
        .await
        .is_empty());
    alive_response.await.unwrap();
    rejected_response.await.unwrap();
    adapter.disconnect_all().await;
    router.shutdown().await.unwrap();
    b.close().await;
}

async fn reply_to_liveness(connection: &Connection, admitted: bool) {
    let (mut send, mut receive) = connection.accept_bi().await.unwrap();
    let bytes = receive
        .read_to_end(peer_reachability_protocol::FRAME_SIZE)
        .await
        .unwrap();
    let challenge = peer_reachability_protocol::challenge(&bytes).unwrap();
    send.write_all(&peer_reachability_protocol::reply(&challenge, admitted))
        .await
        .unwrap();
    send.finish().unwrap();
}

#[tokio::test]
async fn repeated_admitted_inbound_connections_have_a_fixed_limit() {
    let a = bound_endpoint().await;
    let b = bound_endpoint().await;
    wait_for_direct_addrs(&b).await;
    let members = Arc::new(MemMemberRepo::default());
    members.seed(member_for_endpoint(&a, "a"));
    let adapter = build_adapter_with_member_repo(
        b.clone(),
        Arc::new(FakePeerAddressRepo::default()),
        members,
    );
    let router = Router::builder((*b).clone())
        .accept(PEER_REACHABILITY_ALPN, adapter.handler())
        .spawn();
    let mut connections = Vec::new();
    for _ in 0..8 {
        let connection = a.connect(b.addr(), PEER_REACHABILITY_ALPN).await.unwrap();
        assert_eq!(
            request_admission_confirmation(&connection).await,
            ADMISSION_ACCEPTED
        );
        connections.push(connection);
        tokio::task::yield_now().await;
        assert!(adapter.handler_state.inbound_connections.lock().await.len() <= 2);
    }
    adapter.disconnect_all().await;
    for connection in connections {
        timeout(Duration::from_secs(2), connection.closed())
            .await
            .unwrap();
    }
    assert!(adapter
        .handler_state
        .inbound_connections
        .lock()
        .await
        .is_empty());
    assert!(adapter.observations.lock().await.verified.is_empty());
    router.shutdown().await.unwrap();
    a.close().await;
}
