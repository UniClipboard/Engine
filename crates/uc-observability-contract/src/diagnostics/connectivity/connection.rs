//! 一次真实建链的本地诊断；不创建业务 span，不保存可识别的对端信息。
use super::{emit_local, millis, record::LocalEvent, ObservationContext};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};
use std::time::Duration;
use std::time::Instant;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionPurpose {
    Admission,
    Presence,
    NetworkRecovery,
    Clipboard,
    ActiveClipboard,
    ClipboardPull,
    TransferProgress,
    GroupUpdate,
    MembershipAttestation,
    MembershipGossip,
    MembershipHistory,
    MembershipRecovery,
    Other,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionFailurePhase {
    AddressLookup,
    Establish,
    Handshake,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionFailureReason {
    NoAddress,
    EndpointClosed,
    LocallyClosed,
    SelfConnect,
    LocallyRejected,
    AuthenticationRejected,
    TimedOut,
    Transport,
    Internal,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ConnectionOutcome {
    Connected,
    Failed {
        phase: ConnectionFailurePhase,
        reason: ConnectionFailureReason,
    },
    Interrupted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum AttemptOutcome {
    Result(ConnectionOutcome),
    CancelledByWinner,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ConnectionEvent {
    Started {
        connect_id: Uuid,
        purpose: ConnectionPurpose,
    },
    Finished {
        connect_id: Uuid,
        outcome: ConnectionOutcome,
        duration_ms: u64,
        attempt_count: u32,
    },
    AttemptStarted {
        connect_id: Uuid,
        attempt_index: u32,
        timeout_ms: u64,
    },
    AttemptFinished {
        connect_id: Uuid,
        attempt_index: u32,
        outcome: AttemptOutcome,
        duration_ms: u64,
    },
}

impl ConnectionEvent {
    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Started { .. } => "connection.started",
            Self::Finished { .. } => "connection.finished",
            Self::AttemptStarted { .. } => "connection.attempt.started",
            Self::AttemptFinished { .. } => "connection.attempt.finished",
        }
    }

    pub(super) fn level(&self) -> &'static str {
        match self {
            Self::Finished {
                outcome: ConnectionOutcome::Failed { .. } | ConnectionOutcome::Interrupted,
                ..
            }
            | Self::AttemptFinished {
                outcome:
                    AttemptOutcome::Result(
                        ConnectionOutcome::Failed { .. } | ConnectionOutcome::Interrupted,
                    ),
                ..
            } => "WARN",
            _ => "INFO",
        }
    }

    pub(super) fn fields(&self) -> Map<String, Value> {
        let mut fields = Map::new();
        match self {
            Self::Started {
                connect_id,
                purpose,
            } => {
                fields.insert("connect_id".into(), json!(connect_id));
                fields.insert("purpose".into(), json!(purpose));
            }
            Self::Finished {
                connect_id,
                outcome,
                duration_ms,
                attempt_count,
            } => {
                fields.insert("connect_id".into(), json!(connect_id));
                fields.insert("duration_ms".into(), json!(duration_ms));
                fields.insert("attempt_count".into(), json!(attempt_count));
                append_outcome(&mut fields, *outcome);
            }
            Self::AttemptStarted {
                connect_id,
                attempt_index,
                timeout_ms,
            } => {
                fields.insert("connect_id".into(), json!(connect_id));
                fields.insert("attempt_index".into(), json!(attempt_index));
                fields.insert("timeout_ms".into(), json!(timeout_ms));
            }
            Self::AttemptFinished {
                connect_id,
                attempt_index,
                outcome,
                duration_ms,
            } => {
                fields.insert("connect_id".into(), json!(connect_id));
                fields.insert("attempt_index".into(), json!(attempt_index));
                fields.insert("duration_ms".into(), json!(duration_ms));
                match outcome {
                    AttemptOutcome::Result(outcome) => append_outcome(&mut fields, *outcome),
                    AttemptOutcome::CancelledByWinner => {
                        fields.insert("outcome".into(), json!("cancelled_by_winner"));
                    }
                };
            }
        }
        fields
    }
}

fn append_outcome(fields: &mut Map<String, Value>, outcome: ConnectionOutcome) {
    match outcome {
        ConnectionOutcome::Connected => {
            fields.insert("outcome".into(), json!("connected"));
        }
        ConnectionOutcome::Interrupted => {
            fields.insert("outcome".into(), json!("interrupted"));
        }
        ConnectionOutcome::Failed { phase, reason } => {
            fields.insert("outcome".into(), json!("failed"));
            fields.insert("error.phase".into(), json!(phase));
            fields.insert("error.reason".into(), json!(reason));
        }
    }
}

// 原始材料只在同步发射期间附着于内存上下文，不进入 tracing attributes 或 wire。
#[derive(Clone)]
struct LocalConnectionPeer([u8; 32]);

pub(super) fn peer_context(peer: [u8; 32]) -> ObservationContext {
    super::address_record::carry_record_keys(ObservationContext(
        ObservationContext::capture()
            .0
            .with_value(LocalConnectionPeer(peer)),
    ))
}

/// 仅供本地处理器在同步编码时分配匿名编号；禁止把返回值写入任何输出。
#[doc(hidden)]
pub fn local_connection_peer() -> Option<[u8; 32]> {
    opentelemetry::Context::current()
        .get::<LocalConnectionPeer>()
        .map(|peer| peer.0)
}

struct ConnectionShared {
    connect_id: Uuid,
    context: ObservationContext,
    attempt_count: AtomicU32,
    won: AtomicBool,
    dispatcher: tracing::Dispatch,
}

impl ConnectionShared {
    fn emit(&self, record: ConnectionEvent) {
        // Future 的取消可能发生在原 subscriber 作用域之外；只保留 dispatcher，
        // 不持有业务 span，使取消终态仍回到开始时的本地输出。
        tracing::dispatcher::with_default(&self.dispatcher, || {
            emit_local(LocalEvent::Connection { record }, &self.context);
        });
    }

    fn emit_connected(&self, record: ConnectionEvent, key: u64) {
        let context = super::physical::with_connection_key(&self.context, key);
        tracing::dispatcher::with_default(&self.dispatcher, || {
            emit_local(LocalEvent::Connection { record }, &context);
        });
    }
}

pub struct ConnectionObservation {
    shared: Arc<ConnectionShared>,
    started: Instant,
    finished: bool,
}

impl ConnectionObservation {
    pub fn begin(purpose: ConnectionPurpose, peer: [u8; 32]) -> Self {
        let context = peer_context(peer);
        let observation = Self {
            shared: Arc::new(ConnectionShared {
                connect_id: Uuid::new_v4(),
                context,
                attempt_count: AtomicU32::new(0),
                won: AtomicBool::new(false),
                dispatcher: tracing::dispatcher::get_default(Clone::clone),
            }),
            started: Instant::now(),
            finished: false,
        };
        observation.shared.emit(ConnectionEvent::Started {
            connect_id: observation.shared.connect_id,
            purpose,
        });
        observation
    }

    pub fn finish(mut self, outcome: ConnectionOutcome) {
        self.finished = true;
        if matches!(outcome, ConnectionOutcome::Connected) {
            self.shared.won.store(true, Ordering::Release);
        }
        self.emit_finish(outcome);
    }

    pub fn connected(mut self, key: u64) {
        self.finished = true;
        self.shared.won.store(true, Ordering::Release);
        self.shared.emit_connected(
            ConnectionEvent::Finished {
                connect_id: self.shared.connect_id,
                outcome: ConnectionOutcome::Connected,
                duration_ms: millis(self.started.elapsed()),
                attempt_count: self.shared.attempt_count.load(Ordering::Acquire),
            },
            key,
        );
    }

    pub fn attempts(&self) -> ConnectionAttempts {
        ConnectionAttempts(Arc::clone(&self.shared))
    }

    pub fn input_candidates(
        &self,
        fingerprint: Option<[u8; 32]>,
        source: super::AddressInputSource,
        summary: super::CandidateSummary,
    ) {
        super::address::emit_used(
            &self.shared.dispatcher,
            &self.shared.context,
            self.shared.connect_id,
            fingerprint,
            source,
            summary,
        );
    }

    fn emit_finish(&self, outcome: ConnectionOutcome) {
        self.shared.emit(ConnectionEvent::Finished {
            connect_id: self.shared.connect_id,
            outcome,
            duration_ms: millis(self.started.elapsed()),
            attempt_count: self.shared.attempt_count.load(Ordering::Acquire),
        });
    }
}

#[derive(Clone)]
pub struct ConnectionAttempts(Arc<ConnectionShared>);

impl ConnectionAttempts {
    /// 必须在实际开始时调用，尚在错峰等待中的任务不分配尝试记录。
    pub fn begin(&self, index: u32, timeout: Duration) -> ConnectionAttemptObservation {
        let _ = self
            .0
            .attempt_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_add(1))
            });
        self.0.emit(ConnectionEvent::AttemptStarted {
            connect_id: self.0.connect_id,
            attempt_index: index,
            timeout_ms: millis(timeout),
        });
        ConnectionAttemptObservation {
            shared: Arc::clone(&self.0),
            index,
            started: Instant::now(),
            finished: false,
        }
    }
}

pub struct ConnectionAttemptObservation {
    shared: Arc<ConnectionShared>,
    index: u32,
    started: Instant,
    finished: bool,
}

impl ConnectionAttemptObservation {
    pub fn connected(mut self, key: u64) {
        self.finished = true;
        self.shared.emit_connected(
            ConnectionEvent::AttemptFinished {
                connect_id: self.shared.connect_id,
                attempt_index: self.index,
                outcome: AttemptOutcome::Result(ConnectionOutcome::Connected),
                duration_ms: millis(self.started.elapsed()),
            },
            key,
        );
    }
    pub fn finish(mut self, outcome: ConnectionOutcome) {
        self.finished = true;
        self.emit_finish(AttemptOutcome::Result(outcome));
    }

    fn emit_finish(&self, outcome: AttemptOutcome) {
        self.shared.emit(ConnectionEvent::AttemptFinished {
            connect_id: self.shared.connect_id,
            attempt_index: self.index,
            outcome,
            duration_ms: millis(self.started.elapsed()),
        });
    }
}

impl Drop for ConnectionAttemptObservation {
    fn drop(&mut self) {
        if !self.finished {
            let outcome = if self.shared.won.load(Ordering::Acquire) {
                AttemptOutcome::CancelledByWinner
            } else {
                AttemptOutcome::Result(ConnectionOutcome::Interrupted)
            };
            self.emit_finish(outcome);
        }
    }
}

impl Drop for ConnectionObservation {
    fn drop(&mut self) {
        if !self.finished {
            self.emit_finish(ConnectionOutcome::Interrupted);
        }
    }
}
