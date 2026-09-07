use std::collections::BTreeSet;

use uc_core::membership::{
    MembershipConflictChoice, MembershipConflictPolicy, VersionedMembershipHistory,
};

use super::{DeviceGroupChoiceImpact, MembershipConflictBranchView};
use crate::space::membership::{
    CurrentSpaceMemberScope, DeviceTrustMembership, LoadedMembershipLedger,
    MembershipConflictMember, MembershipConflictPresentation, MembershipConflictRecord,
    MembershipLedgerError, SpaceMemberPauseReason,
};

pub(super) fn branch_view(
    conflict: &MembershipConflictRecord,
    is_local: bool,
    stored: Option<&MembershipConflictPresentation>,
    record: &LoadedMembershipLedger,
    history: &VersionedMembershipHistory,
    scope: &CurrentSpaceMemberScope,
) -> Result<MembershipConflictBranchView, MembershipLedgerError> {
    let (branch_id, choice) = if is_local {
        (conflict.local_branch_id, conflict.local_choice)
    } else {
        (conflict.remote_branch_id, conflict.remote_choice)
    };
    let members = match stored {
        Some(stored) => Some(if is_local {
            stored.local_members.clone()
        } else {
            stored.remote_members.clone()
        }),
        None if is_local
            && MembershipConflictPolicy::matches_persisted_branch(history, branch_id)
                .map_err(|_| MembershipLedgerError::Corrupt)? =>
        {
            Some(MembershipConflictPresentation::members(history)?)
        }
        None => None,
    };
    let source_device_ids = if is_local {
        record.local_device_id.clone().into_iter().collect()
    } else {
        conflict.evidence_peer_device_ids.iter().cloned().collect()
    };
    let impact = members
        .as_ref()
        .map(|members| impact(members, is_local, choice, record, history, scope))
        .transpose()?;
    Ok(MembershipConflictBranchView {
        branch_id,
        is_local,
        choice,
        members,
        impact,
        source_device_ids,
    })
}

fn impact(
    members: &[MembershipConflictMember],
    is_local: bool,
    choice: MembershipConflictChoice,
    record: &LoadedMembershipLedger,
    history: &VersionedMembershipHistory,
    scope: &CurrentSpaceMemberScope,
) -> Result<DeviceGroupChoiceImpact, MembershipLedgerError> {
    let local_id = record
        .local_device_id
        .as_ref()
        .ok_or(MembershipLedgerError::Corrupt)?;
    let mut known_peers = history
        .effective_members()
        .into_iter()
        .map(|member| {
            history
                .admission_facts_for(member)
                .map(|facts| facts.device_id.clone())
                .ok_or(MembershipLedgerError::Corrupt)
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    known_peers.remove(local_id);
    let local_membership = if choice == MembershipConflictChoice::RePairingRequired {
        DeviceTrustMembership::Removed
    } else if is_local && !scope.local_member_active {
        DeviceTrustMembership::PendingActivation
    } else {
        DeviceTrustMembership::Active
    };
    let mut sync_scope = BTreeSet::new();
    let mut pending = BTreeSet::new();
    if local_membership != DeviceTrustMembership::Removed {
        for member in members
            .iter()
            .filter(|member| &member.device.device_id != local_id)
        {
            let id = &member.device.device_id;
            if !is_local || !member.active || !scope.local_member_active {
                sync_scope.insert(id.clone());
                pending.insert(id.clone());
            } else if scope.usable_peer_device_ids.contains(id) {
                sync_scope.insert(id.clone());
            } else if scope.paused_peer_devices.iter().any(|peer| {
                &peer.device_id == id
                    && matches!(
                        peer.reason,
                        SpaceMemberPauseReason::RelationshipUnconfirmed
                            | SpaceMemberPauseReason::EffectPending
                    )
            }) {
                sync_scope.insert(id.clone());
                pending.insert(id.clone());
            }
        }
    }
    let paused_device_ids = known_peers.difference(&sync_scope).cloned().collect();
    let candidate_ids = members
        .iter()
        .map(|member| member.device.device_id)
        .collect::<BTreeSet<_>>();
    let mut requires_rejoin = known_peers
        .difference(&candidate_ids)
        .copied()
        .collect::<BTreeSet<_>>();
    if local_membership == DeviceTrustMembership::Removed {
        requires_rejoin.insert(*local_id);
    }
    Ok(DeviceGroupChoiceImpact {
        sync_scope_device_ids: sync_scope.into_iter().collect(),
        paused_device_ids,
        pending_confirmation_device_ids: pending.into_iter().collect(),
        requires_rejoin_device_ids: requires_rejoin.into_iter().collect(),
        local_membership,
    })
}
