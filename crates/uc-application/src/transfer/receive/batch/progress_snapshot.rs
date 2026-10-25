//! 从原接收计划与保存事实生成逻辑项状态；网络字节完成不能越过本地交付围栏。
use super::{project_items, OutboundProgressStatus, PlannedReceiveItem, ReceiveFacts};
use std::collections::HashSet;
use uc_core::file_transfer::{
    FileTransferCancellationReason, ProgressActive, ProgressFailureReason, ProgressItem,
    ProgressItemKind, ProgressItemState, ProgressPhase, ProgressTerminal, ProgressTerminalOutcome,
};
use uc_core::ports::{AttemptState, ReceiveItemFailureReason, TrackedFileTransferStatus};

impl ReceiveFacts {
    pub(crate) fn progress_states(
        &self,
        planned: &[PlannedReceiveItem],
        logical: &[ProgressItem],
        committed: &HashSet<usize>,
        fully_persisted: bool,
    ) -> Option<Vec<ProgressItemState>> {
        if planned.len() != logical.len() {
            return None;
        }
        let all_committed;
        let committed =
            if fully_persisted && self.finished && self.settled == Some(AttemptState::Completed) {
                all_committed = planned.iter().map(|item| item.blob_index).collect();
                &all_committed
            } else {
                committed
            };
        let projected = project_items(
            &self.items,
            self.finished,
            planned,
            committed,
            self.settled.unwrap_or(AttemptState::Receiving),
        );
        planned
            .iter()
            .zip(logical)
            .map(|(plan, logical)| {
                let saved = projected.iter().find(|item| item.item_id == plan.id)?;
                let local = self.items.get(&plan.id);
                let bytes = if saved.status == TrackedFileTransferStatus::Completed {
                    plan.bytes
                } else {
                    if logical.kind == ProgressItemKind::DirectoryUnit {
                        self.items
                            .values()
                            .fold(0_u64, |sum, item| sum.saturating_add(item.bytes))
                            .min(plan.bytes)
                    } else {
                        local.map_or(0, |item| item.bytes.min(plan.bytes))
                    }
                };
                let outcome = match saved.status {
                    TrackedFileTransferStatus::Completed => {
                        Some(ProgressTerminalOutcome::Completed)
                    }
                    TrackedFileTransferStatus::Failed => {
                        Some(ProgressTerminalOutcome::Failed(failure(
                            saved
                                .failure_reason
                                .unwrap_or(ReceiveItemFailureReason::Unknown),
                        )))
                    }
                    TrackedFileTransferStatus::Cancelled => {
                        Some(ProgressTerminalOutcome::Cancelled(
                            local
                                .and_then(|item| match item.status {
                                    OutboundProgressStatus::Cancelled { reason } => Some(reason),
                                    _ => None,
                                })
                                .unwrap_or(FileTransferCancellationReason::Unknown),
                        ))
                    }
                    TrackedFileTransferStatus::Pending
                    | TrackedFileTransferStatus::Transferring => None,
                };
                // 实际失败／取消仍在清理时只能显示处理中，不提前发布终态。
                let settling = !self.finished
                    && matches!(
                        outcome,
                        Some(
                            ProgressTerminalOutcome::Failed(_)
                                | ProgressTerminalOutcome::Cancelled(_)
                        )
                    );
                let outcome = if settling { None } else { outcome };
                Some(match outcome {
                    Some(outcome) => ProgressItemState::Terminal(ProgressTerminal {
                        item: *logical,
                        round: 0,
                        sequence: 0,
                        result_revision: 1,
                        bytes,
                        total: Some(plan.bytes),
                        outcome,
                    }),
                    None => ProgressItemState::Active(ProgressActive {
                        item: *logical,
                        round: 0,
                        sequence: 0,
                        bytes,
                        total: Some(plan.bytes),
                        phase: if saved.status == TrackedFileTransferStatus::Pending {
                            ProgressPhase::Waiting
                        } else if settling || bytes == plan.bytes {
                            ProgressPhase::Processing
                        } else {
                            ProgressPhase::Transferring
                        },
                    }),
                })
            })
            .collect()
    }
}
fn failure(reason: ReceiveItemFailureReason) -> ProgressFailureReason {
    match reason {
        ReceiveItemFailureReason::NetworkUnavailable => ProgressFailureReason::NetworkUnavailable,
        ReceiveItemFailureReason::TimedOut => ProgressFailureReason::TimedOut,
        ReceiveItemFailureReason::AccessDenied => ProgressFailureReason::AccessDenied,
        ReceiveItemFailureReason::StorageUnavailable => ProgressFailureReason::StorageUnavailable,
        ReceiveItemFailureReason::IntegrityCheckFailed => {
            ProgressFailureReason::IntegrityCheckFailed
        }
        ReceiveItemFailureReason::Interrupted => ProgressFailureReason::Interrupted,
        ReceiveItemFailureReason::Unknown => ProgressFailureReason::Unknown,
    }
}
