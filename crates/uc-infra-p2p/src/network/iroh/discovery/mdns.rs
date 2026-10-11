use async_trait::async_trait;
use futures_util::StreamExt;
use iroh::address_lookup::{AddressLookup, Item};
use iroh_mdns_address_lookup::{DiscoveryEvent as MdnsEvent, MdnsAddressLookup};
use std::sync::Arc;
use uc_application::deps::{
    DiscoveryError, DiscoveryEvent, DiscoveryPeerId, DiscoveryProvider, DiscoveryProviderId,
    DiscoveryStream,
};

use super::{endpoint_id, observation, peer_id, MDNS};

/// 复用 endpoint 的同一个 mDNS 实例，不增加广播服务或修改线格式。
pub(super) struct MdnsDiscoveryProvider {
    pub(super) mdns: MdnsAddressLookup,
    pub(super) lookup: Arc<dyn AddressLookup>,
}

#[async_trait]
impl DiscoveryProvider for MdnsDiscoveryProvider {
    fn id(&self) -> DiscoveryProviderId {
        MDNS
    }

    async fn subscribe(&self) -> Result<Option<DiscoveryStream>, DiscoveryError> {
        let events = self.mdns.subscribe().await;
        Ok(Some(Box::pin(events.filter_map(|event| {
            std::future::ready(match event {
                MdnsEvent::Discovered {
                    endpoint_info,
                    last_updated,
                } => Some(mdns_observation(Item::new(
                    endpoint_info,
                    "mdns",
                    last_updated,
                ))),
                MdnsEvent::Expired { endpoint_id } => Some(Ok(DiscoveryEvent::Lost {
                    peer: peer_id(endpoint_id),
                })),
                _ => None,
            })
        }))))
    }

    fn resolve(&self, peer: &DiscoveryPeerId) -> Option<DiscoveryStream> {
        let id = endpoint_id(peer)?;
        let results = self.lookup.resolve(id)?;
        Some(Box::pin(results.map(|item| {
            item.map_err(|source| DiscoveryError::Provider {
                source: source.into(),
            })
            .and_then(mdns_observation)
        })))
    }
}

fn mdns_observation(item: Item) -> Result<DiscoveryEvent, DiscoveryError> {
    // 本仓 mDNS 服务提供原观察时间；时间不明时不把缓存读取误当作刷新。
    if item.last_updated().is_none() {
        return Err(DiscoveryError::InvalidObservation);
    }
    observation(item)
}
