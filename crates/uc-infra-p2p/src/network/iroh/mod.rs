//! iroh network adapter (Slice 1+).
//!
//! Groups adapters backed by the `iroh` crate: long-term device identity,
//! endpoint lifecycle, session opener, blob transfer. Slice 1 only ships
//! [`IrohIdentityStore`]; later slices add the rest.

pub mod active_clipboard;
mod addr_filter;
mod admission_transport_material;
pub mod admitted_connections;
pub mod blob_exchange;
pub mod blobs;
mod conn_path;
mod connect;
mod connection_census;
pub mod connection_channel_adapter;
mod connection_diagnostics;
pub mod connectivity;
mod discovery;
pub mod group_update_adapter;
pub mod identity_store;
pub(crate) mod inbound_peer;
mod membership_branch_recovery_adapter;
pub mod membership_history_exchange_adapter;
mod membership_identity_adapter;
mod net_recovery;
#[cfg(test)]
mod network_log_e2e;
mod network_partition;
pub mod node;
mod observed_address_lookup;
mod observed_connections;
mod peer_address_resolver;
pub mod peer_reachability_adapter;
pub mod persistable_addr;
mod protocol_router;
pub mod relay_probe;
pub(crate) mod runtime_consts;
mod session_generation;
pub mod space_admission;
pub(crate) use super::trace_context;

#[cfg(test)]
pub(crate) struct StaticPeerAdmission(pub(crate) bool);

#[cfg(test)]
#[async_trait::async_trait]
impl uc_core::membership::PeerAdmissionPort for StaticPeerAdmission {
    async fn is_admitted(
        &self,
        _device_id: &uc_core::ids::DeviceId,
    ) -> Result<bool, uc_core::membership::PeerAdmissionError> {
        Ok(self.0)
    }
}

pub use active_clipboard::{
    ActiveClipboardWire, IrohActivePullHandler, IrohActiveStateHandler, SessionActivePullLocalWork,
};
pub(crate) use addr_filter::filter_endpoint_addr;
pub use admission_transport_material::IrohAdmissionTransportMaterial;
pub use blobs::{IrohBlobTransferAdapter, BLOBS_ALPN};
pub(crate) use connect::connect_with_staggered_retry;
pub use connection_channel_adapter::IrohConnectionChannelAdapter;
pub use group_update_adapter::{
    IrohGroupUpdateAdapter, IrohGroupUpdateHandler, GROUP_UPDATE_ALPN, LEGACY_GROUP_UPDATE_ALPN,
};
pub use identity_store::{IrohIdentityStore, IDENTITY_STORE_KEY};
pub use membership_branch_recovery_adapter::IrohMembershipBranchRecoveryChannel;
pub use membership_history_exchange_adapter::{
    HistoryConnectionAdmission, IrohMembershipHistoryExchangeHandler,
    IrohVerifiedPeerAddressRefresh, MembershipHistoryWire, SessionMembershipHistoryLocalWork,
    MEMBERSHIP_HISTORY_EXCHANGE_ALPN, MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
};
pub use network_partition::IrohNetworkPartitionGate;
pub use node::{
    ActiveClipboardHandlers, ActiveClipboardPullHandlers, BlobHandlers, ClipboardHandlers,
    IrohNode, IrohNodeBuilder, IrohNodeConfig, IrohNodeError, IrohNodeShutdownError,
    IrohRelayAccessToken, IrohSessionBuilder, PairingInvitationHandlers, PreparedIrohSession,
};
pub use peer_reachability_adapter::{
    IrohPeerReachabilityHandler, LEGACY_PEER_REACHABILITY_ALPN, PEER_REACHABILITY_ALPN,
};
pub use relay_probe::{
    IrohRelayProbeAdapter, RelayProbeError as IrohRelayProbeError,
    RelayProbeReport as IrohRelayProbeReport,
};
pub use space_admission::{
    encode_space_admission_route, IrohSpaceAdmissionHandler, IrohSpaceAdmissionTransport,
    SpaceAdmissionChannelCredentialError, SpaceAdmissionChannelCredentialPort,
    SponsorOpaqueMaterial, SPACE_ADMISSION_ALPN,
};
pub use uc_sync_protocol::clipboard::{CLIPBOARD_ALPN, MULTIPLEXED_CLIPBOARD_ALPN};

/// 产品使用的全部 ALPN。网络日志只输出这些已知取值，对端发来的其他字符串一律不保留。
pub fn product_alpns() -> Vec<&'static [u8]> {
    vec![
        BLOBS_ALPN,
        CLIPBOARD_ALPN,
        uc_sync_protocol::clipboard::ATTRIBUTED_CLIPBOARD_ALPN,
        MULTIPLEXED_CLIPBOARD_ALPN,
        PEER_REACHABILITY_ALPN,
        LEGACY_PEER_REACHABILITY_ALPN,
        SPACE_ADMISSION_ALPN,
        GROUP_UPDATE_ALPN,
        LEGACY_GROUP_UPDATE_ALPN,
        MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
        MULTIPLEXED_MEMBERSHIP_HISTORY_ALPN,
        MEMBERSHIP_BRANCH_RECOVERY_ALPN,
        uc_sync_protocol::active_clipboard_state::ACTIVE_CLIPBOARD_ALPN,
        uc_sync_protocol::active_clipboard_pull::ACTIVE_CLIPBOARD_PULL_ALPN,
        uc_sync_protocol::transfer_progress::v2::TRANSFER_PROGRESS_ALPN,
        uc_sync_protocol::transfer_progress::LEGACY_TRANSFER_PROGRESS_ALPN,
    ]
}
pub use uc_sync_protocol::membership_branch_recovery::MEMBERSHIP_BRANCH_RECOVERY_ALPN;

#[cfg(feature = "test-util")]
pub mod test_support;

pub mod transfer_progress_v2;
