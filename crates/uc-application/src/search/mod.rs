use std::sync::Arc;
use uc_core::error_class::ErrorClass;

use thiserror::Error;

pub(crate) mod assembly;
pub(crate) mod coordinator;
pub(crate) mod live_index;
pub(crate) mod mutation_gate;
pub(crate) mod projection;
pub(crate) mod query;
pub(crate) mod runtime;
pub(crate) mod tagging;
mod task_scope;

use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::SearchIndexPort;
use uc_core::search::tag::TagId;
use uc_core::search::{
    ContentType, QueryOperator, SearchError, SearchQuery, SearchResultsPage, TagMatchMode,
    TimeRangeFilter,
};

use crate::clipboard::history_tags::{
    load_history_tag_ids, HistoryEntryTagReaderPort, HistoryTagStoreError,
};
use crate::search::query::SearchClipboardEntriesUseCase;
use crate::space::QuerySpaceAccessStateError;

pub use assembly::SearchAssembly;
use coordinator::{ManualRebuildResult, SearchCoordinator};
pub use coordinator::{SearchRebuildProgressView, SearchStatusSnapshot};
pub use projection::SearchProjectionBuilder;
pub use task_scope::SearchTaskError;

#[derive(Debug, Error)]
pub enum SearchShutdownError {
    #[error("search coordinator failed")]
    Coordinator {
        #[source]
        source: anyhow::Error,
    },
    #[error("search coordinator task failed")]
    Task {
        #[source]
        source: tokio::task::JoinError,
    },
}

impl ErrorClass for SearchShutdownError {
    fn class(&self) -> &'static str {
        match self {
            Self::Coordinator { .. } => "coordinator",
            Self::Task { .. } => "task",
        }
    }
}

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
    /// carrying any of them (or all, per `tag_match`).
    pub tags: Option<String>,
    /// 标签维度的组合方式：`any`（默认，命中任一）或 `all`（必须全部携带）。
    pub tag_match: Option<String>,
    pub limit: u32,
    pub offset: u32,
}

/// 单次计数请求最多包含的查询数。
const MAX_COUNT_BATCH: usize = 32;
/// 单次按日统计最多包含的桶数（覆盖一年多，足够日历视图）。
const MAX_COUNT_BUCKETS: usize = 400;

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

#[derive(Debug, Error)]
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
    /// 无法确认加密会话是否已就绪，聚合查询按失败关闭处理。
    #[error("encryption session state is unavailable")]
    SessionStateUnavailable(#[source] QuerySpaceAccessStateError),
    #[error("search rebuild is already running")]
    RebuildAlreadyRunning,
    #[error("search failed")]
    Internal(#[source] SearchError),
}

pub struct SearchFacade {
    query_uc: SearchClipboardEntriesUseCase,
    coordinator: Arc<SearchCoordinator>,
    /// 本机历史标签的权威关联：结果中的用户标签 id 与标签计数由它补齐。
    history_entry_tags: Option<Arc<dyn HistoryEntryTagReaderPort>>,
}

impl SearchFacade {
    fn with_runtime(
        search_index: Arc<dyn SearchIndexPort>,
        coordinator: Arc<SearchCoordinator>,
        history_entry_tags: Option<Arc<dyn HistoryEntryTagReaderPort>>,
    ) -> Self {
        Self {
            query_uc: SearchClipboardEntriesUseCase::from_port(search_index),
            coordinator,
            history_entry_tags,
        }
    }

    /// 为结果页补齐用户历史标签 id。索引只保存不可逆的标签词项，结果中的 id 来自
    /// 权威关联；会话锁定时失败关闭。
    async fn with_history_tags(
        &self,
        mut page: SearchResultsPage,
    ) -> Result<SearchResultsPage, SearchFacadeError> {
        let entry_ids: Vec<EntryId> = page
            .items
            .iter()
            .map(|item| item.entry_id.clone())
            .collect();
        let mut tags = load_history_tag_ids(self.history_entry_tags.as_deref(), &entry_ids)
            .await
            .map_err(history_tag_error)?;
        for item in &mut page.items {
            if let Some(history_tags) = tags.remove(&item.entry_id) {
                item.tags.extend(history_tags);
            }
        }
        Ok(page)
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
                let page = self.with_history_tags(page).await?;
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
                let page = self.with_history_tags(page).await?;
                Ok(search_page_to_view(page, SEARCH_STATE_DEGRADED))
            }
            Err(SearchError::IndexNotReady) => Err(SearchFacadeError::IndexRebuilding),
            Err(other) => Err(map_search_error(other)),
        }
    }

    /// 批量统计与 `query` 过滤语义一致的匹配数，按输入顺序返回。分页字段被忽略。
    ///
    /// 只服务索引；索引未就绪时不降级为主库浏览，一律返回 `IndexRebuilding`，
    /// 因为计数只用于提示，不能给出与筛选不一致的近似值。
    pub async fn count(
        &self,
        inputs: Vec<SearchQueryInput>,
    ) -> Result<Vec<u32>, SearchFacadeError> {
        if inputs.len() > MAX_COUNT_BATCH {
            return Err(SearchFacadeError::BadRequest(format!(
                "at most {MAX_COUNT_BATCH} count queries per request"
            )));
        }
        let queries = inputs
            .into_iter()
            .map(parse_search_query)
            .collect::<Result<Vec<_>, _>>()?;
        let mut counts = Vec::with_capacity(queries.len());
        for query in queries {
            match self.query_uc.count(query).await {
                Ok(total) => counts.push(total),
                Err(SearchError::IndexNotReady) => return Err(SearchFacadeError::IndexRebuilding),
                Err(other) => return Err(map_search_error(other)),
            }
        }
        Ok(counts)
    }

    /// 按调用方给出的桶边界统计每个桶内的条目数（见 `SearchIndexPort::count_by_active_time`）。
    pub async fn daily_counts(
        &self,
        boundaries_ms: Vec<i64>,
    ) -> Result<Vec<u32>, SearchFacadeError> {
        if boundaries_ms.len() < 2 || boundaries_ms.len() > MAX_COUNT_BUCKETS + 1 {
            return Err(SearchFacadeError::BadRequest(format!(
                "boundaries must contain between 2 and {} entries",
                MAX_COUNT_BUCKETS + 1
            )));
        }
        if boundaries_ms.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(SearchFacadeError::BadRequest(
                "boundaries must be strictly increasing".to_string(),
            ));
        }
        match self.query_uc.count_by_active_time(&boundaries_ms).await {
            Ok(counts) => Ok(counts),
            Err(SearchError::IndexNotReady) => Err(SearchFacadeError::IndexRebuilding),
            Err(other) => Err(map_search_error(other)),
        }
    }

    /// List the tags present in the index with their entry counts. Returns both
    /// builtin and custom tags; the caller applies lock-based visibility (custom
    /// tags are hidden while the session is locked, §4.6).
    pub async fn tags(&self) -> Result<Vec<SearchTagView>, SearchFacadeError> {
        let counts = self.query_uc.list_tags().await.map_err(map_search_error)?;
        let mut views: Vec<SearchTagView> = counts
            .into_iter()
            .map(|c| SearchTagView {
                is_builtin: c.tag_id.is_builtin(),
                tag_id: c.tag_id.to_string(),
                count: c.count,
            })
            .collect();
        views.extend(self.history_tag_counts().await?);
        Ok(views)
    }

    /// 用户历史标签在索引中的条目数，与按标签过滤的结果同源。会话锁定或索引未就绪时
    /// 不返回用户标签（与“锁定时隐藏自定义标签”一致），不影响内置标签。
    async fn history_tag_counts(&self) -> Result<Vec<SearchTagView>, SearchFacadeError> {
        let Some(reader) = &self.history_entry_tags else {
            return Ok(Vec::new());
        };
        let tag_ids = reader.tag_ids().await.map_err(history_tag_error)?;
        let mut views = Vec::with_capacity(tag_ids.len());
        for tag_id in tag_ids {
            let query = SearchQuery {
                tags: vec![tag_id.clone()],
                ..browse_query_template()
            };
            match self.query_uc.count(query).await {
                Ok(0) => {}
                Ok(count) => views.push(SearchTagView {
                    tag_id: tag_id.to_string(),
                    count,
                    is_builtin: false,
                }),
                Err(SearchError::SessionLocked | SearchError::IndexNotReady) => {
                    return Ok(Vec::new())
                }
                Err(other) => return Err(map_search_error(other)),
            }
        }
        Ok(views)
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
    pub(crate) async fn on_session_ready(&self) -> Result<(), SearchTaskError> {
        self.coordinator.on_session_ready().await
    }

    pub(crate) async fn pause_background_activity(&self) -> Result<(), SearchTaskError> {
        self.coordinator.pause_background_activity().await
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
        tag_match: parse_tag_match(input.tag_match.as_deref())?,
        extensions: parse_extensions(input.extensions.as_deref()),
        source_devices: parse_source_devices(input.source_devices.as_deref()),
        limit: input.limit.min(200),
        offset: input.offset,
    })
}

fn parse_tag_match(raw: Option<&str>) -> Result<TagMatchMode, SearchFacadeError> {
    match raw.map(|value| value.trim().to_lowercase()).as_deref() {
        None | Some("") | Some("any") => Ok(TagMatchMode::Any),
        Some("all") => Ok(TagMatchMode::All),
        Some(other) => Err(SearchFacadeError::BadRequest(format!(
            "invalid tagMatch: {other}"
        ))),
    }
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

fn history_tag_error(error: HistoryTagStoreError) -> SearchFacadeError {
    match error {
        HistoryTagStoreError::Locked => SearchFacadeError::SessionLocked,
        other => SearchFacadeError::Internal(SearchError::Internal(Box::new(other))),
    }
}

/// 只按标签计数的查询模板：无关键词、无其他筛选、不分页。
fn browse_query_template() -> SearchQuery {
    SearchQuery {
        query_string: String::new(),
        operator: QueryOperator::And,
        time_range: None,
        content_types: Vec::new(),
        tags: Vec::new(),
        tag_match: TagMatchMode::Any,
        extensions: Vec::new(),
        source_devices: Vec::new(),
        limit: 0,
        offset: 0,
    }
}

pub fn map_search_error(error: SearchError) -> SearchFacadeError {
    match error {
        SearchError::InvalidQuery(message) => SearchFacadeError::InvalidQuery(message),
        SearchError::SessionLocked => SearchFacadeError::SessionLocked,
        SearchError::IndexNotReady => SearchFacadeError::IndexNotReady,
        SearchError::IndexUnavailable => SearchFacadeError::IndexUnavailable,
        error @ SearchError::Internal(_) => SearchFacadeError::Internal(error),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    fn browse_query() -> SearchQuery {
        SearchQuery {
            query_string: String::new(),
            operator: QueryOperator::And,
            time_range: None,
            content_types: Vec::new(),
            tags: Vec::new(),
            tag_match: TagMatchMode::Any,
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

    #[tokio::test]
    async fn shutdown_errors_preserve_typed_sources_without_exposing_details() {
        let coordinator = SearchShutdownError::Coordinator {
            source: anyhow::Error::new(std::io::Error::other("private coordinator detail")),
        };
        assert!(coordinator.source().is_some());
        assert!(!coordinator
            .to_string()
            .contains("private coordinator detail"));

        let source = tokio::spawn(async { panic!("private task detail") })
            .await
            .expect_err("panic must produce a join error");
        let task = SearchShutdownError::Task { source };
        assert!(task.source().is_some());
        assert!(!task.to_string().contains("private task detail"));
    }
}
