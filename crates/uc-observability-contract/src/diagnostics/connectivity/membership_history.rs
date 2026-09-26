//! 成员历史交换的安全本地诊断分类。

#[derive(Clone, Copy)]
pub enum MembershipHistoryFailurePhase {
    ConnectPeer,
    ExchangeHistory,
}

#[derive(Clone, Copy)]
pub enum MembershipHistoryFailureReason {
    PeerOffline,
    PairingInProgress,
    PeerRejected,
    Transport,
}

#[derive(Clone, Copy)]
pub struct MembershipHistoryFailureDetail {
    pub phase: MembershipHistoryFailurePhase,
    pub reason: MembershipHistoryFailureReason,
}

impl MembershipHistoryFailurePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConnectPeer => "connect_peer",
            Self::ExchangeHistory => "exchange_history",
        }
    }
}

impl MembershipHistoryFailureReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PeerOffline => "peer_offline",
            Self::PairingInProgress => "pairing_in_progress",
            Self::PeerRejected => "peer_rejected",
            Self::Transport => "transport",
        }
    }
}
