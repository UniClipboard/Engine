#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProgressBatchCounts {
    pub total: usize,
    pub waiting: usize,
    /// 传输、自动重试与本地处理均仍在运行，不代表实际完成。
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub unconfirmed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressBatchState {
    /// 没有可汇总的任务，不能推断为空批次成功。
    NoItems,
    Waiting,
    InProgress,
    Unconfirmed,
    PartiallyUnconfirmed,
    Completed,
    Failed,
    PartiallyFailed,
    Cancelled,
    PartiallyCancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressBatchSummary {
    pub state: ProgressBatchState,
    pub counts: ProgressBatchCounts,
}
