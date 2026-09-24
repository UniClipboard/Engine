use std::collections::BTreeSet;

use uc_core::ids::DeviceId;
use uc_core::membership::{AdmissionChangeFacts, MembershipLedger, PeerLink};

use super::MembershipLedgerError;

/// 成员读模型计划：由已验证成员状态推导，随成员记录在同一事务中落实。
///
/// 读模型只保存仍需要资料的设备：本机、当前已激活成员、仍在离开中的设备，以及未完成效果影响的
/// 设备；可信身份只授予当前已激活的对端。历史记录不直接拥有读模型删除资格。
#[derive(Clone)]
pub struct MembershipProjectionPlan {
    pub local_device_id: DeviceId,
    pub members: Vec<AdmissionChangeFacts>,
    pub trusted_device_ids: BTreeSet<DeviceId>,
}

impl MembershipProjectionPlan {
    pub(crate) fn from_ledger(ledger: &MembershipLedger) -> Result<Self, MembershipLedgerError> {
        let history = ledger.history();
        let local_device_id = *ledger.local_device_id();
        let mut required = BTreeSet::from([local_device_id]);
        let mut trusted_device_ids = BTreeSet::new();
        for member in history.active_members() {
            let facts = history
                .admission_facts_for(member)
                .ok_or_else(MembershipLedgerError::corrupt)?;
            required.insert(facts.device_id);
            if facts.device_id != local_device_id {
                trusted_device_ids.insert(facts.device_id);
            }
        }
        required.extend(
            ledger
                .peers()
                .filter(|(_, link)| matches!(link, PeerLink::Departing(_)))
                .map(|(device_id, _)| *device_id),
        );
        required.extend(
            ledger
                .unfinished_effects()
                .flat_map(|effect| effect.affected_device_ids().iter().copied()),
        );
        let ids: Vec<_> = required.into_iter().collect();
        let members = ids
            .iter()
            .map(|id| {
                let member = history
                    .member_for_device(id, &ids)
                    .ok_or_else(MembershipLedgerError::corrupt)?;
                history
                    .admission_facts_for(member)
                    .cloned()
                    .ok_or_else(MembershipLedgerError::corrupt)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            local_device_id,
            members,
            trusted_device_ids,
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
