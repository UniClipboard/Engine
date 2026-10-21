//! 整个接收动作的通知聚合；子获取只能记录事实，持久结算后才交接终态。
use super::work::PlannedReceiveItem;
use crate::transfer::blob::fetch_failure_reason;
use crate::transfer::blob::notifications::OutboundNotifications;
use crate::transfer::policy::PROGRESS_REPORT_INTERVAL;
use anyhow::Error as SourceError;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::time::Instant;
use uc_core::file_transfer::{
    FileTransferCancellationReason, FileTransferFailureReason, OutboundProgressStatus,
};
use uc_core::ids::DeviceId;
use uc_core::ports::{
    group_receive_items, AttemptState, ReceiveItemFailureReason, ReceiveItemKind,
    ReceiveItemProgress, TrackedFileTransferStatus,
};

#[derive(Default)]
pub(crate) struct ReceiveBatch(Mutex<State>);
#[derive(Default)]
struct State {
    planned: usize,
    total: u64,
    items: HashMap<String, Item>,
    routes: HashMap<(DeviceId, String), Arc<OutboundNotifications>>,
    settled: Option<AttemptState>,
    finished: bool,
    last_progress: Option<Instant>,
}
struct Item {
    bytes: u64,
    status: OutboundProgressStatus,
    source: Option<Arc<SourceError>>,
    failure_reason: Option<ReceiveItemFailureReason>,
}
impl fmt::Debug for ReceiveBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReceiveBatch")
    }
}
impl ReceiveBatch {
    pub(crate) fn plan(&self, count: usize, total: u64) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.planned = count;
        state.total = total;
    }
    pub(crate) fn record(
        &self,
        reporter: Arc<OutboundNotifications>,
        target: DeviceId,
        transfer: &str,
        item: &str,
        bytes: u64,
        status: OutboundProgressStatus,
    ) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.finished {
            return;
        }
        state
            .routes
            .entry((target, transfer.to_owned()))
            .or_insert(reporter.clone());
        let previous = state.items.entry(item.to_owned()).or_insert(Item {
            bytes: 0,
            status: OutboundProgressStatus::InProgress,
            source: None,
            failure_reason: None,
        });
        previous.bytes = previous.bytes.max(bytes);
        if matches!(previous.status, OutboundProgressStatus::InProgress) {
            previous.status = status;
        }
        // 中间进度不把一项的失败变成整批终态，也不等待远端通知。
        let transferred = state
            .items
            .values()
            .fold(0u64, |sum, item| sum.saturating_add(item.bytes));
        let now = Instant::now();
        if state
            .last_progress
            .is_some_and(|previous| now.duration_since(previous) < PROGRESS_REPORT_INTERVAL)
        {
            return;
        }
        state.last_progress = Some(now);
        reporter.report(
            &target,
            transfer,
            transferred.min(state.total),
            Some(state.total),
            OutboundProgressStatus::InProgress,
        );
    }
    pub(crate) fn item_result(
        &self,
        item: &str,
        bytes: u64,
        status: OutboundProgressStatus,
        source: Option<SourceError>,
    ) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.finished {
            let verified = state
                .items
                .get(item)
                .map_or(bytes, |previous| previous.bytes.max(bytes));
            let reason = match &status {
                OutboundProgressStatus::Failed => Some(
                    source
                        .as_ref()
                        .map_or(ReceiveItemFailureReason::Unknown, failure_reason),
                ),
                OutboundProgressStatus::Cancelled {
                    reason: FileTransferCancellationReason::Timeout,
                } => Some(ReceiveItemFailureReason::TimedOut),
                _ => None,
            };
            state.items.insert(
                item.to_owned(),
                Item {
                    bytes: verified,
                    status,
                    failure_reason: reason,
                    source: source.map(Arc::new),
                },
            );
        }
    }
    pub(crate) fn items(
        &self,
        planned: &[PlannedReceiveItem],
        persisted: bool,
        attempt: AttemptState,
    ) -> Vec<ReceiveItemProgress> {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        project_items(&state.items, state.finished, planned, persisted, attempt)
    }

    pub(crate) fn facts(&self) -> ReceiveFacts {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        ReceiveFacts {
            finished: state.finished,
            items: state
                .items
                .iter()
                .map(|(id, item)| {
                    (
                        id.clone(),
                        Item {
                            bytes: item.bytes,
                            status: item.status,
                            failure_reason: item.failure_reason,
                            source: None,
                        },
                    )
                })
                .collect(),
        }
    }
    pub(crate) fn settled(&self, state: AttemptState) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .settled = Some(state);
    }
    pub(crate) fn finish(&self, action_succeeded: bool) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.finished {
            return;
        }
        state.finished = true;
        let status = match state.settled {
            Some(AttemptState::Cancelled) => OutboundProgressStatus::Cancelled {
                reason: FileTransferCancellationReason::RemotePeer,
            },
            Some(AttemptState::Failed) => OutboundProgressStatus::Failed,
            _ if !action_succeeded
                || state.items.len() != state.planned
                || state
                    .items
                    .values()
                    .any(|item| matches!(item.status, OutboundProgressStatus::Failed)) =>
            {
                OutboundProgressStatus::Failed
            }
            _ if state
                .items
                .values()
                .any(|item| matches!(item.status, OutboundProgressStatus::Cancelled { .. })) =>
            {
                OutboundProgressStatus::Cancelled {
                    reason: FileTransferCancellationReason::RemotePeer,
                }
            }
            _ if state
                .items
                .values()
                .all(|item| matches!(item.status, OutboundProgressStatus::Completed)) =>
            {
                OutboundProgressStatus::Completed
            }
            _ => OutboundProgressStatus::Failed,
        };
        let bytes = state
            .items
            .values()
            .fold(0u64, |sum, item| sum.saturating_add(item.bytes))
            .min(state.total);
        for ((target, transfer), reporter) in &state.routes {
            reporter.report(target, transfer, bytes, Some(state.total), status.clone());
        }
        for item in state.items.values_mut() {
            item.source = None;
        }
    }
}

#[cfg(test)]
mod tests;

fn tracked_status(status: &OutboundProgressStatus) -> TrackedFileTransferStatus {
    match status {
        OutboundProgressStatus::InProgress => TrackedFileTransferStatus::Transferring,
        OutboundProgressStatus::Completed => TrackedFileTransferStatus::Completed,
        OutboundProgressStatus::Failed => TrackedFileTransferStatus::Failed,
        OutboundProgressStatus::Cancelled { .. } => TrackedFileTransferStatus::Cancelled,
    }
}
fn failure_reason(error: &SourceError) -> ReceiveItemFailureReason {
    match fetch_failure_reason(error) {
        FileTransferFailureReason::NetworkUnavailable => {
            ReceiveItemFailureReason::NetworkUnavailable
        }
        FileTransferFailureReason::TimedOut => ReceiveItemFailureReason::TimedOut,
        FileTransferFailureReason::AccessDenied => ReceiveItemFailureReason::AccessDenied,
        FileTransferFailureReason::StorageUnavailable => {
            ReceiveItemFailureReason::StorageUnavailable
        }
        FileTransferFailureReason::IntegrityCheckFailed => {
            ReceiveItemFailureReason::IntegrityCheckFailed
        }
        FileTransferFailureReason::Unknown => ReceiveItemFailureReason::Unknown,
    }
}

pub(crate) struct ReceiveFacts {
    items: HashMap<String, Item>,
    finished: bool,
}
impl ReceiveFacts {
    pub(crate) fn items(
        &self,
        planned: &[PlannedReceiveItem],
        persisted: bool,
        attempt: AttemptState,
    ) -> Vec<ReceiveItemProgress> {
        project_items(&self.items, self.finished, planned, persisted, attempt)
    }
}
fn project_items(
    items: &HashMap<String, Item>,
    finished: bool,
    planned: &[PlannedReceiveItem],
    persisted: bool,
    attempt: AttemptState,
) -> Vec<ReceiveItemProgress> {
    if planned
        .first()
        .is_some_and(|item| item.kind == ReceiveItemKind::DirectoryUnit)
    {
        let members = items
            .iter()
            .map(|(id, item)| ReceiveItemProgress {
                item_id: id.clone(),
                kind: ReceiveItemKind::Unknown,
                status: tracked_status(&item.status),
                failure_reason: item.failure_reason,
            })
            .collect();
        return group_receive_items(attempt, true, members);
    }
    planned
        .iter()
        .map(|planned| {
            let item = items.get(&planned.id);
            let mut status = item.map_or(TrackedFileTransferStatus::Pending, |item| {
                tracked_status(&item.status)
            });
            let mut reason = item.and_then(|item| item.failure_reason);
            let downloaded = status == TrackedFileTransferStatus::Completed;
            if status == TrackedFileTransferStatus::Completed
                && (!persisted || !attempt.is_terminal())
            {
                status = TrackedFileTransferStatus::Transferring;
            }
            if attempt.is_terminal()
                && (status == TrackedFileTransferStatus::Pending
                    || status == TrackedFileTransferStatus::Transferring)
            {
                if !finished && !persisted {
                    status = TrackedFileTransferStatus::Transferring;
                } else if attempt == AttemptState::Cancelled {
                    status = TrackedFileTransferStatus::Cancelled;
                } else {
                    status = TrackedFileTransferStatus::Failed;
                    reason = Some(if downloaded {
                        ReceiveItemFailureReason::Unknown
                    } else {
                        ReceiveItemFailureReason::Interrupted
                    });
                }
            }
            if status == TrackedFileTransferStatus::Failed && reason.is_none() {
                reason = Some(ReceiveItemFailureReason::Unknown);
            }
            ReceiveItemProgress {
                item_id: planned.id.clone(),
                kind: planned.kind,
                status,
                failure_reason: reason,
            }
        })
        .collect()
}
