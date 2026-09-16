use serde::{Deserialize, Serialize};
use uc_core::membership::{
    MemberInstanceId, MembershipBranchId, MembershipConflictDevice, MembershipConflictExplanation,
    MembershipConflictPolicy, VersionedMembershipHistory,
};

use super::{MembershipConflictRecord, MembershipLedgerError};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipConflictMember {
    pub device: MembershipConflictDevice,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipConflictPresentation {
    pub local_branch_id: MembershipBranchId,
    pub remote_branch_id: MembershipBranchId,
    pub local_members: Vec<MembershipConflictMember>,
    pub remote_members: Vec<MembershipConflictMember>,
    pub explanation: MembershipConflictExplanation,
}

impl MembershipConflictPresentation {
    pub(crate) fn from_verified_histories(
        local: &VersionedMembershipHistory,
        remote: &VersionedMembershipHistory,
        local_member: MemberInstanceId,
    ) -> Result<Self, MembershipLedgerError> {
        Ok(Self {
            local_branch_id: MembershipConflictPolicy::branch_id(local)
                .map_err(|_| MembershipLedgerError::Corrupt)?,
            remote_branch_id: MembershipConflictPolicy::branch_id(remote)
                .map_err(|_| MembershipLedgerError::Corrupt)?,
            local_members: Self::members(local)?,
            remote_members: Self::members(remote)?,
            explanation: MembershipConflictPolicy::explain(local, remote, local_member)
                .map_err(|_| MembershipLedgerError::Corrupt)?,
        })
    }

    pub(crate) fn members(
        history: &VersionedMembershipHistory,
    ) -> Result<Vec<MembershipConflictMember>, MembershipLedgerError> {
        let active = history.active_members();
        let mut members = history
            .effective_members()
            .into_iter()
            .map(|member| {
                let facts = history
                    .admission_facts_for(member)
                    .ok_or(MembershipLedgerError::Corrupt)?;
                Ok(MembershipConflictMember {
                    device: MembershipConflictDevice {
                        device_id: facts.device_id.clone(),
                        display_name: facts.device_name.clone(),
                    },
                    active: active.contains(&member),
                })
            })
            .collect::<Result<Vec<_>, MembershipLedgerError>>()?;
        members.sort_by(|a, b| a.device.device_id.cmp(&b.device.device_id));
        Ok(members)
    }

    pub(crate) fn matches_record(&self, record: &MembershipConflictRecord) -> bool {
        self.local_branch_id == record.local_branch_id
            && self.remote_branch_id == record.remote_branch_id
            && [&self.local_members, &self.remote_members]
                .into_iter()
                .all(|members| {
                    !members.is_empty()
                        && members
                            .windows(2)
                            .all(|pair| pair[0].device.device_id < pair[1].device.device_id)
                })
    }
}
