use super::super::{
    ElapsedClockError, ElapsedClockRefusal, ProgressReceiveError, ProgressSnapshotRead,
    ProgressSourceTask, ProgressTaskDescriptor,
};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uc_core::file_transfer::{
    ProgressIdentity, ProgressItem, ProgressTerminal, TerminalRetransmit,
};
use uc_core::ids::DeviceId;
/// 多个同时核对者共享同一不可变结果及完整错误来源，不复制整份快照。
pub type ProgressQueryResult = Arc<Result<ProgressSnapshotRead, ProgressReceiveError>>;
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Key {
    pub peer: DeviceId,
    pub source: ProgressIdentity,
    pub attempt: ProgressIdentity,
    pub item: ProgressItem,
    pub producer: ProgressIdentity,
    pub revision: u64,
}
impl Key {
    pub fn new(peer: DeviceId, task: ProgressTaskDescriptor, result: ProgressTerminal) -> Self {
        Self {
            peer,
            source: task.source,
            attempt: task.attempt,
            item: result.item,
            producer: task.producer,
            revision: task.revision,
        }
    }
}
pub(super) struct Entry {
    pub descriptor: ProgressTaskDescriptor,
    pub result: ProgressTerminal,
    pub generated: Duration,
    pub epoch: u64,
    pub cancel: CancellationToken,
    pub retry: TerminalRetransmit,
    pub queued: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct QueryKey {
    pub peer: DeviceId,
    source: ProgressIdentity,
    attempt: ProgressIdentity,
    producer: ProgressIdentity,
    revision: u64,
    count: u16,
}
impl QueryKey {
    pub fn new(peer: DeviceId, task: ProgressTaskDescriptor) -> Self {
        Self {
            peer,
            source: task.source,
            attempt: task.attempt,
            producer: task.producer,
            revision: task.revision,
            count: task.item_count,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Work {
    Terminal(Key),
    Snapshot(QueryKey),
}
impl Work {
    pub fn peer(self) -> DeviceId {
        match self {
            Self::Terminal(key) => key.peer,
            Self::Snapshot(key) => key.peer,
        }
    }
}
pub(super) enum Action {
    Terminal {
        descriptor: ProgressTaskDescriptor,
        result: ProgressTerminal,
    },
    Snapshot(ProgressSourceTask),
}
pub(super) struct Job {
    pub work: Work,
    pub epoch: u64,
    pub deadline: Duration,
    pub cancel: CancellationToken,
    pub action: Action,
}
pub(super) enum Completion {
    Terminal(Result<(), ProgressReceiveError>),
    Snapshot(Result<ProgressSnapshotRead, ProgressReceiveError>),
}
pub(super) fn clock_failure(source: Option<Arc<ElapsedClockError>>) -> ProgressQueryResult {
    Arc::new(Err(ProgressReceiveError::Clock(ElapsedClockError {
        source: match source {
            Some(source) => Box::new(source),
            None => Box::new(ElapsedClockRefusal::ContinuityLost),
        },
    })))
}
