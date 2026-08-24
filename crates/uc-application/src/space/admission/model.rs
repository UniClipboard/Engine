#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInboundMember {
    pub device_id: uc_core::DeviceId,
    pub display_name: String,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentJoinStatus {
    Active {
        join_id: [u8; 16],
        joined_space: JoinedSpace,
    },
    Pending {
        join_id: [u8; 16],
        target_space_id: Option<String>,
        sponsor_device_id: Option<uc_core::DeviceId>,
        sponsor_identity_fingerprint: Option<uc_core::security::IdentityFingerprint>,
        cancel_requested: bool,
    },
    Rejected {
        join_id: [u8; 16],
        reason: uc_core::membership::AdmissionRejectionReasonV1,
    },
}
