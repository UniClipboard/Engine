use super::{MaterializeResult, ReceiveWorkPlan};
use crate::facade::blob_transfer::{
    BlobTransferFacade, FetchBlobCommand, FetchBlobResult, FetchBlobToPathCommand,
    FetchBlobToPathResult, FetchTransferContext,
};
use anyhow::Result;
use async_trait::async_trait;
use uc_core::file_transfer::FileTransferFailureReason;

#[async_trait]
pub trait InboundBlobMaterializer: Send + Sync {
    /// 完整计划的唯一入口；内部负责获取、输出、目录发布和结果聚合。
    async fn materialize_plan(&self, plan: ReceiveWorkPlan) -> Result<MaterializeResult>;
}

#[async_trait]
pub trait InboundBlobFetcher: Send + Sync {
    /// In-memory fetch path — used by representation-bound blobs (e.g.
    /// oversized images that we splice back into `snapshot.representations`).
    async fn fetch_blob(&self, command: FetchBlobCommand) -> Result<FetchBlobResult>;

    /// Streaming fetch path — used by free-standing files. The blob is
    /// written directly to `command.target_path` (reflink on CoW
    /// filesystems) so receiving a 1 GiB clipboard transfer no longer
    /// routes the full plaintext through `Bytes`. GH#487 Phase 2.
    async fn fetch_blob_to_path(
        &self,
        command: FetchBlobToPathCommand,
    ) -> Result<FetchBlobToPathResult>;

    async fn record_unfetched_failure(
        &self,
        _context: FetchTransferContext,
        _reason: FileTransferFailureReason,
    ) {
    }
}

#[async_trait]
impl InboundBlobFetcher for BlobTransferFacade {
    async fn fetch_blob(&self, command: FetchBlobCommand) -> Result<FetchBlobResult> {
        // 保留 thiserror 类型链:materializer 用 `is_cancel_error` downcast
        // 判断是否 user-cancel,与真正的 fetch 失败区分对待。
        BlobTransferFacade::fetch_blob(self, command)
            .await
            .map_err(anyhow::Error::from)
    }

    async fn fetch_blob_to_path(
        &self,
        command: FetchBlobToPathCommand,
    ) -> Result<FetchBlobToPathResult> {
        BlobTransferFacade::fetch_blob_to_path(self, command)
            .await
            .map_err(anyhow::Error::from)
    }

    async fn record_unfetched_failure(
        &self,
        context: FetchTransferContext,
        reason: FileTransferFailureReason,
    ) {
        BlobTransferFacade::record_unfetched_failure(self, context, reason).await;
    }
}
