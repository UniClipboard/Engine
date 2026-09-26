use std::collections::BTreeSet;

use uc_core::ids::DeviceId;
use uc_core::membership::{AdmissionChangeFacts, MembershipLedger};

use super::MembershipLedgerError;

/// 成员读模型计划：由成员账本推导（规则见 `MembershipLedger::read_model`），随成员记录在同一事务中落实。
#[derive(Clone)]
pub struct MembershipProjectionPlan {
    pub local_device_id: DeviceId,
    pub members: Vec<AdmissionChangeFacts>,
    pub trusted_device_ids: BTreeSet<DeviceId>,
}

impl MembershipProjectionPlan {
    pub(crate) fn from_ledger(ledger: &MembershipLedger) -> Result<Self, MembershipLedgerError> {
        let read_model = ledger
            .read_model()
            .map_err(MembershipLedgerError::corrupt_from)?;
        Ok(Self {
            local_device_id: *ledger.local_device_id(),
            members: read_model.members,
            trusted_device_ids: read_model.trusted_device_ids,
        })
    }
}

impl std::fmt::Debug for MembershipProjectionPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MembershipProjectionPlan")
            .field("member_count", &self.members.len())
            .field("trusted_device_count", &self.trusted_device_ids.len())
            .finish()
    }
}
