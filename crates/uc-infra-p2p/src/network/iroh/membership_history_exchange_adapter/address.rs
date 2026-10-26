use super::super::peer_address_resolver::PeerAddressResolver;
use super::super::persistable_addr::{
    observed_refreshed_remote_addr, persist_observed_stable_addr,
};
use async_trait::async_trait;
use iroh::Endpoint;
use std::sync::Arc;
use uc_application::deps::RefreshVerifiedPeerAddressPort;
use uc_core::ids::DeviceId;
use uc_core::ports::{ClockPort, PeerAddressRepositoryPort};
use uc_observability_contract::uc_warn;

pub struct IrohVerifiedPeerAddressRefresh {
    endpoint: Arc<Endpoint>,
    resolver: PeerAddressResolver,
    repository: Arc<dyn PeerAddressRepositoryPort>,
    clock: Arc<dyn ClockPort>,
}
impl IrohVerifiedPeerAddressRefresh {
    pub fn new(
        endpoint: Arc<Endpoint>,
        repository: Arc<dyn PeerAddressRepositoryPort>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            endpoint,
            resolver: PeerAddressResolver::new(repository.clone()),
            repository,
            clock,
        }
    }
}
#[async_trait]
impl RefreshVerifiedPeerAddressPort for IrohVerifiedPeerAddressRefresh {
    async fn refresh_verified_peer_address(&self, peer: &DeviceId) {
        let address = match self.resolver.resolve(peer).await {
            Ok(Some(address)) => address,
            Ok(None) => return,
            Err(source) => {
                uc_warn!(error_kind = source.kind(), "成员历史验证后地址读取失败");
                return;
            }
        };
        // 合并而不是替换：使用中的路径是新证据，未被否定的旧直连线索继续保留。
        if let Some(observed) = observed_refreshed_remote_addr(&self.endpoint, &address).await {
            persist_observed_stable_addr(
                self.repository.as_ref(),
                self.clock.as_ref(),
                peer,
                observed,
            )
            .await;
        }
    }
}
