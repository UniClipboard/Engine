//! 字段只表达线格式，真实身份与当前任务资格由完整 Application 负责人核验。
use crate::WireTraceContext;
use std::fmt;
use thiserror::Error;
use uc_core::error_class::ErrorClass;

/// 线上观测载体由 Infra 注入，业务消息不依赖上下文或观测运行时。
#[derive(Clone, PartialEq, Eq)]
pub struct Envelope {
    pub message: Message,
    pub trace_context: Option<WireTraceContext>,
}

impl fmt::Debug for Envelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Envelope")
            .field("message", &self.message)
            .field("has_trace_context", &self.trace_context.is_some())
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct WireId(pub [u8; 16]);

impl fmt::Debug for WireId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WireId(<opaque>)")
    }
}

#[derive(Debug, Error)]
#[error("unknown transfer progress {kind}")]
pub struct WireValueError {
    kind: &'static str,
}

impl ErrorClass for WireValueError {
    fn class(&self) -> &'static str {
        "transfer_progress_unknown_value"
    }
}

// 数值与变体同源；解码不能另维护一份数值清单。
macro_rules! byte_enum {
    ($visibility:vis enum $name:ident { $($variant:ident = $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        $visibility enum $name { $($variant = $value),+ }

        impl TryFrom<u8> for $name {
            type Error = WireValueError;
            fn try_from(value: u8) -> Result<Self, Self::Error> {
                match value {
                    $(code if code == Self::$variant as u8 => Ok(Self::$variant)),+,
                    _ => Err(WireValueError { kind: stringify!($name) }),
                }
            }
        }
    };
}

byte_enum! {
    pub enum ItemKind { File = 1, Representation = 2, DirectoryUnit = 3 }
}
byte_enum! {
    pub enum Phase { Waiting = 1, Transferring = 2, Processing = 3, Retrying = 4 }
}
byte_enum! {
    pub enum FailureReason {
        NetworkUnavailable = 1, TimedOut = 2, AccessDenied = 3, StorageUnavailable = 4,
        IntegrityCheckFailed = 5, Interrupted = 6, Unknown = 7
    }
}
byte_enum! {
    pub enum CancelReason {
        LocalUser = 1, RemotePeer = 2, Replaced = 3, Timeout = 4,
        ConnectivityRecovery = 5, Unknown = 6
    }
}
byte_enum! {
    pub enum RejectionReason {
        AccessDenied = 1, Stale = 2, Busy = 3, UnknownTask = 4, InvalidScope = 5
    }
}
byte_enum! {
    pub(super) enum MessageKind {
        Register = 1, Challenge = 2, CurrentDescriptor = 3, Bound = 4, Update = 5,
        Contact = 6, Terminal = 7, TerminalAck = 8, Query = 9, QueryResult = 10,
        Rejected = 11
    }
}
byte_enum! {
    pub(super) enum QueryKind {
        Current = 1, Superseded = 2, NotRetained = 3, Unsupported = 4, AccessDenied = 5
    }
}
byte_enum! {
    pub(super) enum OutcomeKind { Completed = 1, Failed = 2, Cancelled = 3 }
}
byte_enum! {
    pub(super) enum ItemStateKind { Active = 1, Terminal = 2 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskScope {
    pub source_entry: WireId,
    pub attempt: WireId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProducerDescriptor {
    pub producer: WireId,
    pub owner_revision: u64,
    pub scope: TaskScope,
    /// 逻辑结果项数；目录为一个完整单元，不是目录成员数。
    pub item_count: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LogicalItem {
    pub kind: ItemKind,
    pub index: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveProgress {
    pub item: LogicalItem,
    pub round: u32,
    pub sequence: u64,
    pub bytes: u64,
    pub total: Option<u64>,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalOutcome {
    Completed,
    Failed(FailureReason),
    Cancelled(CancelReason),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalResult {
    pub item: LogicalItem,
    pub round: u32,
    pub sequence: u64,
    pub result_revision: u64,
    pub bytes: u64,
    pub total: Option<u64>,
    pub outcome: TerminalOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemState {
    Active(ActiveProgress),
    Terminal(TerminalResult),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotPage {
    pub snapshot_revision: u64,
    pub offset: u16,
    pub total: u16,
    pub items: Vec<ItemState>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueryResult {
    Current {
        descriptor: ProducerDescriptor,
        page: SnapshotPage,
    },
    Superseded {
        descriptor: ProducerDescriptor,
        page: SnapshotPage,
    },
    NotRetained,
    Unsupported,
    AccessDenied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Register {
        source_entry: WireId,
    },
    Challenge {
        nonce: WireId,
    },
    CurrentDescriptor {
        challenge: WireId,
        descriptor: ProducerDescriptor,
    },
    Bound {
        challenge: WireId,
        binding: WireId,
        descriptor: ProducerDescriptor,
    },
    Update {
        binding: WireId,
        scope: TaskScope,
        owner_revision: u64,
        progress: ActiveProgress,
    },
    Contact {
        binding: WireId,
        scope: TaskScope,
        owner_revision: u64,
        sequence: u64,
    },
    Terminal {
        binding: WireId,
        scope: TaskScope,
        owner_revision: u64,
        result: TerminalResult,
    },
    TerminalAck {
        binding: WireId,
        scope: TaskScope,
        owner_revision: u64,
        result: TerminalResult,
    },
    Query {
        challenge: WireId,
        source_entry: WireId,
        expected_attempt: Option<WireId>,
        offset: u16,
        snapshot_revision: Option<u64>,
    },
    QueryResult {
        challenge: WireId,
        result: QueryResult,
    },
    Rejected {
        challenge: WireId,
        reason: RejectionReason,
    },
}
