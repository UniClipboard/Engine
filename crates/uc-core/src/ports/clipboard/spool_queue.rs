use crate::ids::RepresentationId;

#[derive(Debug, Clone)]
pub struct SpoolRequest {
    pub rep_id: RepresentationId,
    pub bytes: Vec<u8>,
}

#[async_trait::async_trait]
pub trait SpoolQueuePort: Send + Sync {
    /// 持久保存待处理内容，不发布处理任务。
    async fn persist(&self, request: SpoolRequest) -> anyhow::Result<()>;

    /// 请求处理已提交的表示；耐久内容由既有恢复扫描保留。
    fn notify_committed(&self, rep_id: &RepresentationId);
}
