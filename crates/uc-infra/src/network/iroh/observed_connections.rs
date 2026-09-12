//! 节点拥有的有界连接观察任务；只保留弱连接句柄，不参与连接决策。
use futures_util::StreamExt;
use iroh::endpoint::{AfterHandshakeOutcome, Connection, EndpointHooks, PathEvent, Side};
use iroh::TransportAddr;
use sha2::Digest;
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;
use tokio::task::JoinSet;
use uc_observability_contract::diagnostics::connectivity::{
    ConnectionCloseReason, ConnectionDirection, NetworkPathKind, NetworkRecorder, ObserverFailure,
    PathObservationKind,
};

const MAX_OBSERVERS: usize = 4096;

struct State {
    closed: bool,
    tasks: JoinSet<()>,
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
            })),
            recorder,
        }
    }

    pub(super) async fn shutdown(&self) {
        let mut tasks = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.closed = true;
            std::mem::take(&mut state.tasks)
        };
        if tokio::time::timeout(Duration::from_millis(500), async {
            while tasks.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
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
            while state.tasks.try_join_next().is_some() {}
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
