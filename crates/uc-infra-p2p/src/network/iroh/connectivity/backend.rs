use super::super::blobs::BLOBS_ALPN;
use super::super::connect::connect_with_staggered_retry_classified;
use super::super::peer_address_resolver::PeerAddressResolver;
use super::IrohPeerConnection;
use async_trait::async_trait;
use iroh::Endpoint;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{ConnectivityBackend, ConnectivityError, PeerConnection};
use uc_core::ids::DeviceId;
use uc_core::ports::{PeerAddressRepositoryPort, PeerReachabilityPort};
use uc_observability_contract::diagnostics::connectivity::AddressInputSource;
use uc_sync_protocol::active_clipboard_pull::ACTIVE_CLIPBOARD_PULL_ALPN;
use uc_sync_protocol::active_clipboard_state::ACTIVE_CLIPBOARD_ALPN;
use uc_sync_protocol::clipboard::{CLIPBOARD_ALPN, MULTIPLEXED_CLIPBOARD_ALPN};
use uc_sync_protocol::group_update::{GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN};
use uc_sync_protocol::membership_history::{
    MEMBERSHIP_HISTORY_EXCHANGE_ALPN, MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
};
use uc_sync_protocol::peer_reachability::{LEGACY_PEER_REACHABILITY_ALPN, PEER_REACHABILITY_ALPN};

/// 同一 endpoint 上的独立协议连接池只使用各自固定的协商配置。
#[derive(Clone, Copy)]
pub enum ConnectivityProfile {
    Blob,
    Presence,
    ActiveClipboardState,
    ActiveClipboardPull,
    Clipboard,
    MembershipHistory,
    GroupUpdate,
    /// 仅供固定旧端互通验收；实际能力仍由协商结果决定。
    LegacyGroupUpdate,
}

pub struct IrohConnectivityBackend {
    endpoint: Arc<Endpoint>,
    addresses: PeerAddressResolver,
    reachability: Option<Arc<dyn PeerReachabilityPort>>,
    profile: ConnectivityProfile,
}
impl IrohConnectivityBackend {
    pub fn new(
        endpoint: Arc<Endpoint>,
        addresses: Arc<dyn PeerAddressRepositoryPort>,
        reachability: Option<Arc<dyn PeerReachabilityPort>>,
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
        cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        if cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        let address = self
            .addresses
            .resolve(&target)
            .await
            .map_err(|error| ConnectivityError::AddressLookup(Box::new(error)))?
            .ok_or(if matches!(self.profile, ConnectivityProfile::Presence) {
                ConnectivityError::AddressMissing
            } else {
                ConnectivityError::Offline
            })?;
        if cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        let (preferred, fallback, label) = match self.profile {
            ConnectivityProfile::Blob => (BLOBS_ALPN, BLOBS_ALPN, "blob"),
            ConnectivityProfile::Presence => (
                PEER_REACHABILITY_ALPN,
                LEGACY_PEER_REACHABILITY_ALPN,
                "presence",
            ),
            ConnectivityProfile::ActiveClipboardPull => (
                ACTIVE_CLIPBOARD_PULL_ALPN,
                ACTIVE_CLIPBOARD_PULL_ALPN,
                "active-clipboard-pull",
            ),
            ConnectivityProfile::ActiveClipboardState => (
                ACTIVE_CLIPBOARD_ALPN,
                ACTIVE_CLIPBOARD_ALPN,
                "active-clipboard",
            ),
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
        let dial = connect_with_staggered_retry_classified(
            self.endpoint.clone(),
            address,
            preferred,
            vec![fallback.to_vec()],
            label,
            AddressInputSource::Stored,
        );
        let result = tokio::select! {
            _ = cancel.cancelled() => return Err(ConnectivityError::Stopped),
            result = dial => result,
        };
        match result {
            Ok(connection) => {
                // 握手完成后才订阅，旧连接的离线事件不能误关刚完成握手的新连接。
                Ok(match &self.reachability {
                    Some(reachability) => IrohPeerConnection::wrap_outbound(
                        connection,
                        target,
                        reachability.subscribe(),
                    ),
                    None => IrohPeerConnection::wrap(connection),
                })
            }
            Err(error) => {
                if let Some(reachability) = &self.reachability {
                    reachability.report_communication_failure(&target).await;
                }
                Err(ConnectivityError::Backend(Box::new(error)))
            }
        }
    }
}

#[cfg(test)]
mod tests;
