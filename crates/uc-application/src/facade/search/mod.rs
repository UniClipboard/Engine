use std::sync::Arc;

use thiserror::Error;

mod coordinator;
mod projection;
mod runtime;

use uc_core::ids::DeviceId;
use uc_core::ports::SearchIndexPort;
use uc_core::search::tag::TagId;
use uc_core::search::{ContentType, QueryOperator, SearchError, SearchQuery, TimeRangeFilter};

use crate::usecases::search::SearchClipboardEntriesUseCase;

use coordinator::{ManualRebuildResult, SearchCoordinator};
pub use coordinator::{
    SearchCoordinatorDeps as SearchRuntimeDeps, SearchRebuildProgressView, SearchStatusSnapshot,
};
pub use projection::SearchProjectionBuilder;
pub use runtime::{SearchRuntime, SearchRuntimeError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQueryInput {
    pub query: String,
    pub operator: Option<String>,
    pub time_preset: Option<String>,
    pub from_ms: Option<i64>,
    pub to_ms: Option<i64>,
    pub content_types: Option<String>,
    pub extensions: Option<String>,
    /// Comma-separated source device ids; restricts results to those origins.
    pub source_devices: Option<String>,
    /// Comma-separated tag ids (e.g. `link,favorited`); restricts to entries
    /// carrying any of them.
    pub tags: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

/// Response freshness for a `query()` page: the index served the page.
pub const SEARCH_STATE_READY: &str = "ready";
/// The index was not ready and this filter-less browse was served from the main
/// store instead (§4.7).
pub const SEARCH_STATE_DEGRADED: &str = "degraded";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPageView {
    pub total: u32,
    pub has_more: bool,
    pub items: Vec<SearchResultView>,
    /// [`SEARCH_STATE_READY`] when served from the index, or
    /// [`SEARCH_STATE_DEGRADED`] when the index was not ready and this filter-less
    /// browse was served from the main store (§4.7).
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResultView {
    pub entry_id: String,
    pub content_type: String,
    pub active_time_ms: i64,
    /// Tag ids as transparent strings (e.g. `"link"`, `"favorited"`).
    pub tags: Vec<String>,
    pub text_preview: Option<String>,
    /// Full character count of the entry's primary text content, so the UI can
    /// show the real total length instead of the capped preview length. `None`
    /// for entries with no inline text.
    pub char_count: Option<i64>,
    pub mime_type: String,
    pub file_extensions: Vec<String>,
    pub file_names: Vec<String>,
    /// Local filesystem paths of referenced files, aligned with `file_names` by
    /// index; empty when none.
    pub file_paths: Vec<String>,
    pub link_urls: Vec<String>,
    pub source_device: Option<String>,
    pub payload_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchStatusView {
    pub state: String,
    pub reason: Option<String>,
    pub last_rebuild_started_at_ms: Option<i64>,
    pub last_rebuild_completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRebuildAcceptedView {
    pub accepted: bool,
}

/// A tag and its entry count, plus whether it is a builtin (always visible) or a
/// custom tag (hidden while the session is locked — gating is applied by the
/// caller).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTagView {
    pub tag_id: String,
    pub count: u32,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum SearchFacadeError {
    #[error("invalid query: {0}")]
    InvalidQuery(String),
    #[error("bad search request: {0}")]
    BadRequest(String),
    #[error("search session is locked")]
    SessionLocked,
    #[error("search index is not ready")]
    IndexNotReady,
    /// The index is not ready and the request carried a keyword or filter, so it
    /// cannot be served from the main-store browse fallback (§4.7). A filter-less
    /// browse degrades to a 200 instead; this is the non-browse counterpart.
    #[error("search index is rebuilding")]
    IndexRebuilding,
    #[error("search index is unavailable")]
    IndexUnavailable,
    #[error("search service is unavailable: {0}")]
    ServiceUnavailable(String),
    #[error("search rebuild is already running")]
    RebuildAlreadyRunning,
    #[error("search failed: {0}")]
    Internal(String),
}

pub struct SearchFacade {
    query_uc: SearchClipboardEntriesUseCase,
    coordinator: Arc<SearchCoordinator>,
}

impl SearchFacade {
    fn with_runtime(
        search_index: Arc<dyn SearchIndexPort>,
        coordinator: Arc<SearchCoordinator>,
    ) -> Self {
        Self {
            query_uc: SearchClipboardEntriesUseCase::from_port(search_index),
            coordinator,
        }
    }

    pub async fn query(
        &self,
        input: SearchQueryInput,
    ) -> Result<SearchPageView, SearchFacadeError> {
        let query = parse_search_query(input)?;
        // Captured before `query` is moved into the index search: decides whether
        // an unavailable index can degrade to a main-store browse (§4.7).
        let pure_browse = is_pure_browse(&query);
        let limit = query.limit as usize;
        let offset = query.offset as usize;

        match self.query_uc.execute(query).await {
            Ok(page) => {
                // Rows whose render payload failed to decode come back blanked;
                // hand their ids to the coordinator for a coalesced re-projection
                // repair. Non-blocking and best-effort.
                if !page.corrupted_entry_ids.is_empty() {
                    self.coordinator
                        .schedule_repair(page.corrupted_entry_ids.clone());
                }
                Ok(search_page_to_view(page, SEARCH_STATE_READY))
            }
            // §4.7: a filter-less browse degrades to a direct main-store read so
            // the user keeps browsing during a rebuild; a keyword or filtered
            // query instead surfaces a stable rebuilding error.
            Err(SearchError::IndexNotReady) if pure_browse => {
                let page = self
                    .coordinator
                    .browse_projection(limit, offset)
                    .await
                    .map_err(map_search_error)?;
                Ok(search_page_to_view(page, SEARCH_STATE_DEGRADED))
            }
            Err(SearchError::IndexNotReady) => Err(SearchFacadeError::IndexRebuilding),
            Err(other) => Err(map_search_error(other)),
        }
    }

    /// List the tags present in the index with their entry counts. Returns both
    /// builtin and custom tags; the caller applies lock-based visibility (custom
    /// tags are hidden while the session is locked, §4.6).
    pub async fn tags(&self) -> Result<Vec<SearchTagView>, SearchFacadeError> {
        let counts = self.query_uc.list_tags().await.map_err(map_search_error)?;
        Ok(counts
            .into_iter()
            .map(|c| SearchTagView {
                is_builtin: c.tag_id.is_builtin(),
                tag_id: c.tag_id.to_string(),
                count: c.count,
            })
            .collect())
    }

    pub async fn status(&self) -> Result<SearchStatusView, SearchFacadeError> {
        self.coordinator
            .status_view()
            .await
            .map_err(map_search_error)
    }

    /// Notify the search subsystem that the encryption session just became ready.
    ///
    /// Drives any rebuild or purge that a locked cold start could not run.
    pub(crate) async fn on_session_ready(&self) {
        self.coordinator.on_session_ready().await;
    }

    pub(crate) async fn pause_background_activity(&self) {
        self.coordinator.pause_background_activity().await;
    }

    pub async fn request_rebuild(&self) -> Result<SearchRebuildAcceptedView, SearchFacadeError> {
        match self.coordinator.request_manual_rebuild().await {
            ManualRebuildResult::Accepted => Ok(SearchRebuildAcceptedView { accepted: true }),
            ManualRebuildResult::AlreadyInProgress => Err(SearchFacadeError::RebuildAlreadyRunning),
            ManualRebuildResult::Unavailable => Err(SearchFacadeError::ServiceUnavailable(
                "search coordinator stopped".to_string(),
            )),
        }
    }
}

/// True when the query carries no keyword and no filters — a plain browse. Only
/// such queries qualify for the §4.7 degraded main-store fallback; anything with
/// a keyword or filter needs the index and surfaces `IndexRebuilding` instead.
fn is_pure_browse(query: &SearchQuery) -> bool {
    query.query_string.trim().is_empty()
        && query.content_types.is_empty()
        && query.tags.is_empty()
        && query.source_devices.is_empty()
        && query.extensions.is_empty()
        && query.time_range.is_none()
}

fn search_page_to_view(page: uc_core::search::SearchResultsPage, state: &str) -> SearchPageView {
    SearchPageView {
        state: state.to_string(),
        total: page.total,
        has_more: page.has_more,
        items: page
            .items
            .into_iter()
            .map(|item| SearchResultView {
                entry_id: item.entry_id.to_string(),
                content_type: search_content_type_to_string(&item.content_type),
                active_time_ms: item.active_time_ms,
                tags: item.tags.iter().map(|t| t.to_string()).collect(),
                text_preview: item.text_preview,
                char_count: item.char_count,
                mime_type: item.mime_type,
                file_extensions: item.file_extensions,
                file_names: item.file_names,
                file_paths: item.file_paths,
                link_urls: item.link_urls,
                source_device: item.source_device,
                payload_state: item.payload_state,
            })
            .collect(),
    }
}

fn search_content_type_to_string(content_type: &ContentType) -> String {
    match content_type {
        ContentType::Text => "text",
        ContentType::Html => "html",
        ContentType::File => "file",
        ContentType::Image => "image",
        ContentType::Other => "other",
    }
    .to_string()
}

fn parse_search_query(input: SearchQueryInput) -> Result<SearchQuery, SearchFacadeError> {
    let (query_string, inferred_operator) = strip_and_infer_operator(&input.query)?;

    let operator = if let Some(operator) = input.operator.as_deref() {
        match operator.to_lowercase().as_str() {
            "and" => QueryOperator::And,
            "or" => QueryOperator::Or,
            _ => {
                return Err(SearchFacadeError::BadRequest(format!(
                    "invalid operator: {operator}"
                )))
            }
        }
    } else {
        inferred_operator.unwrap_or(QueryOperator::And)
    };

    Ok(SearchQuery {
        query_string,
        operator,
        time_range: parse_time_range(&input)?,
        content_types: parse_content_types(input.content_types.as_deref())?,
        tags: parse_tags(input.tags.as_deref()),
        extensions: parse_extensions(input.extensions.as_deref()),
        source_devices: parse_source_devices(input.source_devices.as_deref()),
        limit: input.limit.min(200),
        offset: input.offset,
    })
}

fn strip_and_infer_operator(
    raw: &str,
) -> Result<(String, Option<QueryOperator>), SearchFacadeError> {
    let tokens: Vec<&str> = raw.split_whitespace().collect();

    let mut has_and = false;
    let mut has_or = false;
    let mut non_operator_tokens: Vec<&str> = Vec::new();

    for token in &tokens {
        match token.to_uppercase().as_str() {
            "AND" => has_and = true,
            "OR" => has_or = true,
            _ => non_operator_tokens.push(token),
        }
    }

    if has_and && has_or {
        return Err(SearchFacadeError::InvalidQuery(
            "mixed AND/OR operators are not supported".to_string(),
        ));
    }

    let inferred = if has_and {
        Some(QueryOperator::And)
    } else if has_or {
        Some(QueryOperator::Or)
    } else {
        None
    };

    Ok((non_operator_tokens.join(" "), inferred))
}

fn parse_time_range(
    input: &SearchQueryInput,
) -> Result<Option<TimeRangeFilter>, SearchFacadeError> {
    let has_from = input.from_ms.is_some();
    let has_to = input.to_ms.is_some();

    if has_from != has_to {
        return Err(SearchFacadeError::BadRequest(
            "fromMs and toMs must both be present or both absent".to_string(),
        ));
    }

    if let (Some(from_ms), Some(to_ms)) = (input.from_ms, input.to_ms) {
        if from_ms < 0 || to_ms < 0 {
            return Err(SearchFacadeError::BadRequest(
                "fromMs and toMs must be non-negative".to_string(),
            ));
        }
        return Ok(Some(TimeRangeFilter::Absolute {
            from_ms: from_ms as u64,
            to_ms: to_ms as u64,
        }));
    }

    let Some(preset) = input.time_preset.as_deref() else {
        return Ok(None);
    };

    let filter = match preset {
        "today" => TimeRangeFilter::Today,
        "yesterday" => TimeRangeFilter::Yesterday,
        "last_24h" => TimeRangeFilter::Last24h,
        "last_7d" => TimeRangeFilter::Last7d,
        "last_30d" => TimeRangeFilter::Last30d,
        "this_week" => TimeRangeFilter::ThisWeek,
        "this_month" => TimeRangeFilter::ThisMonth,
        other => {
            return Err(SearchFacadeError::BadRequest(format!(
                "invalid timePreset: {other}"
            )))
        }
    };
    Ok(Some(filter))
}

fn parse_content_types(raw: Option<&str>) -> Result<Vec<ContentType>, SearchFacadeError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };

    let mut result = Vec::new();
    for value in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let content_type = match value {
            "text" => ContentType::Text,
            "html" => ContentType::Html,
            "file" => ContentType::File,
            "image" => ContentType::Image,
            "other" => ContentType::Other,
            // `link` is no longer a content_type; it is a derived tag filtered
            // via the `tags` query parameter.
            unknown => {
                return Err(SearchFacadeError::BadRequest(format!(
                    "invalid fileType: {unknown}"
                )))
            }
        };
        result.push(content_type);
    }
    Ok(result)
}

/// Parse a comma-separated tag id list (e.g. `link,favorited`). Unknown/custom
/// ids are passed through as opaque [`TagId`]s; the route-layer lock guard and
/// the (future) custom-tag registry decide acceptance. None/empty yields no tag
/// restriction.
fn parse_tags(raw: Option<&str>) -> Vec<TagId> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(TagId::new)
        .collect()
}

fn parse_extensions(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split(',')
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_source_devices(raw: Option<&str>) -> Vec<DeviceId> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(DeviceId::new)
        .collect()
}

pub fn map_search_error(error: SearchError) -> SearchFacadeError {
    match error {
        SearchError::InvalidQuery(message) => SearchFacadeError::InvalidQuery(message),
        SearchError::SessionLocked => SearchFacadeError::SessionLocked,
        SearchError::IndexNotReady => SearchFacadeError::IndexNotReady,
        SearchError::IndexUnavailable => SearchFacadeError::IndexUnavailable,
        SearchError::Internal(message) => SearchFacadeError::Internal(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browse_query() -> SearchQuery {
        SearchQuery {
            query_string: String::new(),
            operator: QueryOperator::And,
            time_range: None,
            content_types: Vec::new(),
            tags: Vec::new(),
            extensions: Vec::new(),
            source_devices: Vec::new(),
            limit: 50,
            offset: 0,
        }
    }

    #[test]
    fn is_pure_browse_true_for_empty_query_and_filters() {
        assert!(is_pure_browse(&browse_query()));
        // Whitespace-only keyword is still a browse.
        let mut q = browse_query();
        q.query_string = "   ".to_string();
        assert!(is_pure_browse(&q));
    }

    #[test]
    fn is_pure_browse_false_when_any_keyword_or_filter_present() {
        let mut keyword = browse_query();
        keyword.query_string = "hello".to_string();
        assert!(!is_pure_browse(&keyword));

        let mut typed = browse_query();
        typed.content_types = vec![ContentType::Image];
        assert!(!is_pure_browse(&typed));

        let mut tagged = browse_query();
        tagged.tags = vec![TagId::link()];
        assert!(!is_pure_browse(&tagged));

        let mut sourced = browse_query();
        sourced.source_devices = vec![DeviceId::new("dev-1")];
        assert!(!is_pure_browse(&sourced));

        let mut extended = browse_query();
        extended.extensions = vec!["md".to_string()];
        assert!(!is_pure_browse(&extended));

        let mut timed = browse_query();
        timed.time_range = Some(TimeRangeFilter::Today);
        assert!(!is_pure_browse(&timed));
    }
}
