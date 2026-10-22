//! 整个接收动作的通知聚合；子获取只能记录事实，持久结算后才交接终态。
use super::materialization::FileProgressCommitError;
use super::work::PlannedReceiveItem;
use super::{DownloadSettlement, LocalSettlement, LocalSettlementOutcome};
use crate::transfer::blob::fetch_failure_reason;
use crate::transfer::blob::notifications::OutboundNotifications;
use crate::transfer::policy::PROGRESS_REPORT_INTERVAL;
use anyhow::Error as SourceError;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};
use std::{fmt, mem};
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
    local_work: HashMap<String, DownloadSettlement>,
    /// 最后一个实际下载的本地交接覆盖父保存；下一项启动前不积累旧许可。
    commit_handoff: Option<DownloadSettlement>,
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
    pub(in super::super) fn local_settlement(&self, item: &str) -> DownloadSettlement {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.commit_handoff = None;
        state.local_work.entry(item.to_owned()).or_default().clone()
    }
    pub(crate) fn require_file_commit(&self, item: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .local_work
            .entry(item.to_owned())
            .or_default()
            .require_local_commit();
    }
    pub(in super::super) fn file_completion(&self, item: &str) -> Option<LocalSettlement> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .local_work
            .get(item)
            .and_then(DownloadSettlement::local_completion)
    }
    pub(crate) fn begin_file_commit(&self, item: &str) -> bool {
        self.file_completion(item)
            .is_some_and(|completion| completion.begin_commit())
    }
    pub(crate) fn fail_unsettled_files(&self, source: Arc<SourceError>) {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for work in state.local_work.values().chain(state.commit_handoff.iter()) {
            work.abandon_local(source.clone());
        }
    }
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
            let local = state.local_work.remove(item);
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
            let source = source.map(Arc::new);
            if let Some(local) = &local {
                match status {
                    OutboundProgressStatus::Cancelled { .. } => {
                        local.complete_local(Ok(LocalSettlementOutcome::Cancelled))
                    }
                    OutboundProgressStatus::Failed => {
                        if let Some(source) = &source {
                            local.abandon_local(source.clone());
                        }
                    }
                    _ => {}
                }
            }
            if state.local_work.is_empty() && local.is_some() {
                state.commit_handoff = local;
            }
            state.items.insert(
                item.to_owned(),
                Item {
                    bytes: verified,
                    status,
                    failure_reason: reason,
                    source,
                },
            );
        }
    }
    pub(crate) fn item_materialized(&self, item: &str, bytes: u64) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.finished {
            state.items.insert(
                item.to_owned(),
                Item {
                    bytes,
                    status: OutboundProgressStatus::InProgress,
                    source: None,
                    failure_reason: None,
                },
            );
        }
    }

    /// 只有实际保存成功的本项才可在父动作结束之前归还原许可。
    pub(crate) fn item_saved(&self, item: &str, bytes: u64) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.finished {
            state.items.insert(
                item.to_owned(),
                Item {
                    bytes,
                    status: OutboundProgressStatus::Completed,
                    source: None,
                    failure_reason: None,
                },
            );
            if let Some(local) = state.local_work.remove(item) {
                local.complete_local(Ok(LocalSettlementOutcome::Committed));
            }
        }
    }

    pub(crate) fn inherit_result(&self, id: &str, previous: &ReceiveItemProgress) {
        let status = if previous.status == TrackedFileTransferStatus::Cancelled {
            OutboundProgressStatus::Cancelled {
                reason: FileTransferCancellationReason::LocalUser,
            }
        } else {
            OutboundProgressStatus::Failed
        };
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.finished {
            state.items.insert(
                id.to_owned(),
                Item {
                    bytes: 0,
                    status,
                    source: None,
                    failure_reason: previous.failure_reason,
                },
            );
        }
    }

    pub(crate) fn items(
        &self,
        planned: &[PlannedReceiveItem],
        committed: &HashSet<usize>,
        attempt: AttemptState,
    ) -> Vec<ReceiveItemProgress> {
        let state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        project_items(&state.items, state.finished, planned, committed, attempt)
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
        state.local_work.clear();
        state.commit_handoff = None;
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
    if error
        .chain()
        .any(|source| source.is::<FileProgressCommitError>())
    {
        return ReceiveItemFailureReason::StorageUnavailable;
    }
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
    /// 两组标识均由同一份原始清单生成；只改变尝试身份，不采用新投递的数组位置。
    pub(super) fn rebase(
        &mut self,
        previous: &[PlannedReceiveItem],
        current: &[PlannedReceiveItem],
    ) {
        let mut items = mem::take(&mut self.items);
        self.items = previous
            .iter()
            .zip(current)
            .filter_map(|(previous, current)| {
                items
                    .remove(&previous.id)
                    .map(|item| (current.id.clone(), item))
            })
            .collect();
    }

    pub(crate) fn items(
        &self,
        planned: &[PlannedReceiveItem],
        committed: &HashSet<usize>,
        attempt: AttemptState,
    ) -> Vec<ReceiveItemProgress> {
        project_items(&self.items, self.finished, planned, committed, attempt)
    }
}
fn project_items(
    items: &HashMap<String, Item>,
    finished: bool,
    planned: &[PlannedReceiveItem],
    committed: &HashSet<usize>,
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
        return group_receive_items(attempt, true, members, &HashSet::new());
    }
    planned
        .iter()
        .map(|planned| {
            let persisted = committed.contains(&planned.blob_index);
            let item = items.get(&planned.id);
            let mut status = item.map_or(TrackedFileTransferStatus::Pending, |item| {
                tracked_status(&item.status)
            });
            let mut reason = item.and_then(|item| item.failure_reason);
            if persisted {
                status = TrackedFileTransferStatus::Completed;
                reason = None;
            }
            let downloaded = status == TrackedFileTransferStatus::Completed;
            if status == TrackedFileTransferStatus::Completed && !persisted {
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
