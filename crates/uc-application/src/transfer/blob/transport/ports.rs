use super::BlobExchangeError;
use crate::connectivity::{ConnectivityError, StreamLease};
use async_trait::async_trait;
use bytes::Bytes;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_core::ids::DeviceId;
use uc_core::ports::blob::{BlobDigest, BlobTicket};

/// ticket 的来源及本请求地址材料；内容身份和设备身份不得由文件名推导。
pub struct BlobSource {
    pub source_key: [u8; 32],
    pub digest: BlobDigest,
    pub address_hint: Vec<u8>,
}
#[async_trait]
pub trait BlobSourcePort: Send + Sync {
    fn inspect(&self, ticket: &BlobTicket) -> Result<BlobSource, BlobExchangeError>;
    async fn resolve_peer(&self, key: &[u8; 32]) -> Result<DeviceId, BlobExchangeError>;
}

/// 纯本地内容能力；不接触 Endpoint，不自拨，不负责业务重试。
#[async_trait]
pub trait BlobReadStorePort: Send + Sync {
    async fn protect(&self, digest: BlobDigest) -> Result<Option<BlobReceived>, BlobExchangeError>;
    async fn read(&self, digest: BlobDigest) -> Result<Bytes, BlobExchangeError>;
    async fn export(&self, digest: BlobDigest, target: &Path) -> Result<u64, BlobExchangeError>;
}

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
    /// 只有原生实现能证明暂态网络来源；未知实现默认不重试。
    fn connection_failure(&self, source: ConnectivityError) -> BlobExchangeError {
        BlobExchangeError::from(source)
    }
    /// logical_deadline 保留原逻辑期限；response_timeout 在本地保护完成后才起算。
    /// 网络响应期限为 min(logical_deadline, 本地保护完成时刻 + response_timeout)。
    /// 该期限只限制请求写入和响应头等待；成功响应头必须严格早于期限。
    /// 真实响应错误保留原结果；超时只停止本流并结算，不允许丢弃已开始的本地 import。
    async fn receive(
        &self,
        stream: StreamLease,
        logical_deadline: Instant,
        response_timeout: Duration,
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

/// 完整传输关闭后停止同一内容存储，不由单次交换关闭共享资源。
#[async_trait]
pub trait BlobStoreLifecyclePort: Send + Sync {
    async fn shutdown(&self) -> Result<(), BlobExchangeError>;
}
