//! 已应用分支、祖先与历史包含关系。

use super::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipEventId, VersionedMembershipHistory,
};
use std::collections::BTreeSet;

impl VersionedMembershipHistory {
    pub fn lineage_id(&self) -> &str {
        &self.lineage_id
    }

    pub(crate) fn has_same_activation_baseline(&self, other: &Self) -> bool {
        self.activation_baseline == other.activation_baseline
    }

    pub(crate) fn closest_common_ancestor(&self, other: &Self) -> Option<MembershipEventId> {
        let mut local_ancestors = BTreeSet::new();
        let mut cursor = self.known_head;
        while let Some(event_id) = cursor {
            local_ancestors.insert(event_id);
            cursor = self
                .events
                .get(&event_id)
                .and_then(|event| event.parent_event_id);
        }
        let mut cursor = other.known_head;
        while let Some(event_id) = cursor {
            if local_ancestors.contains(&event_id) {
                return Some(event_id);
            }
            cursor = other
                .events
                .get(&event_id)
                .and_then(|event| event.parent_event_id);
        }
        None
    }

    pub fn is_complete_extension_of(&self, previous: &Self) -> bool {
        if self.lineage_id != previous.lineage_id
            || self.activation_baseline != previous.activation_baseline
            || !previous
                .events
                .iter()
                .all(|(id, event)| self.events.get(id) == Some(event))
            || !previous
                .activation_receipts
                .iter()
                .all(|(id, receipt)| self.activation_receipts.get(id) == Some(receipt))
            || !previous
                .peer_decisions
                .iter()
                .all(|(id, decision)| self.peer_decisions.get(id) == Some(decision))
        {
            return false;
        }
        let Some(previous_head) = previous.known_head else {
            return true;
        };
        let mut cursor = self.known_head;
        while let Some(event_id) = cursor {
            if event_id == previous_head {
                return true;
            }
            cursor = self
                .events
                .get(&event_id)
                .and_then(|event| event.parent_event_id);
        }
        false
    }

    pub fn is_authorized_active_member_extension_of(
        &self,
        previous: &Self,
        source_member: MemberInstanceId,
    ) -> bool {
        if self.lineage_id != previous.lineage_id
            || self.activation_baseline != previous.activation_baseline
            || !previous
                .events
                .iter()
                .all(|(id, event)| self.events.get(id) == Some(event))
            || !previous
                .activation_receipts
                .iter()
                .all(|(id, receipt)| self.activation_receipts.get(id) == Some(receipt))
            || self.peer_decisions.iter().any(|(id, decision)| {
                previous.peer_decisions.get(id) != Some(decision)
                    && decision.decided_by_member_instance_id != source_member
            })
        {
            return false;
        }
        let Some(previous_head) = previous.known_head else {
            return true;
        };
        let mut cursor = self.known_head;
        while let Some(event_id) = cursor {
            if event_id == previous_head {
                return true;
            }
            cursor = self
                .events
                .get(&event_id)
                .and_then(|event| event.parent_event_id);
        }
        false
    }

    /// 判断远端位置是否是本机已验证分支中的严格祖先。
    /// 摘要规划只用它阻止“新节点向旧节点索取旧历史”的反向覆盖。
    pub fn contains_strict_ancestor_position(
        &self,
        position: &BaseMembershipHistoryPosition,
    ) -> bool {
        self.known_head
            .and_then(|head| self.depth(head))
            .is_some_and(|local_depth| position.depth < local_depth)
            && position
                .event_id
                .is_some_and(|event_id| self.events.contains_key(&event_id))
    }
}
