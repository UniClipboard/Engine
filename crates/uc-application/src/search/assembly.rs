//! 搜索领域对象图装配。
//!
//! Engine 只选择具体 adapter；查询、投影、修复、维护与运行期生命周期的
//! 组合由本模块唯一持有。

use std::sync::Arc;

use crate::deps::AppDeps;

use super::coordinator::SearchCoordinatorDeps;
use super::runtime::SearchRuntime;
use super::{SearchFacade, SearchShutdownError};

/// 搜索领域唯一对象图 owner。
pub struct SearchAssembly {
    runtime: SearchRuntime,
}

impl SearchAssembly {
    pub fn start(deps: &AppDeps) -> Self {
        let runtime = SearchRuntime::start(SearchCoordinatorDeps::new(
            Arc::clone(&deps.search.search_index),
            Arc::clone(&deps.search.search_maintenance),
            Arc::clone(&deps.search.search_key_derivation),
            Arc::clone(&deps.search.search_pipeline),
            Arc::clone(&deps.clipboard.entry_ports.list),
            Arc::clone(&deps.clipboard.entry_ports.get),
            Arc::clone(&deps.clipboard.representation_ports.list_for_event),
            Arc::clone(&deps.clipboard.selection_repo),
            Arc::clone(&deps.clipboard.clipboard_event_reader_repo),
            Arc::clone(&deps.storage.entry_file_set_repo),
        ));
        Self { runtime }
    }

    pub fn facade(&self) -> Arc<SearchFacade> {
        self.runtime.facade()
    }

    pub async fn shutdown(self) -> Result<(), SearchShutdownError> {
        self.runtime.shutdown().await
    }
}
