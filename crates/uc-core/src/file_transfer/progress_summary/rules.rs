use super::{
    OutboundProgressSummary, ProgressBatchCounts, ProgressBatchState, ProgressBatchSummary,
};
use crate::file_transfer::{
    merge_progress_snapshot, FileTransferCancellationReason, OutboundProgressStatus,
    ProgressFreshnessView, ProgressItemState, ProgressPhase, ProgressSnapshotRefusal,
    ProgressTerminalOutcome,
};

/// 输入须来自同一已校验原计划的完整快照：每个逻辑项一次，目录为唯一完整单元。
/// 缺少通知且联系未过期的项仍等待远端状态；失联仅把未结算项归为未确认。
/// 该函数不读取时间、不修改投影、不产生网络或保存义务。
pub fn summarize_progress(
    items: impl IntoIterator<Item = Option<ProgressItemState>>,
    freshness: ProgressFreshnessView,
) -> ProgressBatchSummary {
    let mut counts = ProgressBatchCounts::default();
    for item in items {
        counts.total += 1;
        match item {
            Some(ProgressItemState::Terminal(result)) => match result.outcome {
                ProgressTerminalOutcome::Completed => counts.completed += 1,
                ProgressTerminalOutcome::Failed(_) => counts.failed += 1,
                ProgressTerminalOutcome::Cancelled(_) => counts.cancelled += 1,
            },
            _ if freshness == ProgressFreshnessView::Unconfirmed => counts.unconfirmed += 1,
            Some(ProgressItemState::Active(active)) if active.phase != ProgressPhase::Waiting => {
                counts.running += 1;
            }
            _ => counts.waiting += 1,
        }
    }
    let state = if counts.total == 0 {
        ProgressBatchState::NoItems
    } else if counts.unconfirmed == counts.total {
        ProgressBatchState::Unconfirmed
    } else if counts.unconfirmed > 0 {
        ProgressBatchState::PartiallyUnconfirmed
    } else if counts.running > 0 {
        ProgressBatchState::InProgress
    } else if counts.waiting > 0 {
        ProgressBatchState::Waiting
    } else if counts.failed == counts.total {
        ProgressBatchState::Failed
    } else if counts.failed > 0 {
        ProgressBatchState::PartiallyFailed
    } else if counts.cancelled == counts.total {
        ProgressBatchState::Cancelled
    } else if counts.cancelled > 0 {
        ProgressBatchState::PartiallyCancelled
    } else {
        ProgressBatchState::Completed
    };
    ProgressBatchSummary { state, counts }
}

/// 只从完整实际事实生成粗粒度通知；仍在等待、传输或处理的项阻止整批终态。
pub fn summarize_outbound_progress(
    items: &[ProgressItemState],
) -> Result<OutboundProgressSummary, ProgressSnapshotRefusal> {
    let plan = items
        .iter()
        .copied()
        .map(ProgressItemState::item)
        .collect::<Vec<_>>();
    merge_progress_snapshot(&plan, &vec![None; items.len()], items)?;
    let mut bytes = 0_u64;
    let mut total = Some(0_u64);
    let mut cancellation = None;
    let mut mixed_cancellation = false;
    for item in items {
        let (item_bytes, item_total) = match item {
            ProgressItemState::Active(value) => (value.bytes, value.total),
            ProgressItemState::Terminal(value) => {
                if let ProgressTerminalOutcome::Cancelled(reason) = value.outcome {
                    if cancellation.is_some_and(|current| current != reason) {
                        mixed_cancellation = true;
                    }
                    cancellation = Some(reason);
                }
                (value.bytes, value.total)
            }
        };
        bytes = bytes
            .checked_add(item_bytes)
            .ok_or(ProgressSnapshotRefusal::InvalidState)?;
        total = total
            .zip(item_total)
            .and_then(|(sum, value)| sum.checked_add(value));
    }
    let summary = summarize_progress(items.iter().copied().map(Some), ProgressFreshnessView::Live);
    let status = match summary.state {
        ProgressBatchState::Completed => OutboundProgressStatus::Completed,
        ProgressBatchState::Failed | ProgressBatchState::PartiallyFailed => {
            OutboundProgressStatus::Failed
        }
        ProgressBatchState::Cancelled | ProgressBatchState::PartiallyCancelled => {
            OutboundProgressStatus::Cancelled {
                reason: if mixed_cancellation {
                    FileTransferCancellationReason::Unknown
                } else {
                    cancellation.unwrap_or(FileTransferCancellationReason::Unknown)
                },
            }
        }
        _ => OutboundProgressStatus::InProgress,
    };
    Ok(OutboundProgressSummary {
        bytes,
        total,
        status,
    })
}
