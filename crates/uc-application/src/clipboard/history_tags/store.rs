//! 历史标签流程需要的持久化能力。
//!
//! 标签定义与条目关联是本机历史的权威元数据；搜索索引只在查询时读取关联，
//! 不保存它的副本。

use std::collections::HashMap;

use async_trait::async_trait;
use uc_core::clipboard::HistoryTagName;
use uc_core::ids::EntryId;
use uc_core::search::tag::TagId;

/// 已持久化的历史标签及其关联条目数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryTagRecord {
    pub tag_id: TagId,
    /// 解密后的名称；密文无法打开时为 `None`，该行仍可被删除。
    pub name: Option<HistoryTagName>,
    pub created_at_ms: i64,
    /// 当前关联的历史条目数。
    pub entry_count: u32,
}

/// 批量关联或移除的结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryTagBatchOutcome {
    /// 实际新增或删除的关联数。
    pub changed: u32,
    /// 本来就处于目标状态的条目数。
    pub unchanged: u32,
    /// 不存在或已删除、因此被跳过的条目。
    pub missing_entry_ids: Vec<EntryId>,
    /// 标签集合发生变化的条目及其变化后的全部标签，用于同步搜索索引。
    pub changed_entries: Vec<(EntryId, Vec<TagId>)>,
}

/// 一组条目的标签汇总。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryEntryTagSummary {
    /// 所选条目中仍存在的数量。
    pub selected: u32,
    /// 每个标签在仍存在的所选条目中的携带数量，按 tag id 排序。
    pub tags: Vec<(TagId, u32)>,
}

/// 合并结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryTagMergeOutcome {
    /// 新转移到目标标签的关联数。
    pub moved: u32,
    /// 来源关联的条目原本已携带目标标签的数量。
    pub already_on_target: u32,
    /// 标签集合发生变化的条目及其变化后的全部标签，用于同步搜索索引。
    pub changed_entries: Vec<(EntryId, Vec<TagId>)>,
}

/// 删除结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryTagDeleteOutcome {
    /// 解除的关联数。
    pub detached: u32,
    /// 标签集合发生变化的条目及其变化后的全部标签，用于同步搜索索引。
    pub changed_entries: Vec<(EntryId, Vec<TagId>)>,
}

/// 历史标签仓储失败。
#[derive(Debug, thiserror::Error)]
pub enum HistoryTagStoreError {
    /// 指定的标签（或合并中的任一来源/目标）不存在。
    #[error("history tag not found")]
    TagNotFound,
    /// 当前 profile 的存储格式无法保存加密的标签名称。
    #[error("history tags are unavailable for this profile")]
    Unavailable,
    /// 当前加密会话未就绪，无法读写标签名称。
    #[error("history tag protection is locked")]
    Locked,
    /// 持久化或加密失败。
    #[error("history tag storage failed")]
    Storage(#[source] anyhow::Error),
}

/// 历史标签定义与关联的维护能力。
///
/// 契约：
/// - 名称、创建时间与条目关联只以密文持久化；无法打开的名称以 `name = None` 返回，
///   不使整个列表失败。
/// - 每个写操作在单个事务内完成：要么全部生效，要么不改变任何事实。
/// - 条目被删除时，其关联随之消失；标签被删除时，条目内容不变。
/// - 写操作由调用方串行执行；名称唯一性由调用方判定，端口不解析名称。
#[async_trait]
pub trait HistoryTagStorePort: Send + Sync {
    /// 列出全部标签及各自关联的条目数。
    async fn list_tags(&self) -> Result<Vec<HistoryTagRecord>, HistoryTagStoreError>;

    /// 保存一个新标签。
    async fn create_tag(
        &self,
        tag_id: &TagId,
        name: &HistoryTagName,
        created_at_ms: i64,
    ) -> Result<(), HistoryTagStoreError>;

    /// 替换已有标签的名称；标签不存在时返回 [`HistoryTagStoreError::TagNotFound`]。
    async fn rename_tag(
        &self,
        tag_id: &TagId,
        name: &HistoryTagName,
    ) -> Result<(), HistoryTagStoreError>;

    /// 把标签关联到仍存在的条目；不存在的条目被跳过并返回。
    async fn add_entries(
        &self,
        tag_id: &TagId,
        entry_ids: &[EntryId],
    ) -> Result<HistoryTagBatchOutcome, HistoryTagStoreError>;

    /// 从仍存在的条目移除标签；不存在的条目被跳过并返回。
    async fn remove_entries(
        &self,
        tag_id: &TagId,
        entry_ids: &[EntryId],
    ) -> Result<HistoryTagBatchOutcome, HistoryTagStoreError>;

    /// 汇总一组条目中每个标签的携带数量。
    async fn summarize_entries(
        &self,
        entry_ids: &[EntryId],
    ) -> Result<HistoryEntryTagSummary, HistoryTagStoreError>;

    /// 把来源标签的关联并入目标标签并删除来源标签。
    async fn merge_tags(
        &self,
        source_tag_ids: &[TagId],
        target_tag_id: &TagId,
    ) -> Result<HistoryTagMergeOutcome, HistoryTagStoreError>;

    /// 删除标签及其全部关联。
    async fn delete_tag(
        &self,
        tag_id: &TagId,
    ) -> Result<HistoryTagDeleteOutcome, HistoryTagStoreError>;
}

/// 读取条目携带的历史标签 id，供搜索补齐结果与重建索引。
#[async_trait]
pub trait HistoryEntryTagReaderPort: Send + Sync {
    /// 解密一组条目的标签关联；只返回仍存在的标签。
    async fn entry_tags(
        &self,
        entry_ids: &[EntryId],
    ) -> Result<Vec<(EntryId, TagId)>, HistoryTagStoreError>;

    /// 全部标签 id（不透明行键，不需要解密）。
    async fn tag_ids(&self) -> Result<Vec<TagId>, HistoryTagStoreError>;
}

/// 读取一组条目的用户历史标签，供搜索投影补齐；未接入标签存储时为空。
pub(crate) async fn load_history_tag_ids(
    reader: Option<&dyn HistoryEntryTagReaderPort>,
    entry_ids: &[EntryId],
) -> Result<HashMap<EntryId, Vec<TagId>>, HistoryTagStoreError> {
    let mut tags: HashMap<EntryId, Vec<TagId>> = HashMap::new();
    let Some(reader) = reader else {
        return Ok(tags);
    };
    if entry_ids.is_empty() {
        return Ok(tags);
    }
    for (entry_id, tag_id) in reader.entry_tags(entry_ids).await? {
        tags.entry(entry_id).or_default().push(tag_id);
    }
    Ok(tags)
}
