//! 重启只读取既有归属与原始清单，不恢复运行期通知、attempt 或投递结论。
use async_trait::async_trait;
use uc_core::clipboard::{EntryFileSet, MimeType};
use uc_core::ids::{DeviceId, EntryId};

pub struct ProgressHistoryEvidence {
    pub file_set: Option<EntryFileSet>,
    /// 原事件的所有表示元数据；不读取内容或假定旧打包位置。
    pub representations: Vec<ProgressHistoryRepresentation>,
    pub sole_paste: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("transfer progress history could not be read")]
pub struct ProgressHistoryReadError(#[source] pub anyhow::Error);

/// 仅读既有本机发送归属；不从消息头投递状态推断文件结果。
#[async_trait]
pub trait ReadProgressHistoryPort: Send + Sync {
    /// 最多256个范围，每个peer最多64个；只包含未删除的本机文件/图片条目。
    async fn list_owned(
        &self,
        local: DeviceId,
        peers: &[DeviceId],
    ) -> Result<Vec<(DeviceId, EntryId)>, ProgressHistoryReadError>;
    async fn read_owned(
        &self,
        local: DeviceId,
        peer: DeviceId,
        entry: &EntryId,
    ) -> Result<Option<ProgressHistoryEvidence>, ProgressHistoryReadError>;
}

/// 只读表示类型与原始长度，不载入内容本体或密文缓冲。
pub struct ProgressHistoryRepresentation {
    pub mime_type: Option<MimeType>,
    pub size_bytes: i64,
}
