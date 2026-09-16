use serde::{Deserialize, Serialize};

use super::{MembershipConflictPolicy, MembershipConflictPolicyError};
use crate::ids::DeviceId;
use crate::membership::{
    AdmissionChangeFacts, MemberInstanceId, MembershipEventId, MembershipEventV2,
    MembershipOperationV2, RemovalDecision, VersionedMembershipHistory,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipConflictReason {
    Unknown,
    PendingRemoval,
    DifferentRemovals,
    RemovalDecisionDisagreement,
    DivergedHistory,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipConflictDevice {
    pub device_id: DeviceId,
    pub display_name: String,
}

impl std::fmt::Debug for MembershipConflictDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("MembershipConflictDevice(REDACTED)")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipChangeSide {
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipChangeKind {
    AddedDevice,
    RemovedDevice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipChangeFact {
    pub side: MembershipChangeSide,
    pub kind: MembershipChangeKind,
    pub actor: MembershipConflictDevice,
    pub target: MembershipConflictDevice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipDecisionFact {
    pub device: MembershipConflictDevice,
    pub decision: RemovalDecision,
    pub target: MembershipConflictDevice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipConflictExplanation {
    pub reason: MembershipConflictReason,
    pub changes: Vec<MembershipChangeFact>,
    pub decisions: Vec<MembershipDecisionFact>,
    /// 只返回共同祖先之后的关键变化；存在更多后继变化时明确不完整。
    pub details_complete: bool,
}

impl MembershipConflictExplanation {
    pub fn unknown() -> Self {
        Self {
            reason: MembershipConflictReason::Unknown,
            changes: Vec::new(),
            decisions: Vec::new(),
            details_complete: false,
        }
    }

    pub fn pending_removal(
        history: &VersionedMembershipHistory,
        id: MembershipEventId,
    ) -> Result<Self, MembershipConflictPolicyError> {
        let event = history
            .event(id)
            .ok_or(MembershipConflictPolicyError::InvalidConflict)?;
        if !matches!(event.operation, MembershipOperationV2::RemoveDevice { .. }) {
            return Err(MembershipConflictPolicyError::InvalidConflict);
        }
        Ok(Self {
            reason: MembershipConflictReason::PendingRemoval,
            changes: vec![change_fact(history, event, MembershipChangeSide::Remote)?],
            decisions: Vec::new(),
            details_complete: true,
        })
    }
}

impl MembershipConflictPolicy {
    pub fn explain(
        local: &VersionedMembershipHistory,
        remote: &VersionedMembershipHistory,
        local_member: MemberInstanceId,
    ) -> Result<MembershipConflictExplanation, MembershipConflictPolicyError> {
        Self::describe(local, remote, local_member)?;
        let ancestor = local
            .closest_common_ancestor(remote)
            .ok_or(MembershipConflictPolicyError::InvalidConflict)?;
        let (local_event, local_count) = first_change(local, ancestor)?;
        let (remote_event, remote_count) = first_change(remote, ancestor)?;
        let mut changes = Vec::new();
        if let Some(event) = local_event {
            changes.push(change_fact(local, event, MembershipChangeSide::Local)?);
        }
        if let Some(event) = remote_event {
            changes.push(change_fact(remote, event, MembershipChangeSide::Remote)?);
        }
        let disagreement = local_event.is_none()
            && remote_event.is_some_and(|event| rejected(local, event))
            || remote_event.is_none() && local_event.is_some_and(|event| rejected(remote, event));
        let different_removals = changes.len() == 2
            && changes
                .iter()
                .all(|change| change.kind == MembershipChangeKind::RemovedDevice)
            && changes[0].target.device_id != changes[1].target.device_id;
        let reason = if disagreement {
            MembershipConflictReason::RemovalDecisionDisagreement
        } else if different_removals {
            MembershipConflictReason::DifferentRemovals
        } else {
            MembershipConflictReason::DivergedHistory
        };
        let mut decisions = Vec::new();
        for event in [local_event, remote_event].into_iter().flatten() {
            let MembershipOperationV2::RemoveDevice { member } = event.operation else {
                continue;
            };
            for history in [local, remote] {
                for decision in history.removal_decisions_for(event.event_id()) {
                    let item = MembershipDecisionFact {
                        device: device(
                            history
                                .admission_facts_for(decision.decided_by_member_instance_id)
                                .ok_or(MembershipConflictPolicyError::InvalidConflict)?,
                        ),
                        decision: decision.decision,
                        target: device(
                            history
                                .admission_facts_for(member)
                                .ok_or(MembershipConflictPolicyError::InvalidConflict)?,
                        ),
                    };
                    if !decisions.contains(&item) {
                        decisions.push(item);
                    }
                }
            }
        }
        decisions.sort_by(|a, b| {
            (
                &a.device.device_id,
                &a.target.device_id,
                a.decision == RemovalDecision::Reject,
            )
                .cmp(&(
                    &b.device.device_id,
                    &b.target.device_id,
                    b.decision == RemovalDecision::Reject,
                ))
        });
        Ok(MembershipConflictExplanation {
            reason,
            changes,
            decisions,
            details_complete: local_count <= 1 && remote_count <= 1,
        })
    }
}

fn rejected(history: &VersionedMembershipHistory, event: &MembershipEventV2) -> bool {
    history
        .removal_decisions_for(event.event_id())
        .any(|decision| decision.decision == RemovalDecision::Reject)
}

fn first_change(
    history: &VersionedMembershipHistory,
    ancestor: MembershipEventId,
) -> Result<(Option<&MembershipEventV2>, usize), MembershipConflictPolicyError> {
    let mut cursor = history.current_head();
    let mut first = None;
    let mut count = 0;
    while cursor != Some(ancestor) {
        let event = cursor
            .and_then(|id| history.event(id))
            .ok_or(MembershipConflictPolicyError::InvalidConflict)?;
        first = Some(event);
        count += 1;
        cursor = event.parent_event_id;
    }
    Ok((first, count))
}

fn change_fact(
    history: &VersionedMembershipHistory,
    event: &MembershipEventV2,
    side: MembershipChangeSide,
) -> Result<MembershipChangeFact, MembershipConflictPolicyError> {
    let actor = history
        .admission_facts_for(event.author_member_instance_id)
        .ok_or(MembershipConflictPolicyError::InvalidConflict)?;
    let (kind, target) = match &event.operation {
        MembershipOperationV2::AddDevice { admission } => {
            (MembershipChangeKind::AddedDevice, &admission.facts)
        }
        MembershipOperationV2::RemoveDevice { member } => (
            MembershipChangeKind::RemovedDevice,
            history
                .admission_facts_for(*member)
                .ok_or(MembershipConflictPolicyError::InvalidConflict)?,
        ),
    };
    Ok(MembershipChangeFact {
        side,
        kind,
        actor: device(actor),
        target: device(target),
    })
}

fn device(facts: &AdmissionChangeFacts) -> MembershipConflictDevice {
    MembershipConflictDevice {
        device_id: facts.device_id.clone(),
        display_name: facts.device_name.clone(),
    }
}
