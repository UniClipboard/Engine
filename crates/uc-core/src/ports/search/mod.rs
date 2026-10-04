//! 搜索 port——由 `uc-infra-storage` 实现的异步 trait，
//! 通过 `Arc<dyn Port + Send + Sync>` 注入用例。

pub mod maintenance;
pub mod search_index;
pub mod search_key;
pub mod search_pipeline;

pub use maintenance::SearchIndexMaintenancePort;
pub use search_index::SearchIndexPort;
pub use search_key::SearchKeyDerivationPort;
pub use search_pipeline::SearchPipelinePort;
