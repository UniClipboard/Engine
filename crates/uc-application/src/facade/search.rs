//! Search 门面对外入口(ADR-018 阶段 3)。
//!
//! `SearchFacade` 与查询/索引运行期、协调器和投影构建全部位于
//! `crate::search`,本模块只做对外白名单再导出;facade 目录不再容纳
//! 实现。

pub use crate::search::{
    map_search_error, SearchAssembly, SearchFacade, SearchFacadeError, SearchPageView,
    SearchProjectionBuilder, SearchQueryInput, SearchRebuildAcceptedView,
    SearchRebuildProgressView, SearchResultView, SearchShutdownError, SearchStatusSnapshot,
    SearchStatusView, SearchTagView,
};
