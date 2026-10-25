//! 从已校验原计划的逐项投影生成批次结果；联系状态不替代真实结算。
mod model;
mod rules;

pub use model::{ProgressBatchCounts, ProgressBatchState, ProgressBatchSummary};
pub use rules::summarize_progress;

#[cfg(test)]
mod tests;
