//! 本机历史标签：用户创建的分类及其与历史条目的手动关联。
//!
//! 本模块是标签定义、关联、改名、合并与删除的唯一流程负责人。标签只属于本机历史，
//! 不进入同步；搜索在查询时读取权威关联，因此索引重建不会影响用户标签。

mod flow;
mod store;
mod views;

pub(crate) use flow::HistoryTags;
pub(crate) use store::load_history_tag_ids;
pub use store::{
    HistoryEntryTagReaderPort, HistoryEntryTagSummary, HistoryTagBatchOutcome,
    HistoryTagDeleteOutcome, HistoryTagMergeOutcome, HistoryTagRecord, HistoryTagStoreError,
    HistoryTagStorePort,
};
pub use views::{
    HistoryEntryTagSummaryView, HistoryTagApplicationView, HistoryTagBatchView,
    HistoryTagCreatedView, HistoryTagError, HistoryTagMergeView, HistoryTagRenameView,
    HistoryTagView,
};
