use uc_application::facade::{
    DeviceTrustImpact, MembershipConflictBranchView, PendingDeviceTrustChange,
};
use uc_core::membership::{
    MembershipChangeKind, MembershipChangeSide, MembershipConflictChoice,
    MembershipConflictExplanation, MembershipConflictReason, RemovalDecision,
};

use crate::operations::device::member::device_membership;
use crate::{
    DeviceGroupChangeKindSummary, DeviceGroupChangeSideSummary, DeviceGroupChangeSummary,
    DeviceGroupChoiceDeviceSummary, DeviceGroupChoiceImpactSummary, DeviceGroupChoiceMemberSummary,
    DeviceGroupChoiceOptionSummary, DeviceGroupChoiceReasonKind, DeviceGroupChoiceReasonSummary,
    DeviceGroupDecisionSummary, DeviceGroupRemovalDecisionSummary,
};

pub(super) fn reason(value: MembershipConflictExplanation) -> DeviceGroupChoiceReasonSummary {
    DeviceGroupChoiceReasonSummary {
        kind: match value.reason {
            MembershipConflictReason::Unknown => DeviceGroupChoiceReasonKind::Unknown,
            MembershipConflictReason::PendingRemoval => DeviceGroupChoiceReasonKind::PendingRemoval,
            MembershipConflictReason::DifferentRemovals => {
                DeviceGroupChoiceReasonKind::DifferentRemovals
            }
            MembershipConflictReason::RemovalDecisionDisagreement => {
                DeviceGroupChoiceReasonKind::RemovalDecisionDisagreement
            }
            MembershipConflictReason::DivergedHistory => {
                DeviceGroupChoiceReasonKind::DivergedHistory
            }
        },
        changes: value
            .changes
            .into_iter()
            .map(|change| DeviceGroupChangeSummary {
                side: match change.side {
                    MembershipChangeSide::Local => DeviceGroupChangeSideSummary::Local,
                    MembershipChangeSide::Remote => DeviceGroupChangeSideSummary::Remote,
                },
                kind: match change.kind {
                    MembershipChangeKind::AddedDevice => DeviceGroupChangeKindSummary::AddedDevice,
                    MembershipChangeKind::RemovedDevice => {
                        DeviceGroupChangeKindSummary::RemovedDevice
                    }
                },
                actor: device(change.actor),
                target: device(change.target),
            })
            .collect(),
        decisions: value
            .decisions
            .into_iter()
            .map(|decision| DeviceGroupDecisionSummary {
                device: device(decision.device),
                target: device(decision.target),
                decision: match decision.decision {
                    RemovalDecision::Accept => DeviceGroupRemovalDecisionSummary::Accepted,
                    RemovalDecision::Reject => DeviceGroupRemovalDecisionSummary::Rejected,
                },
            })
            .collect(),
        details_complete: value.details_complete,
    }
}

fn device(value: uc_core::membership::MembershipConflictDevice) -> DeviceGroupChoiceDeviceSummary {
    DeviceGroupChoiceDeviceSummary {
        device_id: value.device_id.to_string(),
        display_name: value.display_name,
    }
}

fn members(
    values: &[uc_application::deps::MembershipConflictMember],
    local: &str,
) -> Vec<DeviceGroupChoiceMemberSummary> {
    values
        .iter()
        .map(|member| DeviceGroupChoiceMemberSummary {
            device_id: member.device.device_id.to_string(),
            display_name: member.device.display_name.clone(),
            is_local: member.device.device_id.as_str() == local,
            active: member.active,
        })
        .collect()
}

pub(super) fn branch(
    value: MembershipConflictBranchView,
    local: &str,
) -> DeviceGroupChoiceOptionSummary {
    let members_complete = value.members.is_some();
    let members = members(value.members.as_deref().unwrap_or_default(), local);
    DeviceGroupChoiceOptionSummary {
        choice_id: format!("b:{}", super::encode(value.branch_id.as_bytes())),
        is_current_group: value.is_local,
        requires_re_pairing: value.choice == MembershipConflictChoice::RePairingRequired,
        member_device_ids: members
            .iter()
            .map(|member| member.device_id.clone())
            .collect(),
        members,
        members_complete,
        source_device_ids: ids(&value.source_device_ids),
        impact: value.impact.map(|impact| DeviceGroupChoiceImpactSummary {
            sync_scope_device_ids: ids(&impact.sync_scope_device_ids),
            paused_device_ids: ids(&impact.paused_device_ids),
            pending_confirmation_device_ids: ids(&impact.pending_confirmation_device_ids),
            requires_rejoin_device_ids: ids(&impact.requires_rejoin_device_ids),
            local_device_outcome: device_membership(impact.local_membership),
        }),
    }
}

pub(super) fn pending(
    change: &PendingDeviceTrustChange,
    local: &str,
) -> Vec<DeviceGroupChoiceOptionSummary> {
    [
        (
            "apply",
            false,
            change.includes_local_device,
            &change.apply_impact,
        ),
        ("keep", true, false, &change.keep_current_impact),
    ]
    .into_iter()
    .map(|(choice, current, rejoin, impact)| {
        let members = members(&impact.members, local);
        DeviceGroupChoiceOptionSummary {
            choice_id: choice.to_owned(),
            is_current_group: current,
            requires_re_pairing: rejoin,
            member_device_ids: members
                .iter()
                .map(|member| member.device_id.clone())
                .collect(),
            members,
            members_complete: true,
            source_device_ids: vec![if current {
                local.to_owned()
            } else {
                change.proposed_by_device_id.to_string()
            }],
            impact: Some(pending_impact(impact, local)),
        }
    })
    .collect()
}

fn pending_impact(value: &DeviceTrustImpact, local: &str) -> DeviceGroupChoiceImpactSummary {
    let peers = |values: &[uc_core::DeviceId]| {
        values
            .iter()
            .filter(|id| id.as_str() != local)
            .map(ToString::to_string)
            .collect()
    };
    DeviceGroupChoiceImpactSummary {
        sync_scope_device_ids: peers(&value.usable_device_ids),
        paused_device_ids: peers(&value.paused_device_ids),
        pending_confirmation_device_ids: peers(&value.pending_confirmation_device_ids),
        requires_rejoin_device_ids: ids(&value.requires_rejoin_device_ids),
        local_device_outcome: device_membership(value.local_membership),
    }
}

fn ids(values: &[uc_core::DeviceId]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_preserves_user_names_without_localized_messages() {
        let device = |id: &str, name: &str| uc_core::membership::MembershipConflictDevice {
            device_id: uc_core::DeviceId::new(id),
            display_name: name.to_owned(),
        };
        let value = reason(MembershipConflictExplanation {
            reason: MembershipConflictReason::RemovalDecisionDisagreement,
            changes: vec![uc_core::membership::MembershipChangeFact {
                side: MembershipChangeSide::Remote,
                kind: MembershipChangeKind::RemovedDevice,
                actor: device("private-actor", "办公电脑"),
                target: device("private-target", "我的手机"),
            }],
            decisions: vec![uc_core::membership::MembershipDecisionFact {
                device: device("private-decider", "家里的电脑"),
                decision: RemovalDecision::Reject,
                target: device("private-target", "我的手机"),
            }],
            details_complete: true,
        });
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(json["kind"], "removal_decision_disagreement");
        assert_eq!(json["changes"][0]["actor"]["display_name"], "办公电脑");
        assert_eq!(json["changes"][0]["side"], "remote");
        assert_eq!(json["decisions"][0]["decision"], "rejected");
        for key in ["message", "locale", "i18n_key", "description"] {
            assert!(json.get(key).is_none());
        }
        let debug = format!("{value:?}");
        for private in [
            "private-actor",
            "private-target",
            "private-decider",
            "办公电脑",
            "我的手机",
            "家里的电脑",
        ] {
            assert!(!debug.contains(private));
        }
    }

    #[test]
    fn missing_presentation_is_not_a_known_empty_group() {
        let value = DeviceGroupChoiceOptionSummary {
            choice_id: "opaque-choice".to_owned(),
            is_current_group: false,
            requires_re_pairing: false,
            member_device_ids: Vec::new(),
            members: Vec::new(),
            members_complete: false,
            source_device_ids: Vec::new(),
            impact: None,
        };
        let json = serde_json::to_value(value).unwrap();
        assert_eq!(json["members_complete"], false);
        assert!(json["impact"].is_null());
        assert_eq!(
            serde_json::to_value(DeviceGroupChoiceReasonSummary::default()).unwrap()["kind"],
            "unknown"
        );
    }
}
