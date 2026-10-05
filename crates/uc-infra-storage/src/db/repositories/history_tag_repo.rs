//! 历史标签持久化实现。
//!
//! 标签的名称与创建时间密封在 `history_tag.payload_ct`（AAD 绑定 tag_id）；每个带
//! 用户标签的条目的标签 id 集合密封在 `history_tag_assignment.tags_ct`（AAD 绑定
//! entry_id）。明文只剩随机 tag_id 行键与“该条目有一行关联”的存在性。加解密都在
//! 同步 SQLite 边界之外完成；写入在单个事务内提交，进程中断时只会看到操作前或操作
//! 后的状态。调用方负责串行化写操作，因此读取、改写与提交之间不会有并发的标签写入；
//! 期间被删除的条目在提交时被识别并作为缺失条目返回。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_types::{Binary, Text};
use diesel::SqliteConnection;
use serde::{Deserialize, Serialize};
use tokio::task::spawn_blocking;
use tracing::Span;
use uc_application::facade::clipboard_history::{
    HistoryEntryTagReaderPort, HistoryEntryTagSummary, HistoryTagBatchOutcome,
    HistoryTagDeleteOutcome, HistoryTagMergeOutcome, HistoryTagRecord, HistoryTagStoreError,
    HistoryTagStorePort,
};
use uc_core::clipboard::HistoryTagName;
use uc_core::crypto::domain::{Aad, Ciphertext, Plaintext};
use uc_core::ids::EntryId;
use uc_core::search::tag::TagId;
use uc_infra_security::{ContentProtection, ContentProtectionError};
use uc_observability_contract::{log_fields::log_id, uc_warn};

use crate::db::ports::DbExecutor;
use crate::db::schema::{clipboard_entry, history_tag, history_tag_assignment};

const TAG_AAD_PREFIX: &str = "uc:history_tag:v1|";
const ASSIGNMENT_AAD_PREFIX: &str = "uc:history_tag_assignment:v1|";

/// 标签行密封的内容。
#[derive(Serialize, Deserialize)]
struct TagPayload {
    name: String,
    created_at_ms: i64,
}

type Assignments = BTreeMap<String, BTreeSet<String>>;

/// 一次关联改写：`Some` 为新的密文，`None` 表示该条目已没有用户标签。
type AssignmentWrite = (String, Option<Vec<u8>>);

pub struct DieselHistoryTagRepository<E> {
    executor: Arc<E>,
    /// V3 profile 的内容保护；旧格式 profile 没有它，标签读写返回 `Unavailable`。
    protection: Option<Arc<ContentProtection>>,
}

impl<E> DieselHistoryTagRepository<E> {
    pub fn new_v3(executor: E, protection: Arc<ContentProtection>) -> Self {
        Self {
            executor: Arc::new(executor),
            protection: Some(protection),
        }
    }

    /// 旧格式 profile：没有可用的内容保护，不保存也不返回任何标签。
    pub fn new_legacy(executor: E) -> Self {
        Self {
            executor: Arc::new(executor),
            protection: None,
        }
    }

    fn protection(&self) -> Result<&ContentProtection, HistoryTagStoreError> {
        self.protection
            .as_deref()
            .ok_or(HistoryTagStoreError::Unavailable)
    }

    async fn seal(&self, aad: String, plaintext: Vec<u8>) -> Result<Vec<u8>, HistoryTagStoreError> {
        let protection = self.protection()?;
        let sealed = protection
            .seal_for_active(&Plaintext::new(plaintext), &Aad::new(aad.into_bytes()))
            .await
            .map_err(protection_error)?;
        Ok(sealed.into_bytes())
    }

    /// 打开一个密文；会话未就绪时整体失败，其余失败返回 `None` 由调用方降级。
    async fn open(
        &self,
        aad: String,
        ciphertext: &[u8],
    ) -> Result<Option<Vec<u8>>, HistoryTagStoreError> {
        let opened = self
            .protection()?
            .open(
                &Ciphertext::new(ciphertext.to_vec()),
                &Aad::new(aad.into_bytes()),
            )
            .await;
        match opened {
            Ok(plaintext) => Ok(Some(plaintext.into_bytes())),
            Err(error @ ContentProtectionError::NotActive { .. }) => Err(protection_error(error)),
            Err(_) => Ok(None),
        }
    }

    async fn seal_tag(
        &self,
        tag_id: &str,
        name: &HistoryTagName,
        created_at_ms: i64,
    ) -> Result<Vec<u8>, HistoryTagStoreError> {
        let payload = serde_json::to_vec(&TagPayload {
            name: name.as_str().to_string(),
            created_at_ms,
        })
        .map_err(storage)?;
        self.seal(format!("{TAG_AAD_PREFIX}{tag_id}"), payload)
            .await
    }

    /// 打开标签载荷；无法打开或内容无效时名称为 `None`、创建时间为 0。
    async fn open_tag(
        &self,
        tag_id: &str,
        ciphertext: &[u8],
    ) -> Result<(Option<HistoryTagName>, i64), HistoryTagStoreError> {
        let payload = self
            .open(format!("{TAG_AAD_PREFIX}{tag_id}"), ciphertext)
            .await?
            .and_then(|bytes| serde_json::from_slice::<TagPayload>(&bytes).ok());
        let Some(payload) = payload else {
            uc_warn!(
                tag_id = log_id(&tag_id),
                error_kind = "history_tag_payload_unreadable",
                "history tag payload could not be opened; returning it without a name"
            );
            return Ok((None, 0));
        };
        Ok((
            HistoryTagName::parse(&payload.name).ok(),
            payload.created_at_ms,
        ))
    }

    async fn seal_assignment(
        &self,
        entry_id: &str,
        tags: &BTreeSet<String>,
    ) -> Result<Option<Vec<u8>>, HistoryTagStoreError> {
        if tags.is_empty() {
            return Ok(None);
        }
        let payload = serde_json::to_vec(tags).map_err(storage)?;
        self.seal(format!("{ASSIGNMENT_AAD_PREFIX}{entry_id}"), payload)
            .await
            .map(Some)
    }

    /// 打开一组关联行；损坏的行按“没有用户标签”降级并记录日志。
    async fn open_assignments(
        &self,
        rows: Vec<(String, Vec<u8>)>,
    ) -> Result<Assignments, HistoryTagStoreError> {
        let mut assignments = Assignments::new();
        for (entry_id, ciphertext) in rows {
            let tags = self
                .open(format!("{ASSIGNMENT_AAD_PREFIX}{entry_id}"), &ciphertext)
                .await?
                .and_then(|bytes| serde_json::from_slice::<BTreeSet<String>>(&bytes).ok());
            match tags {
                Some(tags) => {
                    assignments.insert(entry_id, tags);
                }
                None => uc_warn!(
                    entry_id = log_id(&entry_id),
                    error_kind = "history_tag_assignment_unreadable",
                    "history tag assignment could not be opened; treating the entry as untagged"
                ),
            }
        }
        Ok(assignments)
    }
}

impl<E: DbExecutor + 'static> DieselHistoryTagRepository<E> {
    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut SqliteConnection) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T, HistoryTagStoreError> {
        let executor = Arc::clone(&self.executor);
        let span = Span::current();
        spawn_blocking(move || span.in_scope(|| executor.run(operation)))
            .await
            .map_err(storage)?
            .map_err(HistoryTagStoreError::Storage)
    }

    async fn known_tags(&self) -> Result<BTreeSet<String>, HistoryTagStoreError> {
        let tag_ids = self
            .run(|conn| {
                Ok(history_tag::table
                    .select(history_tag::tag_id)
                    .load::<String>(conn)?)
            })
            .await?;
        Ok(tag_ids.into_iter().collect())
    }

    /// 读取并打开关联；`entry_ids = None` 表示全部条目。只保留仍存在的标签。
    async fn assignments(
        &self,
        entry_ids: Option<Vec<String>>,
        known: &BTreeSet<String>,
    ) -> Result<Assignments, HistoryTagStoreError> {
        let rows = self
            .run(move |conn| {
                let query = history_tag_assignment::table
                    .select((
                        history_tag_assignment::entry_id,
                        history_tag_assignment::tags_ct,
                    ))
                    .into_boxed();
                let query = match entry_ids {
                    Some(ids) => query.filter(history_tag_assignment::entry_id.eq_any(ids)),
                    None => query,
                };
                Ok(query.load::<(String, Vec<u8>)>(conn)?)
            })
            .await?;
        let mut assignments = self.open_assignments(rows).await?;
        for tags in assignments.values_mut() {
            tags.retain(|tag| known.contains(tag));
        }
        Ok(assignments)
    }

    /// 在单个事务内提交关联改写并删除指定标签；返回提交时已不存在的条目。
    async fn commit(
        &self,
        writes: Vec<AssignmentWrite>,
        removed_tags: Vec<String>,
    ) -> Result<BTreeSet<String>, HistoryTagStoreError> {
        self.run(move |conn| {
            conn.immediate_transaction(|conn| {
                let mut vanished = BTreeSet::new();
                for (entry_id, ciphertext) in &writes {
                    match ciphertext {
                        Some(ciphertext) => {
                            let written = diesel::sql_query(
                                "INSERT INTO history_tag_assignment (entry_id, tags_ct) \
                                 SELECT ?, ? WHERE EXISTS \
                                 (SELECT 1 FROM clipboard_entry WHERE entry_id = ?) \
                                 ON CONFLICT (entry_id) DO UPDATE SET tags_ct = excluded.tags_ct",
                            )
                            .bind::<Text, _>(entry_id)
                            .bind::<Binary, _>(ciphertext)
                            .bind::<Text, _>(entry_id)
                            .execute(conn)?;
                            if written == 0 {
                                vanished.insert(entry_id.clone());
                            }
                        }
                        None => {
                            diesel::delete(
                                history_tag_assignment::table
                                    .filter(history_tag_assignment::entry_id.eq(entry_id)),
                            )
                            .execute(conn)?;
                        }
                    }
                }
                if !removed_tags.is_empty() {
                    diesel::delete(
                        history_tag::table.filter(history_tag::tag_id.eq_any(&removed_tags)),
                    )
                    .execute(conn)?;
                }
                Ok(vanished)
            })
        })
        .await
    }

    /// 按输入顺序去重后，分出仍存在与已不存在的条目。
    async fn partition_entries(
        &self,
        entry_ids: &[EntryId],
    ) -> Result<(Vec<String>, Vec<String>), HistoryTagStoreError> {
        let requested: Vec<String> = entry_ids.iter().map(ToString::to_string).collect();
        let lookup = requested.clone();
        let existing: BTreeSet<String> = self
            .run(move |conn| {
                Ok(clipboard_entry::table
                    .filter(clipboard_entry::entry_id.eq_any(lookup))
                    .select(clipboard_entry::entry_id)
                    .load::<String>(conn)?
                    .into_iter()
                    .collect())
            })
            .await?;
        let mut seen = BTreeSet::new();
        let (mut present, mut missing) = (Vec::new(), Vec::new());
        for entry_id in requested {
            if !seen.insert(entry_id.clone()) {
                continue;
            }
            if existing.contains(&entry_id) {
                present.push(entry_id);
            } else {
                missing.push(entry_id);
            }
        }
        Ok((present, missing))
    }

    /// 对一组条目的标签集合做同一种修改并提交，返回批量结果。
    async fn change_entries(
        &self,
        tag_id: &TagId,
        entry_ids: &[EntryId],
        modify: impl Fn(&mut BTreeSet<String>, &str) -> bool,
    ) -> Result<HistoryTagBatchOutcome, HistoryTagStoreError> {
        self.protection()?;
        let known = self.known_tags().await?;
        let tag = tag_id.to_string();
        if !known.contains(&tag) {
            return Err(HistoryTagStoreError::TagNotFound);
        }
        let (present, mut missing) = self.partition_entries(entry_ids).await?;
        let mut assignments = self.assignments(Some(present.clone()), &known).await?;
        let mut writes = Vec::new();
        let mut changed = BTreeMap::new();
        for entry_id in &present {
            let tags = assignments.entry(entry_id.clone()).or_default();
            if modify(tags, &tag) {
                writes.push((
                    entry_id.clone(),
                    self.seal_assignment(entry_id, tags).await?,
                ));
                changed.insert(entry_id.clone(), tags.clone());
            }
        }
        let vanished = self.commit(writes, Vec::new()).await?;
        changed.retain(|entry_id, _| !vanished.contains(entry_id));
        missing.extend(vanished.iter().cloned());
        let unchanged = present.len() - changed.len() - vanished.len();
        Ok(HistoryTagBatchOutcome {
            changed: count(changed.len()),
            unchanged: count(unchanged),
            missing_entry_ids: missing.into_iter().map(EntryId::from).collect(),
            changed_entries: changed_entries(changed),
        })
    }
}

fn protection_error(error: ContentProtectionError) -> HistoryTagStoreError {
    match error {
        ContentProtectionError::NotActive { .. } => HistoryTagStoreError::Locked,
        other => HistoryTagStoreError::Storage(other.into()),
    }
}

fn storage(error: impl Into<anyhow::Error>) -> HistoryTagStoreError {
    HistoryTagStoreError::Storage(error.into())
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn changed_entries(changed: BTreeMap<String, BTreeSet<String>>) -> Vec<(EntryId, Vec<TagId>)> {
    changed
        .into_iter()
        .map(|(entry_id, tags)| {
            (
                EntryId::from(entry_id),
                tags.into_iter().map(TagId::new).collect(),
            )
        })
        .collect()
}

#[async_trait]
impl<E: DbExecutor + 'static> HistoryTagStorePort for DieselHistoryTagRepository<E> {
    async fn list_tags(&self) -> Result<Vec<HistoryTagRecord>, HistoryTagStoreError> {
        self.protection()?;
        let rows = self
            .run(|conn| {
                Ok(history_tag::table
                    .select((history_tag::tag_id, history_tag::payload_ct))
                    .load::<(String, Vec<u8>)>(conn)?)
            })
            .await?;
        let known: BTreeSet<String> = rows.iter().map(|(tag_id, _)| tag_id.clone()).collect();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for tags in self.assignments(None, &known).await?.into_values() {
            for tag in tags {
                *counts.entry(tag).or_default() += 1;
            }
        }
        let mut records = Vec::with_capacity(rows.len());
        for (tag_id, payload_ct) in rows {
            let (name, created_at_ms) = self.open_tag(&tag_id, &payload_ct).await?;
            records.push(HistoryTagRecord {
                entry_count: count(counts.get(&tag_id).copied().unwrap_or(0)),
                tag_id: TagId::new(tag_id),
                name,
                created_at_ms,
            });
        }
        Ok(records)
    }

    async fn create_tag(
        &self,
        tag_id: &TagId,
        name: &HistoryTagName,
        created_at_ms: i64,
    ) -> Result<(), HistoryTagStoreError> {
        let tag_id = tag_id.to_string();
        let payload_ct = self.seal_tag(&tag_id, name, created_at_ms).await?;
        self.run(move |conn| {
            diesel::insert_into(history_tag::table)
                .values((
                    history_tag::tag_id.eq(&tag_id),
                    history_tag::payload_ct.eq(&payload_ct),
                ))
                .execute(conn)?;
            Ok(())
        })
        .await
    }

    async fn rename_tag(
        &self,
        tag_id: &TagId,
        name: &HistoryTagName,
    ) -> Result<(), HistoryTagStoreError> {
        self.protection()?;
        let tag_id = tag_id.to_string();
        let lookup = tag_id.clone();
        let current = self
            .run(move |conn| {
                Ok(history_tag::table
                    .filter(history_tag::tag_id.eq(&lookup))
                    .select(history_tag::payload_ct)
                    .first::<Vec<u8>>(conn)
                    .optional()?)
            })
            .await?
            .ok_or(HistoryTagStoreError::TagNotFound)?;
        let (_, created_at_ms) = self.open_tag(&tag_id, &current).await?;
        let payload_ct = self.seal_tag(&tag_id, name, created_at_ms).await?;
        let updated = self
            .run(move |conn| {
                Ok(
                    diesel::update(history_tag::table.filter(history_tag::tag_id.eq(&tag_id)))
                        .set(history_tag::payload_ct.eq(&payload_ct))
                        .execute(conn)?,
                )
            })
            .await?;
        if updated == 0 {
            return Err(HistoryTagStoreError::TagNotFound);
        }
        Ok(())
    }

    async fn add_entries(
        &self,
        tag_id: &TagId,
        entry_ids: &[EntryId],
    ) -> Result<HistoryTagBatchOutcome, HistoryTagStoreError> {
        let attach = |tags: &mut BTreeSet<String>, tag: &str| tags.insert(tag.to_string());
        self.change_entries(tag_id, entry_ids, attach).await
    }

    async fn remove_entries(
        &self,
        tag_id: &TagId,
        entry_ids: &[EntryId],
    ) -> Result<HistoryTagBatchOutcome, HistoryTagStoreError> {
        let detach = |tags: &mut BTreeSet<String>, tag: &str| tags.remove(tag);
        self.change_entries(tag_id, entry_ids, detach).await
    }

    async fn summarize_entries(
        &self,
        entry_ids: &[EntryId],
    ) -> Result<HistoryEntryTagSummary, HistoryTagStoreError> {
        self.protection()?;
        let known = self.known_tags().await?;
        let (present, _) = self.partition_entries(entry_ids).await?;
        let mut applied: BTreeMap<String, usize> = BTreeMap::new();
        for tags in self
            .assignments(Some(present.clone()), &known)
            .await?
            .into_values()
        {
            for tag in tags {
                *applied.entry(tag).or_default() += 1;
            }
        }
        Ok(HistoryEntryTagSummary {
            selected: count(present.len()),
            tags: applied
                .into_iter()
                .map(|(tag_id, applied)| (TagId::new(tag_id), count(applied)))
                .collect(),
        })
    }

    async fn merge_tags(
        &self,
        source_tag_ids: &[TagId],
        target_tag_id: &TagId,
    ) -> Result<HistoryTagMergeOutcome, HistoryTagStoreError> {
        self.protection()?;
        let known = self.known_tags().await?;
        let sources: BTreeSet<String> = source_tag_ids.iter().map(ToString::to_string).collect();
        let target = target_tag_id.to_string();
        if !known.contains(&target) || !sources.iter().all(|source| known.contains(source)) {
            return Err(HistoryTagStoreError::TagNotFound);
        }
        let mut writes = Vec::new();
        let mut changed = BTreeMap::new();
        let (mut moved, mut already_on_target) = (0, 0);
        for (entry_id, mut tags) in self.assignments(None, &known).await? {
            if tags.is_disjoint(&sources) {
                continue;
            }
            tags.retain(|tag| !sources.contains(tag));
            if tags.insert(target.clone()) {
                moved += 1;
            } else {
                already_on_target += 1;
            }
            writes.push((
                entry_id.clone(),
                self.seal_assignment(&entry_id, &tags).await?,
            ));
            changed.insert(entry_id, tags);
        }
        let vanished = self.commit(writes, sources.into_iter().collect()).await?;
        changed.retain(|entry_id, _| !vanished.contains(entry_id));
        Ok(HistoryTagMergeOutcome {
            moved: count(moved),
            already_on_target: count(already_on_target),
            changed_entries: changed_entries(changed),
        })
    }

    async fn delete_tag(
        &self,
        tag_id: &TagId,
    ) -> Result<HistoryTagDeleteOutcome, HistoryTagStoreError> {
        self.protection()?;
        let known = self.known_tags().await?;
        let tag = tag_id.to_string();
        if !known.contains(&tag) {
            return Err(HistoryTagStoreError::TagNotFound);
        }
        let mut writes = Vec::new();
        let mut changed = BTreeMap::new();
        for (entry_id, mut tags) in self.assignments(None, &known).await? {
            if tags.remove(&tag) {
                writes.push((
                    entry_id.clone(),
                    self.seal_assignment(&entry_id, &tags).await?,
                ));
                changed.insert(entry_id, tags);
            }
        }
        let detached = count(changed.len());
        let vanished = self.commit(writes, vec![tag]).await?;
        changed.retain(|entry_id, _| !vanished.contains(entry_id));
        Ok(HistoryTagDeleteOutcome {
            detached,
            changed_entries: changed_entries(changed),
        })
    }
}

#[async_trait]
impl<E: DbExecutor + 'static> HistoryEntryTagReaderPort for DieselHistoryTagRepository<E> {
    async fn entry_tags(
        &self,
        entry_ids: &[EntryId],
    ) -> Result<Vec<(EntryId, TagId)>, HistoryTagStoreError> {
        // 旧格式 profile 没有用户标签。
        if self.protection.is_none() || entry_ids.is_empty() {
            return Ok(Vec::new());
        }
        let known = self.known_tags().await?;
        if known.is_empty() {
            return Ok(Vec::new());
        }
        let requested = entry_ids.iter().map(ToString::to_string).collect();
        Ok(self
            .assignments(Some(requested), &known)
            .await?
            .into_iter()
            .flat_map(|(entry_id, tags)| {
                let entry_id = EntryId::from(entry_id);
                tags.into_iter()
                    .map(move |tag| (entry_id.clone(), TagId::new(tag)))
            })
            .collect())
    }

    async fn tag_ids(&self) -> Result<Vec<TagId>, HistoryTagStoreError> {
        Ok(self
            .known_tags()
            .await?
            .into_iter()
            .map(TagId::new)
            .collect())
    }
}
