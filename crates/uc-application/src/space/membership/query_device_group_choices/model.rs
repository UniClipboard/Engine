use crate::space::membership::{DeviceTrustStatus, MembershipConflictsView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceGroupChoicesView {
    pub revision: u64,
    pub device_trust: DeviceTrustStatus,
    pub conflicts: MembershipConflictsView,
}
