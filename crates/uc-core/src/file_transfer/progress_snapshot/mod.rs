//! 同一登记与原始计划的快照覆盖；只在最后一页完整通过后授权发布。
mod coverage;
mod model;
pub use coverage::ProgressSnapshotCoverage;
pub use model::{
    ProgressActive, ProgressItemState, ProgressPhase, ProgressSnapshotEffect,
    ProgressSnapshotInput, ProgressSnapshotRefusal, MAX_SNAPSHOT_PAGE_ITEMS,
};
#[cfg(test)]
mod tests;
