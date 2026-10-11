use async_trait::async_trait;
use futures_util::StreamExt;
use iroh::address_lookup::AddressLookup;
use uc_application::deps::{
    DiscoveryError, DiscoveryPeerId, DiscoveryProvider, DiscoveryProviderId, DiscoveryStream,
};

use super::{endpoint_id, observation, CLOUD};

/// 既有 DNS/PKARR 解析按需发布候选；不列举设备、不建立连接或新增轮询。
pub(super) struct CloudDiscoveryProvider<T>(pub(super) T);

#[async_trait]
impl<T: AddressLookup> DiscoveryProvider for CloudDiscoveryProvider<T> {
    fn id(&self) -> DiscoveryProviderId {
        CLOUD
    }

    async fn subscribe(&self) -> Result<Option<DiscoveryStream>, DiscoveryError> {
        Ok(None)
    }

    fn resolve(&self, peer: &DiscoveryPeerId) -> Option<DiscoveryStream> {
        let results = self.0.resolve(endpoint_id(peer)?)?;
        Some(Box::pin(results.map(|item| {
            item.map_err(|source| DiscoveryError::Provider {
                source: source.into(),
            })
            .and_then(observation)
        })))
    }
}
