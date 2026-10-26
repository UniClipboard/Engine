use super::super::{ProgressExchangeDeadline, ProgressProjectionRegistry, ProgressReceiveError};
use super::exchange::{record_exchange, settle};
use super::ProgressInboundWirePort;
use crate::connectivity::{
    ConnectivityError, ConnectivityManager, ElapsedClockPort, PeerConnection,
};
use futures::future::{join_all, BoxFuture, Shared};
use futures::FutureExt;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::{oneshot, Semaphore};
use tokio::task::JoinError;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    EXCHANGE_GLOBAL_ACTIVE, PROGRESS_RETAINED_CONNECTIONS, TERMINAL_ROUND_LIMIT,
};
use uc_observability_contract::diagnostics::{
    record_task_join_failure, DiagnosticTaskKind, ObservationContext,
};

type Task = Shared<BoxFuture<'static, Result<(), Arc<JoinError>>>>;
type Shutdown = Shared<BoxFuture<'static, Result<(), Arc<ConnectivityError>>>>;

struct State {
    closed: bool,
    tasks: Vec<Task>,
    shutdown: Option<Shutdown>,
    failure: Option<Arc<JoinError>>,
}
/// Router只交接完整连接；准入、交换寿命、保留及实际本地工作由本负责人统一结算。
pub struct TransferProgressTransport {
    pub(super) manager: ConnectivityManager,
    pub(super) wire: Arc<dyn ProgressInboundWirePort>,
    pub(super) clock: Arc<dyn ElapsedClockPort>,
    namespace: ProgressProjectionRegistry,
    stop: CancellationToken,
    owners: Arc<Semaphore>,
    state: Mutex<State>,
}
pub(super) struct CloseConnection(pub Arc<dyn PeerConnection>);
impl Drop for CloseConnection {
    fn drop(&mut self) {
        self.0.close();
    }
}
struct StopWaiting(CancellationToken);
impl Drop for StopWaiting {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl TransferProgressTransport {
    pub(crate) fn new(
        manager: ConnectivityManager,
        wire: Arc<dyn ProgressInboundWirePort>,
        clock: Arc<dyn ElapsedClockPort>,
        namespace: ProgressProjectionRegistry,
    ) -> Arc<Self> {
        Arc::new(Self {
            manager,
            wire,
            clock,
            namespace,
            stop: CancellationToken::new(),
            // 仅覆盖原16保留连接与4个在途入口，不新增独立业务队列。
            owners: Arc::new(Semaphore::new(
                PROGRESS_RETAINED_CONNECTIONS + EXCHANGE_GLOBAL_ACTIVE,
            )),
            state: Mutex::new(State {
                closed: false,
                tasks: Vec::new(),
                shutdown: None,
                failure: None,
            }),
        })
    }
    pub async fn accept(
        self: &Arc<Self>,
        connection: Arc<dyn PeerConnection>,
    ) -> Result<(), ProgressReceiveError> {
        let connection = CloseConnection(connection);
        let deadline = ProgressExchangeDeadline::new(
            self.clock.clone(),
            self.clock.elapsed()?.saturating_add(TERMINAL_ROUND_LIMIT),
        )?;
        let cancel = self.stop.child_token();
        let _waiting = StopWaiting(cancel.clone());
        let receive = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            for task in std::mem::take(&mut state.tasks) {
                match task.clone().now_or_never() {
                    Some(result) => self.record_task(&mut state, result),
                    None => state.tasks.push(task),
                }
            }
            if state.closed || self.stop.is_cancelled() {
                return Err(ConnectivityError::Stopped.into());
            }
            let capacity = self
                .owners
                .clone()
                .try_acquire_owned()
                .map_err(ConnectivityError::Busy)?;
            let generation = self.namespace.connection_generation()?;
            // 在第一次异步读取之前固定原会话围栏及未知身份许可。
            let pending = self.manager.accept(connection.0.clone());
            let owner = Arc::clone(self);
            let observation = ObservationContext::capture();
            let (send, receive) = oneshot::channel();
            let task = tokio::spawn(observation.scope(async move {
                let _capacity = capacity;
                let adopted = settle(
                    async { Ok(pending.await.map_err(ConnectivityError::from)?) },
                    &deadline,
                    &cancel,
                    || connection.0.close(),
                )
                .await;
                let result = match adopted {
                    Ok(lease) => {
                        owner
                            .receive_connection(lease, generation, deadline, &cancel)
                            .await
                    }
                    Err(source) => Err(source),
                };
                // 连接退出时仍由同一任务持有owner，调用方丢弃等待不会abort实际本地采用。
                drop(connection);
                if let Err(result) = send.send(result) {
                    record_exchange(result);
                }
            }));
            state
                .tasks
                .push(async move { task.await.map_err(Arc::new) }.boxed().shared());
            receive
        };
        receive
            .await
            .map_err(ConnectivityError::InboundResultClosed)?
    }
    pub(super) fn exchange_task(
        &self,
        result: Result<Result<(), ProgressReceiveError>, JoinError>,
    ) -> bool {
        match result {
            Ok(result) => {
                record_exchange(result);
                false
            }
            Err(source) => {
                let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
                self.record_task(&mut state, Err(Arc::new(source)));
                true
            }
        }
    }
    fn record_task(&self, state: &mut State, result: Result<(), Arc<JoinError>>) {
        if let Err(source) = result {
            state.failure.get_or_insert(source);
            record_task_join_failure(DiagnosticTaskKind::TransferProgressConnectivityWorker);
        }
    }
    pub async fn shutdown(self: &Arc<Self>) -> Result<(), ProgressReceiveError> {
        let completion = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(completion) = &state.shutdown {
                completion.clone()
            } else {
                state.closed = true;
                self.stop.cancel();
                let tasks = state.tasks.clone();
                let owner = Arc::clone(self);
                // 关闭本身也有实际负责人；等待future被丢弃不能abort准入或业务采用。
                let task = tokio::spawn(async move {
                    let (manager, joined) = tokio::join!(owner.manager.shutdown(), join_all(tasks));
                    let failure = {
                        let mut state = owner.state.lock().unwrap_or_else(PoisonError::into_inner);
                        for result in joined {
                            owner.record_task(&mut state, result);
                        }
                        state.tasks.clear();
                        state.failure.clone()
                    };
                    match (failure, manager) {
                        (Some(source), Err(additional)) => Err(ConnectivityError::Settlement {
                            primary: Box::new(ConnectivityError::Task(source)),
                            additional: Box::new(additional),
                        }),
                        (Some(source), Ok(())) => Err(ConnectivityError::Task(source)),
                        (None, result) => result,
                    }
                });
                let completion = async move {
                    match task.await {
                        Ok(result) => result.map_err(Arc::new),
                        Err(source) => Err(Arc::new(ConnectivityError::Task(Arc::new(source)))),
                    }
                }
                .boxed()
                .shared();
                state.shutdown = Some(completion.clone());
                completion
            }
        };
        completion
            .await
            .map_err(|source| ProgressReceiveError::Connection(ConnectivityError::Shared(source)))
    }
}
