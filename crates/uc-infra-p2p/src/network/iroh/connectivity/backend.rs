use super::super::blobs::BLOBS_ALPN;
use super::super::connect::{
    connect_with_staggered_retry_classified, restrict_dial_addr_if_lan_only,
};
use super::super::peer_address_resolver::PeerAddressResolver;
use super::IrohPeerConnection;
use async_trait::async_trait;
use iroh::endpoint::ConnectOptions;
use iroh::{Endpoint, EndpointAddr};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{ConnectivityBackend, ConnectivityError, PeerConnection};
use uc_core::ids::DeviceId;
use uc_core::ports::{PeerAddressRepositoryPort, PeerReachabilityPort};
use uc_observability_contract::diagnostics::connectivity::AddressInputSource;
use uc_observability_contract::diagnostics::within_network_scope;
use uc_sync_protocol::active_clipboard_pull::ACTIVE_CLIPBOARD_PULL_ALPN;
use uc_sync_protocol::active_clipboard_state::ACTIVE_CLIPBOARD_ALPN;
use uc_sync_protocol::clipboard::{
    ATTRIBUTED_CLIPBOARD_ALPN, CLIPBOARD_ALPN, MULTIPLEXED_CLIPBOARD_ALPN,
};
use uc_sync_protocol::group_update::{GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN};
use uc_sync_protocol::membership_history::{
    MEMBERSHIP_HISTORY_EXCHANGE_ALPN, MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
};
use uc_sync_protocol::peer_reachability::{LEGACY_PEER_REACHABILITY_ALPN, PEER_REACHABILITY_ALPN};
use uc_sync_protocol::transfer_progress::v2::TRANSFER_PROGRESS_ALPN;
use uc_sync_protocol::transfer_progress::LEGACY_TRANSFER_PROGRESS_ALPN;

/// 同一 endpoint 上的独立协议连接池只使用各自固定的协商配置。
#[derive(Clone, Copy)]
pub enum ConnectivityProfile {
    TransferProgress,
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
        address_hint: Option<Vec<u8>>,
        cancel: CancellationToken,
    ) -> Result<Arc<dyn PeerConnection>, ConnectivityError> {
        if cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        let stored = self
            .addresses
            .resolve(&target)
            .await
            .map_err(|error| ConnectivityError::AddressLookup(Box::new(error)))?;
        let address = match address_hint {
            Some(encoded) if matches!(self.profile, ConnectivityProfile::Blob) => {
                let mut hinted: EndpointAddr = postcard::from_bytes(&encoded)
                    .map_err(|source| ConnectivityError::AddressLookup(Box::new(source)))?;
                if let Some(stored) = stored {
                    if stored.id != hinted.id {
                        return Err(ConnectivityError::Denied);
                    }
                    hinted.addrs.extend(stored.addrs);
                }
                hinted
            }
            Some(_) => return Err(ConnectivityError::Denied),
            None => stored.ok_or(if matches!(self.profile, ConnectivityProfile::Presence) {
                ConnectivityError::AddressMissing
            } else {
                ConnectivityError::Offline
            })?,
        };
        // 每个 profile 的原生拨号都从这里取地址：LAN-only 边界只在此处统一收敛一次。
        let address = restrict_dial_addr_if_lan_only(address);
        if cancel.is_cancelled() {
            return Err(ConnectivityError::Stopped);
        }
        let (preferred, fallback, label) = match self.profile {
            ConnectivityProfile::Blob => (BLOBS_ALPN, BLOBS_ALPN, "blob"),
            ConnectivityProfile::TransferProgress => (
                TRANSFER_PROGRESS_ALPN,
                LEGACY_TRANSFER_PROGRESS_ALPN,
                "transfer-progress",
            ),
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
            ConnectivityProfile::Clipboard => (
                ATTRIBUTED_CLIPBOARD_ALPN,
                MULTIPLEXED_CLIPBOARD_ALPN,
                "clipboard",
            ),
            ConnectivityProfile::LegacyGroupUpdate => (
                LEGACY_GROUP_UPDATE_ALPN,
                LEGACY_GROUP_UPDATE_ALPN,
                "group-update",
            ),
            ConnectivityProfile::GroupUpdate => {
                (GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN, "group-update")
            }
        };
        let mut additional = vec![fallback.to_vec()];
        if matches!(self.profile, ConnectivityProfile::Clipboard) {
            additional.push(CLIPBOARD_ALPN.to_vec());
        }
        let result = if matches!(self.profile, ConnectivityProfile::Blob) {
            // blob 的每轮只发起一次原生建连；恢复次数与退避由完整接收负责人统一计算。
            tokio::select! {
                _ = cancel.cancelled() => return Err(ConnectivityError::Stopped),
                result = self.endpoint.connect(address, preferred) => result.map_err(|source| ConnectivityError::Backend(Box::new(source))),
            }
        } else if matches!(self.profile, ConnectivityProfile::TransferProgress) {
            // 方向名额覆盖唯一原生拨号；兼容ALPN在同一次握手提供，不交错创建影子连接。
            let dial = async {
                let connecting = within_network_scope(self.endpoint.connect_with_opts(
                    address,
                    preferred,
                    ConnectOptions::new().with_additional_alpns(additional),
                ))
                .await
                .map_err(|source| ConnectivityError::Backend(Box::new(source)))?;
                within_network_scope(connecting)
                    .await
                    .map_err(|source| ConnectivityError::Backend(Box::new(source)))
            };
            tokio::select! {
                _=cancel.cancelled()=>return Err(ConnectivityError::Stopped),
                result=dial=>result,
            }
        } else {
            let dial = connect_with_staggered_retry_classified(
                self.endpoint.clone(),
                address,
                preferred,
                additional,
                label,
                AddressInputSource::Stored,
            );
            tokio::select! {
                _ = cancel.cancelled() => return Err(ConnectivityError::Stopped),
                result = dial => result.map_err(|source| ConnectivityError::Backend(Box::new(source))),
            }
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
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests;
