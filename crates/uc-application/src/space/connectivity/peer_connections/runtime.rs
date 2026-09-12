use super::*;
use futures::{future::BoxFuture, stream::FuturesUnordered, FutureExt, StreamExt};
use tokio::sync::broadcast;
use tokio::time::Instant;
use uc_core::ports::PeerReachabilityChanged;

const MAX_CONCURRENT: usize = 4;
const DIAL_BUDGET: Duration = Duration::from_secs(10);
const SCOPE_BUDGET: Duration = Duration::from_secs(1);
const SCOPE_RECHECK: Duration = Duration::from_secs(60);
const COALESCE: Duration = Duration::from_millis(250);
const MIN_START_INTERVAL: Duration = Duration::from_secs(1);

struct Peer {
    failures: usize,
    due: Option<Instant>,
    started: Option<Instant>,
    in_flight: Option<CancellationToken>,
    rerun: bool,
    force: bool,
    online: bool,
    observation_revision: u64,
    started_observation_revision: u64,
    generation: u64,
    next_trigger: &'static str,
    active_trigger: &'static str,
}

struct Refresh {
    pending: HashSet<DeviceId>,
    report: PresenceRefreshReport,
    response: oneshot::Sender<Result<PresenceRefreshReport, PeerConnectionError>>,
}

#[derive(Debug, thiserror::Error)]
enum AttemptError {
    #[error("peer scope read failed")]
    Scope(#[source] CurrentSpaceMemberScopeError),
    #[error("peer qualification read timed out")]
    ScopeTimeout(#[source] tokio::time::error::Elapsed),
    #[error("peer reachability check failed")]
    Presence(#[source] uc_core::ports::PresenceError),
    #[error("peer connection attempt timed out")]
    Timeout(#[source] tokio::time::error::Elapsed),
    #[error("peer is no longer eligible")]
    Ineligible,
}
impl AttemptError {
    fn kind(&self) -> &'static str {
        match self {
            Self::Scope(_) | Self::ScopeTimeout(_) => "scope_unavailable",
            Self::Presence(_) => "presence_failed",
            Self::Timeout(_) => "timeout",
            Self::Ineligible => "ineligible",
        }
    }
}
enum DialResult {
    State(ReachabilityState),
    Error(AttemptError),
    Cancelled,
}
type Dial = BoxFuture<'static, (DeviceId, u64, DialResult)>;

pub(super) struct ConnectionRuntime {
    scope: Arc<dyn CurrentSpaceMemberScopePort>,
    presence: Arc<dyn PeerReachabilityPort>,
    scope_changes: watch::Receiver<()>,
    presence_changes: broadcast::Receiver<PeerReachabilityChanged>,
    hints: BoxStream<'static, Result<ConnectionHint, anyhow::Error>>,
    commands: mpsc::Receiver<Command>,
    opportunities: watch::Receiver<ConnectivityOpportunity>,
    cancel: CancellationToken,
    peers: HashMap<DeviceId, Peer>,
    dials: FuturesUnordered<Dial>,
    refreshes: Vec<Refresh>,
    generation: u64,
    paused: bool,
}

impl ConnectionRuntime {
    pub(super) fn new(
        scope: Arc<dyn CurrentSpaceMemberScopePort>,
        presence: Arc<dyn PeerReachabilityPort>,
        hints: BoxStream<'static, Result<ConnectionHint, anyhow::Error>>,
        commands: mpsc::Receiver<Command>,
        opportunities: watch::Receiver<ConnectivityOpportunity>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            scope_changes: scope.subscribe_changes(),
            presence_changes: presence.subscribe(),
            scope,
            presence,
            hints,
            commands,
            opportunities,
            cancel,
            peers: HashMap::new(),
            dials: FuturesUnordered::new(),
            refreshes: vec![],
            generation: 0,
            paused: false,
        }
    }

    pub(super) async fn run(mut self) {
        let mut next_scope = Instant::now();
        let mut scope_open = true;
        let mut presence_open = true;
        let mut hints_open = true;
        loop {
            if self.cancel.is_cancelled() {
                break;
            }
            self.refreshes
                .retain(|request| !request.response.is_closed());
            let due = self
                .peers
                .values()
                .filter(|peer| peer.in_flight.is_none())
                .filter_map(|peer| peer.due)
                .min();
            let next = if self.paused {
                Instant::now() + SCOPE_RECHECK
            } else if self.dials.len() >= MAX_CONCURRENT {
                next_scope
            } else {
                due.map_or(next_scope, |at| at.min(next_scope))
            };
            tokio::select! {
                _ = self.cancel.cancelled() => break,
                command = self.commands.recv() => match command {
                    Some(Command::Pause(response)) => {
                        self.paused = true;
                        self.clear().await;
                        let _ = response.send(());
                    }
                    Some(Command::Resume(response)) => {
                        self.paused = false;
                        next_scope = Instant::now();
                        let _ = response.send(());
                    }
                    Some(Command::Refresh(response)) => {
                        if self.paused { let _ = response.send(Err(PeerConnectionError::Paused)); }
                        else if self.refreshes.len() >= 32 { let _ = response.send(Err(PeerConnectionError::Busy)); }
                        else {
                            match self.reconcile().await {
                                Ok(()) => {
                                    let pending = self.peers.keys().copied().collect::<HashSet<_>>();
                                    let report = PresenceRefreshReport { total: pending.len(), online: 0, offline: 0, errors: 0 };
                                    if pending.is_empty() { let _ = response.send(Ok(report)); }
                                    else {
                                        for peer in self.peers.values_mut() {
                                            if peer.in_flight.is_none() { peer.due = Some(Instant::now()); peer.force = true; peer.next_trigger = "manual_refresh"; }
                                        }
                                        self.refreshes.push(Refresh { pending, report, response });
                                    }
                                }
                                Err(error) => { let _ = response.send(Err(error)); }
                            }
                        }
                    }
                    None => break,
                },
                result = self.dials.next(), if !self.dials.is_empty() => {
                    if let Some((device, generation, result)) = result { self.finish(device, generation, result); }
                }
                changed = self.scope_changes.changed(), if scope_open => {
                    if changed.is_err() { scope_open = false; }
                    else { next_scope = Instant::now(); }
                }
                changed = self.opportunities.changed() => {
                    if changed.is_err() { break; }
                    if !self.paused {
                        next_scope = Instant::now();
                        let reason = match *self.opportunities.borrow_and_update() {
                            ConnectivityOpportunity::Foreground => "foreground",
                            ConnectivityOpportunity::SystemWake => "system_wake",
                            ConnectivityOpportunity::NetworkChanged => "network_changed",
                        };
                        self.opportunity(None, reason);
                    }
                }
                hint = self.hints.next(), if hints_open => match hint {
                    Some(Ok(ConnectionHint::NetworkChanged)) if !self.paused => { self.opportunity(None, "network_changed"); next_scope = Instant::now(); }
                    Some(Ok(ConnectionHint::PeerAddressChanged(device))) if !self.paused => self.opportunity(Some(device), "peer_discovered"),
                    Some(Err(source)) => {
                        let failure = PeerConnectionError::Environment(source);
                        tracing::warn!(error.type = "unavailable", "peer connection environment observation failed");
                        drop(failure);
                        next_scope = Instant::now();
                    }
                    Some(Ok(_)) => {},
                    None => hints_open = false,
                },
                event = self.presence_changes.recv(), if presence_open => match event {
                    Ok(event) if !self.paused => self.presence_changed(event),
                    Ok(_) => {},
                    Err(broadcast::error::RecvError::Lagged(_)) => next_scope = Instant::now(),
                    Err(broadcast::error::RecvError::Closed) => presence_open = false,
                },
                _ = tokio::time::sleep_until(next) => {
                    if !self.paused && Instant::now() >= next_scope {
                        let _ = self.reconcile().await;
                        next_scope = Instant::now() + SCOPE_RECHECK;
                    }
                }
            }
            if !self.paused {
                self.start_due();
            }
        }
        self.clear().await;
    }

    async fn reconcile(&mut self) -> Result<(), PeerConnectionError> {
        let scope = match tokio::time::timeout(SCOPE_BUDGET, self.scope.snapshot()).await {
            Ok(Ok(scope)) if scope.local_member_active => scope,
            other => {
                self.clear().await;
                return match other {
                    Ok(Ok(_)) => Ok(()),
                    Ok(Err(error)) => Err(PeerConnectionError::Scope(error)),
                    Err(error) => Err(PeerConnectionError::ScopeTimeout(error)),
                };
            }
        };
        let current: HashSet<_> = scope.usable_peer_device_ids.into_iter().collect();
        let removed: Vec<_> = self
            .peers
            .keys()
            .filter(|device| !current.contains(device))
            .copied()
            .collect();
        for device in removed {
            if let Some(peer) = self.peers.remove(&device) {
                if let Some(cancel) = peer.in_flight {
                    cancel.cancel();
                }
            }
            self.presence.forget(&device).await;
            self.settle_refresh(device, &DialResult::Error(AttemptError::Ineligible));
        }
        self.presence.activate().await;
        for device in current {
            let state = self.presence.current_state(&device).await;
            if let Some(peer) = self.peers.get_mut(&device) {
                if state != ReachabilityState::Online && peer.online && peer.in_flight.is_none() {
                    peer.online = false;
                    peer.due = Some(Instant::now());
                }
            } else {
                self.generation = self.generation.wrapping_add(1);
                self.peers.insert(
                    device,
                    Peer {
                        failures: 0,
                        due: Some(Instant::now()),
                        started: None,
                        in_flight: None,
                        rerun: false,
                        force: false,
                        online: state == ReachabilityState::Online,
                        observation_revision: 0,
                        started_observation_revision: 0,
                        generation: self.generation,
                        next_trigger: "member_available",
                        active_trigger: "member_available",
                    },
                );
            }
        }
        Ok(())
    }

    fn opportunity(&mut self, device: Option<DeviceId>, reason: &'static str) {
        let now = Instant::now();
        for (id, peer) in &mut self.peers {
            if device.is_some_and(|device| device != *id) {
                continue;
            }
            peer.next_trigger = reason;
            if peer.in_flight.is_some() {
                peer.rerun = true;
            } else {
                let at =
                    (now + COALESCE).max(peer.started.map_or(now, |at| at + MIN_START_INTERVAL));
                peer.due = Some(peer.due.map_or(at, |due| due.min(at)));
                peer.force = true;
            }
        }
    }

    fn presence_changed(&mut self, event: PeerReachabilityChanged) {
        let awaiting_refresh = self
            .refreshes
            .iter()
            .any(|refresh| refresh.pending.contains(&event.device_id));
        let Some(peer) = self.peers.get_mut(&event.device_id) else {
            return;
        };
        peer.observation_revision = peer.observation_revision.wrapping_add(1);
        match event.state {
            ReachabilityState::Online => {
                peer.online = true;
                peer.failures = 0;
                peer.rerun = false;
                if peer.in_flight.is_none() && !awaiting_refresh {
                    peer.due = None;
                }
            }
            _ => {
                let was_online = peer.online;
                peer.online = false;
                if was_online && peer.in_flight.is_none() {
                    self.opportunity(Some(event.device_id), "peer_disconnected");
                }
            }
        }
    }

    fn start_due(&mut self) {
        let now = Instant::now();
        let mut due: Vec<_> = self
            .peers
            .iter()
            .filter_map(|(device, peer)| {
                peer.due
                    .filter(|at| *at <= now && peer.in_flight.is_none())
                    .map(|at| (at, *device))
            })
            .collect();
        due.sort_by_key(|(at, device)| (*at, *device));
        for (_, device) in due
            .into_iter()
            .take(MAX_CONCURRENT.saturating_sub(self.dials.len()))
        {
            let Some(peer) = self.peers.get_mut(&device) else {
                continue;
            };
            if peer.online && !peer.force {
                peer.due = None;
                continue;
            }
            let cancel = self.cancel.child_token();
            peer.in_flight = Some(cancel.clone());
            peer.started = Some(now);
            peer.started_observation_revision = peer.observation_revision;
            peer.due = None;
            peer.force = false;
            peer.active_trigger = peer.next_trigger;
            let generation = peer.generation;
            let scope = Arc::clone(&self.scope);
            let presence = Arc::clone(&self.presence);
            self.dials.push(
                async move {
                    let attempt = async {
                        let scope = tokio::time::timeout(SCOPE_BUDGET, scope.snapshot())
                            .await
                            .map_err(AttemptError::ScopeTimeout)?
                            .map_err(AttemptError::Scope)?;
                        if !scope.local_member_active
                            || !scope.usable_peer_device_ids.contains(&device)
                        {
                            return Err(AttemptError::Ineligible);
                        }
                        presence
                            .verify_reachable(&device)
                            .await
                            .map_err(AttemptError::Presence)
                    };
                    let result = tokio::select! {
                        biased;
                        _ = cancel.cancelled() => DialResult::Cancelled,
                        result = tokio::time::timeout(DIAL_BUDGET, attempt) => match result {
                            Ok(Ok(state)) => DialResult::State(state),
                            Ok(Err(source)) => DialResult::Error(source),
                            Err(source) => DialResult::Error(AttemptError::Timeout(source)),
                        },
                    };
                    (device, generation, result)
                }
                .boxed(),
            );
        }
    }

    fn finish(&mut self, device: DeviceId, generation: u64, mut result: DialResult) {
        let Some(peer) = self.peers.get_mut(&device) else {
            return;
        };
        if peer.generation != generation {
            return;
        }
        peer.in_flight = None;
        if matches!(result, DialResult::Cancelled) {
            return;
        }
        // 超时发生在适配器之外；仍应保留本次尝试开始后收到的更新成功。
        // 后续离线通知会清除 online，旧缓存也没有新的修订，二者都不能掩盖失败。
        if peer.online
            && peer.observation_revision != peer.started_observation_revision
            && matches!(
                result,
                DialResult::State(ReachabilityState::Offline | ReachabilityState::Unknown)
                    | DialResult::Error(AttemptError::Timeout(_) | AttemptError::Presence(_))
            )
        {
            result = DialResult::State(ReachabilityState::Online);
        }
        let (outcome, error_kind) = match &result {
            DialResult::State(ReachabilityState::Online) => ("online", "none"),
            DialResult::State(ReachabilityState::Offline) => ("offline", "connect_failed"),
            DialResult::State(ReachabilityState::Unknown) => ("unknown", "unavailable"),
            DialResult::Error(source) => ("error", source.kind()),
            DialResult::Cancelled => ("cancelled", "none"),
        };
        tracing::info!(
            trigger = peer.active_trigger,
            outcome,
            error_kind,
            "peer connection recovery attempt finished"
        );
        if matches!(result, DialResult::State(ReachabilityState::Online)) {
            peer.online = true;
            peer.failures = 0;
            peer.due = None;
            peer.rerun = false;
        } else {
            peer.online = false;
            let seconds = [1, 2, 5, 10, 30, 60][peer.failures.min(5)];
            peer.failures = peer.failures.saturating_add(1);
            let jitter = rand::random::<u32>() as u64 % (seconds * 200 + 1);
            peer.due = Some(Instant::now() + Duration::from_millis(seconds * 1000 + jitter));
            if peer.rerun {
                peer.rerun = false;
                let reason = peer.next_trigger;
                self.opportunity(Some(device), reason);
            } else {
                peer.next_trigger = "retry";
            }
        }
        self.settle_refresh(device, &result);
    }

    fn settle_refresh(&mut self, device: DeviceId, result: &DialResult) {
        for request in &mut self.refreshes {
            if request.pending.remove(&device) {
                match result {
                    DialResult::State(ReachabilityState::Online) => request.report.online += 1,
                    DialResult::State(ReachabilityState::Offline) => request.report.offline += 1,
                    _ => request.report.errors += 1,
                }
            }
        }
        let mut index = 0;
        while index < self.refreshes.len() {
            if self.refreshes[index].pending.is_empty() {
                let request = self.refreshes.swap_remove(index);
                let _ = request.response.send(Ok(request.report));
            } else {
                index += 1;
            }
        }
    }

    async fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.dials.clear();
        self.peers.clear();
        for request in self.refreshes.drain(..) {
            let _ = request.response.send(Err(PeerConnectionError::Paused));
        }
        self.presence.disconnect_all().await;
    }
}
