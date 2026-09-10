//! 历史文件安全检查只读取可能包含受管路径的表示，不解密无关正文。
use async_trait::async_trait;
use uc_core::ids::{EntryId, RepresentationId};

pub struct HistoryFileReference {
    pub entry_id: EntryId,
    pub representation_id: RepresentationId,
    pub uri_list: Vec<u8>,
}

#[async_trait]
pub trait HistoryFileReferencePort: Send + Sync {
    /// 按表示 ID 升序分页；仅返回 URI-list 或 files 格式，正文已认证解密。
    async fn list_file_references(
        &self,
        after: Option<&RepresentationId>,
        limit: usize,
    ) -> anyhow::Result<Vec<HistoryFileReference>>;
}
