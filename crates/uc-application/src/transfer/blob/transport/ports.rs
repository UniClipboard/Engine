use super::BlobExchangeError;
use crate::connectivity::StreamLease;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ports::blob::BlobDigest;

/// 只交接 SDK 已验证的 Raw 内容末尾位置；实现不得等待通知网络或宿主事件。
pub trait BlobValidatedProgress: Send + Sync {
    fn validated_through(&self, end: u64, total: u64) -> Result<(), BlobExchangeError>;
}

/// 不透明的内容及实际工作保护，完成引用、输出和必要结算后才允许释放。
pub trait BlobRetainedContent: Send + Sync {}

#[must_use = "received content must stay protected until local settlement"]
pub struct BlobReceived {
    pub digest: BlobDigest,
    pub size: u64,
    pub protection: Box<dyn BlobRetainedContent>,
}

/// 只使用已经准入的通用流；不接触端点、不自拨，也不拥有业务重试或提交权威。
#[async_trait]
pub trait BlobExchangePort: Send + Sync {
    /// 取消或网络失败后仍等待同次 import 完成，不允许直接丢弃此 future。
    async fn receive(
        &self,
        stream: StreamLease,
        digest: BlobDigest,
        cancel: CancellationToken,
        progress: Arc<dyn BlobValidatedProgress>,
    ) -> Result<BlobReceived, BlobExchangeError>;

    /// 准入和请求头共用原截止时刻；取消后只停止本流并排空本次 SDK export。
    async fn provide(
        &self,
        stream: StreamLease,
        request_deadline: Instant,
        cancel: CancellationToken,
    ) -> Result<(), BlobExchangeError>;
}
