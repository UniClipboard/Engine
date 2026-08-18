//! Maintains connections for the accepted current-member scope.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uc_core::ids::DeviceId;
use uc_core::membership::CurrentWorkspacePeerScopePort;
use uc_core::ports::{
    PeerAddressRepositoryPort, PresenceError, PresenceEvent, PresencePort, ReachabilityState,
};

const BASE_INTERVAL: Duration = Duration::from_secs(25);
const BACKOFF_LADDER: [Duration; 3] = [
    BASE_INTERVAL,
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
];
const SLEEP_AFTER_FAILURES: u32 = 3;
const SLEEP_FALLBACK_INTERVAL: Duration = Duration::from_secs(30 * 60);

pub struct MembershipConnectivityDeps {
    pub peer_addresses: Arc<dyn PeerAddressRepositoryPort>,
    pub presence: Arc<dyn PresencePort>,
    pub local_device_id: DeviceId,
    pub peer_scope: Arc<dyn CurrentWorkspacePeerScopePort>,
}

pub struct MembershipConnectivityRuntime {
    cancel: CancellationToken,
    task: JoinHandle<()>,
    activity: MembershipConnectivityActivity,
}

#[derive(Clone)]
pub struct MembershipConnectivityActivity {
    wake: Arc<Notify>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BackoffState {
    Active {
        consecutive_failures: u32,
        next_dial_at: Instant,
    },
    Sleeping,
}

impl BackoffState {
    fn ready_now(now: Instant) -> Self {
        Self::Active {
            consecutive_failures: 0,
            next_dial_at: now,
        }
    }

    fn ready(&self, now: Instant) -> bool {
        match self {
            Self::Active { next_dial_at, .. } => now >= *next_dial_at,
            Self::Sleeping => false,
        }
    }

    fn is_sleeping(&self) -> bool {
        matches!(self, Self::Sleeping)
    }

    fn on_success(&mut self, now: Instant) {
        *self = Self::Active {
            consecutive_failures: 0,
            next_dial_at: now + BACKOFF_LADDER[0],
        };
    }

    fn on_failure(&mut self, now: Instant) {
        if let Self::Active {
            consecutive_failures,
            ..
        } = self
        {
            let failures = consecutive_failures.saturating_add(1);
            if failures >= SLEEP_AFTER_FAILURES {
                *self = Self::Sleeping;
            } else {
                *self = Self::Active {
                    consecutive_failures: failures,
                    next_dial_at: now + BACKOFF_LADDER[failures as usize],
                };
            }
        }
    }
}

struct MembershipConnectivity {
    deps: MembershipConnectivityDeps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MembershipConnectivityPass {
    Completed,
    ScopeUnavailable,
    Stopped,
}

impl MembershipConnectivity {
    async fn run(
        self,
        mut presence_events: broadcast::Receiver<PresenceEvent>,
        cancel: CancellationToken,
        wake: Arc<Notify>,
    ) {
        let mut ticker = tokio::time::interval(BASE_INTERVAL);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ticker.tick().await;
        let mut fallback_ticker = tokio::time::interval(SLEEP_FALLBACK_INTERVAL);
        fallback_ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        fallback_ticker.tick().await;
        let mut backoff = HashMap::new();
        let initial_outcome = self.dial_due_peers(&mut backoff, false, &cancel).await;
        if initial_outcome == MembershipConnectivityPass::Stopped {
            return;
        }

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break,
                event = presence_events.recv() => match event {
                    Ok(event) if event.state == ReachabilityState::Online => {
                        let allowed = self
                            .deps
                            .peer_scope
                            .snapshot()
                            .await
                            .is_ok_and(|scope| scope.peer_device_ids.contains(&event.device_id));
                        if !allowed {
                            continue;
                        }
                        let now = Instant::now();
                        backoff
                            .entry(event.device_id.as_str().to_string())
                            .or_insert_with(|| BackoffState::ready_now(now))
                            .on_success(now);
                        tokio::select! {
                            biased;
                            _ = cancel.cancelled() => break,
                            _ = self.deps.presence.ensure_reachable(&event.device_id) => {}
                        }
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                },
                _ = ticker.tick() => {
                    let outcome = self.dial_due_peers(&mut backoff, false, &cancel).await;
                    if outcome == MembershipConnectivityPass::Stopped {
                        break;
                    }
                }
                _ = fallback_ticker.tick() => {
                    let outcome = self.dial_due_peers(&mut backoff, true, &cancel).await;
                    if outcome == MembershipConnectivityPass::Stopped {
                        break;
                    }
                }
                _ = wake.notified() => {
                    let outcome = self.dial_due_peers(&mut backoff, false, &cancel).await;
                    if outcome == MembershipConnectivityPass::Stopped {
                        break;
                    }
                }
            }
        }
    }

    async fn dial_due_peers(
        &self,
        backoff: &mut HashMap<String, BackoffState>,
        sleeping_only: bool,
        cancel: &CancellationToken,
    ) -> MembershipConnectivityPass {
        let Ok(records) = self.deps.peer_addresses.list().await else {
            warn!("failed to list paired peers for membership connectivity");
            return MembershipConnectivityPass::Completed;
        };
        let Ok(scope) = self.deps.peer_scope.snapshot().await else {
            warn!("current peer scope is unavailable for membership connectivity");
            backoff.clear();
            return MembershipConnectivityPass::ScopeUnavailable;
        };
        let addressable = records
            .into_iter()
            .map(|record| record.device_id)
            .collect::<HashSet<_>>();
        let peers = scope
            .peer_device_ids
            .into_iter()
            .filter(|device| device != &self.deps.local_device_id)
            .filter(|device| addressable.contains(device))
            .collect::<Vec<_>>();
        let paired = peers
            .iter()
            .map(|device| device.as_str().to_string())
            .collect::<HashSet<_>>();
        backoff.retain(|device, _| paired.contains(device));

        let now = Instant::now();
        let due = peers
            .into_iter()
            .filter(|device| {
                let state = backoff
                    .entry(device.as_str().to_string())
                    .or_insert_with(|| BackoffState::ready_now(now));
                if sleeping_only {
                    state.is_sleeping()
                } else {
                    state.ready(now)
                }
            })
            .collect::<Vec<_>>();
        if self.dispatch_dials(due, backoff, cancel).await {
            MembershipConnectivityPass::Completed
        } else {
            MembershipConnectivityPass::Stopped
        }
    }

    async fn dispatch_dials(
        &self,
        devices: Vec<DeviceId>,
        backoff: &mut HashMap<String, BackoffState>,
        cancel: &CancellationToken,
    ) -> bool {
        let mut dials: JoinSet<(DeviceId, Result<ReachabilityState, PresenceError>)> =
            JoinSet::new();
        for device in devices {
            let presence = Arc::clone(&self.deps.presence);
            dials.spawn(async move {
                let result = presence.ensure_reachable(&device).await;
                (device, result)
            });
        }

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    dials.abort_all();
                    while dials.join_next().await.is_some() {}
                    return false;
                }
                joined = dials.join_next() => {
                    let Some(joined) = joined else { return true; };
                    match joined {
                        Ok((device, Ok(ReachabilityState::Online))) => {
                            if let Some(state) = backoff.get_mut(device.as_str()) {
                                state.on_success(Instant::now());
                            }
                        }
                        Ok((device, _)) => {
                            if let Some(state) = backoff.get_mut(device.as_str()) {
                                state.on_failure(Instant::now());
                            }
                        }
                        Err(_) => warn!("membership connectivity dial task failed"),
                    }
                }
            }
        }
    }
}

pub fn start_membership_connectivity(
    deps: MembershipConnectivityDeps,
    presence_events: broadcast::Receiver<PresenceEvent>,
) -> MembershipConnectivityRuntime {
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let wake = Arc::new(Notify::new());
    let task_wake = Arc::clone(&wake);
    let task = tokio::spawn(async move {
        info!("membership connectivity started");
        MembershipConnectivity { deps }
            .run(presence_events, task_cancel, task_wake)
            .await;
        debug!("membership connectivity stopped");
    });
    MembershipConnectivityRuntime {
        cancel,
        task,
        activity: MembershipConnectivityActivity { wake },
    }
}

impl MembershipConnectivityRuntime {
    pub(crate) fn activity(&self) -> MembershipConnectivityActivity {
        self.activity.clone()
    }

    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task.await;
    }
}

impl MembershipConnectivityActivity {
    pub(crate) fn resume(&self) {
        self.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tokio::sync::Notify;

    struct EmptyPeerAddresses;

    #[async_trait::async_trait]
    impl PeerAddressRepositoryPort for EmptyPeerAddresses {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            Ok(None)
        }

        async fn upsert(
            &self,
            _record: &uc_core::ports::PeerAddressRecord,
        ) -> Result<(), uc_core::ports::PeerAddressError> {
            Ok(())
        }

        async fn list(
            &self,
        ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            Ok(Vec::new())
        }

        async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
            Ok(())
        }
    }

    struct BlockingPresence {
        started: Notify,
        events: broadcast::Sender<PresenceEvent>,
    }

    struct RecordingPresence {
        reached: std::sync::Mutex<Vec<DeviceId>>,
        started: Notify,
        events: broadcast::Sender<PresenceEvent>,
    }

    struct EmptyPeerScope;

    struct FixedPeerScope(Vec<DeviceId>);

    struct ReadinessGatedPeerScope {
        ready: std::sync::Mutex<bool>,
        unavailable: Notify,
    }

    impl ReadinessGatedPeerScope {
        fn mark_ready(&self) {
            *self
                .ready
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        }
    }

    #[async_trait::async_trait]
    impl uc_core::membership::CurrentWorkspacePeerScopePort for FixedPeerScope {
        async fn snapshot(
            &self,
        ) -> Result<
            uc_core::membership::CurrentWorkspacePeerSnapshot,
            uc_core::membership::CurrentWorkspacePeerScopeError,
        > {
            Ok(uc_core::membership::CurrentWorkspacePeerSnapshot {
                revision: 1,
                source: uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory,
                local_membership: uc_core::membership::CurrentWorkspaceLocalMembership::Active,
                peer_device_ids: self.0.clone(),
            })
        }
    }

    #[async_trait::async_trait]
    impl uc_core::membership::CurrentWorkspacePeerScopePort for EmptyPeerScope {
        async fn snapshot(
            &self,
        ) -> Result<
            uc_core::membership::CurrentWorkspacePeerSnapshot,
            uc_core::membership::CurrentWorkspacePeerScopeError,
        > {
            Ok(uc_core::membership::CurrentWorkspacePeerSnapshot {
                revision: 1,
                source: uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory,
                local_membership: uc_core::membership::CurrentWorkspaceLocalMembership::Active,
                peer_device_ids: Vec::new(),
            })
        }
    }

    #[async_trait::async_trait]
    impl uc_core::membership::CurrentWorkspacePeerScopePort for ReadinessGatedPeerScope {
        async fn snapshot(
            &self,
        ) -> Result<
            uc_core::membership::CurrentWorkspacePeerSnapshot,
            uc_core::membership::CurrentWorkspacePeerScopeError,
        > {
            if !*self
                .ready
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
            {
                self.unavailable.notify_one();
                return Err(uc_core::membership::CurrentWorkspacePeerScopeError::Locked);
            }
            Ok(uc_core::membership::CurrentWorkspacePeerSnapshot {
                revision: 1,
                source: uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory,
                local_membership: uc_core::membership::CurrentWorkspaceLocalMembership::Active,
                peer_device_ids: vec![DeviceId::new("device-b")],
            })
        }
    }

    struct SinglePeerAddress;

    #[async_trait::async_trait]
    impl PeerAddressRepositoryPort for SinglePeerAddress {
        async fn get(
            &self,
            _device: &DeviceId,
        ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            Ok(None)
        }

        async fn upsert(
            &self,
            _record: &uc_core::ports::PeerAddressRecord,
        ) -> Result<(), uc_core::ports::PeerAddressError> {
            Ok(())
        }

        async fn list(
            &self,
        ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError>
        {
            Ok(vec![uc_core::ports::PeerAddressRecord {
                device_id: DeviceId::new("device-b"),
                addr_blob: vec![1],
                observed_at: Utc::now(),
            }])
        }

        async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl PresencePort for BlockingPresence {
        async fn ensure_reachable(
            &self,
            _device: &DeviceId,
        ) -> Result<ReachabilityState, PresenceError> {
            self.started.notify_one();
            std::future::pending().await
        }

        async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
            ReachabilityState::Unknown
        }

        fn subscribe(&self) -> broadcast::Receiver<PresenceEvent> {
            self.events.subscribe()
        }
    }

    #[async_trait::async_trait]
    impl PresencePort for RecordingPresence {
        async fn ensure_reachable(
            &self,
            device: &DeviceId,
        ) -> Result<ReachabilityState, PresenceError> {
            self.reached
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(device.clone());
            self.started.notify_one();
            Ok(ReachabilityState::Online)
        }

        async fn current_state(&self, _device: &DeviceId) -> ReachabilityState {
            ReachabilityState::Unknown
        }

        fn subscribe(&self) -> broadcast::Receiver<PresenceEvent> {
            self.events.subscribe()
        }
    }

    #[test]
    fn failures_walk_the_ladder_then_sleep() {
        let now = Instant::now();
        let mut state = BackoffState::ready_now(now);
        state.on_failure(now);
        assert!(!state.ready(now + Duration::from_secs(59)));
        assert!(state.ready(now + Duration::from_secs(60)));
        state.on_failure(now);
        assert!(state.ready(now + Duration::from_secs(5 * 60)));
        state.on_failure(now);
        assert!(state.is_sleeping());
    }

    #[test]
    fn sleeping_failure_keeps_the_peer_asleep() {
        let mut state = BackoffState::Sleeping;
        state.on_failure(Instant::now());
        assert!(state.is_sleeping());
    }

    // 流程：B 上线触发 A 拨号，但拨号永久不返回；A 关闭时必须打断拨号并及时退出。
    #[tokio::test]
    async fn shutdown_interrupts_an_online_event_dial() {
        let (events, presence_events) = broadcast::channel(4);
        let presence = Arc::new(BlockingPresence {
            started: Notify::new(),
            events: events.clone(),
        });
        let runtime = start_membership_connectivity(
            MembershipConnectivityDeps {
                peer_addresses: Arc::new(EmptyPeerAddresses),
                presence: presence.clone(),
                local_device_id: DeviceId::new("device-a"),
                peer_scope: Arc::new(FixedPeerScope(vec![DeviceId::new("device-b")])),
            },
            presence_events,
        );
        events
            .send(PresenceEvent {
                device_id: DeviceId::new("device-b"),
                state: ReachabilityState::Online,
                at: Utc::now(),
            })
            .expect("presence event has a receiver");
        tokio::time::timeout(Duration::from_secs(1), presence.started.notified())
            .await
            .expect("online event starts a dial");

        tokio::time::timeout(Duration::from_secs(1), runtime.shutdown())
            .await
            .expect("shutdown interrupts the in-flight online event dial");
    }

    #[tokio::test]
    async fn online_event_for_a_non_current_peer_does_not_start_a_dial() {
        let (events, presence_events) = broadcast::channel(4);
        let presence = Arc::new(BlockingPresence {
            started: Notify::new(),
            events: events.clone(),
        });
        let runtime = start_membership_connectivity(
            MembershipConnectivityDeps {
                peer_addresses: Arc::new(EmptyPeerAddresses),
                presence: presence.clone(),
                local_device_id: DeviceId::new("device-a"),
                peer_scope: Arc::new(EmptyPeerScope),
            },
            presence_events,
        );
        events
            .send(PresenceEvent {
                device_id: DeviceId::new("removed-device"),
                state: ReachabilityState::Online,
                at: Utc::now(),
            })
            .expect("presence event has a receiver");

        assert!(
            tokio::time::timeout(Duration::from_millis(100), presence.started.notified())
                .await
                .is_err()
        );
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn retries_admitted_peer_when_scope_becomes_ready_after_startup() {
        let (events, presence_events) = broadcast::channel(4);
        let presence = Arc::new(RecordingPresence {
            reached: std::sync::Mutex::new(Vec::new()),
            started: Notify::new(),
            events,
        });
        let scope = Arc::new(ReadinessGatedPeerScope {
            ready: std::sync::Mutex::new(false),
            unavailable: Notify::new(),
        });
        let runtime = start_membership_connectivity(
            MembershipConnectivityDeps {
                peer_addresses: Arc::new(SinglePeerAddress),
                presence: presence.clone(),
                local_device_id: DeviceId::new("device-a"),
                peer_scope: scope.clone(),
            },
            presence_events,
        );

        tokio::time::timeout(Duration::from_secs(1), scope.unavailable.notified())
            .await
            .expect("startup attempts to load the current admitted-member scope");
        scope.mark_ready();
        runtime.activity().resume();
        tokio::time::timeout(Duration::from_secs(2), presence.started.notified())
            .await
            .expect("ready admitted peer is retried by the session-ready event");
        assert_eq!(
            presence
                .reached
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            &[DeviceId::new("device-b")]
        );

        runtime.shutdown().await;
    }
}
