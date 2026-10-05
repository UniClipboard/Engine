//! 历史标签流程的对外结果与错误。

use uc_core::clipboard::HistoryTagNameError;

/// 一个历史标签及其关联条目数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTagView {
    pub tag_id: String,
    /// 名称无法解密时为 `None`；该标签仍可删除。
    pub name: Option<String>,
    pub created_at_ms: i64,
    pub entry_count: u32,
}

/// 创建结果：同名标签已存在时返回已有标签，`created = false`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTagCreatedView {
    pub tag: HistoryTagView,
    pub created: bool,
}

/// 批量关联或移除的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTagBatchView {
    pub changed: u32,
    pub unchanged: u32,
    pub missing_entry_ids: Vec<String>,
}

/// 一组条目中单个标签的携带数量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTagApplicationView {
    pub tag_id: String,
    pub applied: u32,
}

/// 一组条目的标签汇总。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntryTagSummaryView {
    pub selected: u32,
    pub tags: Vec<HistoryTagApplicationView>,
}

/// 改名结果；新名称与另一个标签同名时不写入并返回冲突标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryTagRenameView {
    Renamed(HistoryTagView),
    NameConflict { existing_tag_id: String },
}

/// 合并结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTagMergeView {
    pub moved: u32,
    pub already_on_target: u32,
}

/// 历史标签操作失败。
#[derive(Debug, thiserror::Error)]
pub enum HistoryTagError {
    #[error("invalid history tag name")]
    InvalidName(#[source] HistoryTagNameError),
    /// 批量大小、标签数量上限或合并来源与目标不合法。
    #[error("invalid history tag request")]
    InvalidInput,
    #[error("history tag not found")]
    NotFound,
    #[error("history tags are locked")]
    Locked,
    #[error("history tags are unavailable")]
    Unavailable,
    #[error("history tag operation failed")]
    Internal(#[source] anyhow::Error),
}
