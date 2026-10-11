//! 将已有发现服务转换为 Uni 候选，并在 iroh 的实际地址查找入口消费目录。

mod cloud;
mod lookup;
mod mdns;
mod runtime;

pub(super) use lookup::DirectoryAddressLookupBuilder;
pub(super) use runtime::DiscoveryRuntime;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use iroh::{address_lookup::Item, EndpointAddr, EndpointId};
use uc_application::deps::{
    BackendId, DiscoveredPeer, DiscoveryError, DiscoveryEvent, DiscoveryPeerId, DiscoveryProviderId,
};

const IROH: BackendId = BackendId("iroh");
const MDNS: DiscoveryProviderId = DiscoveryProviderId("mdns");
const CLOUD: DiscoveryProviderId = DiscoveryProviderId("cloud_dns");
// 原服务未提供 DNS TTL；为目录副本设置保守上限，每次真实结果刷新，不修改原服务缓存。
const OBSERVATION_TTL: Duration = Duration::from_secs(60);

fn peer_id(endpoint: EndpointId) -> DiscoveryPeerId {
    DiscoveryPeerId::new(IROH, endpoint.as_bytes().to_vec())
}

pub(super) fn endpoint_id(peer: &DiscoveryPeerId) -> Option<EndpointId> {
    if peer.backend != IROH {
        return None;
    }
    EndpointId::try_from(peer.identity()).ok()
}

fn observation(item: Item) -> Result<DiscoveryEvent, DiscoveryError> {
    let ttl = if let Some(updated) = item.last_updated() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|source| DiscoveryError::Provider {
                source: source.into(),
            })?;
        let age = now
            .checked_sub(Duration::from_micros(updated))
            .ok_or(DiscoveryError::InvalidObservation)?;
        OBSERVATION_TTL
            .checked_sub(age)
            .filter(|ttl| !ttl.is_zero())
            .ok_or(DiscoveryError::InvalidObservation)?
    } else {
        OBSERVATION_TTL
    };
    let address = item.into_endpoint_addr();
    let peer = peer_id(address.id);
    let locator = postcard::to_stdvec(&address).map_err(|source| DiscoveryError::Provider {
        source: source.into(),
    })?;
    Ok(DiscoveryEvent::Found(DiscoveredPeer { peer, locator, ttl }))
}

fn decode(peer: &DiscoveryPeerId, locator: &[u8]) -> Result<EndpointAddr, DiscoveryError> {
    let address: EndpointAddr =
        postcard::from_bytes(locator).map_err(|source| DiscoveryError::Provider {
            source: source.into(),
        })?;
    if Some(address.id) != endpoint_id(peer) {
        return Err(DiscoveryError::InvalidObservation);
    }
    Ok(address)
}

#[cfg(test)]
mod tests;
