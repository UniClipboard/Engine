use async_trait::async_trait;
use futures::stream::BoxStream;

use super::{DiscoveryError, DiscoveryEvent, DiscoveryPeerId, DiscoveryProviderId};

pub type DiscoveryStream = BoxStream<'static, Result<DiscoveryEvent, DiscoveryError>>;

/// 来源只观察和解析地址，不拨号，不处理业务资格。流被丢弃时取消本次订阅/查询。
#[async_trait]
pub trait DiscoveryProvider: Send + Sync {
    fn id(&self) -> DiscoveryProviderId;
    /// 持续发现；按需来源没有被动订阅，返回 None。
    async fn subscribe(&self) -> Result<Option<DiscoveryStream>, DiscoveryError>;
    /// 按不透明身份查询；没有该身份的解析能力时返回 None。
    fn resolve(&self, peer: &DiscoveryPeerId) -> Option<DiscoveryStream>;
}
