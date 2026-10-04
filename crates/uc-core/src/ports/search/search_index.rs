//! SearchIndexPort——由 `uc-infra-storage` 实现的异步 trait（Phase 91）。
//!
//! All methods return Result<_, SearchError> to preserve typed error info
//! across the port boundary (per D-03, D-04, D-05). Infra adapters may use
//! anyhow::Error internally but MUST map to SearchError at method return.

use crate::ids::EntryId;
use crate::search::tag::SearchTagCount;
use crate::search::{
    RebuildProgress, SearchDocument, SearchError, SearchIndexMeta, SearchPosting, SearchQuery,
    SearchResultsPage,
};
use async_trait::async_trait;
use tokio::sync::mpsc::Sender;

/// Port for indexing and querying the local encrypted search index.
///
/// 由 `uc-infra-storage` 实现（Phase 91），以 `Arc<dyn SearchIndexPort + Send + Sync>`
/// 注入用例与 daemon 状态。
#[async_trait]
pub trait SearchIndexPort: Send + Sync {
    /// Index (insert or replace) a clipboard entry's document and its postings.
    ///
    /// Called by IndexClipboardEntry use case (Phase 89) after capture and persistence.
    /// If a document for `entry_id` already exists, it is replaced atomically.
    async fn index_entry(
        &self,
        document: SearchDocument,
        postings: Vec<SearchPosting>,
    ) -> Result<(), SearchError>;

    /// Remove a clipboard entry from the search index (document + all postings).
    ///
    /// Called synchronously by DeleteClipboardEntry (Phase 89) — hard-delete.
    async fn remove_entry(&self, entry_id: &EntryId) -> Result<(), SearchError>;

    /// Execute a structured query and return a paged result with full render metadata.
    ///
    /// Returns `SearchResultsPage` (not `Vec<EntryId>`) per D-01 / D-02 — avoids a second
    /// query in the route layer to hydrate UI row metadata or compute pagination truth.
    async fn search(&self, query: SearchQuery) -> Result<SearchResultsPage, SearchError>;

    /// Full index rebuild from a supplied entry list.
    ///
    /// Uses a channel to emit `RebuildProgress` so the daemon can forward events
    /// over WebSocket without uc-core knowing about WS (D-07).
    /// Phase 91 implements version-flag atomic swap inside.
    async fn rebuild(
        &self,
        entries: Vec<(SearchDocument, Vec<SearchPosting>)>,
        progress_tx: Sender<RebuildProgress>,
    ) -> Result<(), SearchError>;

    /// Read-only projection of search_index_meta (index_version, search_blocked, timestamps).
    async fn get_index_meta(&self) -> Result<SearchIndexMeta, SearchError>;

    /// Mirror an entry's favorited user-state into its tag membership.
    ///
    /// `favorited = true` records the builtin favorited tag for the entry;
    /// `false` removes it. Idempotent: repeating the same value is a no-op. The
    /// favorited tag's authoritative source is the entry's user-state held
    /// elsewhere; this keeps the derived membership consistent with it without a
    /// full re-index. Adapters that do not maintain tag membership keep the
    /// default no-op.
    async fn set_entry_favorite_tag(
        &self,
        entry_id: &EntryId,
        favorited: bool,
    ) -> Result<(), SearchError> {
        let _ = (entry_id, favorited);
        Ok(())
    }

    /// List every tag present in the index with the count of entries carrying
    /// it. Filter-only over the membership table: it needs no search key and is
    /// available while the session is locked. Lock-based visibility of custom
    /// tags is the caller's responsibility (§4.6).
    async fn list_tags(&self) -> Result<Vec<SearchTagCount>, SearchError> {
        Ok(Vec::new())
    }

    /// 统计与 [`search`](Self::search) 过滤语义完全一致的匹配条目数。
    ///
    /// 默认实现取零条数的一页的 `total`，因此关键词、标签“或/且”、时间、类型、来源等
    /// 语义与会话锁定行为都与 `search` 同源，不会分叉。分页字段被忽略。
    async fn count(&self, query: SearchQuery) -> Result<u32, SearchError> {
        let page = self
            .search(SearchQuery {
                limit: 0,
                offset: 0,
                ..query
            })
            .await?;
        Ok(page.total)
    }

    /// 按活跃时间分桶统计条目数。
    ///
    /// `boundaries_ms` 是严格递增的桶边界，第 `i` 个桶为 `[boundaries[i], boundaries[i+1])`，
    /// 返回长度为 `boundaries.len() - 1`。边界本身已经是绝对时间戳，因此本契约不含时区与
    /// 夏令时规则。会话锁定时与 `search` 一样返回 `SessionLocked`。
    /// 没有默认实现：包装适配器必须显式转发，不能悄悄退化成“不可用”。
    async fn count_by_active_time(&self, boundaries_ms: &[i64]) -> Result<Vec<u32>, SearchError>;
}
