use super::acquisition::Acquisition;
use super::checks::checked;
use super::connection::ConnectionRecord;
use super::inbound::{InboundAdmissionPolicy, InboundReservation};
use super::lifetime::ConnectionLifetime;
use super::pending::{PendingInboundLease, UnboundConnection};
use super::retention::{ConnectionActivity, IdleRetentionPolicy};
use super::{
    AdmissionAttempt, AdmissionGrant, ConnectionLease, ConnectionMode, ConnectionRequest,
    ConnectivityBackend, ConnectivityError, InboundConnectionError, InboundIdentityReservation,
    PeerConnection, SessionConnectionAdmissionPort, StreamLease,
};
use futures::future::{ready, Either};
use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{oneshot, watch, Semaphore};
use tokio::task::{JoinError, JoinSet};
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use tokio_util::task::AbortOnDropHandle;
use tracing::instrument::WithSubscriber;
use tracing::{Instrument, Span};
use uc_core::ids::DeviceId;
use uc_observability_contract::diagnostics::{record_task_join_failure, DiagnosticTaskKind};

#[derive(Clone, Copy)]
pub struct ConnectivityPolicy {
    /// presence 首流准入与长期保留采用独立生命周期，其他协议保持默认策略。
    pub presence_retention: bool,
    /// 取消／到期后等待已开始的拨号与本地准入工作实际结束。
    pub supervise_local_work: bool,
    /// 显式配置时，未知身份与识别后准入使用不同额度及同一绝对期限。
    pub inbound_admission: Option<InboundAdmissionPolicy>,
    /// 包括拨号中与关闭未完成的连接；不把方向名额当作交换额度。
    pub connections_per_peer_direction: Option<usize>,
    pub outbound_per_peer: usize,
    pub inbound_per_connection: usize,
    pub inbound_per_session: usize,
    pub waiters_per_peer: usize,
    pub waiters_per_session: usize,
    pub flights_per_session: usize,
    pub idle_retention: Option<IdleRetentionPolicy>,
    pub accepted_stream_timeout: Option<Duration>,
    pub task_kind: DiagnosticTaskKind,
}
impl Default for ConnectivityPolicy {
    fn default() -> Self {
        Self {
            presence_retention: false,
            supervise_local_work: false,
            inbound_admission: None,
            connections_per_peer_direction: None,
            outbound_per_peer: 4,
            inbound_per_connection: 4,
            inbound_per_session: 16,
            waiters_per_peer: 32,
            waiters_per_session: 128,
            flights_per_session: 64,
            idle_retention: None,
            accepted_stream_timeout: None,
            task_kind: DiagnosticTaskKind::ClipboardConnectivityWorker,
        }
    }
}
impl ConnectivityPolicy {
    pub(super) fn wait_for_local_work(self) -> bool {
        self.presence_retention
            || self.supervise_local_work
            || self.inbound_admission.is_some()
            || self.connections_per_peer_direction.is_some()
    }
}
pub(super) struct PeerState {
    pub outbound: Arc<Semaphore>,
    pub waiting: Arc<Semaphore>,
    pub pending_inbound: Arc<Semaphore>,
    pub inbound_connections: Option<Arc<Semaphore>>,
    pub outbound_connections: Option<Arc<Semaphore>>,
}
struct FlightOutcome {
    mode: ConnectionMode,
    record: Mutex<Option<Arc<ConnectionRecord>>>,
}
type FlightResult = Result<Arc<FlightOutcome>, Arc<ConnectivityError>>;
struct Flight {
    cancel: CancellationToken,
    waiters: AtomicUsize,
    result: watch::Sender<Option<FlightResult>>,
    permit: Mutex<Option<OwnedSemaphorePermit>>,
}
impl Flight {
    async fn settled(&self) -> Result<(), ConnectivityError> {
        let mut result = self.result.subscribe();
        loop {
            if let Some(outcome) = result.borrow_and_update().as_ref() {
                return match outcome {
                    Ok(_) => Ok(()),
                    Err(source) if matches!(source.as_ref(), ConnectivityError::Stopped) => Ok(()),
                    Err(source) => Err(ConnectivityError::Shared(source.clone())),
                };
            }
            result
                .changed()
                .await
                .map_err(ConnectivityError::ResultClosed)?;
        }
    }
}
struct Waiter(Arc<Flight>);
impl Drop for Waiter {
    fn drop(&mut self) {
        if self.0.waiters.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.cancel.cancel();
        }
    }
}
#[derive(Default)]
struct State {
    peers: HashMap<DeviceId, Weak<PeerState>>,
    active: HashMap<DeviceId, Weak<ConnectionRecord>>,
    flights: HashMap<DeviceId, Arc<Flight>>,
    live: Vec<Weak<ConnectionRecord>>,
}
pub(super) struct Inner {
    backend: Arc<dyn ConnectivityBackend>,
    pub(super) admission: Arc<dyn SessionConnectionAdmissionPort>,
    policy: ConnectivityPolicy,
    state: Mutex<State>,
    retained: Mutex<HashMap<(DeviceId, bool), Arc<ConnectionRecord>>>,
    tasks: Mutex<JoinSet<()>>,
    stop: CancellationToken,
    session: Mutex<CancellationToken>,
    failure: Mutex<Option<Arc<JoinError>>>,
    waiting: Arc<Semaphore>,
    flights: Arc<Semaphore>,
    inbound: Arc<Semaphore>,
    pub(super) incoming: Arc<Semaphore>,
    shutdown: tokio::sync::Mutex<()>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// 同一协议会话的完整连接负责人。任务由自身 JoinSet 持有。
#[derive(Clone)]
pub struct ConnectivityManager {
    pub(super) inner: Arc<Inner>,
}
impl ConnectivityManager {
    pub fn new(
        backend: Arc<dyn ConnectivityBackend>,
        admission: Arc<dyn SessionConnectionAdmissionPort>,
        policy: ConnectivityPolicy,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend,
                admission,
                policy,
                state: Mutex::new(State::default()),
                retained: Mutex::new(HashMap::new()),
                tasks: Mutex::new(JoinSet::new()),
                failure: Mutex::new(None),
                stop: CancellationToken::new(),
                session: Mutex::new(CancellationToken::new()),
                waiting: Arc::new(Semaphore::new(policy.waiters_per_session)),
                flights: Arc::new(Semaphore::new(policy.flights_per_session)),
                inbound: Arc::new(Semaphore::new(policy.inbound_per_session)),
                incoming: Arc::new(Semaphore::new(policy.inbound_admission.map_or_else(
                    || {
                        if policy.presence_retention {
                            16
                        } else {
                            policy.waiters_per_session
                        }
                    },
                    |limits| limits.unidentified,
                ))),
                shutdown: tokio::sync::Mutex::new(()),
            }),
        }
    }
    pub(super) fn peer(&self, target: DeviceId) -> Arc<PeerState> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.peers.retain(|_, peer| peer.strong_count() > 0);
        if let Some(peer) = state.peers.get(&target).and_then(Weak::upgrade) {
            return peer;
        }
        let peer = Arc::new(PeerState {
            inbound_connections: self
                .inner
                .policy
                .connections_per_peer_direction
                .map(|limit| Arc::new(Semaphore::new(limit))),
            outbound_connections: self
                .inner
                .policy
                .connections_per_peer_direction
                .map(|limit| Arc::new(Semaphore::new(limit))),
            outbound: Arc::new(Semaphore::new(self.inner.policy.outbound_per_peer)),
            waiting: Arc::new(Semaphore::new(self.inner.policy.waiters_per_peer)),
            pending_inbound: Arc::new(Semaphore::new(
                self.inner
                    .policy
                    .inbound_admission
                    .map_or(2, |limits| limits.identified_per_peer),
            )),
        });
        state.peers.insert(target, Arc::downgrade(&peer));
        peer
    }
    /// 同步冻结本请求的准入与会话；协议可在自己的有界队列之前创建它。
    pub fn request(&self, target: DeviceId) -> ConnectionRequest {
        ConnectionRequest {
            manager: self.clone(),
            target,
            attempt: Arc::from(self.inner.admission.begin(target)),
            session: self.stop_token(),
        }
    }
    pub(super) async fn acquire_request(
        &self,
        request: &ConnectionRequest,
        deadline: Instant,
        cancel: &CancellationToken,
    ) -> Result<ConnectionLease, ConnectivityError> {
        if cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        if Instant::now() >= deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        let target = request.target;
        let attempt = request.attempt.clone();
        let peer = self.peer(target);
        let _global = self
            .inner
            .waiting
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy)?;
        let _local = peer
            .waiting
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy)?;
        let (flight, launch) = {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if self.inner.stop.is_cancelled() {
                return Err(ConnectivityError::Stopped);
            }
            state.active.retain(|_, record| record.strong_count() > 0);
            if let Some(record) = state.active.get(&target).and_then(Weak::upgrade) {
                if !record.connection.is_closed() && !record.cache_cancel.is_cancelled() {
                    return ConnectionLease::outbound(record, attempt.clone());
                }
            }
            let (flight, launch) = match state.flights.get(&target) {
                Some(flight) if !flight.cancel.is_cancelled() => (flight.clone(), false),
                _ => {
                    let (result, _) = watch::channel(None);
                    let flight = Arc::new(Flight {
                        cancel: self.stop_token().child_token(),
                        waiters: AtomicUsize::new(0),
                        result,
                        permit: Mutex::new(Some(
                            self.inner
                                .flights
                                .clone()
                                .try_acquire_owned()
                                .map_err(ConnectivityError::Busy)?,
                        )),
                    });
                    state.flights.insert(target, flight.clone());
                    (flight, true)
                }
            };
            flight.waiters.fetch_add(1, Ordering::AcqRel);
            (flight, launch)
        };
        let _waiter = Waiter(flight.clone());
        if launch {
            self.launch(target, peer.clone(), flight.clone(), attempt.clone());
        }
        let mut rx = flight.result.subscribe();
        let awaiting = timeout_at(deadline, async {
            loop {
                if let Some(result) = rx.borrow_and_update().clone() {
                    return result.map_err(ConnectivityError::Shared);
                }
                tokio::select! {
                    _ = self.inner.stop.cancelled() => return Err(ConnectivityError::Stopped),
                    result = rx.changed() => { result.map_err(ConnectivityError::ResultClosed)?; }
                }
            }
        });
        let outcome = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                drop(_waiter);
                if self.inner.policy.wait_for_local_work() && flight.waiters.load(Ordering::Acquire) == 0 {
                    if let Err(additional) = flight.settled().await {
                        return Err(ConnectivityError::Settlement {
                            primary: Box::new(ConnectivityError::Stopped),
                            additional: Box::new(additional),
                        });
                    }
                }
                return Err(ConnectivityError::Stopped);
            }
            result = awaiting => result,
        };
        let outcome = match outcome {
            Ok(result) => result?,
            Err(source) => {
                drop(_waiter);
                if self.inner.policy.wait_for_local_work()
                    && flight.waiters.load(Ordering::Acquire) == 0
                {
                    if let Err(additional) = flight.settled().await {
                        return Err(ConnectivityError::Settlement {
                            primary: Box::new(ConnectivityError::TimedOut(source)),
                            additional: Box::new(additional),
                        });
                    }
                }
                return Err(ConnectivityError::TimedOut(source));
            }
        };
        if self.inner.stop.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        let record = {
            let mut record = outcome
                .record
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match outcome.mode {
                ConnectionMode::Multiplexed => record.clone(),
                ConnectionMode::SingleExchange | ConnectionMode::RetainedSingleExchange => {
                    record.take()
                }
            }
        };
        match record {
            Some(record) if !record.connection.is_closed() => {
                ConnectionLease::outbound(record, attempt.clone())
            }
            Some(_) | None => {
                if self.inner.policy.presence_retention {
                    return Err(ConnectivityError::Offline);
                }
                // 旧端已领取或共同结果已经失活；负载尚未发送，原请求期限内有界取得新连接。
                let record = tokio::select! {
                    _ = self.inner.stop.cancelled() => return Err(ConnectivityError::Stopped),
                    result = timeout_at(deadline, self.acquire(target, peer, attempt.clone())) => result.map_err(ConnectivityError::TimedOut)??,
                };
                ConnectionLease::outbound(record, attempt.clone())
            }
        }
    }
    /// 尚未发送请求的已失活连接可重获；原请求撤销快照不能随新拨号刷新。
    pub async fn refresh(
        &self,
        previous: &ConnectionLease,
        deadline: Instant,
    ) -> Result<ConnectionLease, ConnectivityError> {
        let request = previous.request.clone().ok_or(ConnectivityError::Denied)?;
        request.check_current().await?;
        if !previous.is_closed() && !previous.is_unreachable() {
            return Err(ConnectivityError::Denied);
        }
        let frozen = ConnectionRequest {
            manager: self.clone(),
            target: previous.peer(),
            attempt: request,
            session: previous.record.stop.clone(),
        };
        let lease = frozen.connect(deadline, &CancellationToken::new()).await?;
        lease.check_current().await?;
        Ok(lease)
    }
    fn launch(
        &self,
        target: DeviceId,
        peer: Arc<PeerState>,
        flight: Arc<Flight>,
        attempt: Arc<dyn AdmissionAttempt>,
    ) {
        let weak = Arc::downgrade(&self.inner);
        let task_kind = self.inner.policy.task_kind;
        let mut acquisition = self.acquisition();
        acquisition.cancel = flight.cancel.clone();
        let pending = acquisition.pending.clone();
        let settle_local = self.inner.policy.wait_for_local_work();
        let mut tasks = self
            .inner
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.inner.stop.is_cancelled() {
            flight
                .permit
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            self.inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .flights
                .remove(&target);
            flight
                .result
                .send_replace(Some(Err(Arc::new(ConnectivityError::Stopped))));
            return;
        }
        while let Some(result) = tasks.try_join_next() {
            Self::record_task_result(&self.inner, result);
        }
        tasks.spawn(
            async move {
                let acquisition = async move { acquisition.connect(target, peer, attempt).await };
                let mut worker = AbortOnDropHandle::new(tokio::spawn(
                    acquisition
                        .instrument(Span::none())
                        .with_current_subscriber(),
                ));
                let joined = tokio::select! {
                    _ = flight.cancel.cancelled() => {
                        if let Some(connection) = pending.lock().unwrap_or_else(PoisonError::into_inner).as_ref() { connection.close(); }
                        if !settle_local { worker.abort(); }
                        worker.await
                    },
                    result = &mut worker => result,
                };
                let mut result = match joined {
                    Ok(result) => result.map_err(Arc::new),
                    Err(error) if error.is_cancelled() && flight.cancel.is_cancelled() => {
                        Err(Arc::new(ConnectivityError::Stopped))
                    }
                    Err(error) => {
                        let error = Arc::new(error);
                        record_task_join_failure(task_kind);
                        if let Some(inner) = weak.upgrade() {
                            *inner.failure.lock().unwrap_or_else(PoisonError::into_inner) =
                                Some(error.clone());
                        }
                        Err(Arc::new(ConnectivityError::Task(error)))
                    }
                };
                flight
                    .permit
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(inner) = weak.upgrade() {
                    let mut state = inner.state.lock().unwrap_or_else(PoisonError::into_inner);
                    // shutdown 可能已移除 flight；结果发布仍必须检查取消，不依赖表中所有权。
                    if result.is_ok() && (inner.stop.is_cancelled() || flight.cancel.is_cancelled()) {
                        result = Err(Arc::new(ConnectivityError::Stopped));
                    }
                    if state
                        .flights
                        .get(&target)
                        .is_some_and(|current| Arc::ptr_eq(current, &flight))
                    {
                        state.flights.remove(&target);
                        if let Ok(record) = &result {
                            state.live.retain(|record| record.strong_count() > 0);
                            state.live.push(Arc::downgrade(record));
                            if record.connection.mode() == ConnectionMode::Multiplexed
                                && !flight.cancel.is_cancelled()
                            {
                                state.active.insert(target, Arc::downgrade(record));
                            }
                        }
                    }
                }
                let result = result.map(|record| {
                    Arc::new(FlightOutcome {
                        mode: record.connection.mode(),
                        record: Mutex::new(Some(record)),
                    })
                });
                flight.result.send_replace(Some(result));
            }
            .instrument(Span::none())
            .with_current_subscriber(),
        );
    }
    fn acquisition(&self) -> Acquisition {
        Acquisition {
            owner: Arc::downgrade(&self.inner),
            backend: self.inner.backend.clone(),
            policy: self.inner.policy,
            inbound: self.inner.inbound.clone(),
            waiting: self.inner.waiting.clone(),
            stop: self.stop_token(),
            cancel: self.stop_token().child_token(),
            pending: Arc::new(Mutex::new(None)),
        }
    }
    async fn acquire(
        &self,
        target: DeviceId,
        peer: Arc<PeerState>,
        attempt: Arc<dyn AdmissionAttempt>,
    ) -> Result<Arc<ConnectionRecord>, ConnectivityError> {
        let _permit = self
            .inner
            .flights
            .clone()
            .acquire_owned()
            .await
            .map_err(ConnectivityError::ResourceClosed)?;
        let record = self.acquisition().connect(target, peer, attempt).await?;
        self.track(&record)?;
        Ok(record)
    }
    pub(super) async fn admit_pending(
        &self,
        connection: Arc<dyn PeerConnection>,
        reservation: &dyn InboundIdentityReservation,
    ) -> Result<Box<dyn AdmissionGrant>, ConnectivityError> {
        self.inner
            .admission
            .adopt_inbound(connection, reservation)
            .await
    }
    pub(super) fn stop_token(&self) -> CancellationToken {
        self.inner
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    pub(super) fn policy(&self) -> ConnectivityPolicy {
        self.inner.policy
    }
    pub(super) fn inbound_capacity(&self) -> Arc<Semaphore> {
        self.inner.inbound.clone()
    }
    pub(super) fn waiting_capacity(&self) -> Arc<Semaphore> {
        self.inner.waiting.clone()
    }
    /// 单次协议从入口起共用额度和期限；准入、开流及交接均不续期。
    pub async fn accept_exchange(
        &self,
        connection: Arc<dyn PeerConnection>,
        deadline: Instant,
    ) -> Result<(ConnectionLease, StreamLease), ConnectivityError> {
        let fence = self.inner.admission.inbound_fence();
        let session = self.stop_token();
        let raw = UnboundConnection {
            raw: connection,
            adopted: false,
        };
        let incoming = self
            .inner
            .incoming
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy)?;
        let lifetime = Arc::new(ConnectionLifetime::default());
        Self::watch_connection(
            &self.inner,
            raw.raw.clone(),
            lifetime.clone(),
            session.clone(),
        );
        tokio::select! {
            _ = self.inner.stop.cancelled() => Err(ConnectivityError::Stopped),
            result = timeout_at(deadline, async {
                let global = self.inner.inbound.clone().acquire_owned().await
                    .map_err(ConnectivityError::ResourceClosed)?;
                let grant = self.inner.admission.adopt_inbound(raw.raw.clone(), &fence).await?;
                if let Some(fence) = &fence { fence.check_current(&grant.peer())?; }
                grant.check_current().await?;
                let pair = raw.raw.accept_bi().await?;
                PendingInboundLease {
                    send: pair.0, receive: pair.1, deadline,
                    received_at: Instant::now(), raw, manager: self.clone(),
                    incoming, global, fence, lifetime, session, identified_capacity: None, identified_peer: None,
                }.bind(grant).await
            }) => result.map_err(ConnectivityError::TimedOut)?,
        }
    }
    pub fn accept_pending(
        &self,
        connection: Arc<dyn PeerConnection>,
        deadline: Instant,
    ) -> impl Future<Output = Result<PendingInboundLease, ConnectivityError>> + Send + 'static {
        let manager = self.clone();
        let fence = manager.inner.admission.inbound_fence();
        let session = manager.stop_token();
        let raw = UnboundConnection {
            raw: connection,
            adopted: false,
        };
        let incoming = manager
            .inner
            .incoming
            .clone()
            .try_acquire_owned()
            .map_err(ConnectivityError::Busy);
        if incoming.is_err() {
            raw.raw.close();
        }
        async move {
            let incoming = incoming?;
            if session.is_cancelled() {
                return Err(ConnectivityError::Stopped);
            }
            let lifetime = Arc::new(ConnectionLifetime::default());
            Self::watch_connection(
                &manager.inner,
                raw.raw.clone(),
                lifetime.clone(),
                session.clone(),
            );
            let (pair, global, received_at) = tokio::select! {
                _ = manager.inner.stop.cancelled() => return Err(ConnectivityError::Stopped),
                result = timeout_at(deadline, async {
                    let pair = raw.raw.accept_bi().await?;
                    let received_at = Instant::now();
                    let global = manager.inner.inbound.clone().acquire_owned().await.map_err(ConnectivityError::ResourceClosed)?;
                    Ok::<_, ConnectivityError>((pair, global, received_at))
                }) => result.map_err(ConnectivityError::TimedOut)??,
            };
            Ok(PendingInboundLease {
                send: pair.0,
                receive: pair.1,
                raw,
                manager: manager.clone(),
                session,
                lifetime,
                incoming,
                global,
                fence,
                deadline,
                received_at,
                identified_capacity: None,
                identified_peer: None,
            })
        }
    }
    pub fn accept(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> impl Future<Output = Result<ConnectionLease, InboundConnectionError>> + Send + 'static
    {
        let pending = match InboundReservation::new(self.clone(), connection) {
            Ok(pending) => pending,
            Err(cause) => {
                return Either::Left(ready(Err(InboundConnectionError {
                    cause,
                    _capacity: None,
                })))
            }
        };
        let work = async move {
            let mut pending = pending;
            let manager = pending.manager.clone();
            if pending.session.is_cancelled() {
                return Err(pending.failed(ConnectivityError::Stopped));
            }
            let admitted = manager
                .inner
                .admission
                .adopt_inbound(pending.raw.raw.clone(), &pending);
            let grant = match pending.deadline {
                Some(deadline) => {
                    let checking = checked(
                        admitted,
                        manager.policy().wait_for_local_work(),
                        deadline,
                        &pending.session,
                    );
                    tokio::pin!(checking);
                    tokio::select! {
                        biased;
                        _ = pending.session.cancelled() => { pending.raw.raw.close(); checking.await },
                        _ = tokio::time::sleep_until(deadline) => { pending.raw.raw.close(); checking.await },
                        result = &mut checking => result,
                    }
                }
                None => admitted.await,
            };
            let grant = match grant {
                Ok(grant) => grant,
                Err(cause) => return Err(pending.failed(cause)),
            };
            if let Err(cause) = pending.verify(grant.peer()) {
                return Err(pending.failed(cause));
            }
            let peer = manager.peer(grant.peer());
            let record = Arc::new(ConnectionRecord {
                connection: pending.raw.raw.clone(),
                activity: Arc::new(ConnectionActivity::default()),
                stop: pending.session.clone(),
                cache_cancel: CancellationToken::new(),
                supervise_local_work: manager.policy().wait_for_local_work(),
                accepted_stream_timeout: manager.policy().accepted_stream_timeout,
                pending: tokio::sync::Mutex::new(None),
                exchange_claimed: AtomicBool::new(false),
                grant,
                peer,
                inbound: Arc::new(Semaphore::new(manager.policy().inbound_per_connection)),
                inbound_global: manager.inner.inbound.clone(),
                waiting_global: manager.inner.waiting.clone(),
                inbound_direction: true,
                _lifetime: pending.lifetime.clone(),
                _inbound_connection: pending.take_unidentified(),
            });
            pending.raw.adopted = true;
            manager
                .track(&record)
                .map_err(|cause| InboundConnectionError {
                    cause,
                    _capacity: None,
                })?;
            Ok(ConnectionLease {
                record,
                request: None,
                _activity: None,
            })
        };
        if self.inner.policy.wait_for_local_work() {
            let (send, receive) = oneshot::channel();
            let weak = Arc::downgrade(&self.inner);
            let task_kind = self.inner.policy.task_kind;
            let mut tasks = self
                .inner
                .tasks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            while let Some(result) = tasks.try_join_next() {
                Self::record_task_result(&self.inner, result);
            }
            // 请求方丢弃等待只关闭结果通道；已开始的本地准入仍归同一 JoinSet。
            tasks.spawn(
                async move {
                    let worker = AbortOnDropHandle::new(tokio::spawn(
                        work.instrument(Span::none()).with_current_subscriber(),
                    ));
                    let result = match worker.await {
                        Ok(result) => result,
                        Err(error) => {
                            record_task_join_failure(task_kind);
                            let error = Arc::new(error);
                            if let Some(inner) = weak.upgrade() {
                                inner
                                    .failure
                                    .lock()
                                    .unwrap_or_else(PoisonError::into_inner)
                                    .get_or_insert_with(|| error.clone());
                            }
                            Err(InboundConnectionError {
                                cause: ConnectivityError::Task(error),
                                _capacity: None,
                            })
                        }
                    };
                    // 已无等待者时直接释放结果，不能留下授权连接或额度。
                    // discarded-source[channel]: 等待方已退出，错误携带结果本体；在此释放已结算结果。
                    let _ = send.send(result);
                }
                .instrument(Span::none())
                .with_current_subscriber(),
            );
            let owner = self.clone();
            Either::Right(Either::Left(async move {
                let _owner = owner;
                receive.await.map_err(|source| InboundConnectionError {
                    cause: ConnectivityError::InboundResultClosed(source),
                    _capacity: None,
                })?
            }))
        } else {
            Either::Right(Either::Right(work))
        }
    }
    /// 交换预算已耗尽时停止复用；在途兄弟流保留原使用权，最后使用者结束才关闭旧连接。
    pub fn evict(&self, lease: &ConnectionLease) {
        lease.record.cache_cancel.cancel();
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if state
                .active
                .get(&lease.peer())
                .and_then(Weak::upgrade)
                .is_some_and(|entry| Arc::ptr_eq(&entry, &lease.record))
            {
                state.active.remove(&lease.peer());
            }
        }
        let key = (lease.peer(), lease.record.inbound_direction);
        let mut retained = self
            .inner
            .retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if retained
            .get(&key)
            .is_some_and(|entry| Arc::ptr_eq(entry, &lease.record))
        {
            retained.remove(&key);
        }
    }
    /// 取消指定使用权的跨轮保留；失效连接不再留给后续请求。
    pub fn retire(&self, lease: &ConnectionLease) {
        let key = (lease.peer(), lease.record.inbound_direction);
        let mut retained = self
            .inner
            .retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if retained
            .get(&key)
            .is_some_and(|entry| Arc::ptr_eq(entry, &lease.record))
        {
            retained.remove(&key);
            lease.close();
        }
    }
    /// 只有完整准入负责人授予当前资格后才持有强引用；返回 false 表示本次交换后释放。
    pub async fn retain(&self, lease: &ConnectionLease) -> Result<bool, ConnectivityError> {
        let Some(policy) = self.inner.policy.idle_retention else {
            return Ok(false);
        };
        if lease.mode() != ConnectionMode::Multiplexed || lease.record.cache_cancel.is_cancelled() {
            return Ok(false);
        }
        if !lease.record.grant.can_retain().await? {
            self.retire(lease);
            return Ok(false);
        }
        lease.check_current().await?;
        let key = (lease.peer(), lease.record.inbound_direction);
        let mut tasks = self
            .inner
            .tasks
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.inner.stop.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        let mut retained = self
            .inner
            .retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        retained.retain(|_, record| !record.connection.is_closed());
        if let Some(existing) = retained.get(&key) {
            return Ok(Arc::ptr_eq(existing, &lease.record));
        }
        if retained.len() >= policy.capacity {
            let oldest = retained
                .iter()
                .filter_map(|(key, record)| record.activity.idle_since().map(|since| (*key, since)))
                .min_by_key(|(_, since)| *since);
            if let Some((oldest, since)) = oldest {
                if let Some(record) = retained.get(&oldest) {
                    if record.activity.retire_if_idle(record, since) {
                        retained.remove(&oldest);
                    }
                }
            }
            if retained.len() >= policy.capacity {
                return Ok(false);
            }
        }
        if lease.record.connection.is_closed() {
            return Err(ConnectivityError::Stopped);
        }
        retained.insert(key, lease.record.clone());
        let record = lease.record.clone();
        let weak = Arc::downgrade(&self.inner);
        let stop = self.inner.stop.clone();
        tasks.spawn(async move {
            loop {
                let changed = record.activity.changed.notified();
                let deadline = record.activity.idle_since().map(|since| since + policy.timeout);
                tokio::select! {
                    _ = stop.cancelled() => break,
                    _ = record.cache_cancel.cancelled() => break,
                    _ = record.connection.closed() => break,
                    _ = changed => continue,
                    _ = async {
                        match deadline {
                            Some(deadline) => tokio::time::sleep_until(deadline).await,
                            None => std::future::pending().await,
                        }
                    } => {
                        if record.activity.retire_if_idle(&record, Instant::now() - policy.timeout) { break; }
                    }
                }
            }
            if let Some(inner) = weak.upgrade() {
                let mut retained = inner.retained.lock().unwrap_or_else(PoisonError::into_inner);
                if retained.get(&key).is_some_and(|entry| Arc::ptr_eq(entry, &record)) { retained.remove(&key); }
            }
        }.instrument(Span::none()).with_current_subscriber());
        Ok(true)
    }
    pub(super) fn track(&self, record: &Arc<ConnectionRecord>) -> Result<(), ConnectivityError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.inner.stop.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        state.live.retain(|record| record.strong_count() > 0);
        state.live.push(Arc::downgrade(record));
        Ok(())
    }
    pub(super) fn watch_connection(
        inner: &Inner,
        connection: Arc<dyn PeerConnection>,
        lifetime: Arc<ConnectionLifetime>,
        stop: CancellationToken,
    ) {
        let mut tasks = inner.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        while let Some(result) = tasks.try_join_next() {
            Self::record_task_result(inner, result);
        }
        // 不持 record/活动租约；方向名额等真实关闭与所有本地工作都完成后才释放。
        tasks.spawn(
            async move {
                let _lifetime = lifetime;
                tokio::select! {
                    _ = stop.cancelled() => { connection.close(); connection.closed().await; },
                    _ = connection.closed() => {},
                }
            }
            .instrument(Span::none())
            .with_current_subscriber(),
        );
    }
    fn record_task_result(inner: &Inner, result: Result<(), JoinError>) {
        if let Err(error) = result {
            record_task_join_failure(inner.policy.task_kind);
            let mut failure = inner.failure.lock().unwrap_or_else(PoisonError::into_inner);
            if failure.is_none() {
                *failure = Some(Arc::new(error));
            }
        }
    }
    async fn join_tasks(&self) {
        loop {
            let mut tasks = {
                let mut owned = self
                    .inner
                    .tasks
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                if owned.is_empty() {
                    break;
                }
                std::mem::replace(&mut *owned, JoinSet::new())
            };
            while let Some(result) = tasks.join_next().await {
                Self::record_task_result(&self.inner, result);
            }
        }
    }
    /// 会话退出等待真实本地工作及未知身份连接结算；重新激活复用同一 manager。
    pub async fn disconnect_all(&self) -> Result<(), ConnectivityError> {
        let _shutdown = self.inner.shutdown.lock().await;
        self.stop_token().cancel();
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for record in state.live.iter().filter_map(Weak::upgrade) {
                record.connection.close();
            }
            for flight in state.flights.values() {
                flight.cancel.cancel();
            }
            state.active.clear();
            state.live.clear();
            self.inner
                .retained
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
        self.join_tasks().await;
        *self
            .inner
            .session
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = CancellationToken::new();
        match self
            .inner
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            Some(source) => Err(ConnectivityError::Task(source)),
            None => Ok(()),
        }
    }
    pub async fn shutdown(&self) -> Result<(), ConnectivityError> {
        let _shutdown = self.inner.shutdown.lock().await;
        self.inner.stop.cancel();
        self.stop_token().cancel();
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for record in state.live.iter().filter_map(Weak::upgrade) {
                record.connection.close();
            }
            self.inner
                .retained
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
            state.active.clear();
            state.live.clear();
            for flight in state.flights.drain().map(|(_, flight)| flight) {
                flight.cancel.cancel();
            }
        }
        self.join_tasks().await;
        let failure = self
            .inner
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        match failure {
            Some(error) => Err(ConnectivityError::Task(error)),
            None => Ok(()),
        }
    }
}
