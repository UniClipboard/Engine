//! 资源查询所需的关联资料必须来自同一个已提交视图。
use async_trait::async_trait;
use uc_core::error_class::ErrorClass;
use uc_core::{
    clipboard::PersistedClipboardRepresentation,
    ids::{EntryId, EventId, RepresentationId},
};

pub struct EntryResourceSnapshot {
    pub event_id: EventId,
    pub preview_representation: Option<RepresentationId>,
    pub representations: Vec<PersistedClipboardRepresentation>,
}

#[derive(Debug, thiserror::Error)]
#[error("entry resource snapshot could not be read")]
pub struct EntryResourceReadError(
    #[from]
    #[source]
    pub anyhow::Error,
);

/// 一次读取当前条目的选择与全部认证后表示；条目不存在时返回 None。
#[async_trait]
pub trait ReadEntryResourceSnapshotPort: Send + Sync {
    async fn read_entry_resource_snapshot(
        &self,
        entry: &EntryId,
    ) -> Result<Option<EntryResourceSnapshot>, EntryResourceReadError>;
}

impl ErrorClass for EntryResourceReadError {
    fn class(&self) -> &'static str {
        "read"
    }
}
