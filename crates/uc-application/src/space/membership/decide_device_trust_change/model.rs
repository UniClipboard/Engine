use uc_core::membership::MembershipEventId;

use crate::space::membership::DeviceTrustStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTrustChangeChoice {
    ApplyChange,
    KeepCurrentDeviceGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecideDeviceTrustChange {
    pub change_id: MembershipEventId,
    pub choice: DeviceTrustChangeChoice,
    pub confirm_local_removal: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecideDeviceTrustChangeResult {
    Applied {
        change_id: MembershipEventId,
        status: DeviceTrustStatus,
    },
    KeptCurrentDeviceGroup {
        change_id: MembershipEventId,
        status: DeviceTrustStatus,
    },
    AlreadyCompleted {
        change_id: MembershipEventId,
        choice: DeviceTrustChangeChoice,
        status: DeviceTrustStatus,
    },
    StateChanged {
        current_change_id: Option<MembershipEventId>,
        status: DeviceTrustStatus,
    },
    LocalConfirmationRequired {
        change_id: MembershipEventId,
        status: DeviceTrustStatus,
    },
}
