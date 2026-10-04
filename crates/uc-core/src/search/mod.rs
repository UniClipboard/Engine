//! Search domain models — types and errors referenced by SearchIndexPort and
//! SearchKeyDerivationPort in `crate::ports::search`.
//!
//! 本模块只定义契约：没有实现、数据库访问或 HTTP 路由。
//! 实现位于 `uc-infra-storage`（Phase 90+），daemon 路由位于 uc-daemon（Phase 92）。

pub mod document;
pub mod error;
pub mod key;
pub mod pipeline_input;
pub mod query;
pub mod result;
pub mod tag;

pub use document::{ContentType, SearchDocument, SearchIndexMeta, SearchPosting};
pub use error::SearchError;
pub use key::{RenderKey, SearchKey, SearchKeyContext, SearchProtectionRef};
pub use pipeline_input::SearchPipelineInput;
pub use query::{QueryOperator, SearchQuery, TagMatchMode, TimeRangeFilter};
pub use result::{RebuildProgress, RebuildStage, SearchResult, SearchResultsPage};
pub use tag::{builtin as builtin_tags, TagId, TagKind, TagRule, TaggableContent};
