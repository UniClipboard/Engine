//! 节点拥有的有界连接观察任务；只保留弱连接句柄，不参与连接决策。
use futures_util::StreamExt;
use iroh::endpoint::{AfterHandshakeOutcome, Connection, EndpointHooks, PathEvent, Side};
use iroh::TransportAddr;
use sha2::Digest;
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;
use tokio::task::{JoinError, JoinSet};
use tokio::time::{timeout_at, Instant};
use uc_observability_contract::diagnostics::connectivity::{
    ConnectionCloseReason, ConnectionDirection, NetworkPathKind, NetworkRecorder, ObserverFailure,
    PathObservationKind,
};

const MAX_OBSERVERS: usize = 4096;

struct State {
    closed: bool,
    tasks: JoinSet<()>,
    failures: Vec<JoinError>,
}

impl State {
    fn reap_finished(&mut self) {
        while let Some(result) = self.tasks.try_join_next() {
            if let Err(error) = result {
                // 首次异常后不再登记观察任务，失败记录由节点保留且总量有界。
                self.closed = true;
                self.failures.push(error);
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct ObservedConnections {
    state: Arc<Mutex<State>>,
    recorder: NetworkRecorder,
}

impl std::fmt::Debug for ObservedConnections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObservedConnections")
    }
}

impl ObservedConnections {
    pub(super) fn new(recorder: NetworkRecorder) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                closed: false,
                tasks: JoinSet::new(),
                failures: Vec::new(),
            })),
            recorder,
        }
    }

    pub(super) async fn shutdown(&self, deadline: Option<Instant>) -> Vec<JoinError> {
        let deadline = deadline.unwrap_or_else(|| Instant::now() + Duration::from_millis(500));
        let (mut tasks, mut failures) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.closed = true;
            (
                std::mem::take(&mut state.tasks),
                std::mem::take(&mut state.failures),
            )
        };
        if timeout_at(deadline, async {
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result {
                    failures.push(error);
                }
            }
        })
        .await
        .is_err()
        {
            tasks.abort_all();
            while let Some(result) = tasks.join_next().await {
                if let Err(error) = result {
                    failures.push(error);
                }
            }
        }
        failures
    }
}

impl EndpointHooks for ObservedConnections {
    fn after_handshake<'a>(
        &'a self,
        conn: &'a Connection,
    ) -> impl std::future::Future<Output = AfterHandshakeOutcome> + Send + 'a {
        async move {
            if !self.recorder.is_enabled() {
                return AfterHandshakeOutcome::accept();
            }
            let peer = *conn.remote_id().as_bytes();
            let mut state = match self.state.try_lock() {
                Ok(state) => state,
                Err(TryLockError::Poisoned(error)) => error.into_inner(),
                Err(TryLockError::WouldBlock) => {
                    self.recorder
                        .connection_observer_unavailable(peer, ObserverFailure::Busy);
                    return AfterHandshakeOutcome::accept();
                }
            };
            state.reap_finished();
            if state.closed || state.tasks.len() >= MAX_OBSERVERS {
                self.recorder.connection_observer_unavailable(
                    peer,
                    if state.closed {
                        ObserverFailure::Stopped
                    } else {
                        ObserverFailure::CapacityExceeded
                    },
                );
                return AfterHandshakeOutcome::accept();
            }
            let direction = if conn.side() == Side::Client {
                ConnectionDirection::Outbound
            } else {
                ConnectionDirection::Inbound
            };
            let paths = conn.paths();
            let selected = paths.iter().find(|path| path.is_selected());
            let initial = selected.as_ref().map_or(NetworkPathKind::Unknown, |path| {
                path_kind(path.remote_addr())
            });
            let observation = self.recorder.connection_established(
                peer,
                conn.stable_id() as u64,
                direction,
                initial,
            );
            if let Some(path) = selected {
                observation.path(
                    path_key(path.id()),
                    PathObservationKind::Snapshot,
                    path_kind(path.remote_addr()),
                );
            }
            // 在 hook 返回前登记关闭通知，确保主调用方立即 drop 也不会丢失原因。
            let closed = conn.weak_handle().closed();
            let mut events = conn.path_events();
            state.tasks.spawn(async move {
                tokio::pin!(closed);
                let mut events_open = true;
                loop {
                    tokio::select! {
                        result = &mut closed => {
                            if let Some(closed) = result { observation.closed(close_reason(&closed.reason)); }
                            break;
                        }
                        event = events.next(), if events_open => match event {
                            Some(PathEvent::Opened { id, remote_addr, .. }) => observation.path(path_key(id), PathObservationKind::Opened, path_kind(&remote_addr)),
                            Some(PathEvent::Selected { id, remote_addr, .. }) => observation.path(path_key(id), PathObservationKind::Selected, path_kind(&remote_addr)),
                            Some(PathEvent::Closed { id, remote_addr, .. }) => observation.path(path_key(id), PathObservationKind::Closed, path_kind(&remote_addr)),
                            Some(PathEvent::Lagged { missed, .. }) => observation.lagged(missed),
                            Some(_) => {},
                            None => events_open = false,
                        }
                    }
                }
            });
            AfterHandshakeOutcome::accept()
        }
    }
}

fn path_key(id: iroh::endpoint::PathId) -> [u8; 32] {
    // PathId 只公开 Display/Hash；签名仅用作内存 key，运行时输出另分配的随机编号。
    sha2::Sha256::digest(id.to_string().as_bytes()).into()
}

fn path_kind(addr: &TransportAddr) -> NetworkPathKind {
    match addr {
        TransportAddr::Ip(_) => NetworkPathKind::Direct,
        TransportAddr::Relay(_) => NetworkPathKind::Relay,
        _ => NetworkPathKind::Other,
    }
}

fn close_reason(reason: &iroh::endpoint::ConnectionError) -> ConnectionCloseReason {
    use iroh::endpoint::ConnectionError;
    match reason {
        ConnectionError::ApplicationClosed(_) => ConnectionCloseReason::RemoteApplicationClosed,
        ConnectionError::ConnectionClosed(_) => ConnectionCloseReason::RemoteTransportClosed,
        ConnectionError::LocallyClosed => ConnectionCloseReason::LocalClosed,
        ConnectionError::TimedOut => ConnectionCloseReason::TimedOut,
        ConnectionError::Reset => ConnectionCloseReason::RemoteReset,
        ConnectionError::VersionMismatch => ConnectionCloseReason::VersionMismatch,
        _ => ConnectionCloseReason::TransportFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::{Arc, Duration, Instant, NetworkRecorder, ObservedConnections};

    #[tokio::test(start_paused = true)]
    async fn expired_deadline_does_not_start_another_observer_grace_period() {
        let observers = ObservedConnections::new(NetworkRecorder::default());
        observers
            .state
            .lock()
            .unwrap()
            .tasks
            .spawn(std::future::pending());
        let started = Instant::now();
        let deadline = started + Duration::from_millis(10);
        tokio::time::advance(Duration::from_millis(20)).await;
        let failures = observers.shutdown(Some(deadline)).await;
        assert_eq!(failures.len(), 1);
        assert!(failures[0].is_cancelled());
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn reaped_observer_failure_remains_visible_at_shutdown() {
        let observers = ObservedConnections::new(NetworkRecorder::default());
        let (started, ready) = tokio::sync::oneshot::channel();
        observers.state.lock().unwrap().tasks.spawn(async move {
            let _ = started.send(());
            panic!("PRIVATE_OBSERVER_FAILURE");
        });
        ready.await.unwrap();
        tokio::task::yield_now().await;
        {
            let mut state = observers.state.lock().unwrap();
            state.reap_finished();
            assert!(state.closed);
            assert_eq!(state.tasks.len(), 0);
        }
        let failures = observers.shutdown(None).await;
        assert_eq!(failures.len(), 1);
        assert!(failures[0].is_panic());
    }

    #[tokio::test(start_paused = true)]
    async fn forced_observer_exit_is_reported_after_resources_are_released() {
        let observers = ObservedConnections::new(NetworkRecorder::default());
        let resource = Arc::new(());
        let held = Arc::clone(&resource);
        observers.state.lock().unwrap().tasks.spawn(async move {
            let _held = held;
            std::future::pending::<()>().await;
        });
        let failures = observers.shutdown(None).await;
        assert_eq!(failures.len(), 1);
        assert!(failures[0].is_cancelled());
        assert_eq!(Arc::strong_count(&resource), 1);
    }
}
