//! 成员历史的构造、身份与当前资格查询。

use super::{
    members_digest, AdmissionChangeFacts, BaseMembershipHistoryPosition, MemberInstanceId,
    MembershipActivationBaselineV2, MembershipCredential, MembershipDecisionV2, MembershipEventId,
    MembershipEventV2, MembershipHistoryV2Error, MembershipOperationV2, RemovalDecision,
    VersionedMembershipHistory,
};
use crate::ids::DeviceId;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

impl VersionedMembershipHistory {
    pub(crate) fn removal_decisions_for(
        &self,
        id: MembershipEventId,
    ) -> impl Iterator<Item = &super::MembershipDecisionV2> {
        self.peer_decisions
            .values()
            .filter(move |decision| decision.removal_event_id == id)
    }
    pub fn new(lineage_id: String) -> Self {
        Self {
            lineage_id,
            events: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            credentials: BTreeMap::new(),
            operation_ids: BTreeSet::new(),
            activation_receipts: BTreeMap::new(),
            peer_decisions: BTreeMap::new(),
            activation_baseline: None,
            known_head: None,
        }
    }

    pub fn new_single_member_root(
        lineage_id: String,
        facts: AdmissionChangeFacts,
        credential: MembershipCredential,
    ) -> Result<Self, MembershipHistoryV2Error> {
        credential.validate()?;
        if lineage_id.is_empty()
            || facts.member_instance != credential.member_instance_id(&facts.device_id)
        {
            return Err(MembershipHistoryV2Error::InvalidActivationBaseline);
        }
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/membership-history-root/v2\0");
        hasher.update((lineage_id.len() as u64).to_be_bytes());
        hasher.update(lineage_id.as_bytes());
        let facts_payload = facts.signing_payload();
        hasher.update((facts_payload.len() as u64).to_be_bytes());
        hasher.update(facts_payload);
        hasher.update(credential.credential_id.as_bytes());
        Self::from_activation_baseline(MembershipActivationBaselineV2::Established {
            lineage_id,
            head_event_id: MembershipEventId::from_bytes(hasher.finalize().into()),
            head_depth: 0,
            current_members: vec![(facts, credential)],
        })
    }

    pub fn from_activation_baseline(
        mut activation_baseline: MembershipActivationBaselineV2,
    ) -> Result<Self, MembershipHistoryV2Error> {
        let lineage_id = activation_baseline.lineage_id().to_owned();
        let mut credential_index = BTreeMap::new();
        match &mut activation_baseline {
            MembershipActivationBaselineV2::Established {
                current_members, ..
            } => {
                current_members
                    .sort_by(|left, right| left.0.member_instance.cmp(&right.0.member_instance));
                for (facts, credential) in current_members.iter() {
                    credential.validate()?;
                    if facts.member_instance != credential.member_instance_id(&facts.device_id)
                        || credential_index
                            .insert(facts.member_instance, credential.clone())
                            .is_some()
                    {
                        return Err(MembershipHistoryV2Error::CredentialConflict);
                    }
                }
            }
        }
        if lineage_id.is_empty() || credential_index.is_empty() {
            return Err(MembershipHistoryV2Error::InvalidActivationBaseline);
        }
        let (head, _) = activation_baseline.head_and_depth();
        let mut history = Self {
            lineage_id,
            events: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            credentials: credential_index,
            operation_ids: BTreeSet::new(),
            activation_receipts: BTreeMap::new(),
            peer_decisions: BTreeMap::new(),
            activation_baseline: Some(activation_baseline),
            known_head: Some(head),
        };
        history.rebuild_snapshots()?;
        Ok(history)
    }

    pub fn depth(&self, event_id: MembershipEventId) -> Option<u64> {
        self.events
            .get(&event_id)
            .map(|event| event.parent_depth)
            .or_else(|| {
                self.activation_baseline.as_ref().and_then(|baseline| {
                    let (head, depth) = baseline.head_and_depth();
                    (head == event_id).then_some(depth)
                })
            })
    }

    /// 按持久化标识查询事件的因果深度，供不持有领域标识类型的恢复流程排序。
    pub fn depth_by_bytes(&self, event_id: &[u8; 32]) -> Option<u64> {
        self.events
            .iter()
            .find_map(|(candidate_id, event)| {
                (candidate_id.as_bytes() == event_id).then_some(event.parent_depth)
            })
            .or_else(|| {
                self.activation_baseline.as_ref().and_then(|baseline| {
                    let (head, depth) = baseline.head_and_depth();
                    (head.as_bytes() == event_id).then_some(depth)
                })
            })
    }

    pub fn credential_for(&self, member: MemberInstanceId) -> Option<&MembershipCredential> {
        self.credentials.get(&member)
    }

    pub fn admission_facts_for(&self, member: MemberInstanceId) -> Option<&AdmissionChangeFacts> {
        self.events
            .values()
            .find_map(|event| match &event.operation {
                MembershipOperationV2::AddDevice { admission }
                    if admission.facts.member_instance == member =>
                {
                    Some(&admission.facts)
                }
                MembershipOperationV2::AddDevice { .. }
                | MembershipOperationV2::RemoveDevice { .. } => None,
            })
            .or_else(|| match self.activation_baseline.as_ref()? {
                MembershipActivationBaselineV2::Established {
                    current_members, ..
                } => current_members
                    .iter()
                    .find_map(|(facts, _)| (facts.member_instance == member).then_some(facts)),
            })
    }

    pub fn admission_author_for(&self, member: MemberInstanceId) -> Option<MemberInstanceId> {
        self.events
            .values()
            .find_map(|event| match &event.operation {
                MembershipOperationV2::AddDevice { admission }
                    if admission.facts.member_instance == member =>
                {
                    Some(event.author_member_instance_id)
                }
                MembershipOperationV2::AddDevice { .. }
                | MembershipOperationV2::RemoveDevice { .. } => None,
            })
    }

    pub fn current_position(
        &self,
    ) -> Result<BaseMembershipHistoryPosition, MembershipHistoryV2Error> {
        let event_id = self.known_head;
        let depth = event_id
            .and_then(|head| self.depth(head))
            .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
        let encoded = self.encode_persisted_v2()?;
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/membership-history-position/v1\0");
        hasher.update((encoded.len() as u64).to_be_bytes());
        hasher.update(encoded);
        Ok(BaseMembershipHistoryPosition {
            event_id,
            depth,
            history_digest: hasher.finalize().into(),
        })
    }

    pub fn effective_members(&self) -> BTreeSet<MemberInstanceId> {
        self.known_head
            .and_then(|head| self.snapshots.get(&head))
            .map(|snapshot| snapshot.members.clone())
            .unwrap_or_default()
    }

    pub fn active_members(&self) -> BTreeSet<MemberInstanceId> {
        self.known_head
            .and_then(|head| self.snapshots.get(&head))
            .map(|snapshot| snapshot.active_members.clone())
            .unwrap_or_default()
    }

    /// Resolves an effective member only from signed admission facts retained
    /// by the current history. External roster projections never grant
    /// membership or supply missing identity facts.
    pub fn effective_member_for_device(&self, device_id: &DeviceId) -> Option<MemberInstanceId> {
        self.effective_members().into_iter().find(|member| {
            self.admission_facts_for(*member)
                .is_some_and(|facts| &facts.device_id == device_id)
        })
    }

    pub fn effective_members_at(&self, event_id: MembershipEventId) -> BTreeSet<MemberInstanceId> {
        self.snapshots
            .get(&event_id)
            .map(|snapshot| snapshot.members.clone())
            .unwrap_or_default()
    }

    pub fn active_members_at(&self, event_id: MembershipEventId) -> BTreeSet<MemberInstanceId> {
        self.snapshots
            .get(&event_id)
            .map(|snapshot| snapshot.active_members.clone())
            .unwrap_or_default()
    }

    pub fn contains_event_id(&self, event_id: &[u8; 32]) -> bool {
        self.events
            .keys()
            .any(|candidate| candidate.as_bytes() == event_id)
    }

    pub fn device_for_member(
        &self,
        member: &MemberInstanceId,
        candidate_devices: &[DeviceId],
    ) -> Option<DeviceId> {
        self.events
            .values()
            .find_map(|event| match &event.operation {
                MembershipOperationV2::AddDevice { admission }
                    if admission.facts.member_instance == *member =>
                {
                    Some(admission.facts.device_id.clone())
                }
                _ => None,
            })
            .or_else(|| {
                let credential = self.credentials.get(member)?;
                candidate_devices
                    .iter()
                    .find(|device| credential.member_instance_id(device) == *member)
                    .cloned()
            })
    }

    pub fn has_admitted_device(
        &self,
        device_id: &DeviceId,
        candidate_devices: &[DeviceId],
    ) -> bool {
        self.credentials.keys().any(|member| {
            self.device_for_member(member, candidate_devices).as_ref() == Some(device_id)
        })
    }

    pub fn member_for_device(
        &self,
        device_id: &DeviceId,
        candidate_devices: &[DeviceId],
    ) -> Option<MemberInstanceId> {
        self.credentials.keys().copied().find(|member| {
            self.device_for_member(member, candidate_devices).as_ref() == Some(device_id)
        })
    }

    pub fn is_restricted_removed_member_extension_of(
        &self,
        previous: &Self,
        source_member: MemberInstanceId,
    ) -> bool {
        !previous.active_members().contains(&source_member)
            && self.is_authorized_decision_delivery_of(previous, source_member)
    }

    pub fn is_authorized_decision_delivery_of(
        &self,
        previous: &Self,
        source_member: MemberInstanceId,
    ) -> bool {
        self.lineage_id == previous.lineage_id
            && self.activation_baseline == previous.activation_baseline
            && (previous.active_members().contains(&source_member)
                || self.has_removal_decision_by(source_member))
            && self
                .events
                .iter()
                .all(|(id, event)| previous.events.get(id) == Some(event))
            && self
                .activation_receipts
                .iter()
                .all(|(id, receipt)| previous.activation_receipts.get(id) == Some(receipt))
            && self.peer_decisions.iter().all(|(id, decision)| {
                previous.peer_decisions.get(id) == Some(decision)
                    || decision.decided_by_member_instance_id == source_member
            })
    }

    pub(super) fn has_removal_decision_by(&self, member: MemberInstanceId) -> bool {
        self.peer_decisions.values().any(|decision| {
            decision.decided_by_member_instance_id == member
                && self
                    .events
                    .get(&decision.removal_event_id)
                    .is_some_and(|event| {
                        matches!(event.operation, MembershipOperationV2::RemoveDevice { .. })
                    })
        })
    }

    pub fn decision_for(
        &self,
        removal_event_id: MembershipEventId,
        decided_by: MemberInstanceId,
    ) -> Option<&MembershipDecisionV2> {
        self.peer_decisions.get(&(removal_event_id, decided_by))
    }

    pub fn latest_decision_on_removal_authored_by(
        &self,
        author: MemberInstanceId,
        decided_by: MemberInstanceId,
    ) -> Option<RemovalDecision> {
        self.peer_decisions
            .values()
            .filter_map(|decision| {
                let event = self.events.get(&decision.removal_event_id)?;
                (event.author_member_instance_id == author
                    && decision.decided_by_member_instance_id == decided_by)
                    .then_some((event.parent_depth, decision.decision))
            })
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, decision)| decision)
    }

    pub fn removal_decision_recipients_for(
        &self,
        decided_by: MemberInstanceId,
    ) -> BTreeSet<MemberInstanceId> {
        let mut recipients = BTreeSet::new();
        for decision in self
            .peer_decisions
            .values()
            .filter(|decision| decision.decided_by_member_instance_id == decided_by)
        {
            let Some(parent) = self
                .events
                .get(&decision.removal_event_id)
                .and_then(|event| event.parent_event_id)
            else {
                continue;
            };
            if let Some(snapshot) = self.snapshots.get(&parent) {
                recipients.extend(snapshot.members.iter().copied());
            }
        }
        recipients.remove(&decided_by);
        recipients
    }

    pub fn removal_choices_diverge(&self, left: MemberInstanceId, right: MemberInstanceId) -> bool {
        self.events.values().any(|event| {
            if !matches!(event.operation, MembershipOperationV2::RemoveDevice { .. }) {
                return false;
            }
            let choice = |member| {
                if event.author_member_instance_id == member {
                    Some(RemovalDecision::Accept)
                } else {
                    self.decision_for(event.event_id(), member)
                        .map(|decision| decision.decision)
                }
            };
            matches!((choice(left), choice(right)), (Some(left), Some(right)) if left != right)
        })
    }

    pub fn current_head(&self) -> Option<MembershipEventId> {
        self.known_head
    }

    pub fn event(&self, event_id: MembershipEventId) -> Option<&MembershipEventV2> {
        self.events.get(&event_id)
    }

    pub fn members_digest_at(&self, event_id: MembershipEventId) -> Option<[u8; 32]> {
        self.snapshots
            .get(&event_id)
            .map(|snapshot| members_digest(&snapshot.members))
    }

    pub fn pending_removal_decision(
        &self,
        local_member: MemberInstanceId,
    ) -> Option<MembershipEventId> {
        let current_head = self.known_head?;
        let current_members = self.snapshots.get(&current_head)?;
        if !current_members.members.contains(&local_member) {
            return None;
        }
        self.events
            .iter()
            .filter(|(_, event)| {
                event.parent_event_id == Some(current_head)
                    && event.author_member_instance_id != local_member
                    && matches!(event.operation, MembershipOperationV2::RemoveDevice { .. })
            })
            .map(|(event_id, _)| *event_id)
            .find(|event_id| !self.peer_decisions.contains_key(&(*event_id, local_member)))
    }

    pub(crate) fn rejected_removal_heads(
        &self,
        local_member: MemberInstanceId,
    ) -> impl Iterator<Item = MembershipEventId> + '_ {
        self.events
            .iter()
            .filter(move |(id, event)| {
                event.parent_event_id == self.known_head
                    && self
                        .peer_decisions
                        .get(&(**id, local_member))
                        .is_some_and(|decision| decision.decision == RemovalDecision::Reject)
            })
            .map(|(id, _)| *id)
    }
}
