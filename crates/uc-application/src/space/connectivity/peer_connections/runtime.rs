use super::*;
use super::{ATTEMPT_CAPACITY, ATTEMPT_DEADLINE};
use crate::space::membership::CurrentSpaceMemberScope;
use futures::{future::BoxFuture, stream::FuturesUnordered, FutureExt, StreamExt};
use std::sync::{Mutex as BlockingMutex, PoisonError};
use tokio::sync::{broadcast, Semaphore};
use tokio::task::{JoinError, JoinSet};
use tokio::time::Instant;
use uc_core::ports::PeerReachabilityChanged;
use uc_observability_contract::{
    diagnostics::{record_task_join_failure, DiagnosticTaskKind, ObservationContext},
    error_source::io_error_kind,
    uc_info, uc_warn,
};

const MAX_CONCURRENT: usize = 4;
// Bound the complete target operation, including qualification and admission.
const DIAL_BUDGET: Duration = Duration::from_secs(20);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(10);
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
    online: bool,
    observation_revision: u64,
    started_observation_revision: u64,
    generation: u64,
    next_trigger: &'static str,
    active_trigger: &'static str,
}

struct Refresh {
    pending: HashSet<DeviceId>,
    report: PeerReachabilityRefreshReport,
    response: oneshot::Sender<Result<PeerReachabilityRefreshReport, PeerConnectionError>>,
}

#[derive(Debug, thiserror::Error)]
enum AttemptError {
    #[error("peer scope read failed")]
    ScopeRead(#[source] PeerConnectionError),
    #[error("peer qualification read timed out")]
    ScopeTimeout(#[source] tokio::time::error::Elapsed),
    #[error("peer reachability check failed")]
    PeerReachability(#[source] uc_core::ports::PeerReachabilityError),
    #[error("peer connection attempt timed out")]
    Timeout(#[source] tokio::time::error::Elapsed),
    #[error("peer is no longer eligible")]
    Ineligible,
}
impl AttemptError {
    fn kind(&self) -> &'static str {
        match self {
            Self::ScopeRead(_) | Self::ScopeTimeout(_) => "scope_unavailable",
            Self::PeerReachability(_) => "presence_failed",
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

struct ScopeWork {
    reconcile_slot: Arc<Semaphore>,
    tasks: JoinSet<()>,
    failure: Option<JoinError>,
    #[cfg(test)]
    after_send: Option<Arc<Semaphore>>,
}

impl Default for ScopeWork {
    fn default() -> Self {
        Self {
            reconcile_slot: Arc::new(Semaphore::new(1)),
            tasks: JoinSet::new(),
            failure: None,
            #[cfg(test)]
            after_send: None,
        }
    }
}

pub(super) struct ConnectionRuntime {
    scope: Arc<dyn CurrentSpaceMemberScopePort>,
    peer_reachability: Arc<dyn PeerReachabilityPort>,
    scope_changes: watch::Receiver<()>,
    peer_reachability_changes: broadcast::Receiver<PeerReachabilityChanged>,
    hints: BoxStream<'static, Result<ConnectionHint, anyhow::Error>>,
    commands: mpsc::Receiver<Command>,
    opportunities: watch::Receiver<ConnectivityOpportunity>,
    cancel: CancellationToken,
    peers: HashMap<DeviceId, Peer>,
    dials: FuturesUnordered<Dial>,
    attempt_slots: Arc<Semaphore>,
    scope_work: Arc<BlockingMutex<ScopeWork>>,
    scope_failure: Option<JoinError>,
    refreshes: Vec<Refresh>,
    generation: u64,
    paused: bool,
    scope_available: bool,
}

impl ConnectionRuntime {
    pub(super) fn new(
        scope: Arc<dyn CurrentSpaceMemberScopePort>,
        peer_reachability: Arc<dyn PeerReachabilityPort>,
        hints: BoxStream<'static, Result<ConnectionHint, anyhow::Error>>,
        commands: mpsc::Receiver<Command>,
        opportunities: watch::Receiver<ConnectivityOpportunity>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            scope_changes: scope.subscribe_changes(),
            peer_reachability_changes: peer_reachability.subscribe(),
            scope,
            peer_reachability,
            hints,
            commands,
            opportunities,
            cancel,
            peers: HashMap::new(),
            dials: FuturesUnordered::new(),
            attempt_slots: Arc::new(Semaphore::new(MAX_CONCURRENT)),
            scope_work: Arc::new(BlockingMutex::new(ScopeWork::default())),
            scope_failure: None,
            refreshes: vec![],
            generation: 0,
            paused: false,
            scope_available: false,
        }
    }

    pub(super) async fn run(mut self) -> Result<(), PeerConnectionError> {
        let mut next_scope = Instant::now();
        let mut scope_open = true;
        let mut peer_reachability_open = true;
        let mut hints_open = true;
        // 当前连续失败的分类：同一分类只在开始时记录一次，恢复时再记录一次。
        let mut scope_failure: Option<&'static str> = None;
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
            } else if !self.scope_available {
                next_scope
            } else if self.dials.len() >= MAX_CONCURRENT
                || self.attempt_slots.available_permits() == 0
            {
                next_scope
            } else {
                due.map_or(next_scope, |at| at.min(next_scope))
            };
            tokio::select! {
                biased;
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
                                    let report = PeerReachabilityRefreshReport { total: pending.len(), online: 0, offline: 0, errors: 0 };
                                    if pending.is_empty() { let _ = response.send(Ok(report)); }
                                    else {
                                        for peer in self.peers.values_mut() {
                                            if peer.in_flight.is_none() { peer.due = Some(Instant::now()); peer.next_trigger = "manual_refresh"; }
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
                    Some(Ok(ConnectionHint::RelayRecovered)) if !self.paused => { self.relay_recovered(); next_scope = Instant::now(); }
                    Some(Ok(ConnectionHint::PeerAddressChanged(device))) if !self.paused => self.opportunity(Some(device), "peer_discovered"),
                    Some(Ok(ConnectionHint::CommunicationFailed(device))) if !self.paused => self.opportunity(Some(device), "communication_failed"),
                    Some(Err(source)) => {
                        let failure = PeerConnectionError::Environment(source);
                        uc_warn!(error_kind = "unavailable", "peer connection environment observation failed");
                        drop(failure);
                        next_scope = Instant::now();
                    }
                    Some(Ok(_)) => {},
                    None => hints_open = false,
                },
                event = self.peer_reachability_changes.recv(), if peer_reachability_open => match event {
                    Ok(event) if !self.paused => self.peer_reachability_changed(event),
                    Ok(_) => {},
                    Err(broadcast::error::RecvError::Lagged(_)) => next_scope = Instant::now(),
                    Err(broadcast::error::RecvError::Closed) => peer_reachability_open = false,
                },
                _ = tokio::time::sleep_until(next) => {
                    if !self.paused && Instant::now() >= next_scope {
                        match self.reconcile().await {
                            Ok(()) => {
                                if scope_failure.take().is_some() {
                                    uc_info!("peer connection scope recovered");
                                }
                            }
                            Err(error) => {
                                let kind = match error {
                                    PeerConnectionError::Scope(_) => "scope_unavailable",
                                    PeerConnectionError::ScopeTimeout(_) => "scope_timeout",
                                    _ => "internal",
                                };
                                if scope_failure.replace(kind) != Some(kind) {
                                    uc_warn!(
                                        error_kind = kind,
                                        io_error_kind = io_error_kind(&error),
                                        "peer connection scope reconcile failed; new attempts paused"
                                    );
                                }
                            }
                        }
                        next_scope = Instant::now() + SCOPE_RECHECK;
                    }
                }
            }
            if !self.paused && self.scope_available {
                self.start_due();
            }
        }
        self.clear().await;
        match self.scope_failure.take() {
            Some(source) => Err(PeerConnectionError::Task(source)),
            None => Ok(()),
        }
    }

    async fn reconcile(&mut self) -> Result<(), PeerConnectionError> {
        let scope = match tokio::time::timeout(
            SCOPE_BUDGET,
            read_scope(self.scope.clone(), self.scope_work.clone()),
        )
        .await
        {
            Ok(Ok(scope)) if scope.local_member_active => scope,
            Ok(Ok(_)) => {
                self.scope_available = false;
                self.clear().await;
                return Ok(());
            }
            Ok(Err(
                error @ PeerConnectionError::Scope(
                    CurrentSpaceMemberScopeError::NoCurrentSpace
                    | CurrentSpaceMemberScopeError::Locked,
                ),
            )) => {
                self.scope_available = false;
                self.clear().await;
                return Err(error);
            }
            other => {
                self.scope_available = false;
                for peer in self.peers.values_mut() {
                    if let Some(cancel) = &peer.in_flight {
                        cancel.cancel();
                    }
                    peer.due = Some(Instant::now());
                }
                for request in self.refreshes.drain(..) {
                    let _ = request.response.send(Err(PeerConnectionError::Paused));
                }
                return match other {
                    Ok(Err(error)) => Err(error),
                    Err(error) => Err(PeerConnectionError::ScopeTimeout(error)),
                    Ok(Ok(_)) => Ok(()),
                };
            }
        };
        self.scope_available = true;
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
            self.peer_reachability.forget(&device).await;
            self.settle_refresh(device, &DialResult::Error(AttemptError::Ineligible));
        }
        self.peer_reachability.activate().await;
        for device in current {
            let state = self.peer_reachability.current_state(&device).await;
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
            }
        }
    }

    fn relay_recovered(&mut self) {
        let now = Instant::now();
        for peer in self.peers.values_mut() {
            peer.failures = 0;
            peer.next_trigger = "relay_recovered";
            if let Some(cancel) = &peer.in_flight {
                peer.rerun = true;
                cancel.cancel();
            } else {
                let at = now + COALESCE;
                peer.due = Some(peer.due.map_or(at, |due| due.min(at)));
            }
        }
    }

    fn peer_reachability_changed(&mut self, event: PeerReachabilityChanged) {
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
                    peer.due = Some(Instant::now() + HEALTH_CHECK_INTERVAL);
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
            let Ok(capacity) = self.attempt_slots.clone().try_acquire_owned() else {
                break;
            };
            let capacity = Arc::new(capacity);
            let Some(peer) = self.peers.get_mut(&device) else {
                continue;
            };
            let cancel = self.cancel.child_token();
            peer.in_flight = Some(cancel.clone());
            peer.started = Some(now);
            peer.started_observation_revision = peer.observation_revision;
            peer.due = None;
            peer.active_trigger = peer.next_trigger;
            let generation = peer.generation;
            let scope = Arc::clone(&self.scope);
            let scope_work = self.scope_work.clone();
            let peer_reachability = Arc::clone(&self.peer_reachability);
            self.dials.push(
                async move {
                    let attempt = async {
                        let scope = tokio::time::timeout(SCOPE_BUDGET, read_scope(scope, scope_work))
                            .await
                            .map_err(AttemptError::ScopeTimeout)?
                            .map_err(AttemptError::ScopeRead)?;
                        if !scope.local_member_active
                            || !scope.usable_peer_device_ids.contains(&device)
                        {
                            return Err(AttemptError::Ineligible);
                        }
                        if cancel.is_cancelled() { return Err(AttemptError::Ineligible); }
                        peer_reachability
                            .verify_reachable(&device)
                            .await
                            .map_err(AttemptError::PeerReachability)
                    };
                    let deadline = Instant::now() + DIAL_BUDGET;
                    let result = ATTEMPT_CAPACITY.scope(capacity, ATTEMPT_DEADLINE.scope(deadline, async { tokio::select! {
                        biased;
                        _ = cancel.cancelled() => DialResult::Cancelled,
                        result = tokio::time::timeout_at(deadline, attempt) => match result {
                            Ok(Ok(state)) => DialResult::State(state),
                            Ok(Err(source)) => DialResult::Error(source),
                            Err(source) => DialResult::Error(AttemptError::Timeout(source)),
                        },
                    } })).await;
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
            if peer.rerun {
                peer.rerun = false;
                peer.due = Some(Instant::now() + COALESCE);
                return;
            }
            if peer.online && peer.observation_revision != peer.started_observation_revision {
                result = DialResult::State(ReachabilityState::Online);
            } else {
                return;
            }
        }
        // 超时发生在适配器之外；仍应保留本次尝试开始后收到的更新成功。
        // 后续离线通知会清除 online，旧缓存也没有新的修订，二者都不能掩盖失败。
        if peer.online
            && peer.observation_revision != peer.started_observation_revision
            && matches!(
                result,
                DialResult::State(ReachabilityState::Offline | ReachabilityState::Unknown)
                    | DialResult::Error(
                        AttemptError::Timeout(_) | AttemptError::PeerReachability(_)
                    )
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
        uc_info!(
            trigger = peer.active_trigger,
            outcome = outcome,
            error_kind = error_kind,
            "peer connection recovery attempt finished"
        );
        if matches!(result, DialResult::State(ReachabilityState::Online)) {
            peer.online = true;
            peer.failures = 0;
            peer.due = Some(Instant::now() + HEALTH_CHECK_INTERVAL);
            peer.next_trigger = "health_check";
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
        for peer in self.peers.values() {
            if let Some(cancel) = &peer.in_flight {
                cancel.cancel();
            }
        }
        self.dials.clear();
        self.peers.clear();
        for request in self.refreshes.drain(..) {
            let _ = request.response.send(Err(PeerConnectionError::Paused));
        }
        self.peer_reachability.disconnect_all().await;
        let mut work = {
            let mut work = self
                .scope_work
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(source) = work.failure.take() {
                record_task_join_failure(DiagnosticTaskKind::PresenceConnectivityWorker);
                self.scope_failure.get_or_insert(source);
            }
            std::mem::replace(&mut work.tasks, JoinSet::new())
        };
        while let Some(result) = work.join_next().await {
            if let Err(source) = result {
                record_task_join_failure(DiagnosticTaskKind::PresenceConnectivityWorker);
                self.scope_failure.get_or_insert(source);
            }
        }
    }
}

/// 查询被逻辑取消后仍由 coordinator 等待真实任务；保留原完整尝试的许可。
async fn read_scope(
    scope: Arc<dyn CurrentSpaceMemberScopePort>,
    work: Arc<BlockingMutex<ScopeWork>>,
) -> Result<CurrentSpaceMemberScope, PeerConnectionError> {
    let capacity = ATTEMPT_CAPACITY.try_with(Clone::clone).ok();
    let (send, receive) = oneshot::channel();
    {
        let mut work = work.lock().unwrap_or_else(PoisonError::into_inner);
        let reconcile_capacity = if capacity.is_none() {
            Some(
                work.reconcile_slot
                    .clone()
                    .try_acquire_owned()
                    // discarded-source[business-outcome]: 私有单槽从不关闭，耗尽按既有 Busy 结算，没有 IO 失败来源。
                    .map_err(|_| PeerConnectionError::Busy)?,
            )
        } else {
            None
        };
        while let Some(result) = work.tasks.try_join_next() {
            if let Err(source) = result {
                record_task_join_failure(DiagnosticTaskKind::PresenceConnectivityWorker);
                work.failure.get_or_insert(source);
            }
        }
        let observation = ObservationContext::capture();
        #[cfg(test)]
        let after_send = work.after_send.take();
        work.tasks.spawn(observation.scope(async move {
            let _capacity = capacity;
            let result = scope.snapshot().await;
            // 读取实际结束后先归还单槽，再通知调用方，避免新刷新误判繁忙。
            drop(reconcile_capacity);
            if let Err(Err(source)) = send.send(result) {
                uc_warn!(
                    error_kind = "scope_read_settled_after_cancellation",
                    io_error_kind = io_error_kind(&source),
                    "peer scope work settled after cancellation"
                );
            }
            #[cfg(test)]
            if let Some(after_send) = after_send {
                if let Ok(permit) = after_send.acquire().await {
                    permit.forget();
                }
            }
        }));
    }
    // 原始查询错误完整传递；任务通道异常也保留类型来源。
    receive
        .await
        .map_err(PeerConnectionError::Response)?
        .map_err(PeerConnectionError::Scope)
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
