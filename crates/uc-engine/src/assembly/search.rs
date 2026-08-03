use uc_application::facade::{SearchRuntime, SearchRuntimeDeps};

pub(crate) fn build_search_runtime(deps: &uc_application::deps::AppDeps) -> SearchRuntime {
    SearchRuntime::start(SearchRuntimeDeps::new(
        deps.search.search_index.clone(),
        deps.search.search_maintenance.clone(),
        deps.search.search_key_derivation.clone(),
        deps.search.search_pipeline.clone(),
        deps.clipboard.entry_ports.list.clone(),
        deps.clipboard.entry_ports.get.clone(),
        deps.clipboard.representation_ports.list_for_event.clone(),
        deps.clipboard.selection_repo.clone(),
        deps.clipboard.clipboard_event_reader_repo.clone(),
        deps.storage.entry_file_set_repo.clone(),
        uc_infra::search::constants::CURRENT_INDEX_VERSION,
    ))
}
