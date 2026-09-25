//! 入站拒绝只记录固定协议与原因，不携带远端身份或连接资料。
use super::{emit_local, record::LocalEvent, ObservationContext};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundPeerProtocol {
    Presence,
    MembershipHistory,
    MembershipBranchRecovery,
    Clipboard,
    ActiveClipboard,
    ActiveClipboardPull,
    TransferProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundPeerRejectionReason {
    IdentityUnresolved,
    IdentityAmbiguous,
    MemberReadFailed,
    FingerprintUnavailable,
    LedgerDenied,
    LedgerUnavailable,
    NotAccepting,
}

impl InboundPeerRejectionReason {
    pub(super) fn phase(self) -> &'static str {
        match self {
            Self::IdentityUnresolved
            | Self::IdentityAmbiguous
            | Self::MemberReadFailed
            | Self::FingerprintUnavailable => "identity",
            Self::LedgerDenied | Self::LedgerUnavailable | Self::NotAccepting => "admission",
        }
    }
}

pub fn record_inbound_peer_rejection(
    protocol: InboundPeerProtocol,
    reason: InboundPeerRejectionReason,
) {
    emit_local(
        LocalEvent::InboundPeerRejected { protocol, reason },
        &ObservationContext::capture(),
    );
}
