//! Persisted workspace membership state owned by the signed history model.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;
use crate::ports::pairing::PairingSessionId;
use crate::security::IdentityFingerprint;

use super::member_instance::MemberInstanceId;
use super::membership_history::{
    MembershipDecision, MembershipEventId, MembershipHistoryRelationship, MembershipReconciliation,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAppliedMembershipEffect {
    pub event_id: MembershipEventId,
    pub member_facts_completed: bool,
    pub security_update_completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMembershipDecisionDelivery {
    pub recipient: DeviceId,
    pub decision: MembershipDecision,
}

/// Facts required to save a member's local roster and transport record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionChangeFacts {
    pub member_instance: MemberInstanceId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub identity_fingerprint: IdentityFingerprint,
    pub transport_public_key: Vec<u8>,
    pub transport_address_blob: Vec<u8>,
    pub identity_signature: Vec<u8>,
}

impl AdmissionChangeFacts {
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard-workspace-admission/v1\\0");
        bytes.extend_from_slice(self.member_instance.as_bytes());
        bytes.extend_from_slice(self.device_id.as_str().as_bytes());
        bytes.extend_from_slice(self.device_name.as_bytes());
        bytes.extend_from_slice(self.identity_fingerprint.as_display().as_bytes());
        bytes.extend_from_slice(&self.transport_public_key);
        bytes.extend_from_slice(&self.transport_address_blob);
        bytes
    }
}

/// The sponsor's durable record for a pairing session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAdmissionRecord {
    pub joiner_device_id: DeviceId,
    pub invitation_generation: u64,
    pub created_at_ms: i64,
}

/// Facts returned after the sponsor has saved the signed membership event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionSavedFacts {
    pub history_digest: [u8; 32],
    pub history_event_count: u64,
    pub sponsor_facts: AdmissionChangeFacts,
}

/// Stable failure category published without underlying error material.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceFailureCategory {
    SpaceMismatch,
    ContinuityGap,
    IdentityMismatch,
    DigestConflict,
    Unauthorized,
    VersionIncompatible,
    NoEffectiveMembers,
    Storage,
}

/// The local state of a membership history branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePhase {
    LocallyApplied,
    Converging,
    Complete,
    RecoveryRequired,
}

impl WorkspacePhase {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::RecoveryRequired)
    }
}

/// Internal state transitions. Presence and old change delivery are not
/// represented here: only membership history can change membership facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceConvergenceEvent {
    AdmissionBegan {
        session: PairingSessionId,
        joiner_device_id: DeviceId,
        invitation_generation: u64,
    },
    AdmissionCleared {
        session: PairingSessionId,
    },
    PeerHistoryRelationshipUpdated {
        peer: DeviceId,
        relationship: MembershipHistoryRelationship,
    },
    LocalAdmissionReady {
        own_instance: MemberInstanceId,
    },
    IntegrityFailure(WorkspaceFailureCategory),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMergeOutcome {
    Updated,
    Unchanged,
    Stale,
    Rejected(WorkspaceFailureCategory),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceEffect {
    pub persist: bool,
    pub wake_runtime: bool,
    pub publish: bool,
}

impl WorkspaceEffect {
    pub const NONE: Self = Self {
        persist: false,
        wake_runtime: false,
        publish: false,
    };
    pub const PERSIST: Self = Self {
        persist: true,
        wake_runtime: false,
        publish: false,
    };
    pub const PERSIST_AND_PUBLISH: Self = Self {
        persist: true,
        wake_runtime: false,
        publish: true,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceConvergenceError {
    InvalidEvent,
}

/// Complete encrypted state for one workspace. Membership history is the
/// sole source of member instances and the local applied branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConvergenceState {
    pub space_lineage: String,
    pub own_instance: Option<MemberInstanceId>,
    pub peer_history_relationships: BTreeMap<DeviceId, MembershipHistoryRelationship>,
    pub membership_reconciliation: Option<MembershipReconciliation>,
    #[serde(default)]
    pub pending_applied_membership_effects: Vec<PendingAppliedMembershipEffect>,
    #[serde(default)]
    pub pending_membership_decision_deliveries: Vec<PendingMembershipDecisionDelivery>,
    pub pending_admissions: BTreeMap<PairingSessionId, PendingAdmissionRecord>,
    pub phase: WorkspacePhase,
    pub failure_category: Option<WorkspaceFailureCategory>,
    pub revision: u64,
    pub removed: bool,
    pub updated_at_ms: i64,
}

impl Default for WorkspaceConvergenceState {
    fn default() -> Self {
        Self {
            space_lineage: String::new(),
            own_instance: None,
            peer_history_relationships: BTreeMap::new(),
            membership_reconciliation: None,
            pending_applied_membership_effects: Vec::new(),
            pending_membership_decision_deliveries: Vec::new(),
            pending_admissions: BTreeMap::new(),
            phase: WorkspacePhase::LocallyApplied,
            failure_category: None,
            revision: 0,
            removed: false,
            updated_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceDigest([u8; 32]);

impl WorkspaceDigest {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for WorkspaceDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub phase: WorkspacePhase,
    pub revision: u64,
    pub history_event_count: usize,
    pub effective_member_count: usize,
    pub pending_removal_decision_device_ids: Vec<DeviceId>,
    pub pending_removal_decision_event_id: Option<super::MembershipEventId>,
    pub diverged_peer_device_ids: Vec<DeviceId>,
    pub upgrade_required_peer_device_ids: Vec<DeviceId>,
    pub convergence_digest: Option<WorkspaceDigest>,
    pub removed: bool,
    pub updated_at_ms: i64,
    pub failure_category: Option<WorkspaceFailureCategory>,
}

impl WorkspaceConvergenceState {
    pub fn fresh(lineage: String, now_ms: i64) -> Self {
        Self {
            space_lineage: lineage,
            updated_at_ms: now_ms,
            ..Self::default()
        }
    }

    pub fn current_digest(&self) -> Option<WorkspaceDigest> {
        self.membership_reconciliation
            .as_ref()
            .and_then(MembershipReconciliation::applied_members_digest)
            .map(WorkspaceDigest::from_bytes)
    }

    pub fn effective_members(&self) -> BTreeSet<MemberInstanceId> {
        self.membership_reconciliation
            .as_ref()
            .map_or_else(BTreeSet::new, MembershipReconciliation::effective_members)
    }

    pub fn latest_instance_for_device(&self, device_id: &DeviceId) -> Option<MemberInstanceId> {
        self.membership_reconciliation.as_ref().and_then(|history| {
            history
                .effective_members()
                .into_iter()
                .find(|member| history.device_for_member(member).as_ref() == Some(device_id))
        })
    }

    pub fn is_device_removed(&self, device_id: &DeviceId) -> bool {
        self.membership_reconciliation
            .as_ref()
            .is_some_and(|history| {
                history.has_admitted_device(device_id)
                    && history.effective_members().into_iter().all(|member| {
                        history.device_for_member(&member).as_ref() != Some(device_id)
                    })
            })
    }

    pub fn allows_normal_exchange(&self, device_id: &DeviceId) -> bool {
        !matches!(
            self.peer_history_relationships.get(device_id),
            Some(
                MembershipHistoryRelationship::PendingRemovalDecision
                    | MembershipHistoryRelationship::UpgradeRequired
                    | MembershipHistoryRelationship::Diverged
                    | MembershipHistoryRelationship::Invalid
            )
        )
    }

    fn advance(&mut self, now_ms: i64) {
        self.phase = if self.failure_category.is_some() {
            WorkspacePhase::RecoveryRequired
        } else {
            WorkspacePhase::LocallyApplied
        };
        self.updated_at_ms = now_ms;
        self.revision = self.revision.saturating_add(1);
    }

    pub fn apply(
        &mut self,
        event: WorkspaceConvergenceEvent,
        now_ms: i64,
    ) -> Result<(WorkspaceMergeOutcome, WorkspaceEffect), WorkspaceConvergenceError> {
        if self.failure_category.is_some()
            && !matches!(event, WorkspaceConvergenceEvent::IntegrityFailure(_))
        {
            return Ok((WorkspaceMergeOutcome::Unchanged, WorkspaceEffect::NONE));
        }
        match event {
            WorkspaceConvergenceEvent::AdmissionBegan {
                session,
                joiner_device_id,
                invitation_generation,
            } => {
                let record = PendingAdmissionRecord {
                    joiner_device_id,
                    invitation_generation,
                    created_at_ms: now_ms,
                };
                let changed = self.pending_admissions.get(&session) != Some(&record);
                if changed {
                    self.pending_admissions.insert(session, record);
                    self.advance(now_ms);
                }
                Ok((
                    if changed {
                        WorkspaceMergeOutcome::Updated
                    } else {
                        WorkspaceMergeOutcome::Unchanged
                    },
                    if changed {
                        WorkspaceEffect::PERSIST
                    } else {
                        WorkspaceEffect::NONE
                    },
                ))
            }
            WorkspaceConvergenceEvent::AdmissionCleared { session } => {
                let changed = self.pending_admissions.remove(&session).is_some();
                if changed {
                    self.advance(now_ms);
                }
                Ok((
                    if changed {
                        WorkspaceMergeOutcome::Updated
                    } else {
                        WorkspaceMergeOutcome::Unchanged
                    },
                    if changed {
                        WorkspaceEffect::PERSIST
                    } else {
                        WorkspaceEffect::NONE
                    },
                ))
            }
            WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated { peer, relationship } => {
                let changed = self
                    .peer_history_relationships
                    .insert(peer, relationship)
                    .is_none_or(|previous| previous != relationship);
                if changed {
                    self.advance(now_ms);
                }
                Ok((
                    if changed {
                        WorkspaceMergeOutcome::Updated
                    } else {
                        WorkspaceMergeOutcome::Unchanged
                    },
                    if changed {
                        WorkspaceEffect::PERSIST_AND_PUBLISH
                    } else {
                        WorkspaceEffect::NONE
                    },
                ))
            }
            WorkspaceConvergenceEvent::LocalAdmissionReady { own_instance } => {
                let changed = self.own_instance != Some(own_instance);
                self.own_instance = Some(own_instance);
                if changed || self.membership_reconciliation.is_none() {
                    self.membership_reconciliation = Some(MembershipReconciliation::new(
                        self.space_lineage.clone(),
                        own_instance,
                    ));
                    self.pending_admissions.clear();
                    self.removed = false;
                }
                self.advance(now_ms);
                Ok((WorkspaceMergeOutcome::Updated, WorkspaceEffect::PERSIST))
            }
            WorkspaceConvergenceEvent::IntegrityFailure(category) => {
                self.failure_category = Some(category);
                self.advance(now_ms);
                Ok((
                    WorkspaceMergeOutcome::Updated,
                    WorkspaceEffect::PERSIST_AND_PUBLISH,
                ))
            }
        }
    }

    pub fn snapshot(&self) -> WorkspaceSnapshot {
        let history = self.membership_reconciliation.as_ref();
        WorkspaceSnapshot {
            phase: self.phase,
            revision: self.revision,
            history_event_count: history.map_or(0, MembershipReconciliation::known_event_count),
            effective_member_count: self.effective_members().len(),
            pending_removal_decision_device_ids: self
                .peer_history_relationships
                .iter()
                .filter_map(|(device, relationship)| {
                    (*relationship == MembershipHistoryRelationship::PendingRemovalDecision)
                        .then(|| device.clone())
                })
                .collect(),
            pending_removal_decision_event_id: history
                .and_then(MembershipReconciliation::pending_removal_decision),
            diverged_peer_device_ids: self
                .peer_history_relationships
                .iter()
                .filter_map(|(device, relationship)| {
                    (*relationship == MembershipHistoryRelationship::Diverged)
                        .then(|| device.clone())
                })
                .collect(),
            upgrade_required_peer_device_ids: self
                .peer_history_relationships
                .iter()
                .filter_map(|(device, relationship)| {
                    (*relationship == MembershipHistoryRelationship::UpgradeRequired)
                        .then(|| device.clone())
                })
                .collect(),
            convergence_digest: self.current_digest(),
            removed: self.removed,
            updated_at_ms: self.updated_at_ms,
            failure_category: self.failure_category,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn admission(member: MemberInstanceId, device_id: &str) -> AdmissionChangeFacts {
        AdmissionChangeFacts {
            member_instance: member,
            device_id: DeviceId::new(device_id),
            device_name: "device".to_owned(),
            identity_fingerprint: IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                .unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        }
    }

    fn event(
        parent: Option<super::super::MembershipEventId>,
        parent_depth: u64,
        author: MemberInstanceId,
        operation: super::super::MembershipOperation,
        operation_byte: u8,
    ) -> super::super::MembershipEvent {
        super::super::MembershipEvent::new(
            "lineage".to_owned(),
            parent,
            parent_depth,
            [operation_byte; 16],
            author,
            operation,
            [operation_byte; 32],
            [operation_byte.saturating_add(1); 32],
            Vec::new(),
            None,
            vec![operation_byte],
        )
    }

    #[test]
    fn local_admission_creates_the_only_membership_history_source() {
        let own = MemberInstanceId::from_bytes([7; 32]);
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        state
            .apply(
                WorkspaceConvergenceEvent::LocalAdmissionReady { own_instance: own },
                2,
            )
            .unwrap();
        assert_eq!(state.own_instance, Some(own));
        assert_eq!(
            state
                .membership_reconciliation
                .as_ref()
                .map(MembershipReconciliation::known_event_count),
            Some(0)
        );
    }

    #[test]
    fn accepted_removal_blocks_the_removed_device_from_normal_exchange() {
        let a = MemberInstanceId::from_bytes([1; 32]);
        let b = MemberInstanceId::from_bytes([2; 32]);
        let mut history = MembershipReconciliation::new("lineage".to_owned(), a);
        let genesis = event(
            None,
            0,
            a,
            super::super::MembershipOperation::AddDevice {
                admission: admission(a, "device-a"),
            },
            1,
        );
        let addition = event(
            Some(genesis.event_id()),
            1,
            a,
            super::super::MembershipOperation::AddDevice {
                admission: admission(b, "device-b"),
            },
            2,
        );
        let removal = event(
            Some(addition.event_id()),
            2,
            a,
            super::super::MembershipOperation::RemoveDevice { member: b },
            3,
        );
        assert!(history.receive_verified(genesis).is_ok());
        assert!(history.receive_verified(addition.clone()).is_ok());
        assert!(history.receive_verified(removal).is_ok());

        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        state.membership_reconciliation = Some(history);

        assert!(state.is_device_removed(&DeviceId::new("device-b")));
        assert!(!state.is_device_removed(&DeviceId::new("unknown-device")));
    }

    #[test]
    fn upgrade_required_peer_blocks_normal_exchange_and_is_visible_in_snapshot() {
        let peer = DeviceId::new("device-b");
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);

        state
            .apply(
                WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                    peer: peer.clone(),
                    relationship: MembershipHistoryRelationship::UpgradeRequired,
                },
                2,
            )
            .unwrap();

        assert!(!state.allows_normal_exchange(&peer));
        assert_eq!(
            state.snapshot().upgrade_required_peer_device_ids,
            vec![peer]
        );
    }

    #[test]
    fn pending_history_does_not_mark_a_later_unapplied_admission_as_removed() {
        let a = MemberInstanceId::from_bytes([1; 32]);
        let b = MemberInstanceId::from_bytes([2; 32]);
        let c = MemberInstanceId::from_bytes([3; 32]);
        let mut history = MembershipReconciliation::new("lineage".to_owned(), b);
        let genesis = event(
            None,
            0,
            a,
            super::super::MembershipOperation::AddDevice {
                admission: admission(a, "device-a"),
            },
            1,
        );
        let addition = event(
            Some(genesis.event_id()),
            1,
            a,
            super::super::MembershipOperation::AddDevice {
                admission: admission(b, "device-b"),
            },
            2,
        );
        let removal = event(
            Some(addition.event_id()),
            2,
            a,
            super::super::MembershipOperation::RemoveDevice { member: b },
            3,
        );
        let later_addition = event(
            Some(removal.event_id()),
            3,
            a,
            super::super::MembershipOperation::AddDevice {
                admission: admission(c, "device-c"),
            },
            4,
        );
        assert!(history.receive_verified(genesis).is_ok());
        assert!(history.receive_verified(addition).is_ok());
        assert!(history.receive_verified(removal).is_ok());
        assert!(history.receive_verified(later_addition).is_ok());

        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        state.membership_reconciliation = Some(history);

        assert!(!state.is_device_removed(&DeviceId::new("device-c")));
    }
}
