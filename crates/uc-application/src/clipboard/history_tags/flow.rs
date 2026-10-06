//! 历史标签流程负责人：名称规则、同名判定、批量与合并的输入约束及结果整理。
//!
//! 名称与关联只以密文持久化：同名判断必须在解密后完成，关联改写需要先解密再
//! 重新密封。因此同一 profile 的全部标签写入经一个异步互斥串行执行，避免并发请求
//! 各自认为名称可用或覆盖彼此的关联。
//!
//! 权威存储提交后，流程把发生变化的条目的完整标签集合写入搜索索引（只以搜索密钥
//! HMAC 保存）。索引写入失败不回滚已提交的标签，索引重建会从权威存储重新派生。

use std::collections::BTreeSet;
use std::sync::Arc;

use tokio::sync::Mutex;
use uc_core::clipboard::{
    HistoryTagName, HISTORY_TAG_MAX_BATCH_ENTRIES, HISTORY_TAG_MAX_COUNT,
    HISTORY_TAG_MAX_MERGE_SOURCES,
};
use uc_core::ids::EntryId;
use uc_core::ports::search::search_index::SearchIndexPort;
use uc_core::ports::ClockPort;
use uc_core::search::tag::TagId;
use uc_observability_contract::{
    error_source::io_error_kind, log_fields::log_id, uc_info, uc_warn,
};

use super::store::{
    HistoryTagBatchOutcome, HistoryTagRecord, HistoryTagStoreError, HistoryTagStorePort,
};
use super::views::{
    HistoryEntryTagSummaryView, HistoryTagApplicationView, HistoryTagBatchView,
    HistoryTagCreatedView, HistoryTagError, HistoryTagMergeView, HistoryTagRenameView,
    HistoryTagView,
};

pub(crate) struct HistoryTags {
    store: Arc<dyn HistoryTagStorePort>,
    clock: Arc<dyn ClockPort>,
    /// 搜索索引中的用户标签成员；未接入索引的装配场景为 `None`。
    search_index: Option<Arc<dyn SearchIndexPort>>,
    /// 串行化全部标签写入。
    writes: Mutex<()>,
}

impl HistoryTags {
    pub(crate) fn new(
        store: Arc<dyn HistoryTagStorePort>,
        clock: Arc<dyn ClockPort>,
        search_index: Option<Arc<dyn SearchIndexPort>>,
    ) -> Self {
        Self {
            store,
            clock,
            search_index,
            writes: Mutex::new(()),
        }
    }

    /// 把条目变化后的完整标签集合写入搜索索引。失败只记录日志：权威存储已提交，
    /// 索引重建会补齐。
    async fn mirror_into_index(&self, changed_entries: &[(EntryId, Vec<TagId>)]) {
        let Some(search_index) = &self.search_index else {
            return;
        };
        for (entry_id, tag_ids) in changed_entries {
            if let Err(error) = search_index.set_entry_history_tags(entry_id, tag_ids).await {
                uc_warn!(
                    entry_id = log_id(&entry_id),
                    error_kind = "search_history_tags",
                    io_error_kind = io_error_kind(&error),
                    "history tags persisted but search index update failed; rebuild will reconcile"
                );
            }
        }
    }

    /// 列出全部标签：按关联条目数降序，再按名称比较键与 id 排序。
    pub(crate) async fn list(&self) -> Result<Vec<HistoryTagView>, HistoryTagError> {
        let mut records = self.store.list_tags().await.map_err(store_error)?;
        records.sort_by(|a, b| {
            b.entry_count
                .cmp(&a.entry_count)
                .then_with(|| name_key(a).cmp(&name_key(b)))
                .then_with(|| a.tag_id.as_str().cmp(b.tag_id.as_str()))
        });
        Ok(records.into_iter().map(tag_view).collect())
    }

    /// 按名称创建标签；同名标签已存在时返回它。
    pub(crate) async fn create(
        &self,
        raw_name: &str,
    ) -> Result<HistoryTagCreatedView, HistoryTagError> {
        let name = HistoryTagName::parse(raw_name).map_err(HistoryTagError::InvalidName)?;
        let _writes = self.writes.lock().await;
        let records = self.store.list_tags().await.map_err(store_error)?;
        if let Some(existing) = records
            .iter()
            .find(|record| same_name(record, &name))
            .cloned()
        {
            return Ok(HistoryTagCreatedView {
                tag: tag_view(existing),
                created: false,
            });
        }
        if records.len() >= HISTORY_TAG_MAX_COUNT {
            return Err(HistoryTagError::InvalidInput);
        }
        let tag_id = TagId::new(uuid::Uuid::new_v4().to_string());
        let created_at_ms = self.clock.now_ms();
        self.store
            .create_tag(&tag_id, &name, created_at_ms)
            .await
            .map_err(store_error)?;
        uc_info!(tag_id = log_id(&tag_id), "History tag created");
        Ok(HistoryTagCreatedView {
            tag: tag_view(HistoryTagRecord {
                tag_id,
                name: Some(name),
                created_at_ms,
                entry_count: 0,
            }),
            created: true,
        })
    }

    /// 改名；与另一个标签同名时不写入并返回冲突标签。
    pub(crate) async fn rename(
        &self,
        tag_id: &str,
        raw_name: &str,
    ) -> Result<HistoryTagRenameView, HistoryTagError> {
        let name = HistoryTagName::parse(raw_name).map_err(HistoryTagError::InvalidName)?;
        let tag_id = TagId::new(tag_id);
        let _writes = self.writes.lock().await;
        let records = self.store.list_tags().await.map_err(store_error)?;
        let Some(current) = records
            .iter()
            .find(|record| record.tag_id == tag_id)
            .cloned()
        else {
            return Err(HistoryTagError::NotFound);
        };
        if let Some(conflict) = records
            .iter()
            .find(|record| record.tag_id != tag_id && same_name(record, &name))
        {
            return Ok(HistoryTagRenameView::NameConflict {
                existing_tag_id: conflict.tag_id.to_string(),
            });
        }
        self.store
            .rename_tag(&tag_id, &name)
            .await
            .map_err(store_error)?;
        uc_info!(tag_id = log_id(&tag_id), "History tag renamed");
        Ok(HistoryTagRenameView::Renamed(tag_view(HistoryTagRecord {
            name: Some(name),
            ..current
        })))
    }

    pub(crate) async fn add_to_entries(
        &self,
        tag_id: &str,
        entry_ids: &[String],
    ) -> Result<HistoryTagBatchView, HistoryTagError> {
        let entry_ids = batch_entries(entry_ids)?;
        let tag_id = TagId::new(tag_id);
        let _writes = self.writes.lock().await;
        let outcome = self
            .store
            .add_entries(&tag_id, &entry_ids)
            .await
            .map_err(store_error)?;
        self.mirror_into_index(&outcome.changed_entries).await;
        uc_info!(
            tag_id = log_id(&tag_id),
            count = outcome.changed,
            missing_count = outcome.missing_entry_ids.len(),
            "History tag added to entries"
        );
        Ok(batch_view(outcome))
    }

    pub(crate) async fn remove_from_entries(
        &self,
        tag_id: &str,
        entry_ids: &[String],
    ) -> Result<HistoryTagBatchView, HistoryTagError> {
        let entry_ids = batch_entries(entry_ids)?;
        let tag_id = TagId::new(tag_id);
        let _writes = self.writes.lock().await;
        let outcome = self
            .store
            .remove_entries(&tag_id, &entry_ids)
            .await
            .map_err(store_error)?;
        self.mirror_into_index(&outcome.changed_entries).await;
        uc_info!(
            tag_id = log_id(&tag_id),
            count = outcome.changed,
            missing_count = outcome.missing_entry_ids.len(),
            "History tag removed from entries"
        );
        Ok(batch_view(outcome))
    }

    pub(crate) async fn summarize_entries(
        &self,
        entry_ids: &[String],
    ) -> Result<HistoryEntryTagSummaryView, HistoryTagError> {
        let entry_ids = batch_entries(entry_ids)?;
        let summary = self
            .store
            .summarize_entries(&entry_ids)
            .await
            .map_err(store_error)?;
        Ok(HistoryEntryTagSummaryView {
            selected: summary.selected,
            tags: summary
                .tags
                .into_iter()
                .map(|(tag_id, applied)| HistoryTagApplicationView {
                    tag_id: tag_id.to_string(),
                    applied,
                })
                .collect(),
        })
    }

    /// 把来源标签并入目标标签：关联取并集，来源标签删除，条目内容不变。
    pub(crate) async fn merge(
        &self,
        source_tag_ids: &[String],
        target_tag_id: &str,
    ) -> Result<HistoryTagMergeView, HistoryTagError> {
        let sources: BTreeSet<&str> = source_tag_ids.iter().map(String::as_str).collect();
        if sources.is_empty()
            || sources.len() > HISTORY_TAG_MAX_MERGE_SOURCES
            || sources.contains(target_tag_id)
        {
            return Err(HistoryTagError::InvalidInput);
        }
        let sources: Vec<TagId> = sources.into_iter().map(TagId::new).collect();
        let target = TagId::new(target_tag_id);
        let _writes = self.writes.lock().await;
        let outcome = self
            .store
            .merge_tags(&sources, &target)
            .await
            .map_err(store_error)?;
        self.mirror_into_index(&outcome.changed_entries).await;
        uc_info!(
            tag_id = log_id(&target),
            count = outcome.moved,
            "History tags merged"
        );
        Ok(HistoryTagMergeView {
            moved: outcome.moved,
            already_on_target: outcome.already_on_target,
        })
    }

    /// 删除标签及其关联，返回解除的关联数；条目内容不变。
    pub(crate) async fn delete(&self, tag_id: &str) -> Result<u32, HistoryTagError> {
        let tag_id = TagId::new(tag_id);
        let _writes = self.writes.lock().await;
        let outcome = self.store.delete_tag(&tag_id).await.map_err(store_error)?;
        self.mirror_into_index(&outcome.changed_entries).await;
        uc_info!(
            tag_id = log_id(&tag_id),
            count = outcome.detached,
            "History tag deleted"
        );
        Ok(outcome.detached)
    }
}

fn batch_entries(entry_ids: &[String]) -> Result<Vec<EntryId>, HistoryTagError> {
    if entry_ids.is_empty() || entry_ids.len() > HISTORY_TAG_MAX_BATCH_ENTRIES {
        return Err(HistoryTagError::InvalidInput);
    }
    Ok(entry_ids
        .iter()
        .map(|entry_id| EntryId::from(entry_id.as_str()))
        .collect())
}

fn same_name(record: &HistoryTagRecord, name: &HistoryTagName) -> bool {
    record
        .name
        .as_ref()
        .is_some_and(|existing| existing.identity_key() == name.identity_key())
}

/// 排序键：无法解密的名称排在同计数标签之后。
fn name_key(record: &HistoryTagRecord) -> (bool, &str) {
    match &record.name {
        Some(name) => (false, name.identity_key()),
        None => (true, ""),
    }
}

fn tag_view(record: HistoryTagRecord) -> HistoryTagView {
    HistoryTagView {
        tag_id: record.tag_id.to_string(),
        name: record.name.map(HistoryTagName::into_string),
        created_at_ms: record.created_at_ms,
        entry_count: record.entry_count,
    }
}

fn batch_view(outcome: HistoryTagBatchOutcome) -> HistoryTagBatchView {
    HistoryTagBatchView {
        changed: outcome.changed,
        unchanged: outcome.unchanged,
        missing_entry_ids: outcome
            .missing_entry_ids
            .into_iter()
            .map(|entry_id| entry_id.to_string())
            .collect(),
    }
}

fn store_error(error: HistoryTagStoreError) -> HistoryTagError {
    match error {
        HistoryTagStoreError::TagNotFound => HistoryTagError::NotFound,
        HistoryTagStoreError::Unavailable => HistoryTagError::Unavailable,
        HistoryTagStoreError::Locked => HistoryTagError::Locked,
        HistoryTagStoreError::Storage(_) => HistoryTagError::Internal(error.into()),
    }
}
