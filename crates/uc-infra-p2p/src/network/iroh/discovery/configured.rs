//! 用户为已配对设备配置的地址来源。
//!
//! 只在连接需要某个对端的地址时查询：按传输身份找到成员身份目录中的设备，读取设置里该设备的
//! 条目中的 IP 字面量。只接受 IP、不解析主机名，因此不产生 DNS 查询。结果与其他来源一样只是
//! 拨号线索：不订阅、不拨号、不授予成员资格，连接后仍按传输身份验证；LAN-only 下由目录 lookup
//! 统一收敛到本地网络范围。地址不进入日志或错误文本。

use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use async_trait::async_trait;
use futures_util::{stream, StreamExt};
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use uc_application::deps::{
    DiscoveredPeer, DiscoveryError, DiscoveryEvent, DiscoveryPeerId, DiscoveryProvider,
    DiscoveryProviderId, DiscoveryStream, PeerIdentityDirectoryPort,
};
use uc_core::ports::{security::IdentityFingerprintFactoryPort, SettingsPort};

use super::super::inbound_peer::{PeerIdentityError, PeerIdentityResolver};
use super::{endpoint_id, peer_id, CONFIGURED, OBSERVATION_TTL};

/// 用户配置地址的读取能力：成员身份目录把传输身份映射为设备，设置提供该设备的条目。
///
/// 身份目录在没有当前 Space 时为空，此时不交出任何候选。
pub struct ConfiguredPeerAddresses {
    identity: PeerIdentityResolver,
    settings: Arc<dyn SettingsPort>,
}

impl ConfiguredPeerAddresses {
    pub fn new(
        identities: Arc<dyn PeerIdentityDirectoryPort>,
        fingerprints: Arc<dyn IdentityFingerprintFactoryPort>,
        settings: Arc<dyn SettingsPort>,
    ) -> Self {
        Self {
            identity: PeerIdentityResolver::new(identities, fingerprints),
            settings,
        }
    }

    /// 返回该传输身份的配置地址；不是已知设备、没有条目或条目不是 IP 字面量时返回 `None`。
    async fn observe(
        &self,
        endpoint: EndpointId,
    ) -> Result<Option<DiscoveryEvent>, DiscoveryError> {
        let device = match self.identity.resolve(endpoint.as_bytes()).await {
            Ok(device) => device,
            Err(PeerIdentityError::Unresolved | PeerIdentityError::Ambiguous) => return Ok(None),
            Err(source) => {
                return Err(DiscoveryError::Provider {
                    source: source.into(),
                })
            }
        };
        let settings = self
            .settings
            .load()
            .await
            .map_err(|source| DiscoveryError::Provider { source })?;
        let Some(entry) = settings.network.peer_addresses.get(device.as_str()) else {
            return Ok(None);
        };
        // 保存前已校验；手工改坏、不是 IP 字面量的值不作为候选，也不尝试按主机名解析。
        let Ok(ip) = entry.ip.parse::<IpAddr>() else {
            return Ok(None);
        };
        let address = EndpointAddr::from_parts(
            endpoint,
            [TransportAddr::Ip(SocketAddr::new(ip, entry.port))],
        );
        let locator = postcard::to_stdvec(&address).map_err(|source| DiscoveryError::Provider {
            source: source.into(),
        })?;
        Ok(Some(DiscoveryEvent::Found(DiscoveredPeer {
            peer: peer_id(endpoint),
            locator,
            ttl: OBSERVATION_TTL,
        })))
    }
}

impl fmt::Debug for ConfiguredPeerAddresses {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ConfiguredPeerAddresses")
    }
}

/// 把用户配置的地址作为按需来源交给发现负责人；没有被动订阅。
pub(super) struct ConfiguredAddressProvider(pub(super) Arc<ConfiguredPeerAddresses>);

#[async_trait]
impl DiscoveryProvider for ConfiguredAddressProvider {
    fn id(&self) -> DiscoveryProviderId {
        CONFIGURED
    }

    async fn subscribe(&self) -> Result<Option<DiscoveryStream>, DiscoveryError> {
        Ok(None)
    }

    fn resolve(&self, peer: &DiscoveryPeerId) -> Option<DiscoveryStream> {
        let endpoint = endpoint_id(peer)?;
        let source = Arc::clone(&self.0);
        Some(Box::pin(
            stream::once(async move { source.observe(endpoint).await })
                .filter_map(|result| std::future::ready(result.transpose())),
        ))
    }
}

#[cfg(test)]
mod tests;
