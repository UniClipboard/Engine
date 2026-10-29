use std::{
    fmt,
    sync::{Arc, OnceLock},
};

use futures_util::{stream::BoxStream, StreamExt};
use iroh::{
    address_lookup::{
        AddressLookup, AddressLookupBuilder, AddressLookupBuilderError, DnsAddressLookup,
        EndpointData, EndpointInfo, Error, Item,
    },
    Endpoint, EndpointId,
};
use uc_application::deps::{DiscoveryError, DiscoveryManager, DiscoveryProvider, PeerDirectory};
use uc_observability_contract::diagnostics::connectivity::{DiscoverySource, NetworkRecorder};

use super::super::addr_filter::restrict_to_local_network;
use super::super::observed_address_lookup::ObservedAddressLookupBuilder;
use super::{
    cloud::CloudDiscoveryProvider, configured::ConfiguredAddressProvider,
    mdns::MdnsDiscoveryProvider, ConfiguredPeerAddresses, LifetimeMdns,
};
use super::{decode, peer_id};

pub(in super::super) struct DirectoryAddressLookupBuilder {
    /// 长期 mDNS；启动时无法建立则由其在后台重试，目录在此期间只使用其余来源。
    pub(in super::super) mdns: LifetimeMdns,
    /// LAN-only Mode：不装配云端解析，地址发现结果只交出本地网络范围内的直连地址。
    pub(in super::super) lan_only: bool,
    /// 用户为配对设备配置的地址；两种模式都装配，LAN-only 下结果同样经过本地网络范围收敛。
    pub(in super::super) configured: Option<Arc<ConfiguredPeerAddresses>>,
    pub(in super::super) recorder: NetworkRecorder,
    pub(in super::super) manager: Arc<OnceLock<Arc<DiscoveryManager>>>,
}

impl fmt::Debug for DirectoryAddressLookupBuilder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DirectoryAddressLookupBuilder")
    }
}

impl AddressLookupBuilder for DirectoryAddressLookupBuilder {
    fn into_address_lookup(
        self,
        endpoint: &Endpoint,
    ) -> Result<impl AddressLookup, AddressLookupBuilderError> {
        let mut providers: Vec<Arc<dyn DiscoveryProvider>> = Vec::new();
        let lookup: Arc<dyn AddressLookup> = Arc::new(
            ObservedAddressLookupBuilder::new(
                self.mdns.clone(),
                DiscoverySource::Mdns,
                self.recorder.clone(),
            )
            .into_address_lookup(endpoint)?,
        );
        providers.push(Arc::new(MdnsDiscoveryProvider {
            mdns: self.mdns,
            lookup: lookup.clone(),
        }));
        let mdns = Some(lookup);
        if let Some(configured) = self.configured {
            providers.push(Arc::new(ConfiguredAddressProvider(configured)));
        }
        if !self.lan_only {
            let cloud = ObservedAddressLookupBuilder::new(
                DnsAddressLookup::n0_dns(),
                DiscoverySource::Dns,
                self.recorder,
            )
            .into_address_lookup(endpoint)?;
            providers.push(Arc::new(CloudDiscoveryProvider(cloud)));
        }
        let manager = Arc::new(
            DiscoveryManager::new(providers, Arc::new(PeerDirectory::new(1024)))
                .map_err(|source| AddressLookupBuilderError::from_err("uni-directory", source))?,
        );
        // discarded-source[no-information]: 重复 set 只返回原 Arc，没有可用错误来源。
        self.manager.set(manager.clone()).map_err(|_| {
            AddressLookupBuilderError::from_err("uni-directory", DiscoveryError::AlreadyInitialized)
        })?;
        Ok(DirectoryAddressLookup {
            manager,
            mdns,
            lan_only: self.lan_only,
        })
    }
}

/// 原连接调用继续交给 iroh；iroh 的 lookup 统一从发现负责人获得各来源候选。
pub(in super::super) struct DirectoryAddressLookup {
    pub(in super::super) manager: Arc<DiscoveryManager>,
    pub(in super::super) mdns: Option<Arc<dyn AddressLookup>>,
    /// 与 [`DirectoryAddressLookupBuilder::lan_only`] 相同。
    pub(in super::super) lan_only: bool,
}

impl fmt::Debug for DirectoryAddressLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DirectoryAddressLookup")
    }
}

impl AddressLookup for DirectoryAddressLookup {
    fn publish(&self, data: &EndpointData) {
        if let Some(mdns) = &self.mdns {
            mdns.publish(data);
        }
    }

    fn resolve(&self, endpoint: EndpointId) -> Option<BoxStream<'static, Result<Item, Error>>> {
        let results = self.manager.resolve(peer_id(endpoint));
        let lan_only = self.lan_only;
        Some(Box::pin(results.map(move |candidate| {
            let candidate = candidate.map_err(|source| Error::from_err("uni-directory", source))?;
            let mut address = decode(&candidate.peer, &candidate.locator)
                .map_err(|source| Error::from_err("uni-directory", source))?;
            // mDNS 结果来自对端自行发布的全部地址，可能含公网或反射地址。
            if lan_only {
                address = restrict_to_local_network(address);
            }
            Ok(Item::new(
                EndpointInfo::from(address),
                candidate.source.0,
                None,
            ))
        })))
    }
}
