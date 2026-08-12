//! Signed membership history and local destructive-decision rules (ADR-020).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::DeviceId;

use super::{AdmissionChangeFacts, MemberInstanceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipEventId([u8; 32]);

impl MembershipEventId {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipOperation {
    AddDevice { admission: AdmissionChangeFacts },
    RemoveDevice { member: MemberInstanceId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipEvent {
    pub lineage_id: String,
    pub parent_event_id: Option<MembershipEventId>,
    pub parent_depth: u64,
    pub operation_id: [u8; 16],
    pub author_member_instance_id: MemberInstanceId,
    pub operation: MembershipOperation,
    pub resulting_members_digest: [u8; 32],
    pub security_state_digest: [u8; 32],
    /// The protection-state update created by this membership transition.
    /// It is signed with the event so a relaying member cannot substitute
    /// different authority while forwarding the admission history.
    pub security_update_payload: Vec<u8>,
    pub admission_bundle_digest: Option<[u8; 32]>,
    pub signature: Vec<u8>,
}

impl MembershipEvent {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lineage_id: String,
        parent_event_id: Option<MembershipEventId>,
        parent_depth: u64,
        operation_id: [u8; 16],
        author_member_instance_id: MemberInstanceId,
        operation: MembershipOperation,
        resulting_members_digest: [u8; 32],
        security_state_digest: [u8; 32],
        security_update_payload: Vec<u8>,
        admission_bundle_digest: Option<[u8; 32]>,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            lineage_id,
            parent_event_id,
            parent_depth,
            operation_id,
            author_member_instance_id,
            operation,
            resulting_members_digest,
            security_state_digest,
            security_update_payload,
            admission_bundle_digest,
            signature,
        }
    }

    pub fn event_id(&self) -> MembershipEventId {
        MembershipEventId(Sha256::digest(self.canonical_bytes()).into())
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        self.canonical_bytes()
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(256 + self.lineage_id.len() + self.signature.len());
        bytes.extend_from_slice(b"uniclipboard-membership-event/v1\0");
        bytes.extend_from_slice(&(self.lineage_id.len() as u64).to_be_bytes());
        bytes.extend_from_slice(self.lineage_id.as_bytes());
        match self.parent_event_id {
            Some(parent) => {
                bytes.push(1);
                bytes.extend_from_slice(parent.as_bytes());
            }
            None => bytes.push(0),
        }
        bytes.extend_from_slice(&self.parent_depth.to_be_bytes());
        bytes.extend_from_slice(&self.operation_id);
        bytes.extend_from_slice(self.author_member_instance_id.as_bytes());
        match &self.operation {
            MembershipOperation::AddDevice { admission } => {
                bytes.push(1);
                append_admission_facts(&mut bytes, &admission);
            }
            MembershipOperation::RemoveDevice { member } => {
                bytes.push(2);
                bytes.extend_from_slice(member.as_bytes());
            }
        }
        bytes.extend_from_slice(&self.resulting_members_digest);
        bytes.extend_from_slice(&self.security_state_digest);
        append_field(&mut bytes, &self.security_update_payload);
        match self.admission_bundle_digest {
            Some(digest) => {
                bytes.push(1);
                bytes.extend_from_slice(&digest);
            }
            None => bytes.push(0),
        }
        bytes
    }
}

fn append_admission_facts(bytes: &mut Vec<u8>, facts: &AdmissionChangeFacts) {
    bytes.extend_from_slice(facts.member_instance.as_bytes());
    append_field(bytes, facts.device_id.as_str().as_bytes());
    append_field(bytes, facts.device_name.as_bytes());
    append_field(bytes, facts.identity_fingerprint.as_display().as_bytes());
    append_field(bytes, &facts.transport_public_key);
    append_field(bytes, &facts.transport_address_blob);
    append_field(bytes, &facts.identity_signature);
}

fn append_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipDecisionId([u8; 32]);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipDecision {
    pub lineage_id: String,
    pub removal_event_id: MembershipEventId,
    pub decided_by_member_instance_id: MemberInstanceId,
    pub decision: RemovalDecision,
    pub observed_applied_head: Option<MembershipEventId>,
    pub resulting_members_digest: [u8; 32],
    pub decision_nonce: [u8; 16],
    pub signature: Vec<u8>,
}

impl MembershipDecision {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lineage_id: String,
        removal_event_id: MembershipEventId,
        decided_by_member_instance_id: MemberInstanceId,
        decision: RemovalDecision,
        observed_applied_head: Option<MembershipEventId>,
        resulting_members_digest: [u8; 32],
        decision_nonce: [u8; 16],
        signature: Vec<u8>,
    ) -> Self {
        Self {
            lineage_id,
            removal_event_id,
            decided_by_member_instance_id,
            decision,
            observed_applied_head,
            resulting_members_digest,
            decision_nonce,
            signature,
        }
    }

    pub fn decision_id(&self) -> MembershipDecisionId {
        MembershipDecisionId(Sha256::digest(self.canonical_bytes()).into())
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        self.canonical_bytes()
    }

    fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(192 + self.lineage_id.len());
        bytes.extend_from_slice(b"uniclipboard-membership-decision/v1\0");
        bytes.extend_from_slice(&(self.lineage_id.len() as u64).to_be_bytes());
        bytes.extend_from_slice(self.lineage_id.as_bytes());
        bytes.extend_from_slice(self.removal_event_id.as_bytes());
        bytes.extend_from_slice(self.decided_by_member_instance_id.as_bytes());
        bytes.push(match self.decision {
            RemovalDecision::Accept => 1,
            RemovalDecision::Reject => 2,
        });
        match self.observed_applied_head {
            Some(head) => {
                bytes.push(1);
                bytes.extend_from_slice(head.as_bytes());
            }
            None => bytes.push(0),
        }
        bytes.extend_from_slice(&self.resulting_members_digest);
        bytes.extend_from_slice(&self.decision_nonce);
        bytes
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

/// The fixed maximum number of signed events carried by one reconciliation
/// response. Larger histories are fetched by subsequent requests.
pub const MAX_MEMBERSHIP_HISTORY_EVENTS_PER_PAGE: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipHistoryProtocolError {
    InvalidLineage,
    PageLimitExceeded,
    EmptyResponse,
    DiscontinuousResponse,
}

impl fmt::Display for MembershipHistoryProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLineage => "membership history protocol lineage is invalid",
            Self::PageLimitExceeded => "membership history page exceeds the protocol limit",
            Self::EmptyResponse => "membership history response is empty",
            Self::DiscontinuousResponse => "membership history response is not continuous",
        })
    }
}

impl std::error::Error for MembershipHistoryProtocolError {}

/// Bounded greeting sent after an authenticated member connection is ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipHistoryHello {
    pub lineage_id: String,
    pub member_instance_id: MemberInstanceId,
    pub admission: AdmissionChangeFacts,
    pub known_head: Option<MembershipEventId>,
    pub applied_head: Option<MembershipEventId>,
    pub applied_members_digest: Option<[u8; 32]>,
}

/// Bounded request for the next continuous page after a known parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipEventsRequest {
    pub lineage_id: String,
    pub after_event_id: Option<MembershipEventId>,
    pub max_events: u16,
}

impl MembershipEventsRequest {
    pub fn validate(&self) -> Result<(), MembershipHistoryProtocolError> {
        if self.lineage_id.is_empty() || self.lineage_id.len() > 128 {
            return Err(MembershipHistoryProtocolError::InvalidLineage);
        }
        if self.max_events == 0
            || usize::from(self.max_events) > MAX_MEMBERSHIP_HISTORY_EVENTS_PER_PAGE
        {
            return Err(MembershipHistoryProtocolError::PageLimitExceeded);
        }
        Ok(())
    }
}

/// One ordered page of signed history events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipEventsResponse {
    pub lineage_id: String,
    pub after_event_id: Option<MembershipEventId>,
    pub events: Vec<MembershipEvent>,
}

impl MembershipEventsResponse {
    pub fn validate(&self) -> Result<(), MembershipHistoryProtocolError> {
        if self.lineage_id.is_empty() || self.lineage_id.len() > 128 {
            return Err(MembershipHistoryProtocolError::InvalidLineage);
        }
        if self.events.is_empty() {
            return Err(MembershipHistoryProtocolError::EmptyResponse);
        }
        if self.events.len() > MAX_MEMBERSHIP_HISTORY_EVENTS_PER_PAGE {
            return Err(MembershipHistoryProtocolError::PageLimitExceeded);
        }
        let mut parent = self.after_event_id;
        for event in &self.events {
            if event.lineage_id != self.lineage_id || event.parent_event_id != parent {
                return Err(MembershipHistoryProtocolError::DiscontinuousResponse);
            }
            parent = Some(event.event_id());
        }
        Ok(())
    }
}

/// Reconciliation-only messages carried on the authenticated member channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipHistoryMessage {
    Hello(MembershipHistoryHello),
    EventsRequest(MembershipEventsRequest),
    EventsResponse(MembershipEventsResponse),
    Decision(MembershipDecision),
    Ack(MembershipHistoryAck),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipHistoryAck {
    Consistent,
    UpdatesApplied,
    RemovalDecisionRequired { removal_event_id: MembershipEventId },
    RemovalAccepted { removal_event_id: MembershipEventId },
    RemovalRejected { removal_event_id: MembershipEventId },
    Diverged,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipReconciliationOutcome {
    UpdatesApplied,
    RemovalDecisionRequired { removal_event_id: MembershipEventId },
    RemovalAccepted { removal_event_id: MembershipEventId },
    RemovalRejected { removal_event_id: MembershipEventId },
    Diverged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipHistoryError {
    InvalidLineage,
    InvalidGenesis,
    UnknownParent,
    InvalidParentDepth,
    NonLinearHistory,
    OperationReplay,
    UnknownRemoval,
    DecisionFromAnotherMember,
    DecisionForAnotherLineage,
    DecisionAtWrongHead,
    DecisionDigestMismatch,
    DuplicateDecision,
    Diverged,
}

impl fmt::Display for MembershipHistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidLineage => "membership history lineage is invalid",
            Self::InvalidGenesis => "membership history genesis is invalid",
            Self::UnknownParent => "membership history parent is unknown",
            Self::InvalidParentDepth => "membership history parent depth is invalid",
            Self::NonLinearHistory => "membership history does not extend the known head",
            Self::OperationReplay => "membership operation identifier was already used",
            Self::UnknownRemoval => "membership decision does not reference a known removal",
            Self::DecisionFromAnotherMember => "membership decision is not from the local member",
            Self::DecisionForAnotherLineage => "membership decision lineage does not match",
            Self::DecisionAtWrongHead => "membership decision was made at another applied head",
            Self::DecisionDigestMismatch => "membership decision result digest does not match",
            Self::DuplicateDecision => "membership removal already has a decision",
            Self::Diverged => "membership history is diverged",
        })
    }
}

impl std::error::Error for MembershipHistoryError {}

/// The local side of a verified, single-parent membership history.
///
/// Signature verification occurs before [`receive_verified`](Self::receive_verified).
/// This type owns the decision boundary: additions advance automatically, while a
/// removal authored by another member stops at the local user decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipReconciliation {
    lineage_id: String,
    own_member_instance_id: MemberInstanceId,
    events: BTreeMap<MembershipEventId, MembershipEvent>,
    operation_ids: BTreeSet<[u8; 16]>,
    known_head: Option<MembershipEventId>,
    applied_head: Option<MembershipEventId>,
    decisions: BTreeMap<MembershipEventId, MembershipDecision>,
    peer_decisions: BTreeMap<(MembershipEventId, MemberInstanceId), MembershipDecision>,
}

impl MembershipReconciliation {
    pub fn new(lineage_id: String, own_member_instance_id: MemberInstanceId) -> Self {
        Self {
            lineage_id,
            own_member_instance_id,
            events: BTreeMap::new(),
            operation_ids: BTreeSet::new(),
            known_head: None,
            applied_head: None,
            decisions: BTreeMap::new(),
            peer_decisions: BTreeMap::new(),
        }
    }

    pub fn known_head(&self) -> Option<MembershipEventId> {
        self.known_head
    }

    pub fn applied_head(&self) -> Option<MembershipEventId> {
        self.applied_head
    }

    /// Number of verified membership events known to this branch.
    pub fn known_event_count(&self) -> usize {
        self.events.len()
    }

    pub fn applied_members_digest(&self) -> Option<[u8; 32]> {
        self.applied_head
            .and_then(|event_id| self.events.get(&event_id))
            .map(|event| event.resulting_members_digest)
    }

    /// Return the events newly included in the local applied branch after
    /// `previous_head`. Events known beyond an unresolved remote removal are
    /// deliberately absent until the local user accepts that removal.
    pub fn newly_applied_events_after(
        &self,
        previous_head: Option<MembershipEventId>,
    ) -> Vec<MembershipEvent> {
        let mut path = Vec::new();
        let mut cursor = self.applied_head;
        while cursor != previous_head {
            let Some(event_id) = cursor else {
                return Vec::new();
            };
            let Some(event) = self.events.get(&event_id) else {
                return Vec::new();
            };
            path.push(event.clone());
            cursor = event.parent_event_id;
        }
        path.reverse();
        path
    }

    pub fn event(&self, event_id: MembershipEventId) -> Option<&MembershipEvent> {
        self.events.get(&event_id)
    }

    /// Device identity that the signed history bound to a member before the
    /// referenced event. This keeps a removed member verifiable when it
    /// returns the decision for that removal.
    pub fn device_for_member_before(
        &self,
        event_id: MembershipEventId,
        member: &MemberInstanceId,
    ) -> Option<DeviceId> {
        let event = self.events.get(&event_id)?;
        self.device_for_member_at(event.parent_event_id, member)
    }

    pub fn decision_for(
        &self,
        removal_event_id: MembershipEventId,
        member: MemberInstanceId,
    ) -> Option<&MembershipDecision> {
        if member == self.own_member_instance_id {
            self.decisions.get(&removal_event_id)
        } else {
            self.peer_decisions.get(&(removal_event_id, member))
        }
    }

    /// Parent position required for the next locally authored event.
    ///
    /// A rejected remote removal is retained as verified evidence, but it is
    /// not part of the local branch. New local changes must extend the last
    /// applied event rather than that rejected branch.
    pub fn next_event_position(&self) -> (Option<MembershipEventId>, u64) {
        match self.applied_head {
            Some(head) => (
                Some(head),
                self.events
                    .get(&head)
                    .map_or(0, |event| event.parent_depth.saturating_add(1)),
            ),
            None => (None, 0),
        }
    }

    pub fn effective_members(&self) -> BTreeSet<MemberInstanceId> {
        let mut chain = Vec::new();
        let mut cursor = self.applied_head;
        while let Some(event_id) = cursor {
            let Some(event) = self.events.get(&event_id) else {
                return BTreeSet::new();
            };
            chain.push(event);
            cursor = event.parent_event_id;
        }
        chain.reverse();
        let mut members = BTreeSet::new();
        for event in chain {
            match &event.operation {
                MembershipOperation::AddDevice { admission } => {
                    members.insert(admission.member_instance);
                }
                MembershipOperation::RemoveDevice { member } => {
                    members.remove(&member);
                }
            }
        }
        members
    }

    /// Device identifier bound to an effective member by the applied history.
    /// Facts after an unresolved removal are deliberately excluded.
    pub fn device_for_member(&self, member: &MemberInstanceId) -> Option<DeviceId> {
        let mut chain = Vec::new();
        let mut cursor = self.applied_head;
        while let Some(event_id) = cursor {
            let event = self.events.get(&event_id)?;
            chain.push(event);
            cursor = event.parent_event_id;
        }
        chain.reverse();

        let mut devices = BTreeMap::new();
        for event in chain {
            match &event.operation {
                MembershipOperation::AddDevice { admission } => {
                    devices.insert(admission.member_instance, admission.device_id.clone());
                }
                MembershipOperation::RemoveDevice { member } => {
                    devices.remove(member);
                }
            }
        }
        devices.get(member).cloned()
    }

    /// Whether this applied history contains an admission for the device.
    /// A later removal deliberately leaves the admission discoverable so the
    /// content gate can distinguish an excluded member from an unknown peer.
    /// Known events after an unresolved removal are excluded because they are
    /// not yet facts of this device's current branch.
    pub fn has_admitted_device(&self, device_id: &DeviceId) -> bool {
        let mut cursor = self.applied_head;
        while let Some(event_id) = cursor {
            let Some(event) = self.events.get(&event_id) else {
                return false;
            };
            if matches!(
                &event.operation,
                MembershipOperation::AddDevice { admission }
                    if admission.device_id == *device_id
            ) {
                return true;
            }
            cursor = event.parent_event_id;
        }
        false
    }

    /// Return one bounded, continuous page from the current known history.
    ///
    /// The caller supplies the last event it already has. `None` requests
    /// the beginning of this lineage. A missing position never produces a
    /// partial page, so the transport cannot skip an unknown parent.
    pub fn events_after(
        &self,
        after_event_id: Option<MembershipEventId>,
        max_events: usize,
    ) -> Vec<MembershipEvent> {
        if max_events == 0
            || after_event_id.is_some_and(|event_id| !self.events.contains_key(&event_id))
        {
            return Vec::new();
        }

        let mut path = Vec::new();
        let mut cursor = self.known_head;
        while let Some(event_id) = cursor {
            let Some(event) = self.events.get(&event_id) else {
                return Vec::new();
            };
            path.push(event.clone());
            cursor = event.parent_event_id;
        }
        path.reverse();

        let start = after_event_id.map_or(0, |event_id| {
            path.iter()
                .position(|event| event.event_id() == event_id)
                .map_or(path.len(), |index| index.saturating_add(1))
        });
        path.into_iter().skip(start).take(max_events).collect()
    }

    pub fn receive_verified(
        &mut self,
        event: MembershipEvent,
    ) -> Result<MembershipReconciliationOutcome, MembershipHistoryError> {
        let event_id = event.event_id();
        if self.events.contains_key(&event_id) {
            return if self.is_on_known_branch(event_id) {
                Ok(self.current_outcome())
            } else {
                Ok(MembershipReconciliationOutcome::Diverged)
            };
        }
        self.validate_event(&event)?;
        if self.operation_ids.contains(&event.operation_id) {
            return Err(MembershipHistoryError::OperationReplay);
        }
        match (self.known_head, event.parent_event_id) {
            (None, None) => {}
            (Some(known_head), Some(parent)) if known_head == parent => {}
            (_, Some(parent)) if !self.events.contains_key(&parent) => {
                return Err(MembershipHistoryError::UnknownParent);
            }
            (Some(_), Some(_)) => {
                let operation_id = event.operation_id;
                self.events.insert(event_id, event);
                self.operation_ids.insert(operation_id);
                return Ok(MembershipReconciliationOutcome::Diverged);
            }
            _ => return Err(MembershipHistoryError::NonLinearHistory),
        }
        let operation_id = event.operation_id;
        self.events.insert(event_id, event);
        self.operation_ids.insert(operation_id);
        self.known_head = Some(event_id);
        self.advance_applied()
    }

    pub fn record_decision(
        &mut self,
        decision: MembershipDecision,
    ) -> Result<MembershipReconciliationOutcome, MembershipHistoryError> {
        if decision.lineage_id != self.lineage_id {
            return Err(MembershipHistoryError::DecisionForAnotherLineage);
        }
        if decision.decided_by_member_instance_id != self.own_member_instance_id {
            return Err(MembershipHistoryError::DecisionFromAnotherMember);
        }
        if decision.observed_applied_head != self.applied_head {
            return Err(MembershipHistoryError::DecisionAtWrongHead);
        }
        let Some(event) = self.events.get(&decision.removal_event_id) else {
            return Err(MembershipHistoryError::UnknownRemoval);
        };
        if !matches!(event.operation, MembershipOperation::RemoveDevice { .. }) {
            return Err(MembershipHistoryError::UnknownRemoval);
        }
        let expected_digest = match decision.decision {
            RemovalDecision::Accept => event.resulting_members_digest,
            RemovalDecision::Reject => self
                .applied_head
                .and_then(|head| self.events.get(&head))
                .map(|applied| applied.resulting_members_digest)
                .ok_or(MembershipHistoryError::DecisionAtWrongHead)?,
        };
        if decision.resulting_members_digest != expected_digest {
            return Err(MembershipHistoryError::DecisionDigestMismatch);
        }
        if self.decisions.contains_key(&decision.removal_event_id) {
            return Err(MembershipHistoryError::DuplicateDecision);
        }
        let removal_event_id = decision.removal_event_id;
        let choice = decision.decision;
        self.decisions.insert(removal_event_id, decision);
        match choice {
            RemovalDecision::Accept => {
                self.advance_applied()?;
                Ok(MembershipReconciliationOutcome::RemovalAccepted { removal_event_id })
            }
            RemovalDecision::Reject => {
                // Keep the verified remote event for audit and deduplication,
                // while continuing the locally accepted branch.
                self.known_head = self.applied_head;
                Ok(MembershipReconciliationOutcome::RemovalRejected { removal_event_id })
            }
        }
    }

    /// Record a verified decision returned by another member. It cannot
    /// advance this device's branch: only the local user's own decision does
    /// that. The original membership relationship before the removal remains
    /// the authority for who may make this decision.
    pub fn record_peer_decision(
        &mut self,
        decision: MembershipDecision,
    ) -> Result<MembershipReconciliationOutcome, MembershipHistoryError> {
        if decision.lineage_id != self.lineage_id {
            return Err(MembershipHistoryError::DecisionForAnotherLineage);
        }
        if decision.decided_by_member_instance_id == self.own_member_instance_id {
            return Err(MembershipHistoryError::DecisionFromAnotherMember);
        }
        let Some(event) = self.events.get(&decision.removal_event_id) else {
            return Err(MembershipHistoryError::UnknownRemoval);
        };
        if !matches!(event.operation, MembershipOperation::RemoveDevice { .. }) {
            return Err(MembershipHistoryError::UnknownRemoval);
        }
        if decision.decided_by_member_instance_id == event.author_member_instance_id {
            return Err(MembershipHistoryError::DecisionFromAnotherMember);
        }
        if decision.observed_applied_head != event.parent_event_id {
            return Err(MembershipHistoryError::DecisionAtWrongHead);
        }
        if !self.member_was_active_before(
            decision.removal_event_id,
            decision.decided_by_member_instance_id,
        ) {
            return Err(MembershipHistoryError::DecisionFromAnotherMember);
        }
        let expected_digest = match decision.decision {
            RemovalDecision::Accept => event.resulting_members_digest,
            RemovalDecision::Reject => event
                .parent_event_id
                .and_then(|parent| self.events.get(&parent))
                .map(|parent| parent.resulting_members_digest)
                .ok_or(MembershipHistoryError::DecisionAtWrongHead)?,
        };
        if decision.resulting_members_digest != expected_digest {
            return Err(MembershipHistoryError::DecisionDigestMismatch);
        }
        let key = (
            decision.removal_event_id,
            decision.decided_by_member_instance_id,
        );
        if self.peer_decisions.contains_key(&key) {
            let removal_event_id = decision.removal_event_id;
            return match self.peer_decisions.get(&key) {
                Some(existing) if existing == &decision => Ok(match decision.decision {
                    RemovalDecision::Accept => {
                        MembershipReconciliationOutcome::RemovalAccepted { removal_event_id }
                    }
                    RemovalDecision::Reject => {
                        MembershipReconciliationOutcome::RemovalRejected { removal_event_id }
                    }
                }),
                Some(_) => Ok(MembershipReconciliationOutcome::Diverged),
                None => Err(MembershipHistoryError::DuplicateDecision),
            };
        }
        let removal_event_id = decision.removal_event_id;
        let choice = decision.decision;
        self.peer_decisions.insert(key, decision);
        Ok(match choice {
            RemovalDecision::Accept => {
                MembershipReconciliationOutcome::RemovalAccepted { removal_event_id }
            }
            RemovalDecision::Reject => {
                MembershipReconciliationOutcome::RemovalRejected { removal_event_id }
            }
        })
    }

    fn validate_event(&self, event: &MembershipEvent) -> Result<(), MembershipHistoryError> {
        if event.lineage_id.is_empty() || event.lineage_id.len() > 128 {
            return Err(MembershipHistoryError::InvalidLineage);
        }
        if event.lineage_id != self.lineage_id {
            return Err(MembershipHistoryError::InvalidLineage);
        }
        match event.parent_event_id {
            None if event.parent_depth != 0 => Err(MembershipHistoryError::InvalidGenesis),
            Some(parent) => {
                let Some(parent_event) = self.events.get(&parent) else {
                    return Err(MembershipHistoryError::UnknownParent);
                };
                if event.parent_depth != parent_event.parent_depth.saturating_add(1) {
                    return Err(MembershipHistoryError::InvalidParentDepth);
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn current_outcome(&self) -> MembershipReconciliationOutcome {
        if let Some(removal_event_id) = self.pending_removal_decision() {
            return MembershipReconciliationOutcome::RemovalDecisionRequired { removal_event_id };
        }
        MembershipReconciliationOutcome::UpdatesApplied
    }

    fn is_on_known_branch(&self, event_id: MembershipEventId) -> bool {
        let mut cursor = self.known_head;
        while let Some(current) = cursor {
            if current == event_id {
                return true;
            }
            cursor = self
                .events
                .get(&current)
                .and_then(|event| event.parent_event_id);
        }
        false
    }

    /// The first received removal which still requires this device's user
    /// decision. The identifier is safe to use as an opaque action target.
    pub fn pending_removal_decision(&self) -> Option<MembershipEventId> {
        let mut path = Vec::new();
        let mut cursor = self.known_head;
        while cursor != self.applied_head {
            let event_id = cursor?;
            let event = self.events.get(&event_id)?;
            path.push((event_id, event));
            cursor = event.parent_event_id;
        }
        path.reverse();
        path.into_iter().find_map(|(event_id, event)| {
            matches!(event.operation, MembershipOperation::RemoveDevice { .. })
                .then_some(event_id)
                .filter(|id| !self.decisions.contains_key(id))
                .filter(|id| self.own_member_was_active_before(*id))
        })
    }

    fn own_member_was_active_before(&self, event_id: MembershipEventId) -> bool {
        self.member_was_active_before(event_id, self.own_member_instance_id)
    }

    fn member_was_active_before(
        &self,
        event_id: MembershipEventId,
        member: MemberInstanceId,
    ) -> bool {
        let Some(event) = self.events.get(&event_id) else {
            return false;
        };
        let mut chain = Vec::new();
        let mut cursor = event.parent_event_id;
        while let Some(ancestor_id) = cursor {
            let Some(ancestor) = self.events.get(&ancestor_id) else {
                return false;
            };
            chain.push(ancestor);
            cursor = ancestor.parent_event_id;
        }
        chain.reverse();
        let mut members = BTreeSet::new();
        for ancestor in chain {
            match &ancestor.operation {
                MembershipOperation::AddDevice { admission } => {
                    members.insert(admission.member_instance);
                }
                MembershipOperation::RemoveDevice { member } => {
                    members.remove(&member);
                }
            }
        }
        members.contains(&member)
    }

    fn device_for_member_at(
        &self,
        head: Option<MembershipEventId>,
        member: &MemberInstanceId,
    ) -> Option<DeviceId> {
        let mut chain = Vec::new();
        let mut cursor = head;
        while let Some(event_id) = cursor {
            let event = self.events.get(&event_id)?;
            chain.push(event);
            cursor = event.parent_event_id;
        }
        chain.reverse();
        let mut devices = BTreeMap::new();
        for event in chain {
            match &event.operation {
                MembershipOperation::AddDevice { admission } => {
                    devices.insert(admission.member_instance, admission.device_id.clone());
                }
                MembershipOperation::RemoveDevice { member } => {
                    devices.remove(member);
                }
            }
        }
        devices.get(member).cloned()
    }

    fn advance_applied(
        &mut self,
    ) -> Result<MembershipReconciliationOutcome, MembershipHistoryError> {
        let mut path = Vec::new();
        let mut cursor = self.known_head;
        while cursor != self.applied_head {
            let Some(event_id) = cursor else {
                return Err(MembershipHistoryError::NonLinearHistory);
            };
            let Some(event) = self.events.get(&event_id) else {
                return Err(MembershipHistoryError::UnknownParent);
            };
            path.push((event_id, event.clone()));
            cursor = event.parent_event_id;
        }
        path.reverse();
        for (event_id, event) in path {
            if matches!(event.operation, MembershipOperation::RemoveDevice { .. })
                && event.author_member_instance_id != self.own_member_instance_id
                && self.own_member_was_active_before(event_id)
            {
                match self
                    .decisions
                    .get(&event_id)
                    .map(|decision| decision.decision)
                {
                    Some(RemovalDecision::Accept) => {}
                    Some(RemovalDecision::Reject) => {
                        return Ok(MembershipReconciliationOutcome::Diverged);
                    }
                    None => {
                        return Ok(MembershipReconciliationOutcome::RemovalDecisionRequired {
                            removal_event_id: event_id,
                        });
                    }
                }
            }
            self.applied_head = Some(event_id);
        }
        Ok(MembershipReconciliationOutcome::UpdatesApplied)
    }
}
