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
use iroh_mdns_address_lookup::MdnsAddressLookup;
use uc_application::deps::{DiscoveryError, DiscoveryManager, DiscoveryProvider, PeerDirectory};
use uc_observability_contract::diagnostics::connectivity::{DiscoverySource, NetworkRecorder};

use super::super::observed_address_lookup::ObservedAddressLookupBuilder;
use super::{cloud::CloudDiscoveryProvider, mdns::MdnsDiscoveryProvider};
use super::{decode, peer_id};

pub(in super::super) struct DirectoryAddressLookupBuilder {
    pub(in super::super) mdns: MdnsAddressLookup,
    pub(in super::super) cloud_enabled: bool,
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
        let mdns: Arc<dyn AddressLookup> = Arc::new(
            ObservedAddressLookupBuilder::new(
                self.mdns.clone(),
                DiscoverySource::Mdns,
                self.recorder.clone(),
            )
            .into_address_lookup(endpoint)?,
        );
        let mut providers: Vec<Arc<dyn DiscoveryProvider>> =
            vec![Arc::new(MdnsDiscoveryProvider {
                mdns: self.mdns,
                lookup: mdns.clone(),
            })];
        if self.cloud_enabled {
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
        Ok(DirectoryAddressLookup { manager, mdns })
    }
}

/// 原连接调用继续交给 iroh；iroh 的 lookup 统一从发现负责人获得各来源候选。
pub(in super::super) struct DirectoryAddressLookup {
    pub(in super::super) manager: Arc<DiscoveryManager>,
    pub(in super::super) mdns: Arc<dyn AddressLookup>,
}

impl fmt::Debug for DirectoryAddressLookup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DirectoryAddressLookup")
    }
}

impl AddressLookup for DirectoryAddressLookup {
    fn publish(&self, data: &EndpointData) {
        self.mdns.publish(data);
    }

    fn resolve(&self, endpoint: EndpointId) -> Option<BoxStream<'static, Result<Item, Error>>> {
        let results = self.manager.resolve(peer_id(endpoint));
        Some(Box::pin(results.map(|candidate| {
            let candidate = candidate.map_err(|source| Error::from_err("uni-directory", source))?;
            let address = decode(&candidate.peer, &candidate.locator)
                .map_err(|source| Error::from_err("uni-directory", source))?;
            Ok(Item::new(
                EndpointInfo::from(address),
                candidate.source.0,
                None,
            ))
        })))
    }
}
