use super::{ProgressBatchCounts, ProgressBatchState, ProgressBatchSummary};
use crate::file_transfer::{
    ProgressFreshnessView, ProgressItemState, ProgressPhase, ProgressTerminalOutcome,
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
