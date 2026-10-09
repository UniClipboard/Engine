//! Use case that executes a structured search query against the local
//! encrypted search index.

use std::sync::Arc;
use uc_core::ports::SearchIndexPort;
use uc_core::search::tag::SearchTagCount;
use uc_core::search::{SearchError, SearchQuery, SearchResultsPage};
use uc_observability_contract::uc_debug;

pub(crate) struct SearchClipboardEntriesUseCase {
    search_index: Arc<dyn SearchIndexPort>,
}

impl SearchClipboardEntriesUseCase {
    pub(crate) fn from_port(search_index: Arc<dyn SearchIndexPort>) -> Self {
        Self { search_index }
    }

    #[tracing::instrument(
        name = "usecase.search_clipboard_entries.execute",
        skip(self, query),
        fields(
            query_len = query.query_string.len(),
            operator = ?query.operator,
            limit = query.limit,
            offset = query.offset
        )
    )]
    pub(crate) async fn execute(
        &self,
        query: SearchQuery,
    ) -> Result<SearchResultsPage, SearchError> {
        let page = self.search_index.search(query).await?;
        uc_debug!(
            total = page.total,
            returned = page.items.len(),
            has_more = page.has_more,
            "search completed"
        );
        Ok(page)
    }

    /// 与 `execute` 同源的匹配数；不取行、不解密渲染字段。
    pub(crate) async fn count(&self, query: SearchQuery) -> Result<u32, SearchError> {
        self.search_index.count(query).await
    }

    pub(crate) async fn count_by_active_time(
        &self,
        boundaries_ms: &[i64],
    ) -> Result<Vec<u32>, SearchError> {
        self.search_index.count_by_active_time(boundaries_ms).await
    }

    pub(crate) async fn list_tags(&self) -> Result<Vec<SearchTagCount>, SearchError> {
        self.search_index.list_tags().await
    }
}
