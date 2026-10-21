use super::ReceiveCancellationBarrier;
use crate::space::CurrentSpaceMemberScopePort;
use crate::transfer::receive::materialization::ReceiveSelection;
use crate::transfer::receive::work::{ReceiveAction, ReceiveSource};
use std::{fmt, sync::Arc};
use tokio::sync::oneshot;
use uc_core::ids::EntryId;
use uc_core::ports::clipboard::GetEntrySnapshotHashPort;
use uc_core::SystemClipboardSnapshot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetryEntryReceiveOutcome {
    Accepted { attempt_id: String },
    AlreadyReceiving,
    Superseded,
    CancellationIncomplete,
    Unavailable,
    AccessDenied,
}

#[derive(thiserror::Error)]
pub enum RetryEntryReceiveError {
    #[error("receive retry selection is invalid")]
    InvalidInput,
    #[error("receive retry original output is unavailable")]
    Unavailable(#[source] anyhow::Error),
    #[error("receive retry capacity is exhausted")]
    Busy,
    #[error("receive retry could not read its original source")]
    Source(#[source] anyhow::Error),
    #[error("receive retry failed before acceptance")]
    Receive(#[source] anyhow::Error),
    #[error("receive retry supervisor failed")]
    Task(#[source] tokio::task::JoinError),
}

impl fmt::Debug for RetryEntryReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

pub(crate) struct PreparedReceiveRetry {
    pub(crate) cancellation: Arc<dyn ReceiveCancellationBarrier>,
    pub(crate) identity: Arc<dyn GetEntrySnapshotHashPort>,
    pub(crate) scope: Arc<dyn CurrentSpaceMemberScopePort>,
    pub(crate) entry: EntryId,
    pub(crate) expected_attempt: String,
    pub(crate) source: ReceiveSource,
    pub(crate) snapshot: SystemClipboardSnapshot,
    pub(crate) selection: Option<ReceiveSelection>,
    pub(crate) action: Arc<ReceiveAction>,
    pub(crate) accepted: Option<oneshot::Sender<RetryEntryReceiveOutcome>>,
}
