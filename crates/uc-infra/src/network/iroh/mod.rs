//! iroh network adapter (Slice 1+).
//!
//! Groups adapters backed by the `iroh` crate: long-term device identity,
//! endpoint lifecycle, session opener, blob transfer. Slice 1 only ships
//! [`IrohIdentityStore`]; later slices add the rest.

pub mod active_clipboard;
mod addr_filter;
pub mod blobs;
pub mod clipboard_dispatch_adapter;
pub mod clipboard_receiver_adapter;
pub mod clipboard_wire;
mod conn_path;
mod connect;
pub mod connection_channel_adapter;
pub mod group_update_adapter;
pub mod identity_store;
pub mod membership_attestation_adapter;
pub(crate) mod membership_branch_recovery_wire;
pub mod membership_history_exchange_adapter;
mod net_recovery;
pub mod node;
pub mod persistable_addr;
pub mod presence_adapter;
pub mod relay_probe;
pub(crate) mod runtime_consts;
pub mod space_admission;
mod space_admission_wire;
pub mod transfer_progress_adapter;

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
pub mod transfer_progress_wire;

pub use active_clipboard::{
    IrohActiveClipboardDispatchAdapter, IrohActiveClipboardPullClientAdapter,
    IrohActiveClipboardPullServeAdapter, IrohActiveClipboardPullServeHandler,
    IrohActiveClipboardReceiverAdapter, IrohActiveClipboardReceiverHandler, ACTIVE_CLIPBOARD_ALPN,
    ACTIVE_CLIPBOARD_PULL_ALPN,
};
pub(crate) use addr_filter::filter_endpoint_addr;
pub use blobs::{IrohBlobTransferAdapter, BLOBS_ALPN};
pub use clipboard_dispatch_adapter::{IrohClipboardDispatchAdapter, CLIPBOARD_ALPN};
pub use clipboard_receiver_adapter::{IrohClipboardReceiverAdapter, IrohClipboardReceiverHandler};
pub(crate) use connect::connect_with_staggered_retry;
pub use connection_channel_adapter::IrohConnectionChannelAdapter;
pub use group_update_adapter::{IrohGroupUpdateAdapter, IrohGroupUpdateHandler, GROUP_UPDATE_ALPN};
pub use identity_store::{IrohIdentityStore, IDENTITY_STORE_KEY};
pub use membership_history_exchange_adapter::{
    IrohMembershipHistoryExchangeAdapter, IrohMembershipHistoryExchangeHandler,
    MEMBERSHIP_HISTORY_EXCHANGE_ALPN,
};
pub use net_recovery::NetworkRecoveryObservation;
pub use node::{
    ActiveClipboardHandlers, ActiveClipboardPullHandlers, BlobHandlers, ClipboardHandlers,
    GroupUpdateHandlers, IrohNode, IrohNodeBuilder, IrohNodeConfig, IrohNodeError,
    IrohRelayAccessToken, PairingInvitationHandlers, TransferProgressHandlers,
};
pub use presence_adapter::{IrohPresenceAdapter, IrohPresenceHandler, PRESENCE_ALPN};
pub use relay_probe::{
    IrohRelayProbeAdapter, RelayProbeError as IrohRelayProbeError,
    RelayProbeReport as IrohRelayProbeReport,
};
pub use space_admission::{
    encode_space_admission_route, IrohSpaceAdmissionHandler, IrohSpaceAdmissionTransport,
    SpaceAdmissionChannelCredentialError, SpaceAdmissionChannelCredentialPort,
    SponsorOpaqueMaterial, SPACE_ADMISSION_ALPN,
};
