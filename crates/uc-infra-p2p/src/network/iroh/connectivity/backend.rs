use super::super::connect::connect_with_staggered_retry_classified;
use super::super::peer_address_resolver::PeerAddressResolver;
use super::IrohPeerConnection;
use async_trait::async_trait;
use iroh::Endpoint;
use std::sync::Arc;
use uc_application::deps::{ConnectivityBackend, ConnectivityError, PeerConnection};
use uc_core::ids::DeviceId;
use uc_core::ports::{PeerAddressRepositoryPort, PeerReachabilityPort};
use uc_observability_contract::diagnostics::connectivity::AddressInputSource;
use uc_sync_protocol::clipboard::{CLIPBOARD_ALPN, MULTIPLEXED_CLIPBOARD_ALPN};

pub struct IrohConnectivityBackend {
    endpoint: Arc<Endpoint>,
    addresses: PeerAddressResolver,
    reachability: Arc<dyn PeerReachabilityPort>,
}
impl IrohConnectivityBackend {
    pub fn new(
        endpoint: Arc<Endpoint>,
        addresses: Arc<dyn PeerAddressRepositoryPort>,
        reachability: Arc<dyn PeerReachabilityPort>,
    ) -> Self {
        Self {
            endpoint,
            addresses: PeerAddressResolver::new(addresses),
            reachability,
        }
    }
}
#[async_trait]
impl ConnectivityBackend for IrohConnectivityBackend {
    async fn connect(
        &self,
        target: DeviceId,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        let address = self
            .addresses
            .resolve(&target)
            .await
            .map_err(|error| ConnectivityError::AddressLookup(Box::new(error)))?
            .ok_or(ConnectivityError::Offline)?;
        match connect_with_staggered_retry_classified(
            self.endpoint.clone(),
            address,
            MULTIPLEXED_CLIPBOARD_ALPN,
            vec![CLIPBOARD_ALPN.to_vec()],
            "clipboard",
            AddressInputSource::Stored,
        )
        .await
        {
            Ok(connection) => {
                // 握手完成后才订阅，旧连接的离线事件不能误关刚完成握手的新连接。
                let events = self.reachability.subscribe();
                Ok(IrohPeerConnection::wrap_outbound(
                    connection, target, events,
                ))
            }
            Err(error) => {
                let _ = self
                    .reachability
                    .report_communication_failure(&target)
                    .await;
                Err(ConnectivityError::Backend(Box::new(error)))
            }
        }
    }
}
