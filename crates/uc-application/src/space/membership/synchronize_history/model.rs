use uc_core::ids::DeviceId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MembershipSyncTarget {
    AllCurrentPeers,
    AuthenticatedPeer(DeviceId),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MembershipSyncReport {
    pub completed_peer_count: usize,
    pub deferred_peer_count: usize,
    pub stable_failure_count: usize,
}
