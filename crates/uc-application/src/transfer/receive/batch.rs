//! 整个接收动作的通知聚合；子获取只能记录事实，持久结算后才交接终态。
use super::materialization::{DirectoryPublication, FileProgressCommitError};
use super::work::{PlannedReceiveItem, ReceiveProgressSignal};
use super::{DownloadSettlement, LocalSettlement, LocalSettlementOutcome};
use crate::transfer::blob::fetch_failure_reason;
use anyhow::Error as SourceError;
use std::collections::{HashMap, HashSet};
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::{fmt, mem};
use uc_core::file_transfer::{
    FileTransferCancellationReason, FileTransferFailureReason, OutboundProgressStatus,
};
use uc_core::ports::{
    group_receive_items, AttemptState, ReceiveItemFailureReason, ReceiveItemKind,
    ReceiveItemProgress, TrackedFileTransferStatus,
};

mod progress_snapshot;

#[derive(Default)]
pub(crate) struct ReceiveBatch {
    state: Mutex<State>,
    signal: Mutex<Option<ReceiveProgressSignal>>,
}
// 字段按声明顺序释放：先放下事实锁，再发布无阻塞变化，避免与原目录读取形成反锁。
struct BatchChange<'a> {
    state: MutexGuard<'a, State>,
    _signal: SignalOnDrop<'a>,
}
struct SignalOnDrop<'a>(&'a ReceiveBatch);
impl Drop for SignalOnDrop<'_> {
    fn drop(&mut self) {
        self.0.progress_changed();
    }
}
impl Deref for BatchChange<'_> {
    type Target = State;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}
impl DerefMut for BatchChange<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}
#[derive(Default)]
struct State {
    planned: usize,
    items: HashMap<String, Item>,
    local_work: HashMap<String, DownloadSettlement>,
    /// 最后一个实际下载的本地交接覆盖父保存；下一项启动前不积累旧许可。
    commit_handoff: Option<DownloadSettlement>,
    directory_rollback: Option<DirectoryPublication>,
    settled: Option<AttemptState>,
    finished: bool,
}
#[derive(Clone, Copy, Default)]
struct RoundProgress {
    index: u32,
    bytes: u64,
    total: Option<u64>,
    retrying: bool,
}
struct Item {
    round: RoundProgress,
    bytes: u64,
    status: OutboundProgressStatus,
    source: Option<Arc<SourceError>>,
    failure_reason: Option<ReceiveItemFailureReason>,
    /// 原单项失败／取消已经完成实际本地结算；不代表父目录已回滚。
    failure_settled: bool,
}
impl fmt::Debug for ReceiveBatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReceiveBatch")
    }
}
impl ReceiveBatch {
    fn change(&self) -> BatchChange<'_> {
        BatchChange {
            state: self.state.lock().unwrap_or_else(PoisonError::into_inner),
            _signal: SignalOnDrop(self),
        }
    }
    pub(crate) fn observe_progress(&self, signal: ReceiveProgressSignal) {
        *self.signal.lock().unwrap_or_else(PoisonError::into_inner) = Some(signal);
        self.progress_changed();
    }
    pub(crate) fn progress_changed(&self) {
        let signal = self
            .signal
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(signal) = signal {
            signal.changed();
        }
    }
    pub(crate) fn retain_directory_rollback(&self, publication: DirectoryPublication) {
        self.change().directory_rollback.get_or_insert(publication);
    }
    pub(crate) fn take_directory_rollback(&self) -> Option<DirectoryPublication> {
        self.change().directory_rollback.take()
    }
    pub(in super::super) fn local_settlement(&self, item: &str) -> DownloadSettlement {
        let mut state = self.change();
        state.commit_handoff = None;
        state.local_work.entry(item.to_owned()).or_default().clone()
    }
    pub(crate) fn require_file_commit(&self, item: &str) {
        self.change()
            .local_work
            .entry(item.to_owned())
            .or_default()
            .require_local_commit();
    }
    pub(in super::super) fn file_completion(&self, item: &str) -> Option<LocalSettlement> {
        self.state
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
        let state = self.change();
        for work in state.local_work.values().chain(state.commit_handoff.iter()) {
            work.abandon_local(source.clone());
        }
    }
    pub(crate) fn plan(&self, count: usize) {
        let mut state = self.change();
        state.planned = count;
    }
    /// 记录真实网络轮次的位置；历史高水位仍独立保留，迟到旧轮次没有副作用。
    pub(crate) fn record_round(&self, item: &str, round: u32, bytes: u64, total: Option<u64>) {
        let mut state = self.change();
        if state.finished {
            return;
        }
        let value = state.items.entry(item.to_owned()).or_insert(Item {
            round: RoundProgress::default(),
            bytes: 0,
            status: OutboundProgressStatus::InProgress,
            source: None,
            failure_reason: None,
            failure_settled: false,
        });
        if !matches!(value.status, OutboundProgressStatus::InProgress) || round < value.round.index
        {
            return;
        }
        let known_total = total.or(value.round.total);
        if known_total.is_some_and(|total| bytes > total)
            || (round == value.round.index
                && value
                    .round
                    .total
                    .zip(total)
                    .is_some_and(|(previous, current)| previous != current))
        {
            return;
        }
        value.bytes = value.bytes.max(bytes);
        value.round = RoundProgress {
            index: round,
            retrying: false,
            total: known_total,
            bytes: if round == value.round.index {
                value.round.bytes.max(bytes)
            } else {
                bytes
            },
        };
    }

    /// 只由原下载负责人实际进入恢复等待／开始下一次尝试时记录。
    pub(crate) fn record_retrying(&self, item: &str, round: u32) {
        let mut state = self.change();
        if state.finished {
            return;
        }
        let value = state.items.entry(item.to_owned()).or_insert(Item {
            round: RoundProgress::default(),
            bytes: 0,
            status: OutboundProgressStatus::InProgress,
            source: None,
            failure_reason: None,
            failure_settled: false,
        });
        if !matches!(value.status, OutboundProgressStatus::InProgress) || round < value.round.index
        {
            return;
        }
        if round > value.round.index {
            value.round.index = round;
            value.round.bytes = 0;
        }
        value.round.retrying = true;
    }

    /// 仅在本项实际获取及输出清理结算后记录；仍持有本地工作时使用unsettled_fetch_failure。
    pub(crate) fn item_result(
        &self,
        item: &str,
        bytes: u64,
        status: OutboundProgressStatus,
        source: Option<SourceError>,
    ) {
        let mut state = self.change();
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
                        .map_or(ReceiveItemFailureReason::Unknown, receive_failure_reason),
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
            let round = state.items.get(item).map_or(
                RoundProgress {
                    index: 0,
                    bytes,
                    ..RoundProgress::default()
                },
                |previous| previous.round,
            );
            state.items.insert(
                item.to_owned(),
                Item {
                    round,
                    bytes: verified,
                    status,
                    failure_reason: reason,
                    failure_settled: matches!(
                        status,
                        OutboundProgressStatus::Failed | OutboundProgressStatus::Cancelled { .. }
                    ),
                    source,
                },
            );
        }
    }
    /// 获取结果已知但输出仍在清理；只记录失败事实，不完成本地交接或释放资源。
    pub(crate) fn unsettled_fetch_failure(
        &self,
        item: &str,
        cancelled: bool,
        source: &SourceError,
    ) {
        let mut state = self.change();
        if state.finished {
            return;
        }
        let item = state.items.entry(item.to_owned()).or_insert(Item {
            round: RoundProgress::default(),
            bytes: 0,
            status: OutboundProgressStatus::InProgress,
            source: None,
            failure_reason: None,
            failure_settled: false,
        });
        item.failure_settled = false;
        item.status = if cancelled {
            OutboundProgressStatus::Cancelled {
                reason: FileTransferCancellationReason::LocalUser,
            }
        } else {
            OutboundProgressStatus::Failed
        };
        item.failure_reason = if cancelled {
            None
        } else {
            Some(receive_failure_reason(source))
        };
    }

    pub(crate) fn item_materialized(&self, item: &str, bytes: u64) {
        let mut state = self.change();
        if !state.finished {
            let round = state.items.get(item).map_or(
                RoundProgress {
                    bytes,
                    ..RoundProgress::default()
                },
                |previous| previous.round,
            );
            state.items.insert(
                item.to_owned(),
                Item {
                    round,
                    bytes,
                    status: OutboundProgressStatus::InProgress,
                    source: None,
                    failure_reason: None,
                    failure_settled: false,
                },
            );
        }
    }

    /// 只有实际保存成功的本项才可在父动作结束之前归还原许可。
    pub(crate) fn item_saved(&self, item: &str, bytes: u64) {
        let mut state = self.change();
        if !state.finished {
            let round = state.items.get(item).map_or(
                RoundProgress {
                    bytes,
                    ..RoundProgress::default()
                },
                |previous| previous.round,
            );
            state.items.insert(
                item.to_owned(),
                Item {
                    round,
                    bytes,
                    status: OutboundProgressStatus::Completed,
                    source: None,
                    failure_reason: None,
                    failure_settled: false,
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
        let mut state = self.change();
        if !state.finished {
            state.items.insert(
                id.to_owned(),
                Item {
                    round: RoundProgress::default(),
                    bytes: 0,
                    status,
                    source: None,
                    failure_reason: previous.failure_reason,
                    failure_settled: true,
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
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        project_items(&state.items, state.finished, planned, committed, attempt)
    }

    pub(crate) fn facts(&self) -> ReceiveFacts {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        ReceiveFacts {
            settled: state.settled,
            finished: state.finished,
            items: state
                .items
                .iter()
                .map(|(id, item)| {
                    (
                        id.clone(),
                        Item {
                            round: item.round,
                            bytes: item.bytes,
                            status: item.status,
                            failure_reason: item.failure_reason,
                            failure_settled: item.failure_settled,
                            source: None,
                        },
                    )
                })
                .collect(),
        }
    }
    pub(crate) fn settled(&self, state: AttemptState) {
        self.change().settled = Some(state);
    }
    pub(crate) fn finish(&self, action_succeeded: bool) {
        let mut state = self.change();
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
        // 原完整动作已实际结束；快照必须保留与原终态通知相同的最终事实。
        if state.settled.is_none() {
            state.settled = match status {
                OutboundProgressStatus::Completed => Some(AttemptState::Completed),
                OutboundProgressStatus::Failed => Some(AttemptState::Failed),
                OutboundProgressStatus::Cancelled { .. } => Some(AttemptState::Cancelled),
                OutboundProgressStatus::InProgress => None,
            };
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
pub(crate) fn receive_failure_reason(error: &SourceError) -> ReceiveItemFailureReason {
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
    settled: Option<AttemptState>,
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
