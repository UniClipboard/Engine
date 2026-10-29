use super::super::{ATTEMPT_CAPACITY, ATTEMPT_DEADLINE};
use super::{
    model::{Attempt, Evidence},
    PeerReachabilityTransportError, PeerReachabilityTransportPorts,
};
use crate::connectivity::{ConnectivityError, ConnectivityManager, PeerConnection};
use crate::space::KnownPeerContact;
use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot, Semaphore};
use tokio::task::{JoinError, JoinSet};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_core::error_class::ErrorClass;
use uc_core::ids::DeviceId;
use uc_core::membership::PeerAdmissionPort;
use uc_core::ports::{
    ClockPort, PeerReachabilityChanged, PeerReachabilityError, PeerReachabilityPort,
    ReachabilityState,
};
use uc_observability_contract::diagnostics::connectivity::{
    record_presence_check_result, PresenceCheckFeedback, PresenceCheckResult,
};
use uc_observability_contract::diagnostics::ObservationContext;
use uc_observability_contract::{
    diagnostics::{record_task_join_failure, DiagnosticTaskKind},
    error_source::io_error_kind,
    uc_warn,
};

pub(super) const ATTEMPT_BUDGET: Duration = Duration::from_secs(20);
pub(super) const STEP_BUDGET: Duration = Duration::from_secs(5);
pub(super) const FAST_PATH_TTL: Duration = Duration::from_secs(10);
pub(super) type Tasks = Mutex<JoinSet<()>>;

#[derive(Clone)]
pub(super) struct Runner {
    pub manager: ConnectivityManager,
    pub ports: Arc<PeerReachabilityTransportPorts>,
    pub authority: Arc<dyn PeerAdmissionPort>,
    pub clock: Arc<dyn ClockPort>,
    pub contacts: broadcast::Sender<KnownPeerContact>,
    pub evidence: Arc<Mutex<Evidence>>,
    pub events: broadcast::Sender<PeerReachabilityChanged>,
    pub tasks: Weak<Tasks>,
    pub failure: Arc<Mutex<Option<Arc<JoinError>>>>,
}
/// 调度器沿用原完整动作；入站、证据和长期资源由本 owner 结算。
pub struct PeerReachabilityTransport {
    pub(super) runner: Runner,
    pub(super) tasks: Arc<Tasks>,
    attempts: Arc<Semaphore>,
    lifecycle: tokio::sync::Mutex<()>,
}
struct RequestCancellation(CancellationToken);
impl Drop for RequestCancellation {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl PeerReachabilityTransport {
    pub fn new(
        manager: ConnectivityManager,
        authority: Arc<dyn PeerAdmissionPort>,
        ports: PeerReachabilityTransportPorts,
        clock: Arc<dyn ClockPort>,
        contacts: broadcast::Sender<KnownPeerContact>,
    ) -> Arc<Self> {
        let tasks = Arc::new(Mutex::new(JoinSet::new()));
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            runner: Runner {
                manager,
                authority,
                ports: Arc::new(ports),
                clock,
                contacts,
                evidence: Arc::new(Mutex::new(Evidence::default())),
                events,
                tasks: Arc::downgrade(&tasks),
                failure: Arc::new(Mutex::new(None)),
            },
            tasks,
            attempts: Arc::new(Semaphore::new(4)),
            lifecycle: tokio::sync::Mutex::new(()),
        })
    }
    pub async fn accept(
        &self,
        connection: Arc<dyn PeerConnection>,
        received_at: Instant,
    ) -> Result<(), PeerReachabilityTransportError> {
        let deadline = received_at + ATTEMPT_BUDGET;
        let stop = self
            .runner
            .evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stop
            .clone();
        // 此调用在第一次 await 前占入口许可；未知身份也进入同一代次的关闭围栏。
        let pending = self
            .runner
            .manager
            .accept_pending(connection.clone(), deadline.min(received_at + STEP_BUDGET));
        let runner = self.runner.clone();
        let (send, receive) = oneshot::channel();
        self.runner.spawn(async move {
            let worker = async { runner.receive(pending.await?, deadline).await };
            tokio::pin!(worker);
            let result = tokio::select! {
                result = &mut worker => result,
                _ = stop.cancelled() => { connection.close(); worker.await },
                _ = tokio::time::sleep_until(deadline) => { connection.close(); worker.await },
            };
            if let Err(source) = &result {
                Runner::record_failure(source);
            }
            let _ = send.send(result);
        });
        timeout_at(deadline, receive)
            .await
            .map_err(|source| {
                PeerReachabilityTransportError::Connection(ConnectivityError::TimedOut(source))
            })?
            .map_err(PeerReachabilityTransportError::Response)?
    }
    fn submit(
        &self,
        peer: DeviceId,
        fresh: bool,
    ) -> Result<
        (
            oneshot::Receiver<Result<ReachabilityState, PeerReachabilityTransportError>>,
            Instant,
            RequestCancellation,
        ),
        PeerReachabilityTransportError,
    > {
        let attempt = {
            let mut state = self
                .runner
                .evidence
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !state.accepting || state.terminated {
                record_presence_check_result(PresenceCheckResult::Interrupted);
                return Err(PeerReachabilityTransportError::Stopped);
            }
            state.begin(peer)
        };
        let capacity = self
            .attempts
            .clone()
            .try_acquire_owned()
            .map_err(|source| {
                record_presence_check_result(PresenceCheckResult::CapacityExhausted);
                PeerReachabilityTransportError::Connection(ConnectivityError::Busy(source))
            })?;
        let runner = self.runner.clone();
        let deadline = ATTEMPT_DEADLINE
            .try_with(|deadline| *deadline)
            .unwrap_or_else(|_| Instant::now() + ATTEMPT_BUDGET);
        let cancel = attempt.stop.child_token();
        let request = RequestCancellation(cancel.clone());
        let inherited_capacity = ATTEMPT_CAPACITY.try_with(Clone::clone).ok();
        let (send, receive) = oneshot::channel();
        self.runner.spawn(async move {
            let _inherited_capacity = inherited_capacity;
            let _capacity = capacity;
            let result = runner.verify(attempt, fresh, deadline, cancel).await;
            if let Err(source) = &result {
                match source.class() {
                    "connection_busy" => {
                        record_presence_check_result(PresenceCheckResult::CapacityExhausted)
                    }
                    "connection_address_lookup_failed" => {
                        record_presence_check_result(PresenceCheckResult::AddressUnavailable)
                    }
                    "connection_admission_failed" | "connection_admission_timeout" => {
                        record_presence_check_result(PresenceCheckResult::AuthorityUnavailable)
                    }
                    "connection_stopped" | "presence_stopped" => {
                        record_presence_check_result(PresenceCheckResult::Interrupted)
                    }
                    _ => {}
                }
                Runner::record_failure(source);
            }
            let _ = send.send(result);
        });
        Ok((receive, deadline, request))
    }
    pub async fn shutdown(&self) -> Result<(), PeerReachabilityTransportError> {
        self.runner
            .evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .terminated = true;
        self.disconnect_all().await;
        let manager_result = self.runner.manager.shutdown().await;
        if let Some(source) = self
            .runner
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return Err(PeerReachabilityTransportError::Task(source));
        }
        manager_result?;
        Ok(())
    }
}
impl Runner {
    /// 已认证且当前获准的对端刚经 presence 联系本机：记录它正在使用的路径。
    ///
    /// 对端本机地址变化时，其连接协调器借既有的网络变化机会向每个配对设备重新确认在线；本机作为
    /// 应答方据此学到对端的新地址，不新增线上消息、不轮询、不转发第三方地址。刷新只在路径确有
    /// 变化时写入，失败不影响在线结论；在后台执行，不延迟应答。
    pub fn refresh_contact_address(&self, peer: DeviceId) {
        let refresh = Arc::clone(&self.ports.address_refresh);
        self.spawn(async move { refresh.refresh_verified_peer_address(&peer).await });
    }
    pub fn spawn(&self, future: impl std::future::Future<Output = ()> + Send + 'static) {
        let Some(tasks) = self.tasks.upgrade() else {
            return;
        };
        let mut tasks = tasks.lock().unwrap_or_else(PoisonError::into_inner);
        while let Some(result) = tasks.try_join_next() {
            if let Err(error) = result {
                self.failure
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert_with(|| Arc::new(error));
                record_task_join_failure(DiagnosticTaskKind::PresenceConnectivityWorker);
                uc_warn!(error_kind = "presence_task_failed", "presence task failed");
            }
        }
        let observation = ObservationContext::capture();
        let feedback = PresenceCheckFeedback::capture();
        tasks.spawn(observation.scope(feedback.scope(future)));
    }
    pub fn publish(&self, evidence: &mut Evidence, peer: DeviceId, state: ReachabilityState) {
        if evidence.states.insert(peer, state) != Some(state) {
            let at = Utc
                .timestamp_millis_opt(self.clock.now_ms())
                .single()
                .unwrap_or_else(Utc::now);
            let _ = self.events.send(PeerReachabilityChanged {
                device_id: peer,
                state,
                at,
            });
        }
    }
    /// 超时只停止新步骤；已经开始的本地读取实际结算前继续持有原许可。
    pub async fn authorize(
        &self,
        attempt: &Attempt,
        deadline: Instant,
    ) -> Result<bool, PeerReachabilityTransportError> {
        self.authorize_with_cancel(attempt, deadline, &attempt.stop)
            .await
    }
    pub async fn authorize_with_cancel(
        &self,
        attempt: &Attempt,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<bool, PeerReachabilityTransportError> {
        if Instant::now() >= deadline || cancel.is_cancelled() {
            return Err(PeerReachabilityTransportError::Stopped);
        }
        let peer = attempt.peer;
        let work = self.authority.is_admitted(&peer);
        tokio::pin!(work);
        let authorized = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return match work.await {
                    Ok(_) => Err(PeerReachabilityTransportError::Stopped),
                    Err(source) => Err(PeerReachabilityTransportError::Authority(source)),
                };
            }
            result = timeout_at(deadline, &mut work) => result,
        };
        match authorized {
            Ok(Ok(allowed)) => {
                let mut state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
                if state.current(attempt) {
                    state.local_failures.remove(&peer);
                }
                Ok(allowed)
            }
            Ok(Err(source)) => {
                record_presence_check_result(PresenceCheckResult::AuthorityUnavailable);
                self.local_failure(attempt);
                Err(PeerReachabilityTransportError::Authority(source))
            }
            Err(source) => {
                record_presence_check_result(PresenceCheckResult::AuthorityUnavailable);
                self.local_failure(attempt);
                if let Err(error) = work.await {
                    uc_warn!(
                        error_kind = "presence_authority_late_failure",
                        io_error_kind = io_error_kind(&error),
                        "presence authority work settled after its deadline"
                    );
                }
                Err(PeerReachabilityTransportError::AuthorityTimeout(source))
            }
        }
    }
    pub fn local_failure(&self, attempt: &Attempt) {
        let mut state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
        if state.current(attempt) && state.successes.get(&attempt.peer).copied() == attempt.success
        {
            state.local_failures.insert(attempt.peer);
        }
    }
    pub fn failed(&self, attempt: &Attempt) -> ReachabilityState {
        let mut state = self.evidence.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.current(attempt)
            || state.successes.get(&attempt.peer).copied() != attempt.success
            || state
                .connections
                .values()
                .any(|entry| entry.lease.peer() == attempt.peer && !entry.lease.is_closed())
        {
            return state
                .states
                .get(&attempt.peer)
                .copied()
                .unwrap_or(ReachabilityState::Unknown);
        }
        self.publish(&mut state, attempt.peer, ReachabilityState::Offline);
        ReachabilityState::Offline
    }
}
#[async_trait]
impl PeerReachabilityPort for PeerReachabilityTransport {
    async fn ensure_reachable(
        &self,
        peer: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        let (receiver, deadline, _request) = self
            .submit(*peer, false)
            .map_err(|source| source.into_contract(*peer))?;
        timeout_at(deadline, receiver)
            .await
            .map_err(|source| {
                PeerReachabilityTransportError::Connection(ConnectivityError::TimedOut(source))
                    .into_contract(*peer)
            })?
            .map_err(|source| {
                PeerReachabilityTransportError::Response(source).into_contract(*peer)
            })?
            .map_err(|source| source.into_contract(*peer))
    }
    async fn verify_reachable(
        &self,
        peer: &DeviceId,
    ) -> Result<ReachabilityState, PeerReachabilityError> {
        let (receiver, deadline, _request) = self
            .submit(*peer, true)
            .map_err(|source| source.into_contract(*peer))?;
        timeout_at(deadline, receiver)
            .await
            .map_err(|source| {
                PeerReachabilityTransportError::Connection(ConnectivityError::TimedOut(source))
                    .into_contract(*peer)
            })?
            .map_err(|source| {
                PeerReachabilityTransportError::Response(source).into_contract(*peer)
            })?
            .map_err(|source| source.into_contract(*peer))
    }
    async fn report_communication_failure(&self, peer: &DeviceId) {
        self.runner.ports.environment.communication_failed(*peer);
    }
    async fn forget(&self, peer: &DeviceId) {
        let mut state = self
            .runner
            .evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.epochs.remove(peer);
        state.states.remove(peer);
        state.local_failures.remove(peer);
        state.successes.remove(peer);
        let retired: Vec<usize> = state
            .connections
            .iter()
            .filter(|(_, entry)| entry.lease.peer() == *peer)
            .map(|(id, _)| *id)
            .collect();
        for id in retired {
            if let Some(entry) = state.connections.remove(&id) {
                entry.lease.close();
            }
            state.verified.remove(&id);
        }
        self.runner.ports.environment.forget(peer);
    }
    async fn disconnect_all(&self) {
        let _lifecycle = self.lifecycle.lock().await;
        {
            let mut state = self
                .runner
                .evidence
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.accepting = false;
            state.generation = state.generation.wrapping_add(1);
            state.stop.cancel();
            state.epochs.clear();
            state.successes.clear();
            state.states.clear();
            state.local_failures.clear();
            state.verified.clear();
            for (_, entry) in state.connections.drain() {
                entry.lease.close();
            }
            self.runner.ports.environment.disconnect_all();
        }
        if let Err(source) = self.runner.manager.disconnect_all().await {
            uc_warn!(
                error_kind = "presence_manager_shutdown_failed",
                io_error_kind = io_error_kind(&source),
                "presence manager session cleanup failed"
            );
        }
        let mut tasks = {
            let mut tasks = self.tasks.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::replace(&mut *tasks, JoinSet::new())
        };
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result {
                self.runner
                    .failure
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get_or_insert_with(|| Arc::new(error));
                record_task_join_failure(DiagnosticTaskKind::PresenceConnectivityWorker);
                uc_warn!(
                    error_kind = "presence_task_failed",
                    "presence shutdown task failed"
                );
            }
        }
    }
    async fn activate(&self) {
        let _lifecycle = self.lifecycle.lock().await;
        let mut state = self
            .runner
            .evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !state.accepting && !state.terminated {
            state.stop = CancellationToken::new();
            state.accepting = true;
        }
    }
    async fn current_state(&self, peer: &DeviceId) -> ReachabilityState {
        self.runner
            .evidence
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .states
            .get(peer)
            .copied()
            .unwrap_or(ReachabilityState::Unknown)
    }
    fn subscribe(&self) -> broadcast::Receiver<PeerReachabilityChanged> {
        self.runner.events.subscribe()
    }
}
