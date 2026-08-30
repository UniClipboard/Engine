use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{MemberInstanceId, MembershipEventId, VersionedMembershipHistory};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipConflictId([u8; 32]);

impl MembershipConflictId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for MembershipConflictId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MembershipConflictId(REDACTED)")
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipBranchId([u8; 32]);

impl MembershipBranchId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for MembershipBranchId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MembershipBranchId(REDACTED)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipConflictChoice {
    ActiveMemberRecovery,
    RePairingRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipConflictPolicyError {
    InvalidConflict,
}

impl fmt::Display for MembershipConflictPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("the membership histories do not form a selectable conflict")
    }
}

impl std::error::Error for MembershipConflictPolicyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipConflictDescription {
    pub conflict_id: MembershipConflictId,
    pub local_branch_id: MembershipBranchId,
    pub remote_branch_id: MembershipBranchId,
    local_choice: MembershipConflictChoice,
    remote_choice: MembershipConflictChoice,
}

impl MembershipConflictDescription {
    pub fn branch_ids(&self) -> [MembershipBranchId; 2] {
        let mut ids = [self.local_branch_id, self.remote_branch_id];
        ids.sort();
        ids
    }

    pub fn choice_for(&self, branch_id: MembershipBranchId) -> Option<MembershipConflictChoice> {
        if branch_id == self.local_branch_id {
            Some(self.local_choice)
        } else if branch_id == self.remote_branch_id {
            Some(self.remote_choice)
        } else {
            None
        }
    }
}

pub struct MembershipConflictPolicy;

impl MembershipConflictPolicy {
    pub fn describe(
        local: &VersionedMembershipHistory,
        remote: &VersionedMembershipHistory,
        local_member: MemberInstanceId,
    ) -> Result<MembershipConflictDescription, MembershipConflictPolicyError> {
        if local.lineage_id() != remote.lineage_id()
            || !local.has_same_activation_baseline(remote)
            || local.is_complete_extension_of(remote)
            || remote.is_complete_extension_of(local)
        {
            return Err(MembershipConflictPolicyError::InvalidConflict);
        }
        let common_ancestor = local
            .closest_common_ancestor(remote)
            .ok_or(MembershipConflictPolicyError::InvalidConflict)?;
        let local_branch_id = branch_id(local)?;
        let remote_branch_id = branch_id(remote)?;
        if local_branch_id == remote_branch_id {
            return Err(MembershipConflictPolicyError::InvalidConflict);
        }
        let mut branches = [local_branch_id, remote_branch_id];
        branches.sort();
        let conflict_id = conflict_id(
            local.lineage_id(),
            common_ancestor,
            branches[0],
            branches[1],
        );
        Ok(MembershipConflictDescription {
            conflict_id,
            local_branch_id,
            remote_branch_id,
            local_choice: choice_for(local, local_member)?,
            remote_choice: choice_for(remote, local_member)?,
        })
    }
}

fn branch_id(
    history: &VersionedMembershipHistory,
) -> Result<MembershipBranchId, MembershipConflictPolicyError> {
    let position = history
        .current_position()
        .map_err(|_| MembershipConflictPolicyError::InvalidConflict)?;
    let head = position
        .event_id
        .ok_or(MembershipConflictPolicyError::InvalidConflict)?;
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-branch/v1\0");
    hash_field(&mut hasher, history.lineage_id().as_bytes());
    hasher.update(head.as_bytes());
    hasher.update(position.depth.to_be_bytes());
    hasher.update(position.history_digest);
    Ok(MembershipBranchId(hasher.finalize().into()))
}

fn conflict_id(
    lineage_id: &str,
    common_ancestor: MembershipEventId,
    first: MembershipBranchId,
    second: MembershipBranchId,
) -> MembershipConflictId {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-conflict/v1\0");
    hash_field(&mut hasher, lineage_id.as_bytes());
    hasher.update(common_ancestor.as_bytes());
    hasher.update(first.as_bytes());
    hasher.update(second.as_bytes());
    MembershipConflictId(hasher.finalize().into())
}

fn choice_for(
    history: &VersionedMembershipHistory,
    local_member: MemberInstanceId,
) -> Result<MembershipConflictChoice, MembershipConflictPolicyError> {
    if history.active_members().contains(&local_member) {
        return Ok(MembershipConflictChoice::ActiveMemberRecovery);
    }
    if history.credential_for(local_member).is_some()
        && !history.effective_members().contains(&local_member)
    {
        return Ok(MembershipConflictChoice::RePairingRequired);
    }
    Err(MembershipConflictPolicyError::InvalidConflict)
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}
