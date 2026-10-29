//! 用户为已配对设备配置的地址来源。
//!
//! 只在连接需要某个对端的地址时查询：按传输身份找到成员身份目录中的设备，读取设置里该设备的
//! 条目，并在查询时解析 IP 字面量或主机名。结果与其他来源一样只是拨号线索：不订阅、不拨号、
//! 不授予成员资格，连接后仍按传输身份验证；LAN-only 下由目录 lookup 统一收敛到本地网络范围。
//! 主机与地址不进入日志或错误文本。

use std::{
    fmt,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    sync::Arc,
    time::Duration,
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
use super::super::persistable_addr::MAX_DIRECT_HINTS;
use super::{endpoint_id, peer_id, CONFIGURED, OBSERVATION_TTL};

/// 单次主机名解析的上限；超时只让本次查询没有该来源的候选，不影响其他来源。
const HOST_LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);

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

    /// 返回该传输身份的配置地址解析结果；不是已知设备、没有条目或没有解析结果时返回 `None`。
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
        let addresses = resolve_host(entry.host.clone(), entry.port).await?;
        if addresses.is_empty() {
            return Ok(None);
        }
        let address = EndpointAddr::from_parts(
            endpoint,
            addresses
                .into_iter()
                .take(MAX_DIRECT_HINTS)
                .map(TransportAddr::Ip),
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

/// IP 字面量直接使用；主机名交给操作系统解析器（与用户在其他程序中输入主机名的含义一致，
/// 包括 hosts 文件与系统配置的 DNS），并限制等待时间。
async fn resolve_host(host: String, port: u16) -> Result<Vec<SocketAddr>, DiscoveryError> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let lookup = tokio::task::spawn_blocking(move || {
        (host.as_str(), port)
            .to_socket_addrs()
            .map(|addresses| addresses.collect::<Vec<_>>())
    });
    match tokio::time::timeout(HOST_LOOKUP_TIMEOUT, lookup).await {
        Ok(Ok(Ok(addresses))) => Ok(addresses),
        Ok(Ok(Err(source))) => Err(DiscoveryError::Provider {
            source: source.into(),
        }),
        Ok(Err(source)) => Err(DiscoveryError::Provider {
            source: source.into(),
        }),
        Err(source) => Err(DiscoveryError::Provider {
            source: anyhow::Error::new(source).context("configured peer host lookup timed out"),
        }),
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
