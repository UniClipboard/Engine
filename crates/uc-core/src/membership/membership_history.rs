//! Shared identifiers and messages for the current membership history.

use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;

use super::versioned_membership_history::{
    BaseMembershipHistoryPosition, MembershipDecisionV2, MembershipEventV2,
    MembershipHistorySuffixPageV3,
};
use super::MemberInstanceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipEventId([u8; 32]);

impl MembershipEventId {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    pub fn from_hex(value: &str) -> Option<Self> {
        if value.len() != 64 {
            return None;
        }
        let mut bytes = [0u8; 32];
        for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            bytes[index] = u8::try_from((high << 4) | low).ok()?;
        }
        Some(Self(bytes))
    }
}

impl fmt::Display for MembershipEventId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipDecisionId([u8; 32]);

impl MembershipDecisionId {
    pub(crate) fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipHistoryRelationship {
    Unknown,
    Consistent,
    UpgradeRequired,
    PendingRemovalDecision,
    Diverged,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRemovalFacts {
    pub removal_event_id: MembershipEventId,
    pub proposed_by_device_id: DeviceId,
    pub target_device_ids: Vec<DeviceId>,
    target_members: BTreeSet<MemberInstanceId>,
}

impl PendingRemovalFacts {
    pub fn new(
        removal_event_id: MembershipEventId,
        proposed_by_device_id: DeviceId,
        target_device_ids: Vec<DeviceId>,
        target_members: BTreeSet<MemberInstanceId>,
    ) -> Self {
        Self {
            removal_event_id,
            proposed_by_device_id,
            target_device_ids,
            target_members,
        }
    }

    pub fn includes_member(&self, member: MemberInstanceId) -> bool {
        self.target_members.contains(&member)
    }
}

/// Versioned reconciliation messages carried on the authenticated member channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipHistoryMessage {
    SummaryV3(MembershipHistorySummaryV3),
    RequestSuffixV3(MembershipHistorySuffixRequestV3),
    SuffixPageV3(MembershipHistorySuffixPageV3),
    AckV3(MembershipHistoryAckV3),
    /// 仅向被普通成员 scope 排除的对端交付指定成员事件。
    RestrictedEventV3(MembershipEventV2),
    /// 仅向被普通成员 scope 排除的对端交付指定成员决定。
    RestrictedDecisionV3(MembershipDecisionV2),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipHistorySummaryV3 {
    pub lineage_id: String,
    pub current_position: BaseMembershipHistoryPosition,
    pub transfer_id: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipHistorySuffixRequestV3 {
    pub transfer_id: [u8; 32],
    pub known_position: BaseMembershipHistoryPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipHistoryAckV3 {
    Continue {
        transfer_id: [u8; 32],
        next_page_index: u32,
    },
    Confirmed {
        transfer_id: [u8; 32],
        confirmed_position: BaseMembershipHistoryPosition,
    },
    RestrictedApplied,
    RestrictedConsistent,
    Diverged,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipHistoryReconciliationPlan {
    Noop,
    OfferSuffix,
    RequestSuffix,
    Diverged,
    Invalid,
}

pub fn plan_membership_history_reconciliation(
    local_lineage: &str,
    local: &BaseMembershipHistoryPosition,
    remote_lineage: &str,
    remote: &BaseMembershipHistoryPosition,
    remote_position_known_locally: bool,
) -> MembershipHistoryReconciliationPlan {
    if local_lineage.is_empty() || remote_lineage != local_lineage {
        return MembershipHistoryReconciliationPlan::Invalid;
    }
    if local == remote {
        return MembershipHistoryReconciliationPlan::Noop;
    }
    if remote.depth < local.depth {
        return if remote_position_known_locally {
            MembershipHistoryReconciliationPlan::OfferSuffix
        } else {
            MembershipHistoryReconciliationPlan::Diverged
        };
    }
    if remote.depth > local.depth {
        return MembershipHistoryReconciliationPlan::RequestSuffix;
    }
    MembershipHistoryReconciliationPlan::Diverged
}

pub fn ack_confirms_membership_history_target(
    expected_transfer_id: [u8; 32],
    expected_position: &BaseMembershipHistoryPosition,
    ack: &MembershipHistoryAckV3,
) -> bool {
    matches!(
        ack,
        MembershipHistoryAckV3::Confirmed {
            transfer_id,
            confirmed_position,
        } if *transfer_id == expected_transfer_id && confirmed_position == expected_position
    )
}

#[cfg(test)]
mod anti_entropy_tests {
    use super::*;

    fn position(depth: u64, digest: u8) -> BaseMembershipHistoryPosition {
        BaseMembershipHistoryPosition {
            event_id: None,
            depth,
            history_digest: [digest; 32],
        }
    }

    #[test]
    fn planner_keeps_relationship_and_delivery_direction_separate() {
        let local = position(3, 3);
        assert_eq!(
            plan_membership_history_reconciliation("space", &local, "space", &local, true),
            MembershipHistoryReconciliationPlan::Noop
        );
        assert_eq!(
            plan_membership_history_reconciliation("space", &local, "space", &position(2, 2), true,),
            MembershipHistoryReconciliationPlan::OfferSuffix
        );
        assert_eq!(
            plan_membership_history_reconciliation(
                "space",
                &local,
                "space",
                &position(4, 4),
                false,
            ),
            MembershipHistoryReconciliationPlan::RequestSuffix
        );
        assert_eq!(
            plan_membership_history_reconciliation(
                "space",
                &local,
                "space",
                &position(2, 9),
                false,
            ),
            MembershipHistoryReconciliationPlan::Diverged
        );
    }

    #[test]
    fn ack_must_bind_the_exact_transfer_and_target() {
        let target = position(3, 3);
        let ack = MembershipHistoryAckV3::Confirmed {
            transfer_id: [7; 32],
            confirmed_position: target.clone(),
        };
        assert!(ack_confirms_membership_history_target(
            [7; 32], &target, &ack
        ));
        assert!(!ack_confirms_membership_history_target(
            [8; 32], &target, &ack
        ));
        assert!(!ack_confirms_membership_history_target(
            [7; 32],
            &position(4, 4),
            &ack
        ));
    }
}
