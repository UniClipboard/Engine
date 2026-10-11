use super::acquisition::Acquisition;
use super::connection::ConnectionRecord;
use super::{
    AdmissionAttempt, ConnectionLease, ConnectionMode, ConnectivityBackend, ConnectivityError,
    InboundConnectionError, PeerConnection, SessionConnectionAdmissionPort,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::{watch, Semaphore};
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
    pub outbound_per_peer: usize,
    pub inbound_per_connection: usize,
    pub inbound_per_session: usize,
    pub waiters_per_peer: usize,
    pub waiters_per_session: usize,
    pub flights_per_session: usize,
}
impl Default for ConnectivityPolicy {
    fn default() -> Self {
        Self {
            outbound_per_peer: 4,
            inbound_per_connection: 4,
            inbound_per_session: 16,
            waiters_per_peer: 32,
            waiters_per_session: 128,
            flights_per_session: 64,
        }
    }
}
pub(super) struct PeerState {
    pub outbound: Arc<Semaphore>,
    pub waiting: Arc<Semaphore>,
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
struct Inner {
    backend: Arc<dyn ConnectivityBackend>,
    admission: Arc<dyn SessionConnectionAdmissionPort>,
    policy: ConnectivityPolicy,
    state: Mutex<State>,
    tasks: Mutex<JoinSet<()>>,
    stop: CancellationToken,
    failure: Mutex<Option<Arc<JoinError>>>,
    waiting: Arc<Semaphore>,
    flights: Arc<Semaphore>,
    inbound: Arc<Semaphore>,
    incoming: Arc<Semaphore>,
    shutdown: tokio::sync::Mutex<()>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

/// 同一会话普通剪贴板的完整连接负责人。任务由自身 JoinSet 持有。
#[derive(Clone)]
pub struct ConnectivityManager {
    inner: Arc<Inner>,
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
                tasks: Mutex::new(JoinSet::new()),
                failure: Mutex::new(None),
                stop: CancellationToken::new(),
                waiting: Arc::new(Semaphore::new(policy.waiters_per_session)),
                flights: Arc::new(Semaphore::new(policy.flights_per_session)),
                inbound: Arc::new(Semaphore::new(policy.inbound_per_session)),
                incoming: Arc::new(Semaphore::new(policy.waiters_per_session)),
                shutdown: tokio::sync::Mutex::new(()),
            }),
        }
    }
    fn peer(&self, target: DeviceId) -> Arc<PeerState> {
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
            outbound: Arc::new(Semaphore::new(self.inner.policy.outbound_per_peer)),
            waiting: Arc::new(Semaphore::new(self.inner.policy.waiters_per_peer)),
        });
        state.peers.insert(target, Arc::downgrade(&peer));
        peer
    }
    pub async fn connect(
        &self,
        target: DeviceId,
        deadline: Instant,
    ) -> Result<ConnectionLease, ConnectivityError> {
        if Instant::now() >= deadline {
            return Err(ConnectivityError::DeadlineExpired);
        }
        // 在任何排队之前冻结撤销基线。
        let attempt: Arc<dyn AdmissionAttempt> = Arc::from(self.inner.admission.begin(target));
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
                if !record.connection.is_closed() {
                    return Ok(ConnectionLease {
                        record,
                        request: Some(attempt.clone()),
                    });
                }
            }
            let (flight, launch) = match state.flights.get(&target) {
                Some(flight) if !flight.cancel.is_cancelled() => (flight.clone(), false),
                _ => {
                    let (result, _) = watch::channel(None);
                    let flight = Arc::new(Flight {
                        cancel: self.inner.stop.child_token(),
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
        let outcome = timeout_at(deadline, async {
            loop {
                if let Some(result) = rx.borrow_and_update().clone() {
                    return result.map_err(ConnectivityError::Shared);
                }
                tokio::select! {
                    _ = self.inner.stop.cancelled() => return Err(ConnectivityError::Stopped),
                    result = rx.changed() => { result.map_err(ConnectivityError::ResultClosed)?; }
                }
            }
        })
        .await
        .map_err(ConnectivityError::TimedOut)??;
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
                ConnectionMode::SingleExchange => record.take(),
            }
        };
        match record {
            Some(record) if !record.connection.is_closed() => Ok(ConnectionLease {
                record,
                request: Some(attempt.clone()),
            }),
            Some(_) | None => {
                // 旧端已领取或共同结果已经失活；负载尚未发送，原请求期限内有界取得新连接。
                let record = tokio::select! {
                    _ = self.inner.stop.cancelled() => return Err(ConnectivityError::Stopped),
                    result = timeout_at(deadline, self.acquire(target, peer, attempt.clone())) => result.map_err(ConnectivityError::TimedOut)??,
                };
                Ok(ConnectionLease {
                    record,
                    request: Some(attempt.clone()),
                })
            }
        }
    }
    fn launch(
        &self,
        target: DeviceId,
        peer: Arc<PeerState>,
        flight: Arc<Flight>,
        attempt: Arc<dyn AdmissionAttempt>,
    ) {
        let weak = Arc::downgrade(&self.inner);
        let acquisition = self.acquisition();
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
                    _ = flight.cancel.cancelled() => { worker.abort(); worker.await },
                    result = &mut worker => result,
                };
                let mut result = match joined {
                    Ok(result) => result.map_err(Arc::new),
                    Err(error) if error.is_cancelled() && flight.cancel.is_cancelled() => {
                        Err(Arc::new(ConnectivityError::Stopped))
                    }
                    Err(error) => {
                        let error = Arc::new(error);
                        record_task_join_failure(DiagnosticTaskKind::ClipboardConnectivityWorker);
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
                    if inner.stop.is_cancelled() || flight.cancel.is_cancelled() {
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
                if let (Some(inner), Ok(record)) = (weak.upgrade(), &result) {
                    Self::watch_connection(&inner, record.connection.clone());
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
            backend: self.inner.backend.clone(),
            policy: self.inner.policy,
            inbound: self.inner.inbound.clone(),
            waiting: self.inner.waiting.clone(),
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
    pub async fn accept(
        &self,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<ConnectionLease, InboundConnectionError> {
        let capacity = self
            .inner
            .incoming
            .clone()
            .try_acquire_owned()
            .map_err(|error| InboundConnectionError {
                cause: ConnectivityError::Busy(error),
                _capacity: None,
            })?;
        if self.inner.stop.is_cancelled() {
            return Err(InboundConnectionError {
                cause: ConnectivityError::Stopped,
                _capacity: Some(capacity),
            });
        }
        let grant = match self.inner.admission.adopt_inbound(connection.clone()).await {
            Ok(grant) => grant,
            Err(cause) => {
                return Err(InboundConnectionError {
                    cause,
                    _capacity: Some(capacity),
                })
            }
        };
        let peer = self.peer(grant.peer());
        let record = Arc::new(ConnectionRecord {
            connection,
            exchange_claimed: AtomicBool::new(false),
            grant,
            peer,
            inbound: Arc::new(Semaphore::new(self.inner.policy.inbound_per_connection)),
            inbound_global: self.inner.inbound.clone(),
            waiting_global: self.inner.waiting.clone(),
            _inbound_connection: Some(capacity),
        });
        self.track(&record)
            .map_err(|cause| InboundConnectionError {
                cause,
                _capacity: None,
            })?;
        Ok(ConnectionLease {
            record,
            request: None,
        })
    }
    fn track(&self, record: &Arc<ConnectionRecord>) -> Result<(), ConnectivityError> {
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
        drop(state);
        Self::watch_connection(&self.inner, record.connection.clone());
        Ok(())
    }
    fn watch_connection(inner: &Inner, connection: Arc<dyn PeerConnection>) {
        let mut tasks = inner.tasks.lock().unwrap_or_else(PoisonError::into_inner);
        while let Some(result) = tasks.try_join_next() {
            Self::record_task_result(inner, result);
        }
        if inner.stop.is_cancelled() {
            connection.close();
            return;
        }
        let stop = inner.stop.clone();
        // 只持原生能力，不持 record/租约；最后使用者仍立即关闭，没有空闲池。
        tasks.spawn(
            async move {
                tokio::select! {
                    _ = stop.cancelled() => connection.close(),
                    _ = connection.closed() => {},
                }
            }
            .instrument(Span::none())
            .with_current_subscriber(),
        );
    }
    fn record_task_result(inner: &Inner, result: Result<(), JoinError>) {
        if let Err(error) = result {
            record_task_join_failure(DiagnosticTaskKind::ClipboardConnectivityWorker);
            let mut failure = inner.failure.lock().unwrap_or_else(PoisonError::into_inner);
            if failure.is_none() {
                *failure = Some(Arc::new(error));
            }
        }
    }
    pub async fn shutdown(&self) -> Result<(), ConnectivityError> {
        let _shutdown = self.inner.shutdown.lock().await;
        self.inner.stop.cancel();
        {
            let mut state = self
                .inner
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for record in state.live.iter().filter_map(Weak::upgrade) {
                record.connection.close();
            }
            state.active.clear();
            state.live.clear();
            for flight in state.flights.drain().map(|(_, flight)| flight) {
                flight.cancel.cancel();
            }
        }
        let mut tasks = {
            let mut owned = self
                .inner
                .tasks
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            std::mem::replace(&mut *owned, JoinSet::new())
        };
        while let Some(result) = tasks.join_next().await {
            Self::record_task_result(&self.inner, result);
        }
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
