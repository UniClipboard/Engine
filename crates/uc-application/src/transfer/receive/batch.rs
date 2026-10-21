//! 整个接收动作的通知聚合；子获取只能记录事实，持久结算后才交接终态。
use crate::transfer::blob::notifications::OutboundNotifications;
use crate::transfer::policy::PROGRESS_REPORT_INTERVAL;
use anyhow::Error as SourceError;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::time::Instant;
use uc_core::file_transfer::{FileTransferCancellationReason, OutboundProgressStatus};
use uc_core::ids::DeviceId;
use uc_core::ports::AttemptState;

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
    _source: Option<Arc<SourceError>>,
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
            _source: None,
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
            state.items.insert(
                item.to_owned(),
                Item {
                    bytes: verified,
                    status,
                    _source: source.map(Arc::new),
                },
            );
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
    }
}

#[cfg(test)]
mod tests;
