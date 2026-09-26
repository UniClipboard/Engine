#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInboundMember {
    pub device_id: uc_core::DeviceId,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundPairingStatus {
    AwaitingConfirmation,
    ConfirmationMissed,
    NeedsAttention,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundPairing {
    pub pairing_id: [u8; 32],
    pub device_id: Option<uc_core::DeviceId>,
    pub display_name: Option<String>,
    pub status: InboundPairingStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinedSpace {
    pub sponsor_device_id: uc_core::DeviceId,
    pub sponsor_identity_fingerprint: uc_core::security::IdentityFingerprint,
    pub space_id: String,
    pub self_device_id: uc_core::DeviceId,
    pub self_identity_fingerprint: uc_core::security::IdentityFingerprint,
    pub migrated_records: Option<u64>,
    pub preserved_unreadable_records: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinSpaceTerminationReason {
    Cancelled,
    Expired,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinSpaceAttentionReason {
    OutcomeCannotBeProven,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinSpaceAttentionRecovery {
    PreserveDataAndContactSupport,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentJoinStatus {
    Active {
        join_id: [u8; 16],
        joined_space: JoinedSpace,
        peer_upgrade_required: bool,
    },
    Pending {
        join_id: [u8; 16],
        target_space_id: Option<String>,
        sponsor_device_id: Option<uc_core::DeviceId>,
        sponsor_identity_fingerprint: Option<uc_core::security::IdentityFingerprint>,
        cancel_requested: bool,
        peer_upgrade_required: bool,
    },
    Processing {
        join_id: [u8; 16],
        target_space_id: String,
        sponsor_device_id: uc_core::DeviceId,
        sponsor_identity_fingerprint: uc_core::security::IdentityFingerprint,
        peer_upgrade_required: bool,
    },
    NeedsAttention {
        join_id: [u8; 16],
        reason: JoinSpaceAttentionReason,
        recovery: JoinSpaceAttentionRecovery,
        next_retry_at_ms: Option<i64>,
    },
    Rejected {
        join_id: [u8; 16],
        reason: uc_core::membership::SpaceAdmissionRejectionReason,
    },
    Terminated {
        join_id: [u8; 16],
        reason: JoinSpaceTerminationReason,
    },
}
