//! Persisted workspace membership state owned by the signed history model.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;
use crate::security::IdentityFingerprint;

use super::member_instance::MemberInstanceId;
use super::membership_history::MembershipHistoryRelationship;
use super::versioned_membership_history::MembershipHistoryPageV2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMembershipHistoryTransferV2 {
    pub transfer_id: [u8; 32],
    pub page_count: u32,
    pub pages: Vec<MembershipHistoryPageV2>,
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
pub struct SpaceMembershipState {
    pub space_lineage: String,
    pub own_instance: Option<MemberInstanceId>,
    pub peer_history_relationships: BTreeMap<DeviceId, MembershipHistoryRelationship>,
    #[serde(default)]
    pub pending_membership_history_transfers:
        BTreeMap<DeviceId, PendingMembershipHistoryTransferV2>,
    pub phase: WorkspacePhase,
    pub failure_category: Option<WorkspaceFailureCategory>,
    pub revision: u64,
    pub removed: bool,
    pub updated_at_ms: i64,
}

impl Default for SpaceMembershipState {
    fn default() -> Self {
        Self {
            space_lineage: String::new(),
            own_instance: None,
            peer_history_relationships: BTreeMap::new(),
            pending_membership_history_transfers: BTreeMap::new(),
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

impl SpaceMembershipState {
    pub fn fresh(lineage: String, now_ms: i64) -> Self {
        Self {
            space_lineage: lineage,
            updated_at_ms: now_ms,
            ..Self::default()
        }
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
                if changed {
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
        WorkspaceSnapshot {
            phase: self.phase,
            revision: self.revision,
            history_event_count: 0,
            effective_member_count: 0,
            pending_removal_decision_device_ids: self
                .peer_history_relationships
                .iter()
                .filter_map(|(device, relationship)| {
                    (*relationship == MembershipHistoryRelationship::PendingRemovalDecision)
                        .then(|| device.clone())
                })
                .collect(),
            pending_removal_decision_event_id: None,
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
            convergence_digest: None,
            removed: self.removed,
            updated_at_ms: self.updated_at_ms,
            failure_category: self.failure_category,
        }
    }
}
