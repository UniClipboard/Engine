use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::broadcast;
use uc_core::ports::{PeerReachabilityChanged, PresenceError};

struct Scope {
    peers: Mutex<Vec<DeviceId>>,
    changes: watch::Sender<()>,
    unavailable: AtomicBool,
    reads: AtomicUsize,
    stall_on_read: AtomicUsize,
}

#[async_trait::async_trait]
impl CurrentSpaceMemberScopePort for Scope {
    async fn snapshot(
        &self,
    ) -> Result<crate::deps::CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        if self.stall_on_read.load(Ordering::SeqCst) == read {
            futures::future::pending::<()>().await;
        }
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(CurrentSpaceMemberScopeError::Unavailable);
        }
        Ok(crate::deps::CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: true,
            usable_peer_device_ids: self.peers.lock().await.clone(),
            paused_peer_devices: vec![],
        })
    }
    fn subscribe_changes(&self) -> watch::Receiver<()> {
        self.changes.subscribe()
    }
}

struct Presence {
    reachable: AtomicBool,
    calls: AtomicUsize,
    events: broadcast::Sender<PeerReachabilityChanged>,
    states: Mutex<HashMap<DeviceId, ReachabilityState>>,
    blocked: AtomicBool,
    permits: tokio::sync::Semaphore,
    active: AtomicUsize,
    maximum: AtomicUsize,
}

#[async_trait::async_trait]
impl PeerReachabilityPort for Presence {
    async fn ensure_reachable(&self, peer: &DeviceId) -> Result<ReachabilityState, PresenceError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        let _guard = ActiveDial(&self.active);
        if self.blocked.load(Ordering::SeqCst) {
            self.permits.acquire().await.unwrap().forget();
        }
        let state = if self.reachable.load(Ordering::SeqCst) {
            ReachabilityState::Online
        } else {
            ReachabilityState::Offline
        };
        self.states.lock().await.insert(*peer, state);
        let _ = self.events.send(PeerReachabilityChanged {
            device_id: *peer,
            state,
            at: chrono::Utc::now(),
        });
        Ok(state)
    }
    async fn current_state(&self, peer: &DeviceId) -> ReachabilityState {
        self.states
            .lock()
            .await
            .get(peer)
            .copied()
            .unwrap_or(ReachabilityState::Unknown)
    }
    fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
        self.events.subscribe()
    }
    async fn disconnect_all(&self) {
        self.states.lock().await.clear();
    }
    async fn forget(&self, peer: &DeviceId) {
        self.states.lock().await.remove(peer);
    }
}

fn fixture() -> (Arc<PeerConnectionCoordinator>, Arc<Scope>, Arc<Presence>) {
    let scope = Arc::new(Scope {
        peers: Mutex::new(vec![DeviceId::new("peer")]),
        changes: watch::channel(()).0,
        unavailable: AtomicBool::new(false),
        reads: AtomicUsize::new(0),
        stall_on_read: AtomicUsize::new(0),
    });
    let presence = Arc::new(Presence {
        reachable: AtomicBool::new(false),
        calls: AtomicUsize::new(0),
        events: broadcast::channel(64).0,
        states: Mutex::new(HashMap::new()),
        blocked: AtomicBool::new(false),
        permits: tokio::sync::Semaphore::new(0),
        active: AtomicUsize::new(0),
        maximum: AtomicUsize::new(0),
    });
    let owner = PeerConnectionCoordinator::new(
        scope.clone(),
        presence.clone(),
        Box::pin(futures::stream::pending()),
    );
    (owner, scope, presence)
}

async fn settle() {
    for _ in 0..30 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn startup_and_later_online_need_no_refresh() {
    let (owner, _, presence) = fixture();
    owner.start().await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
    presence.reachable.store(true, Ordering::SeqCst);
    tokio::time::advance(Duration::from_millis(1201)).await;
    settle().await;
    assert_eq!(
        presence.current_state(&DeviceId::new("peer")).await,
        ReachabilityState::Online
    );
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn own_offline_events_do_not_bypass_backoff() {
    let (owner, _, presence) = fixture();
    owner.start().await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(Duration::from_millis(999)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn empty_scope_is_woken_when_a_member_becomes_eligible() {
    let (owner, scope, presence) = fixture();
    scope.peers.lock().await.clear();
    owner.start().await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 0);
    scope.peers.lock().await.push(DeviceId::new("new"));
    scope.changes.send_replace(());
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn pause_stops_retries_and_resume_restarts_them() {
    let (owner, _, presence) = fixture();
    owner.start().await;
    settle().await;
    owner.pause().await.unwrap();
    let calls = presence.calls.load(Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(1000)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), calls);
    owner.resume().await.unwrap();
    settle().await;
    assert!(presence.calls.load(Ordering::SeqCst) > calls);
    owner.shutdown().await.unwrap();
}

struct ActiveDial<'a>(&'a AtomicUsize);
impl Drop for ActiveDial<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[tokio::test(start_paused = true)]
async fn every_host_opportunity_bypasses_long_backoff() {
    for reason in [
        ConnectivityOpportunity::Foreground,
        ConnectivityOpportunity::SystemWake,
        ConnectivityOpportunity::NetworkChanged,
    ] {
        let (owner, _, presence) = fixture();
        owner.start().await;
        settle().await;
        for _ in 0..6 {
            tokio::time::advance(Duration::from_secs(80)).await;
            settle().await;
        }
        let before = presence.calls.load(Ordering::SeqCst);
        owner.notify_opportunity(reason).unwrap();
        settle().await;
        tokio::time::advance(Duration::from_millis(1001)).await;
        settle().await;
        assert_eq!(presence.calls.load(Ordering::SeqCst), before + 1);
        owner.shutdown().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn burst_during_a_dial_is_bounded_and_coalesced() {
    let (owner, _, presence) = fixture();
    presence.blocked.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    for _ in 0..1000 {
        owner
            .notify_opportunity(ConnectivityOpportunity::NetworkChanged)
            .unwrap();
    }
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
    presence.permits.add_permits(1);
    settle().await;
    tokio::time::advance(Duration::from_millis(1001)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 2);
    assert_eq!(presence.maximum.load(Ordering::SeqCst), 1);
    owner.shutdown().await.unwrap();
    assert_eq!(presence.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn manual_refresh_joins_automatic_dial_and_waiter_cancellation_is_local() {
    let (owner, _, presence) = fixture();
    presence.blocked.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    let first = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh().await }
    });
    let second = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh().await }
    });
    settle().await;
    first.abort();
    let _ = first.await;
    presence.permits.add_permits(1);
    let report = second.await.unwrap().unwrap();
    assert_eq!((report.total, report.offline, report.errors), (1, 1, 0));
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn removed_target_cancels_in_flight_dial_without_late_online() {
    let (owner, scope, presence) = fixture();
    presence.blocked.store(true, Ordering::SeqCst);
    presence.reachable.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    assert_eq!(presence.active.load(Ordering::SeqCst), 1);
    scope.peers.lock().await.clear();
    scope.changes.send_replace(());
    settle().await;
    presence.permits.add_permits(1);
    settle().await;
    assert_eq!(presence.active.load(Ordering::SeqCst), 0);
    assert_eq!(
        presence.current_state(&DeviceId::new("peer")).await,
        ReachabilityState::Unknown
    );
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn slow_peers_are_bounded_and_do_not_starve_the_rest() {
    let (owner, scope, presence) = fixture();
    *scope.peers.lock().await = (0..8).map(|i| DeviceId::new(format!("peer-{i}"))).collect();
    presence.blocked.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    assert_eq!(presence.active.load(Ordering::SeqCst), 4);
    tokio::time::advance(Duration::from_secs(10)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 8);
    assert_eq!(presence.maximum.load(Ordering::SeqCst), 4);
    owner.shutdown().await.unwrap();
    assert_eq!(presence.active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn retry_never_sleeps_forever_and_healthy_connection_is_reused() {
    let (owner, _, presence) = fixture();
    owner.start().await;
    settle().await;
    for _ in 0..12 {
        let before = presence.calls.load(Ordering::SeqCst);
        tokio::time::advance(Duration::from_millis(72_001)).await;
        settle().await;
        assert!(presence.calls.load(Ordering::SeqCst) > before);
    }
    presence.reachable.store(true, Ordering::SeqCst);
    tokio::time::advance(Duration::from_millis(72_001)).await;
    settle().await;
    let before = presence.calls.load(Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(3600)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), before);
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_work_and_rejects_late_opportunities() {
    let (owner, _, presence) = fixture();
    presence.blocked.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    owner.shutdown().await.unwrap();
    assert_eq!(presence.active.load(Ordering::SeqCst), 0);
    assert!(matches!(
        owner.notify_opportunity(ConnectivityOpportunity::Foreground),
        Err(PeerConnectionError::Closed)
    ));
    assert!(matches!(
        owner.refresh().await,
        Err(PeerConnectionError::Closed)
    ));
    tokio::time::advance(Duration::from_secs(86400)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn scope_failure_is_closed_and_keeps_its_source_then_recovers_without_a_notification() {
    use std::error::Error as _;
    let (owner, scope, presence) = fixture();
    scope.unavailable.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    let error = owner.refresh().await.unwrap_err();
    assert!(error.source().unwrap().is::<CurrentSpaceMemberScopeError>());
    assert_eq!(presence.calls.load(Ordering::SeqCst), 0);
    scope.unavailable.store(false, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(61)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn environment_hints_reset_backoff_but_unknown_discovery_does_not() {
    let (_, scope, presence) = fixture();
    let (sender, receiver) = mpsc::channel(4);
    let hints = futures::stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|event| (event, receiver))
    });
    let owner = PeerConnectionCoordinator::new(scope, presence.clone(), Box::pin(hints));
    owner.start().await;
    settle().await;
    for _ in 0..6 {
        tokio::time::advance(Duration::from_secs(80)).await;
        settle().await;
    }
    let before = presence.calls.load(Ordering::SeqCst);
    sender
        .send(Ok(ConnectionHint::PeerAddressChanged(DeviceId::new(
            "unknown",
        ))))
        .await
        .unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(2)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), before);
    sender
        .send(Ok(ConnectionHint::NetworkChanged))
        .await
        .unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), before + 1);
    drop(sender);
    tokio::time::advance(Duration::from_secs(80)).await;
    settle().await;
    assert!(presence.calls.load(Ordering::SeqCst) > before + 1);
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn lagged_presence_consumer_reconciles_without_a_busy_loop() {
    let (owner, _, presence) = fixture();
    for _ in 0..1000 {
        let _ = presence.events.send(PeerReachabilityChanged {
            device_id: DeviceId::new("unknown"),
            state: ReachabilityState::Offline,
            at: chrono::Utc::now(),
        });
    }
    owner.start().await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 1);
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn scope_unavailability_during_dial_cancels_before_late_success() {
    let (owner, scope, presence) = fixture();
    presence.blocked.store(true, Ordering::SeqCst);
    presence.reachable.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    scope.unavailable.store(true, Ordering::SeqCst);
    scope.changes.send_replace(());
    settle().await;
    presence.permits.add_permits(1);
    settle().await;
    assert_eq!(presence.active.load(Ordering::SeqCst), 0);
    assert!(presence.states.lock().await.is_empty());
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn fresh_online_before_attempt_timeout_settles_refresh_as_online() {
    let (owner, _, presence) = fixture();
    presence.blocked.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    let refresh = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh().await }
    });
    settle().await;
    let peer = DeviceId::new("peer");
    presence
        .states
        .lock()
        .await
        .insert(peer, ReachabilityState::Online);
    presence
        .events
        .send(PeerReachabilityChanged {
            device_id: peer,
            state: ReachabilityState::Online,
            at: chrono::Utc::now(),
        })
        .unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(11)).await;
    settle().await;
    let report = refresh.await.unwrap().unwrap();
    assert_eq!(
        (report.online, report.offline, report.errors),
        (1, 0, 0),
        "the old timeout must not override the new inbound success"
    );
    let calls = presence.calls.load(Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(80)).await;
    settle().await;
    assert_eq!(
        presence.calls.load(Ordering::SeqCst),
        calls,
        "a healthy newly connected peer must not be retried"
    );
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn fresh_success_then_disconnect_does_not_hide_attempt_timeout() {
    let (owner, _, presence) = fixture();
    presence.blocked.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    let refresh = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh().await }
    });
    settle().await;
    let peer = DeviceId::new("peer");
    for state in [ReachabilityState::Online, ReachabilityState::Offline] {
        presence.states.lock().await.insert(peer, state);
        presence
            .events
            .send(PeerReachabilityChanged {
                device_id: peer,
                state,
                at: chrono::Utc::now(),
            })
            .unwrap();
        settle().await;
    }
    tokio::time::advance(Duration::from_secs(11)).await;
    settle().await;
    let report = refresh.await.unwrap().unwrap();
    assert_eq!((report.online, report.errors), (0, 1));
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn old_online_state_does_not_mask_a_forced_check_timeout() {
    let (owner, _, presence) = fixture();
    let peer = DeviceId::new("peer");
    presence
        .states
        .lock()
        .await
        .insert(peer, ReachabilityState::Online);
    presence.blocked.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    let refresh = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh().await }
    });
    settle().await;
    tokio::time::advance(Duration::from_secs(11)).await;
    settle().await;
    let report = refresh.await.unwrap().unwrap();
    assert_eq!((report.online, report.errors), (0, 1));
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn fresh_online_does_not_mask_unfinished_qualification() {
    let (owner, scope, presence) = fixture();
    // 初次范围查询正常，实际尝试的资格复核悬挂；手动刷新自己的范围查询仍可完成。
    scope.stall_on_read.store(2, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    let refresh = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh().await }
    });
    settle().await;
    let peer = DeviceId::new("peer");
    presence
        .states
        .lock()
        .await
        .insert(peer, ReachabilityState::Online);
    presence
        .events
        .send(PeerReachabilityChanged {
            device_id: peer,
            state: ReachabilityState::Online,
            at: chrono::Utc::now(),
        })
        .unwrap();
    settle().await;
    tokio::time::advance(Duration::from_secs(11)).await;
    settle().await;
    let report = refresh.await.unwrap().unwrap();
    assert_eq!(
        (report.online, report.errors),
        (0, 1),
        "unverified qualification must fail closed"
    );
    owner.shutdown().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn incoming_success_does_not_strand_a_queued_manual_refresh() {
    let (owner, scope, presence) = fixture();
    *scope.peers.lock().await = (0..5)
        .map(|index| DeviceId::new(format!("peer-{index}")))
        .collect();
    presence.blocked.store(true, Ordering::SeqCst);
    presence.reachable.store(true, Ordering::SeqCst);
    owner.start().await;
    settle().await;
    assert_eq!(presence.calls.load(Ordering::SeqCst), 4);
    let refresh = tokio::spawn({
        let owner = owner.clone();
        async move { owner.refresh().await }
    });
    settle().await;
    let queued = DeviceId::new("peer-4");
    presence
        .states
        .lock()
        .await
        .insert(queued, ReachabilityState::Online);
    presence
        .events
        .send(PeerReachabilityChanged {
            device_id: queued,
            state: ReachabilityState::Online,
            at: chrono::Utc::now(),
        })
        .unwrap();
    settle().await;
    presence.permits.add_permits(5);
    settle().await;
    let report = tokio::time::timeout(Duration::from_secs(2), refresh)
        .await
        .expect("incoming success must not erase the work a manual refresh is waiting for")
        .unwrap()
        .unwrap();
    assert_eq!((report.total, report.online, report.errors), (5, 5, 0));
    owner.shutdown().await.unwrap();
}
