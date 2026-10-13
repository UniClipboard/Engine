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
use uc_sync_protocol::group_update::{GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN};
use uc_sync_protocol::membership_history::{
    MEMBERSHIP_HISTORY_EXCHANGE_ALPN, MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
};

/// 同一 endpoint 上的独立协议连接池只使用各自固定的协商配置。
#[derive(Clone, Copy)]
pub enum ConnectivityProfile {
    Clipboard,
    MembershipHistory,
    GroupUpdate,
    /// 仅供固定旧端互通验收；实际能力仍由协商结果决定。
    LegacyGroupUpdate,
}

pub struct IrohConnectivityBackend {
    endpoint: Arc<Endpoint>,
    addresses: PeerAddressResolver,
    reachability: Arc<dyn PeerReachabilityPort>,
    profile: ConnectivityProfile,
}
impl IrohConnectivityBackend {
    pub fn new(
        endpoint: Arc<Endpoint>,
        addresses: Arc<dyn PeerAddressRepositoryPort>,
        reachability: Arc<dyn PeerReachabilityPort>,
        profile: ConnectivityProfile,
    ) -> Self {
        Self {
            endpoint,
            addresses: PeerAddressResolver::new(addresses),
            reachability,
            profile,
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
        let (preferred, fallback, label) = match self.profile {
            ConnectivityProfile::MembershipHistory => (
                MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
                MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
                "membership-history",
            ),
            ConnectivityProfile::Clipboard => {
                (MULTIPLEXED_CLIPBOARD_ALPN, CLIPBOARD_ALPN, "clipboard")
            }
            ConnectivityProfile::LegacyGroupUpdate => (
                LEGACY_GROUP_UPDATE_ALPN,
                LEGACY_GROUP_UPDATE_ALPN,
                "group-update",
            ),
            ConnectivityProfile::GroupUpdate => {
                (GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN, "group-update")
            }
        };
        match connect_with_staggered_retry_classified(
            self.endpoint.clone(),
            address,
            preferred,
            vec![fallback.to_vec()],
            label,
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

#[cfg(test)]
mod tests;
