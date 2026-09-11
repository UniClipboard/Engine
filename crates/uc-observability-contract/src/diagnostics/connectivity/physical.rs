//! 已建立连接的局部观察；底层连接/路径标识只留在内存上下文。
use super::{
    connection::peer_context, emit_local, millis, record::LocalEvent, ConnectionCloseReason,
    ConnectionDirection, NetworkRecorder, ObservationContext,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPathKind {
    Direct,
    Relay,
    Other,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathObservationKind {
    Snapshot,
    Opened,
    Selected,
    Closed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObserverFailure {
    CapacityExceeded,
    Busy,
    Stopped,
    TaskFailed,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum PhysicalEvent {
    Established {
        direction: ConnectionDirection,
        initial_path: NetworkPathKind,
    },
    Closed {
        direction: ConnectionDirection,
        reason: ConnectionCloseReason,
        duration_ms: u64,
    },
    Path {
        observation: PathObservationKind,
        path_kind: NetworkPathKind,
    },
    Lagged {
        missed: u64,
    },
    Unavailable {
        reason: ObserverFailure,
    },
    Stopped {
        reason: ObserverFailure,
    },
}

impl PhysicalEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Established { .. } => "connection.established",
            Self::Closed { .. } => "connection.closed",
            Self::Path { .. } => "connection.path.observed",
            Self::Lagged { .. } => "connection.path.lagged",
            Self::Unavailable { .. } => "connection.observer.unavailable",
            Self::Stopped { .. } => "connection.observer.stopped",
        }
    }
    pub(super) fn level(&self) -> &'static str {
        if matches!(
            self,
            Self::Lagged { .. } | Self::Unavailable { .. } | Self::Stopped { .. }
        ) {
            "WARN"
        } else {
            "INFO"
        }
    }
    pub(super) fn fields(&self) -> Map<String, Value> {
        let Value::Object(mut fields) = json!(self) else {
            return Map::new();
        };
        fields.remove("event");
        if matches!(self, Self::Closed { .. }) {
            if let Some(reason) = fields.remove("reason") {
                fields.insert("close.reason".into(), reason);
            }
        }
        fields
    }
}

#[derive(Clone)]
struct ConnectionKey(u64);
#[derive(Clone)]
struct PathKey([u8; 32]);

#[doc(hidden)]
pub fn local_connection_key() -> Option<u64> {
    opentelemetry::Context::current()
        .get::<ConnectionKey>()
        .map(|key| key.0)
}
#[doc(hidden)]
pub fn local_path_key() -> Option<[u8; 32]> {
    opentelemetry::Context::current()
        .get::<PathKey>()
        .map(|key| key.0)
}

pub(super) fn with_connection_key(context: &ObservationContext, key: u64) -> ObservationContext {
    ObservationContext(context.0.with_value(ConnectionKey(key)))
}

pub struct ConnectionLifetimeObservation {
    recorder: NetworkRecorder,
    context: ObservationContext,
    direction: ConnectionDirection,
    started: Instant,
    finished: bool,
}

impl NetworkRecorder {
    pub fn in_connection_scope<T>(
        &self,
        peer: [u8; 32],
        key: u64,
        operation: impl FnOnce() -> T,
    ) -> T {
        let context = with_connection_key(&peer_context(peer), key);
        let _guard = context.0.attach();
        operation()
    }
    pub fn connection_established(
        &self,
        peer: [u8; 32],
        key: u64,
        direction: ConnectionDirection,
        initial_path: NetworkPathKind,
    ) -> ConnectionLifetimeObservation {
        let observation = ConnectionLifetimeObservation {
            recorder: self.clone(),
            context: with_connection_key(&peer_context(peer), key),
            direction,
            started: Instant::now(),
            finished: false,
        };
        observation.emit(
            PhysicalEvent::Established {
                direction,
                initial_path,
            },
            None,
        );
        observation
    }
    pub fn connection_observer_unavailable(&self, peer: [u8; 32], reason: ObserverFailure) {
        let context = peer_context(peer);
        tracing::dispatcher::with_default(&self.dispatcher, || {
            emit_local(
                LocalEvent::Physical {
                    record: PhysicalEvent::Unavailable { reason },
                },
                &context,
            )
        });
    }
}

impl ConnectionLifetimeObservation {
    pub fn path(
        &self,
        key: [u8; 32],
        observation: PathObservationKind,
        path_kind: NetworkPathKind,
    ) {
        self.emit(
            PhysicalEvent::Path {
                observation,
                path_kind,
            },
            Some(key),
        );
    }
    pub fn lagged(&self, missed: u64) {
        self.emit(PhysicalEvent::Lagged { missed }, None);
    }
    pub fn closed(mut self, reason: ConnectionCloseReason) {
        self.finished = true;
        self.emit(
            PhysicalEvent::Closed {
                direction: self.direction,
                reason,
                duration_ms: millis(self.started.elapsed()),
            },
            None,
        );
    }
    fn emit(&self, record: PhysicalEvent, path: Option<[u8; 32]>) {
        let mut context = self.context.clone();
        if let Some(path) = path {
            context.0 = context.0.with_value(PathKey(path));
        }
        tracing::dispatcher::with_default(&self.recorder.dispatcher, || {
            emit_local(LocalEvent::Physical { record }, &context)
        });
    }
}

impl Drop for ConnectionLifetimeObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.emit(
                PhysicalEvent::Stopped {
                    reason: if std::thread::panicking() {
                        ObserverFailure::TaskFailed
                    } else {
                        ObserverFailure::Stopped
                    },
                },
                None,
            );
        }
    }
}
