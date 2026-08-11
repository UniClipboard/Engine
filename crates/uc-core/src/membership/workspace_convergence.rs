//! Workspace-wide convergence domain model (ADR-016).
//!
//! A workspace advances through an ordered, verifiable chain of workspace
//! changes. Each change binds the space lineage, the previous security
//! generation and digest, the change kind and identity, the target security
//! generation and the resulting digest, the verifiable security updates,
//! the admission or removal facts, and the confirming members. All devices
//! derive the same current digest from the same verified chain; delivery
//! path and arrival order cannot change it.
//!
//! This module carries pure domain logic only: change identity derivation,
//! canonical encoding, chain continuity validation, digest computation,
//! phase transitions, and confirmation bookkeeping. Signature verification
//! and the cryptographic verification of security updates are performed by
//! ports outside this module.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::DeviceId;
use crate::ports::pairing::PairingSessionId;
use crate::security::IdentityFingerprint;

use super::gossip::RelayedSecurityUpdate;
use super::removal_intent::{MemberInstanceId, RemovalIntentId, SignedRemovalIntent};

/// Stable identifier of one workspace change, derived from its canonical
/// content. Any field change invalidates the identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WorkspaceChangeId([u8; 32]);

impl WorkspaceChangeId {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for WorkspaceChangeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// What a workspace change does to the effective member set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChangeKind {
    /// A member instance was admitted into the workspace.
    Admission,
    /// Member instances were removed from the workspace.
    Removal,
}

/// Facts needed to verify and apply an admission change: the new member's
/// signed identity, member instance, reachable material, and the material
/// required for direct proof.
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

/// Facts needed to verify and apply a removal change: the verifiable facts
/// for the removed member instances. The removed instances are derived from
/// the validated removal intent set and are listed explicitly so that a
/// receiver can verify the change without reconstructing the intent view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalChangeFacts {
    pub removed_instances: Vec<MemberInstanceId>,
}

/// One verified workspace change.
///
/// The chain is continuous: `previous_epoch + 1 == next_epoch`,
/// `previous_digest` equals the digest of the preceding change (or the
/// initial digest for the first change), and every security update is
/// continuous with the change's generations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceChange {
    pub space_lineage: String,
    pub kind: WorkspaceChangeKind,
    pub previous_epoch: u64,
    pub next_epoch: u64,
    pub previous_digest: [u8; 32],
    pub digest: [u8; 32],
    pub security_updates: Vec<RelayedSecurityUpdate>,
    pub admission: Option<AdmissionChangeFacts>,
    pub removal: Option<RemovalChangeFacts>,
    pub created_at_ms: i64,
}

impl WorkspaceChange {
    /// Stable change identifier derived from the canonical content.
    pub fn change_id(&self) -> WorkspaceChangeId {
        WorkspaceChangeId(compute_change_digest(self))
    }
}

/// Stable rejection category for a malformed or unverifiable change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChangeRejection {
    SpaceMismatch,
    KindMissing,
    EpochGap,
    DigestMismatch,
    SecurityUpdateGap,
    UnknownRemovalTarget,
    InvalidMemberInstance,
}

/// Stable failure category published without any underlying error text.
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

/// Workspace convergence phase published to callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspacePhase {
    /// The local device saved a member change and its local security
    /// effect, or an admitting member saved its minimal relationship,
    /// receivable session and readiness record; other effective members
    /// have not yet been proven to have applied it.
    LocallyApplied,
    /// The continuous changes for the current target are being handed off,
    /// verified, applied or confirmed.
    Converging,
    /// The only remaining condition is one or more known effective members
    /// being temporarily unreachable.
    WaitingForOfflineMember,
    /// Every effective member has saved and applied the same digest and
    /// continuous security state, and all required trusted relationships
    /// and reachable material are on disk.
    Complete,
    /// The current facts cannot safely continue automatically.
    RecoveryRequired,
}

impl WorkspacePhase {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Complete | Self::RecoveryRequired)
    }
}

/// A receiver's verifiable fact that it saved, applied and established
/// relationships for the digest it attests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConfirmation {
    pub member_instance: MemberInstanceId,
    pub digest: [u8; 32],
    pub signature: Vec<u8>,
}

impl WorkspaceConfirmation {
    /// Canonical signing payload.
    pub fn signing_payload(&self) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(32 + 32);
        buffer.extend_from_slice(b"uniclipboard-workspace-confirmation/v1\0");
        buffer.extend_from_slice(self.member_instance.as_bytes());
        buffer.extend_from_slice(&self.digest);
        buffer
    }
}

/// Pending handoff bookkeeping for one recipient that has not yet confirmed
/// the current target digest. Only an acknowledgement of the current target
/// digest clears the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingHandoff {
    pub recipient: MemberInstanceId,
    pub recipient_device: DeviceId,
    /// Highest continuous range the recipient has durably confirmed.
    pub confirmed_epoch: u64,
    pub target_digest: [u8; 32],
    /// Whether another batch remains after the next one.
    pub has_more: bool,
}

/// The sponsor's in-flight admission record for one join attempt. Saved
/// before the sponsor starts waiting for the joiner's readiness, and
/// cleared in the same commit that saves the admission change. It survives
/// restarts so the sponsor re-awaits the same joiner's readiness instead of
/// saving a second member instance or a duplicated change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingAdmissionRecord {
    pub joiner_device_id: DeviceId,
    /// The admission generation the invitation was bound to. The change is
    /// only committed while the generation has not advanced.
    pub invitation_generation: u64,
    pub created_at_ms: i64,
}

/// The sponsor's confirmation that the joiner's admission change and the
/// pending handoff facts were saved in one commit. The joiner records it as
/// its local admission progress before it may take part in ordinary content
/// exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCommittedFacts {
    pub change_digest: [u8; 32],
    pub change_count: u64,
    pub sponsor_facts: AdmissionChangeFacts,
}

/// Event that advances the workspace convergence state machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkspaceConvergenceEvent {
    /// The local device saved a new workspace change (an admission after
    /// the joiner's readiness, a removal, or a verified relayed change).
    CommittedChange(WorkspaceChange),
    /// The sponsor saved the in-flight admission record before waiting for
    /// the joiner's readiness. Idempotent for the same session and joiner.
    AdmissionBegan {
        session: PairingSessionId,
        joiner_device_id: DeviceId,
        invitation_generation: u64,
    },
    /// The sponsor cleared an in-flight admission record in the same commit
    /// that saved the joiner's admission change. Idempotent for a missing
    /// session.
    AdmissionCleared { session: PairingSessionId },
    /// The joiner durably recorded the sponsor's confirmation that its
    /// admission change was saved; it may now take part in ordinary content
    /// exchange and count itself into completion confirmations. Idempotent
    /// for the same digest.
    LocalAdmissionCommitted(AdmissionCommittedFacts),
    /// An effective member confirmed the current digest.
    ConfirmationReceived(WorkspaceConfirmation),
    /// A recipient durably confirmed a continuous range of the current
    /// target digest; `has_more` tells whether another batch remains.
    HandoffAdvanced {
        recipient: MemberInstanceId,
        confirmed_epoch: u64,
        target_digest: [u8; 32],
        has_more: bool,
    },
    /// A peer durably acknowledged a validated removal intent on the
    /// ordinary member channel.
    IntentAcknowledged {
        peer: DeviceId,
        intent_id: RemovalIntentId,
    },
    /// A validated removal notice announced the local instance's removal.
    RemovalNoticeAccepted(RemovalIntentId),
    /// A removal notice was durably delivered to the removed target device
    /// (best-effort delivery progress, not a completion condition).
    RemovalNoticeDelivered(RemovalIntentId),
    /// A pending handoff record was saved for a recipient that has not yet
    /// confirmed the current target digest. Creating or replacing the
    /// record for the same recipient is idempotent for the same target
    /// digest.
    PendingHandoffCreated {
        recipient: MemberInstanceId,
        recipient_device: DeviceId,
        confirmed_epoch: u64,
        target_digest: [u8; 32],
        has_more: bool,
    },
    /// A known effective member is temporarily unreachable.
    MemberUnreachable(MemberInstanceId),
    /// A previously unreachable known effective member came back.
    MemberReachable(MemberInstanceId),
    /// The local device became a member instance and saved its minimal
    /// relationship, receivable session and readiness record.
    LocalAdmissionReady { own_instance: MemberInstanceId },
    /// Current facts cannot safely continue.
    IntegrityFailure(WorkspaceFailureCategory),
}

/// What changed as a result of applying an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceMergeOutcome {
    Updated,
    Unchanged,
    Stale,
    Rejected(WorkspaceFailureCategory),
}

/// Side effects the caller must perform after applying an event.
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
    pub const PERSIST_AND_WAKE: Self = Self {
        persist: true,
        wake_runtime: true,
        publish: false,
    };
    pub const PERSIST_WAKE_AND_PUBLISH: Self = Self {
        persist: true,
        wake_runtime: true,
        publish: true,
    };
}

/// Error returned by the state machine when an event cannot be merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceConvergenceError {
    ChangeRejected(WorkspaceChangeRejection),
    ConfirmationForUnknownMember,
    ConfirmationDigestMismatch,
    HandoffForUnknownRecipient,
    InvalidEvent,
}

/// The complete persisted workspace convergence state.
///
/// A single serializable structure so the state survives restarts without
/// any in-memory-only bookkeeping. All fields that can carry business
/// material are stored encrypted by the persistence layer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceConvergenceState {
    pub space_lineage: String,
    pub own_instance: Option<MemberInstanceId>,
    /// Ordered chain of verified workspace changes.
    pub changes: Vec<WorkspaceChange>,
    /// Confirmed members by member instance.
    pub confirmed: BTreeMap<MemberInstanceId, WorkspaceConfirmation>,
    /// Pending handoff records by recipient instance.
    pub pending_handoffs: BTreeMap<MemberInstanceId, PendingHandoff>,
    /// Effective members currently known to be temporarily unreachable.
    pub waiting_members: BTreeSet<MemberInstanceId>,
    /// Validated removal intent identifiers still contributing to the
    /// current target.
    pub removal_intents: BTreeSet<RemovalIntentId>,
    /// Validated removal intents, used to compute the unified target when a
    /// new removal change is formed.
    pub removal_intent_records: Vec<SignedRemovalIntent>,
    /// Intent propagation progress on the ordinary member channel:
    /// (recipient device, intent id) → acknowledgement time.
    pub peer_intent_acks: BTreeMap<(DeviceId, RemovalIntentId), i64>,
    /// Removal intents whose notice was durably delivered to the removed
    /// target device. Best-effort progress, not a completion condition.
    pub notified_removals: BTreeSet<RemovalIntentId>,
    /// Removal notices already accepted by this local instance. This is kept
    /// separately from the sender's delivery progress so a repeated notice
    /// can return a stable idempotent result.
    pub accepted_removal_notices: BTreeSet<RemovalIntentId>,
    /// Stable member instance to device mapping, derived from admission
    /// changes. Removed instances stay in the mapping so the content-send
    /// gate and the removal-notice planner can resolve historical targets.
    pub member_devices: BTreeMap<MemberInstanceId, DeviceId>,
    /// In-flight sponsor admission records by pairing session. Saved before
    /// the sponsor waits for the joiner's readiness; cleared in the commit
    /// that saves the admission change. Survives restarts.
    pub pending_admissions: BTreeMap<PairingSessionId, PendingAdmissionRecord>,
    /// The sponsor's admission-saved confirmation recorded by the local
    /// joiner. `None` until the joiner receives the confirmation.
    pub local_admission_committed: Option<AdmissionCommittedFacts>,
    pub phase: WorkspacePhase,
    pub failure_category: Option<WorkspaceFailureCategory>,
    /// Monotonic revision bumped on every successful persist.
    pub revision: u64,
    /// True when the local member instance has observed its own removal.
    pub removed: bool,
    pub updated_at_ms: i64,
}

impl Default for WorkspaceConvergenceState {
    fn default() -> Self {
        Self {
            space_lineage: String::new(),
            own_instance: None,
            changes: Vec::new(),
            confirmed: BTreeMap::new(),
            pending_handoffs: BTreeMap::new(),
            waiting_members: BTreeSet::new(),
            removal_intents: BTreeSet::new(),
            removal_intent_records: Vec::new(),
            peer_intent_acks: BTreeMap::new(),
            notified_removals: BTreeSet::new(),
            accepted_removal_notices: BTreeSet::new(),
            member_devices: BTreeMap::new(),
            pending_admissions: BTreeMap::new(),
            local_admission_committed: None,
            phase: WorkspacePhase::LocallyApplied,
            failure_category: None,
            revision: 0,
            removed: false,
            updated_at_ms: 0,
        }
    }
}

/// The current workspace digest, derived from the lineage, the effective
/// member set and the ordered change chain.
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

impl std::fmt::Display for WorkspaceDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Stable summary published to callers by `QueryWorkspaceConvergence` and
/// `WorkspaceConvergenceChanged`. Contains no device names, member instances,
/// addresses, keys, security material or content. Device identifiers appear
/// only for members that currently block convergence while offline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSnapshot {
    pub phase: WorkspacePhase,
    pub revision: u64,
    pub change_count: usize,
    pub removal_intent_count: usize,
    pub effective_member_count: usize,
    pub confirmed_member_count: usize,
    pub waiting_member_device_ids: Vec<DeviceId>,
    pub waiting_member_count: usize,
    pub convergence_digest: Option<WorkspaceDigest>,
    pub removed: bool,
    pub updated_at_ms: i64,
    pub failure_category: Option<WorkspaceFailureCategory>,
}

impl WorkspaceConvergenceState {
    /// Create the initial state for a fresh workspace lineage.
    pub fn fresh(lineage: String, now_ms: i64) -> Self {
        Self {
            space_lineage: lineage,
            phase: WorkspacePhase::LocallyApplied,
            updated_at_ms: now_ms,
            ..Self::default()
        }
    }

    /// Current security generation: the target generation of the last
    /// change, or zero when no change exists yet.
    pub fn current_epoch(&self) -> u64 {
        self.changes.last().map_or(0, |change| change.next_epoch)
    }

    /// Current workspace digest, derived from the effective member set and
    /// the ordered change chain.
    pub fn current_digest(&self) -> Option<WorkspaceDigest> {
        if self.changes.is_empty() {
            return None;
        }
        let effective = self.effective_members();
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard-workspace-digest/v1\0");
        hasher.update(self.space_lineage.as_bytes());
        for change in &self.changes {
            hasher.update(change.change_id().as_bytes());
            hasher.update(change.next_epoch.to_be_bytes());
        }
        for member in &effective {
            hasher.update(member.as_bytes());
        }
        Some(WorkspaceDigest(hasher.finalize().into()))
    }

    /// Current effective member instances, derived from the ordered chain:
    /// admissions add, removals remove.
    pub fn effective_members(&self) -> BTreeSet<MemberInstanceId> {
        let mut members = BTreeSet::new();
        for change in &self.changes {
            match change.kind {
                WorkspaceChangeKind::Admission => {
                    if let Some(facts) = &change.admission {
                        members.insert(facts.member_instance);
                    }
                }
                WorkspaceChangeKind::Removal => {
                    if let Some(facts) = &change.removal {
                        for instance in &facts.removed_instances {
                            members.remove(instance);
                        }
                    }
                }
            }
        }
        members
    }

    /// The most recent member instance of a device, derived from the ordered
    /// change chain. `None` when the device never appears in any admission
    /// change.
    pub fn latest_instance_for_device(&self, device_id: &DeviceId) -> Option<MemberInstanceId> {
        self.changes.iter().rev().find_map(|change| {
            change
                .admission
                .as_ref()
                .filter(|facts| facts.device_id == *device_id)
                .map(|facts| facts.member_instance)
        })
    }

    /// Whether a device's most recent known instance is no longer an
    /// effective member. Unknown devices are not considered removed.
    pub fn is_device_removed(&self, device_id: &DeviceId) -> bool {
        let Some(latest) = self.latest_instance_for_device(device_id) else {
            return false;
        };
        !self.effective_members().contains(&latest)
    }

    /// Members that confirmed the current digest.
    pub fn confirmed_members(&self) -> BTreeSet<MemberInstanceId> {
        let Some(digest) = self.current_digest() else {
            return BTreeSet::new();
        };
        self.confirmed
            .iter()
            .filter(|(_, confirmation)| confirmation.digest == *digest.as_bytes())
            .map(|(member, _)| *member)
            .collect()
    }

    fn recompute_phase(&self) -> WorkspacePhase {
        if self.failure_category.is_some() {
            return WorkspacePhase::RecoveryRequired;
        }
        if self.changes.is_empty() {
            return WorkspacePhase::LocallyApplied;
        }
        let effective = self.effective_members();
        if effective.is_empty() {
            return WorkspacePhase::RecoveryRequired;
        }
        let digest = self.current_digest();
        let confirmed = self.confirmed_members();
        let all_confirmed = digest.is_some()
            && confirmed.len() == effective.len()
            && self.pending_handoffs.is_empty()
            && self.waiting_members.is_empty();
        if all_confirmed {
            return WorkspacePhase::Complete;
        }
        if !self.waiting_members.is_empty() && !self.pending_handoffs.is_empty() {
            let remaining = effective
                .iter()
                .filter(|member| !confirmed.contains(*member))
                .copied()
                .collect::<BTreeSet<_>>();
            if remaining.is_subset(&self.waiting_members) {
                return WorkspacePhase::WaitingForOfflineMember;
            }
        }
        if self.changes.is_empty() {
            WorkspacePhase::LocallyApplied
        } else {
            WorkspacePhase::Converging
        }
    }

    fn waiting_member_device_ids_for(
        &self,
        phase: WorkspacePhase,
    ) -> Result<Vec<DeviceId>, WorkspaceFailureCategory> {
        if phase != WorkspacePhase::WaitingForOfflineMember {
            return Ok(Vec::new());
        }

        let effective = self.effective_members();
        let confirmed = self.confirmed_members();
        let mut devices = BTreeSet::new();
        for member in effective {
            if confirmed.contains(&member)
                || !self.waiting_members.contains(&member)
                || self.own_instance == Some(member)
            {
                continue;
            }
            let device_id = self
                .member_devices
                .get(&member)
                .ok_or(WorkspaceFailureCategory::IdentityMismatch)?;
            devices.insert(device_id.clone());
        }
        Ok(devices.into_iter().collect())
    }

    fn advance(&mut self, now_ms: i64) {
        let phase = self.recompute_phase();
        self.phase = match self.waiting_member_device_ids_for(phase) {
            Ok(_) => phase,
            Err(category) => {
                self.failure_category = Some(category);
                WorkspacePhase::RecoveryRequired
            }
        };
        self.updated_at_ms = now_ms;
        self.revision = self.revision.saturating_add(1);
    }

    /// Validate that every member in the published waiting set has a stable
    /// device identifier. This protects persisted state written before the
    /// current process loaded it.
    pub fn ensure_waiting_members_are_resolvable(&mut self, now_ms: i64) -> bool {
        if self.phase != WorkspacePhase::WaitingForOfflineMember
            || self.waiting_member_device_ids_for(self.phase).is_ok()
        {
            return false;
        }
        self.failure_category = Some(WorkspaceFailureCategory::IdentityMismatch);
        self.advance(now_ms);
        true
    }

    /// Apply a committed workspace change: validate continuity, append to
    /// the chain, remove the changed members from confirmations and pending
    /// handoffs, and recompute the phase.
    ///
    /// Admission changes also refresh the removal intent set by removing
    /// any target that is no longer effective.
    pub fn apply_change(&mut self, change: WorkspaceChange, now_ms: i64) -> WorkspaceEffect {
        if self
            .changes
            .iter()
            .any(|existing| existing.change_id() == change.change_id())
        {
            return WorkspaceEffect::NONE;
        }
        if change.space_lineage != self.space_lineage {
            self.failure_category = Some(WorkspaceFailureCategory::SpaceMismatch);
            self.advance(now_ms);
            return WorkspaceEffect::PERSIST_AND_PUBLISH;
        }
        if let Err(rejection) = validate_change(self, &change) {
            self.failure_category = Some(match rejection {
                WorkspaceChangeRejection::SpaceMismatch => WorkspaceFailureCategory::SpaceMismatch,
                WorkspaceChangeRejection::EpochGap
                | WorkspaceChangeRejection::SecurityUpdateGap
                | WorkspaceChangeRejection::DigestMismatch => {
                    WorkspaceFailureCategory::ContinuityGap
                }
                WorkspaceChangeRejection::KindMissing
                | WorkspaceChangeRejection::UnknownRemovalTarget
                | WorkspaceChangeRejection::InvalidMemberInstance => {
                    WorkspaceFailureCategory::IdentityMismatch
                }
            });
            self.advance(now_ms);
            return WorkspaceEffect::PERSIST_AND_PUBLISH;
        }
        self.changes.push(change);
        let effective = self.effective_members();
        self.confirmed
            .retain(|member, _| effective.contains(member));
        self.pending_handoffs
            .retain(|recipient, _| effective.contains(recipient));
        self.waiting_members
            .retain(|member| effective.contains(member));
        if let Some(facts) = self.changes.last().and_then(|last| last.admission.clone()) {
            self.member_devices
                .insert(facts.member_instance, facts.device_id);
        }
        if let Some(facts) = self.changes.last().and_then(|last| last.removal.clone()) {
            if self
                .own_instance
                .is_some_and(|own| facts.removed_instances.contains(&own))
            {
                self.removed = true;
            }
        }
        self.advance(now_ms);
        WorkspaceEffect::PERSIST_WAKE_AND_PUBLISH
    }

    /// Record a confirmed member for the current digest.
    pub fn apply_confirmation(
        &mut self,
        confirmation: WorkspaceConfirmation,
        now_ms: i64,
    ) -> Result<WorkspaceEffect, WorkspaceConvergenceError> {
        let Some(digest) = self.current_digest() else {
            return Err(WorkspaceConvergenceError::ConfirmationDigestMismatch);
        };
        if !self
            .effective_members()
            .contains(&confirmation.member_instance)
        {
            return Err(WorkspaceConvergenceError::ConfirmationForUnknownMember);
        }
        if confirmation.digest != *digest.as_bytes() {
            return Err(WorkspaceConvergenceError::ConfirmationDigestMismatch);
        }
        let changed = self
            .confirmed
            .get(&confirmation.member_instance)
            .is_none_or(|existing| existing.digest != *digest.as_bytes());
        if changed {
            self.confirmed
                .insert(confirmation.member_instance, confirmation);
        }
        self.advance(now_ms);
        Ok(if changed {
            WorkspaceEffect::PERSIST_AND_PUBLISH
        } else {
            WorkspaceEffect::PERSIST
        })
    }

    /// Apply a recipient's durable acknowledgement of a continuous range.
    /// When the acknowledgement reaches the current target, the recipient
    /// is recorded as confirmed for that digest (the ack is the evidence;
    /// no separate signed confirmation is needed from that member).
    pub fn apply_handoff_advanced(
        &mut self,
        recipient: MemberInstanceId,
        confirmed_epoch: u64,
        target_digest: [u8; 32],
        has_more: bool,
        now_ms: i64,
    ) -> Result<WorkspaceEffect, WorkspaceConvergenceError> {
        let Some(record) = self.pending_handoffs.get_mut(&recipient) else {
            return Err(WorkspaceConvergenceError::HandoffForUnknownRecipient);
        };
        if target_digest != record.target_digest {
            return Err(WorkspaceConvergenceError::HandoffForUnknownRecipient);
        }
        record.confirmed_epoch = confirmed_epoch;
        record.has_more = has_more;
        let cleared = !has_more && confirmed_epoch >= self.current_epoch();
        if cleared {
            self.pending_handoffs.remove(&recipient);
            if let Some(digest) = self.current_digest() {
                let changed = self
                    .confirmed
                    .get(&recipient)
                    .is_none_or(|existing| existing.digest != *digest.as_bytes());
                if changed {
                    self.confirmed.insert(
                        recipient,
                        WorkspaceConfirmation {
                            member_instance: recipient,
                            digest: *digest.as_bytes(),
                            signature: Vec::new(),
                        },
                    );
                }
            }
        }
        self.advance(now_ms);
        Ok(if cleared {
            WorkspaceEffect::PERSIST_WAKE_AND_PUBLISH
        } else {
            WorkspaceEffect::PERSIST
        })
    }

    /// Save a pending handoff record for a recipient. Replacing an existing
    /// record for the same recipient and target digest keeps the highest
    /// confirmed range.
    pub fn apply_pending_handoff_created(
        &mut self,
        recipient: MemberInstanceId,
        recipient_device: DeviceId,
        confirmed_epoch: u64,
        target_digest: [u8; 32],
        has_more: bool,
        now_ms: i64,
    ) -> WorkspaceEffect {
        let updated = match self.pending_handoffs.get_mut(&recipient) {
            Some(record) if record.target_digest == target_digest => {
                if confirmed_epoch > record.confirmed_epoch {
                    record.confirmed_epoch = confirmed_epoch;
                    record.has_more = has_more;
                    true
                } else {
                    false
                }
            }
            Some(record) => {
                record.confirmed_epoch = confirmed_epoch;
                record.target_digest = target_digest;
                record.has_more = has_more;
                true
            }
            None => {
                self.pending_handoffs.insert(
                    recipient,
                    PendingHandoff {
                        recipient,
                        recipient_device,
                        confirmed_epoch,
                        target_digest,
                        has_more,
                    },
                );
                true
            }
        };
        self.advance(now_ms);
        if updated {
            WorkspaceEffect::PERSIST_AND_PUBLISH
        } else {
            WorkspaceEffect::NONE
        }
    }

    /// Record a validated removal intent and derive the unified removal
    /// target from the whole validated intent set (ADR-015 precedence).
    ///
    /// Returns the set of member instances that must be removed by the next
    /// removal change, or an empty set when the intents do not change the
    /// current effective membership.
    pub fn record_removal_intent(
        &mut self,
        intent: &SignedRemovalIntent,
        now_ms: i64,
    ) -> Result<BTreeSet<MemberInstanceId>, WorkspaceConvergenceError> {
        if intent.content.space_lineage != self.space_lineage {
            return Err(WorkspaceConvergenceError::InvalidEvent);
        }
        let known = self
            .removal_intent_records
            .iter()
            .any(|known| known.intent_id == intent.intent_id);
        if !known {
            self.removal_intent_records.push(intent.clone());
            self.removal_intents.insert(intent.intent_id);
        }
        if self
            .own_instance
            .is_some_and(|own| intent.content.target == own)
        {
            // The local instance observed its own removal: the same stable
            // fact the removal notice carries, set idempotently.
            self.removed = true;
        }
        // The removal target set is exactly the set of currently effective
        // member instances that validated intents point to. Intent views are
        // causal snapshots taken at intent creation: a member admitted after
        // the intent was created is not part of any view and must never be
        // derived as a removal target (the new instance rule). Only the
        // exact targets may be removed; every other current member stays.
        let current = self.effective_members();
        let to_remove = current
            .iter()
            .filter(|member| {
                self.removal_intent_records
                    .iter()
                    .any(|known| known.content.target == **member)
            })
            .copied()
            .collect::<BTreeSet<_>>();
        self.advance(now_ms);
        Ok(to_remove)
    }

    /// The complete event-driven entry: one external input at a time.
    pub fn apply(
        &mut self,
        event: WorkspaceConvergenceEvent,
        now_ms: i64,
    ) -> Result<(WorkspaceMergeOutcome, WorkspaceEffect), WorkspaceConvergenceError> {
        if self.phase == WorkspacePhase::RecoveryRequired {
            return Ok((WorkspaceMergeOutcome::Unchanged, WorkspaceEffect::NONE));
        }
        if self.phase.is_terminal() {
            if matches!(
                event,
                WorkspaceConvergenceEvent::CommittedChange(_)
                    | WorkspaceConvergenceEvent::LocalAdmissionReady { .. }
                    | WorkspaceConvergenceEvent::RemovalNoticeAccepted(_)
            ) {
                self.phase = WorkspacePhase::Converging;
            } else {
                return Ok((WorkspaceMergeOutcome::Unchanged, WorkspaceEffect::NONE));
            }
        }
        match event {
            WorkspaceConvergenceEvent::CommittedChange(change) => {
                let effect = self.apply_change(change, now_ms);
                Ok((
                    if effect.persist {
                        WorkspaceMergeOutcome::Updated
                    } else {
                        WorkspaceMergeOutcome::Unchanged
                    },
                    effect,
                ))
            }
            WorkspaceConvergenceEvent::AdmissionBegan {
                session,
                joiner_device_id,
                invitation_generation,
            } => {
                let changed = self
                    .pending_admissions
                    .insert(
                        session.clone(),
                        PendingAdmissionRecord {
                            joiner_device_id,
                            invitation_generation,
                            created_at_ms: now_ms,
                        },
                    )
                    .is_none();
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
                self.advance(now_ms);
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
            WorkspaceConvergenceEvent::LocalAdmissionCommitted(facts) => {
                let changed = self.local_admission_committed.as_ref() != Some(&facts);
                if changed {
                    // The sponsor's member facts are persisted with the
                    // confirmation so the joiner can reach its sponsor.
                    self.member_devices.insert(
                        facts.sponsor_facts.member_instance,
                        facts.sponsor_facts.device_id.clone(),
                    );
                    self.local_admission_committed = Some(facts);
                }
                self.advance(now_ms);
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
            WorkspaceConvergenceEvent::ConfirmationReceived(confirmation) => {
                let effect = self.apply_confirmation(confirmation, now_ms)?;
                Ok((WorkspaceMergeOutcome::Updated, effect))
            }
            WorkspaceConvergenceEvent::HandoffAdvanced {
                recipient,
                confirmed_epoch,
                target_digest,
                has_more,
            } => {
                let effect = self.apply_handoff_advanced(
                    recipient,
                    confirmed_epoch,
                    target_digest,
                    has_more,
                    now_ms,
                )?;
                Ok((WorkspaceMergeOutcome::Updated, effect))
            }
            WorkspaceConvergenceEvent::PendingHandoffCreated {
                recipient,
                recipient_device,
                confirmed_epoch,
                target_digest,
                has_more,
            } => {
                let effect = self.apply_pending_handoff_created(
                    recipient,
                    recipient_device,
                    confirmed_epoch,
                    target_digest,
                    has_more,
                    now_ms,
                );
                Ok((WorkspaceMergeOutcome::Updated, effect))
            }
            WorkspaceConvergenceEvent::MemberUnreachable(member) => {
                if !self.effective_members().contains(&member) {
                    return Ok((WorkspaceMergeOutcome::Unchanged, WorkspaceEffect::NONE));
                }
                let changed = self.waiting_members.insert(member);
                self.advance(now_ms);
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
            WorkspaceConvergenceEvent::MemberReachable(member) => {
                let changed = self.waiting_members.remove(&member);
                self.advance(now_ms);
                Ok((
                    if changed {
                        WorkspaceMergeOutcome::Updated
                    } else {
                        WorkspaceMergeOutcome::Unchanged
                    },
                    if changed {
                        WorkspaceEffect::PERSIST_WAKE_AND_PUBLISH
                    } else {
                        WorkspaceEffect::NONE
                    },
                ))
            }
            WorkspaceConvergenceEvent::IntentAcknowledged { peer, intent_id } => {
                let changed = self
                    .peer_intent_acks
                    .insert((peer, intent_id), now_ms)
                    .is_none();
                self.advance(now_ms);
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
            WorkspaceConvergenceEvent::RemovalNoticeAccepted(intent_id) => {
                self.accepted_removal_notices.insert(intent_id);
                self.removed = true;
                self.advance(now_ms);
                Ok((WorkspaceMergeOutcome::Updated, WorkspaceEffect::PERSIST))
            }
            WorkspaceConvergenceEvent::RemovalNoticeDelivered(intent_id) => {
                let changed = self.notified_removals.insert(intent_id);
                self.advance(now_ms);
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
            WorkspaceConvergenceEvent::LocalAdmissionReady { own_instance } => {
                let instance_changed = self.own_instance != Some(own_instance);
                self.own_instance = Some(own_instance);
                if instance_changed {
                    // A re-admission creates a new member instance; old
                    // instance facts (own removal, confirmations, pending
                    // handoffs, admission progress) must not leak into the
                    // new instance.
                    self.removed = false;
                    self.confirmed.clear();
                    self.pending_handoffs.clear();
                    self.pending_admissions.clear();
                    self.local_admission_committed = None;
                }
                self.phase = WorkspacePhase::LocallyApplied;
                self.advance(now_ms);
                Ok((WorkspaceMergeOutcome::Updated, WorkspaceEffect::PERSIST))
            }
            WorkspaceConvergenceEvent::IntegrityFailure(category) => {
                self.failure_category = Some(category);
                self.phase = WorkspacePhase::RecoveryRequired;
                self.advance(now_ms);
                Ok((
                    WorkspaceMergeOutcome::Updated,
                    WorkspaceEffect::PERSIST_AND_PUBLISH,
                ))
            }
        }
    }

    /// Compute the current published snapshot.
    pub fn snapshot(&self) -> WorkspaceSnapshot {
        let effective = self.effective_members();
        let (phase, waiting_member_device_ids, failure_category) =
            match self.waiting_member_device_ids_for(self.phase) {
                Ok(device_ids) => (self.phase, device_ids, self.failure_category),
                Err(category) => (WorkspacePhase::RecoveryRequired, Vec::new(), Some(category)),
            };
        WorkspaceSnapshot {
            phase,
            revision: self.revision,
            change_count: self.changes.len(),
            removal_intent_count: self.removal_intents.len(),
            effective_member_count: effective.len(),
            confirmed_member_count: self.confirmed_members().len(),
            waiting_member_count: waiting_member_device_ids.len(),
            waiting_member_device_ids,
            convergence_digest: self.current_digest(),
            removed: self.removed,
            updated_at_ms: self.updated_at_ms,
            failure_category,
        }
    }
}

/// Validate a change against the current chain before appending it.
pub fn validate_change(
    state: &WorkspaceConvergenceState,
    change: &WorkspaceChange,
) -> Result<(), WorkspaceChangeRejection> {
    if change.space_lineage != state.space_lineage {
        return Err(WorkspaceChangeRejection::SpaceMismatch);
    }
    if change.previous_epoch != state.current_epoch() {
        return Err(WorkspaceChangeRejection::EpochGap);
    }
    if change.next_epoch != change.previous_epoch.saturating_add(1) {
        return Err(WorkspaceChangeRejection::EpochGap);
    }
    let expected_previous_digest = state.changes.last().map_or(
        WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into()),
        |last| WorkspaceDigest(compute_change_digest(last)),
    );
    if change.previous_digest != *expected_previous_digest.as_bytes() {
        return Err(WorkspaceChangeRejection::DigestMismatch);
    }
    if !change.security_updates.is_empty() {
        let mut epoch = change.previous_epoch;
        for update in &change.security_updates {
            if update.previous_epoch != epoch || update.next_epoch != epoch.saturating_add(1) {
                return Err(WorkspaceChangeRejection::SecurityUpdateGap);
            }
            epoch = update.next_epoch;
        }
        if epoch != change.next_epoch {
            return Err(WorkspaceChangeRejection::SecurityUpdateGap);
        }
    }
    match change.kind {
        WorkspaceChangeKind::Admission => {
            let Some(facts) = &change.admission else {
                return Err(WorkspaceChangeRejection::KindMissing);
            };
            if facts.transport_public_key.is_empty() || facts.identity_signature.is_empty() {
                return Err(WorkspaceChangeRejection::InvalidMemberInstance);
            }
            if change.removal.is_some() {
                return Err(WorkspaceChangeRejection::KindMissing);
            }
        }
        WorkspaceChangeKind::Removal => {
            let Some(facts) = &change.removal else {
                return Err(WorkspaceChangeRejection::KindMissing);
            };
            if change.admission.is_some() {
                return Err(WorkspaceChangeRejection::KindMissing);
            }
            if facts.removed_instances.is_empty() {
                return Err(WorkspaceChangeRejection::UnknownRemovalTarget);
            }
            let effective = state.effective_members();
            for instance in &facts.removed_instances {
                if !effective.contains(instance) {
                    return Err(WorkspaceChangeRejection::UnknownRemovalTarget);
                }
            }
        }
    }
    Ok(())
}

/// Deterministic digest of a change, used as the chain's previous-digest
/// anchor for the following change.
pub fn compute_change_digest(change: &WorkspaceChange) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard-workspace-change/v1\0");
    hasher.update(change.space_lineage.as_bytes());
    hasher.update(change.previous_epoch.to_be_bytes());
    hasher.update(change.next_epoch.to_be_bytes());
    hasher.update(change.previous_digest);
    match change.kind {
        WorkspaceChangeKind::Admission => {
            hasher.update(b"admission");
            if let Some(facts) = &change.admission {
                hasher.update(facts.member_instance.as_bytes());
                hasher.update(facts.device_id.as_str().as_bytes());
                hasher.update(facts.identity_fingerprint.as_display().as_bytes());
            }
        }
        WorkspaceChangeKind::Removal => {
            hasher.update(b"removal");
            if let Some(facts) = &change.removal {
                for instance in &facts.removed_instances {
                    hasher.update(instance.as_bytes());
                }
            }
        }
    }
    for update in &change.security_updates {
        hasher.update(update.digest);
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::membership::removal_intent::{RemovalCausalProof, RemovalIntentContent};

    fn admission_change(
        lineage: &str,
        previous_epoch: u64,
        instance: MemberInstanceId,
        previous_digest: [u8; 32],
    ) -> WorkspaceChange {
        WorkspaceChange {
            space_lineage: lineage.to_owned(),
            kind: WorkspaceChangeKind::Admission,
            previous_epoch,
            next_epoch: previous_epoch + 1,
            previous_digest,
            digest: [0; 32],
            security_updates: Vec::new(),
            admission: Some(AdmissionChangeFacts {
                member_instance: instance,
                device_id: DeviceId::new("device-a"),
                device_name: "a".to_owned(),
                identity_fingerprint: crate::security::IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .unwrap(),
                transport_public_key: vec![1; 32],
                transport_address_blob: vec![2; 16],
                identity_signature: vec![3; 64],
            }),
            removal: None,
            created_at_ms: 1,
        }
    }

    fn admission_change_for_device(
        lineage: &str,
        previous_epoch: u64,
        instance: MemberInstanceId,
        device_id: &str,
        previous_digest: [u8; 32],
    ) -> WorkspaceChange {
        let mut change = admission_change(lineage, previous_epoch, instance, previous_digest);
        change.admission.as_mut().unwrap().device_id = DeviceId::new(device_id);
        change
    }

    fn removal_change(
        lineage: &str,
        previous_epoch: u64,
        removed: &[MemberInstanceId],
        previous_digest: [u8; 32],
    ) -> WorkspaceChange {
        WorkspaceChange {
            space_lineage: lineage.to_owned(),
            kind: WorkspaceChangeKind::Removal,
            previous_epoch,
            next_epoch: previous_epoch + 1,
            previous_digest,
            digest: [0; 32],
            security_updates: Vec::new(),
            admission: None,
            removal: Some(RemovalChangeFacts {
                removed_instances: removed.to_vec(),
            }),
            created_at_ms: 1,
        }
    }

    fn instance(byte: u8) -> MemberInstanceId {
        MemberInstanceId::from_bytes([byte; 32])
    }

    fn removal_intent(
        lineage: &str,
        epoch: u64,
        members: &[MemberInstanceId],
        initiator: MemberInstanceId,
        target: MemberInstanceId,
    ) -> SignedRemovalIntent {
        let mut sorted = members.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let content = RemovalIntentContent {
            space_lineage: lineage.to_owned(),
            view_epoch: epoch,
            view_members: sorted,
            initiator,
            target,
        };
        SignedRemovalIntent::new(
            content,
            vec![1, 2, 3],
            RemovalCausalProof::new(epoch, Vec::new()),
        )
    }

    #[test]
    fn admission_then_removal_derives_effective_members_and_digest() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let b = instance(0x0b);
        let first = admission_change("lineage", 0, a, *initial.as_bytes());
        let (outcome, effect) = state
            .apply(WorkspaceConvergenceEvent::CommittedChange(first), 2)
            .unwrap();
        assert_eq!(outcome, WorkspaceMergeOutcome::Updated);
        assert!(effect.persist && effect.publish);
        assert_eq!(state.current_epoch(), 1);
        assert_eq!(state.effective_members(), BTreeSet::from([a]));

        let first_digest = compute_change_digest(&state.changes[0]);
        let second = admission_change("lineage", 1, b, first_digest);
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(second), 3)
            .unwrap();
        assert_eq!(state.effective_members(), BTreeSet::from([a, b]));

        let second_digest = compute_change_digest(&state.changes[1]);
        let removal = removal_change("lineage", 2, &[a], second_digest);
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(removal), 4)
            .unwrap();
        assert_eq!(state.effective_members(), BTreeSet::from([b]));
        assert_ne!(state.current_digest().unwrap().as_bytes(), &second_digest);
    }

    #[test]
    fn gapped_and_skipped_changes_are_rejected_and_enter_recovery() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let b = instance(0x0b);

        let gapped = admission_change("lineage", 5, a, *initial.as_bytes());
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(gapped), 2)
            .unwrap();
        assert_eq!(state.phase, WorkspacePhase::RecoveryRequired);
        assert_eq!(
            state.failure_category,
            Some(WorkspaceFailureCategory::ContinuityGap)
        );
        assert!(state.changes.is_empty());

        let wrong_digest = admission_change("lineage", 0, a, [7; 32]);
        let mut second = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        second
            .apply(WorkspaceConvergenceEvent::CommittedChange(wrong_digest), 2)
            .unwrap();
        assert_eq!(
            second.failure_category,
            Some(WorkspaceFailureCategory::ContinuityGap)
        );

        let mut third = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let removal = removal_change("lineage", 0, &[b], *initial.as_bytes());
        third
            .apply(WorkspaceConvergenceEvent::CommittedChange(removal), 2)
            .unwrap();
        assert_eq!(
            third.failure_category,
            Some(WorkspaceFailureCategory::IdentityMismatch)
        );
    }

    #[test]
    fn duplicate_change_is_idempotent() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let first = admission_change("lineage", 0, a, *initial.as_bytes());
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(first.clone()), 2)
            .unwrap();
        let revision = state.revision;
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(first), 3)
            .unwrap();
        assert_eq!(state.revision, revision);
        assert_eq!(state.changes.len(), 1);
    }

    #[test]
    fn completion_requires_every_effective_member_to_confirm_current_digest() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let b = instance(0x0b);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    0,
                    a,
                    *initial.as_bytes(),
                )),
                2,
            )
            .unwrap();
        let first_digest = compute_change_digest(&state.changes[0]);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    1,
                    b,
                    first_digest,
                )),
                3,
            )
            .unwrap();
        assert_eq!(state.phase, WorkspacePhase::Converging);

        let digest = *state.current_digest().unwrap().as_bytes();
        state
            .apply(
                WorkspaceConvergenceEvent::ConfirmationReceived(WorkspaceConfirmation {
                    member_instance: a,
                    digest,
                    signature: vec![1],
                }),
                4,
            )
            .unwrap();
        assert_eq!(state.phase, WorkspacePhase::Converging);

        state
            .apply(
                WorkspaceConvergenceEvent::ConfirmationReceived(WorkspaceConfirmation {
                    member_instance: b,
                    digest,
                    signature: vec![2],
                }),
                5,
            )
            .unwrap();
        assert_eq!(state.phase, WorkspacePhase::Complete);
        assert_eq!(state.confirmed_members(), BTreeSet::from([a, b]));
    }

    #[test]
    fn offline_member_holds_waiting_for_offline_member_until_reachable() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let b = instance(0x0b);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    0,
                    a,
                    *initial.as_bytes(),
                )),
                2,
            )
            .unwrap();
        let first_digest = compute_change_digest(&state.changes[0]);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    1,
                    b,
                    first_digest,
                )),
                3,
            )
            .unwrap();
        let digest = *state.current_digest().unwrap().as_bytes();
        state
            .apply(
                WorkspaceConvergenceEvent::ConfirmationReceived(WorkspaceConfirmation {
                    member_instance: a,
                    digest,
                    signature: vec![1],
                }),
                4,
            )
            .unwrap();
        state
            .apply(WorkspaceConvergenceEvent::MemberUnreachable(b), 5)
            .unwrap();
        state
            .apply(
                WorkspaceConvergenceEvent::PendingHandoffCreated {
                    recipient: b,
                    recipient_device: DeviceId::new("device-b"),
                    confirmed_epoch: 1,
                    target_digest: digest,
                    has_more: true,
                },
                6,
            )
            .unwrap();
        state
            .apply(
                WorkspaceConvergenceEvent::HandoffAdvanced {
                    recipient: b,
                    confirmed_epoch: 1,
                    target_digest: digest,
                    has_more: true,
                },
                7,
            )
            .unwrap();
        assert_eq!(state.phase, WorkspacePhase::WaitingForOfflineMember);

        state
            .apply(WorkspaceConvergenceEvent::MemberReachable(b), 8)
            .unwrap();
        assert_eq!(state.phase, WorkspacePhase::Converging);
    }

    #[test]
    fn non_waiting_phase_does_not_publish_unreachable_members_as_waiting() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let b = instance(0x0b);
        let c = instance(0x0c);

        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    0,
                    a,
                    *initial.as_bytes(),
                )),
                2,
            )
            .unwrap();
        let first_digest = compute_change_digest(&state.changes[0]);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    1,
                    b,
                    first_digest,
                )),
                3,
            )
            .unwrap();
        let second_digest = compute_change_digest(&state.changes[1]);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    2,
                    c,
                    second_digest,
                )),
                4,
            )
            .unwrap();

        let digest = *state.current_digest().unwrap().as_bytes();
        state
            .apply(
                WorkspaceConvergenceEvent::ConfirmationReceived(WorkspaceConfirmation {
                    member_instance: a,
                    digest,
                    signature: vec![1],
                }),
                5,
            )
            .unwrap();
        state
            .apply(
                WorkspaceConvergenceEvent::PendingHandoffCreated {
                    recipient: b,
                    recipient_device: DeviceId::new("device-b"),
                    confirmed_epoch: 1,
                    target_digest: digest,
                    has_more: true,
                },
                6,
            )
            .unwrap();
        state
            .apply(WorkspaceConvergenceEvent::MemberUnreachable(b), 7)
            .unwrap();

        assert_eq!(state.phase, WorkspacePhase::Converging);
        assert_eq!(state.snapshot().waiting_member_count, 0);
    }

    #[test]
    fn waiting_snapshot_lists_only_current_offline_devices_in_stable_order() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let b = instance(0x0b);
        let c = instance(0x0c);

        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change_for_device(
                    "lineage",
                    0,
                    a,
                    "device-a",
                    *initial.as_bytes(),
                )),
                2,
            )
            .unwrap();
        let first_digest = compute_change_digest(&state.changes[0]);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change_for_device(
                    "lineage",
                    1,
                    b,
                    "device-b",
                    first_digest,
                )),
                3,
            )
            .unwrap();
        let second_digest = compute_change_digest(&state.changes[1]);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change_for_device(
                    "lineage",
                    2,
                    c,
                    "device-c",
                    second_digest,
                )),
                4,
            )
            .unwrap();

        let digest = *state.current_digest().unwrap().as_bytes();
        state
            .apply(
                WorkspaceConvergenceEvent::ConfirmationReceived(WorkspaceConfirmation {
                    member_instance: a,
                    digest,
                    signature: vec![1],
                }),
                5,
            )
            .unwrap();
        for (member, device_id) in [(c, "device-c"), (b, "device-b")] {
            state
                .apply(
                    WorkspaceConvergenceEvent::PendingHandoffCreated {
                        recipient: member,
                        recipient_device: DeviceId::new(device_id),
                        confirmed_epoch: 1,
                        target_digest: digest,
                        has_more: true,
                    },
                    6,
                )
                .unwrap();
            state
                .apply(WorkspaceConvergenceEvent::MemberUnreachable(member), 7)
                .unwrap();
        }

        let snapshot = state.snapshot();
        assert_eq!(snapshot.phase, WorkspacePhase::WaitingForOfflineMember);
        assert_eq!(
            snapshot.waiting_member_device_ids,
            vec![DeviceId::new("device-b"), DeviceId::new("device-c")]
        );
        assert_eq!(snapshot.waiting_member_count, 2);
    }

    #[test]
    fn missing_waiting_member_device_id_requires_recovery_without_a_partial_snapshot() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let b = instance(0x0b);
        for (epoch, member) in [(0, a), (1, b)] {
            let previous = if epoch == 0 {
                *initial.as_bytes()
            } else {
                compute_change_digest(&state.changes[epoch - 1])
            };
            state
                .apply(
                    WorkspaceConvergenceEvent::CommittedChange(admission_change_for_device(
                        "lineage",
                        epoch as u64,
                        member,
                        if member == a { "device-a" } else { "device-b" },
                        previous,
                    )),
                    2,
                )
                .unwrap();
        }
        let digest = *state.current_digest().unwrap().as_bytes();
        state
            .apply(
                WorkspaceConvergenceEvent::ConfirmationReceived(WorkspaceConfirmation {
                    member_instance: a,
                    digest,
                    signature: vec![1],
                }),
                3,
            )
            .unwrap();
        state
            .apply(
                WorkspaceConvergenceEvent::PendingHandoffCreated {
                    recipient: b,
                    recipient_device: DeviceId::new("device-b"),
                    confirmed_epoch: 1,
                    target_digest: digest,
                    has_more: true,
                },
                4,
            )
            .unwrap();
        state
            .apply(WorkspaceConvergenceEvent::MemberUnreachable(b), 5)
            .unwrap();
        assert_eq!(state.phase, WorkspacePhase::WaitingForOfflineMember);

        // This models an incomplete encrypted record loaded from storage.
        state.member_devices.remove(&b);
        assert!(state.ensure_waiting_members_are_resolvable(6));

        let snapshot = state.snapshot();
        assert_eq!(snapshot.phase, WorkspacePhase::RecoveryRequired);
        assert!(snapshot.waiting_member_device_ids.is_empty());
        assert_eq!(snapshot.waiting_member_count, 0);
        assert_eq!(
            snapshot.failure_category,
            Some(WorkspaceFailureCategory::IdentityMismatch)
        );
    }

    #[test]
    fn handoff_clears_only_when_current_target_digest_confirmed() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let a = instance(0x0a);
        let b = instance(0x0b);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    0,
                    a,
                    *initial.as_bytes(),
                )),
                2,
            )
            .unwrap();
        let first_digest = compute_change_digest(&state.changes[0]);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    1,
                    b,
                    first_digest,
                )),
                3,
            )
            .unwrap();
        let digest = *state.current_digest().unwrap().as_bytes();
        state
            .apply(
                WorkspaceConvergenceEvent::PendingHandoffCreated {
                    recipient: b,
                    recipient_device: DeviceId::new("device-b"),
                    confirmed_epoch: 0,
                    target_digest: digest,
                    has_more: true,
                },
                4,
            )
            .unwrap();
        state
            .apply(
                WorkspaceConvergenceEvent::HandoffAdvanced {
                    recipient: b,
                    confirmed_epoch: 1,
                    target_digest: digest,
                    has_more: true,
                },
                5,
            )
            .unwrap();
        assert_eq!(state.pending_handoffs.len(), 1);

        state
            .apply(
                WorkspaceConvergenceEvent::HandoffAdvanced {
                    recipient: b,
                    confirmed_epoch: 2,
                    target_digest: digest,
                    has_more: false,
                },
                6,
            )
            .unwrap();
        assert!(state.pending_handoffs.is_empty());
        assert_eq!(state.phase, WorkspacePhase::Converging);
    }

    #[test]
    fn recovery_required_stops_further_progress() {
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let a = instance(0x0a);
        state
            .apply(
                WorkspaceConvergenceEvent::IntegrityFailure(
                    WorkspaceFailureCategory::ContinuityGap,
                ),
                2,
            )
            .unwrap();
        assert_eq!(state.phase, WorkspacePhase::RecoveryRequired);
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::ConfirmationReceived(WorkspaceConfirmation {
                    member_instance: a,
                    digest: [1; 32],
                    signature: vec![1],
                }),
                3,
            )
            .unwrap();
        assert_eq!(outcome, WorkspaceMergeOutcome::Unchanged);
        assert_eq!(effect, WorkspaceEffect::NONE);
    }

    #[test]
    fn historical_removal_intent_does_not_remove_a_later_admitted_member() {
        // A removal intent created before member D joined names only the old
        // members in its causal view. Recording that intent on D must not
        // derive D as a removal target: only the exact intent target may be
        // removed, and a member admitted after the intent never appears in
        // its view (the new instance rule).
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let c = instance(0x0c);
        let x = instance(0x0e);
        let d = instance(0x0d);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    0,
                    c,
                    *initial.as_bytes(),
                )),
                2,
            )
            .unwrap();
        let first_digest = compute_change_digest(&state.changes[0]);
        state
            .apply(
                WorkspaceConvergenceEvent::CommittedChange(admission_change(
                    "lineage",
                    1,
                    d,
                    first_digest,
                )),
                3,
            )
            .unwrap();
        let historical = removal_intent("lineage", 1, &[c, x], c, x);
        let to_remove = state.record_removal_intent(&historical, 4).unwrap();
        assert!(
            to_remove.is_empty(),
            "a fresh member outside every historical intent view must not be removed"
        );
        assert!(
            !state.removed,
            "the fresh member must not observe its own removal"
        );
        assert!(state.effective_members().contains(&d));
        assert!(
            state.removal_intent_records.len() == 1,
            "the intent stays recorded"
        );
    }

    #[test]
    fn historical_removal_intent_still_removes_its_exact_current_target() {
        // When the intent's exact target is still a current member, recording
        // the intent still derives that target for the next removal change.
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let c = instance(0x0c);
        let x = instance(0x0e);
        let d = instance(0x0d);
        for (epoch, member) in [(0, c), (1, x), (2, d)].into_iter() {
            let previous = if epoch == 0 {
                *initial.as_bytes()
            } else {
                compute_change_digest(&state.changes[epoch - 1])
            };
            state
                .apply(
                    WorkspaceConvergenceEvent::CommittedChange(admission_change(
                        "lineage",
                        epoch as u64,
                        member,
                        previous,
                    )),
                    3,
                )
                .unwrap();
        }
        let historical = removal_intent("lineage", 2, &[c, x], c, x);
        let to_remove = state.record_removal_intent(&historical, 4).unwrap();
        assert_eq!(
            to_remove,
            BTreeSet::from([x]),
            "the exact target that is still current must be removed"
        );
        assert!(!state.removed, "the local instance is not the target");
    }

    #[test]
    fn old_removal_intent_does_not_match_a_rejoined_member_instance() {
        // A device removed as instance X rejoins as a new instance D. The
        // old intent naming X must not mark the new instance removed nor
        // derive it for removal.
        let mut state = WorkspaceConvergenceState::fresh("lineage".to_owned(), 1);
        let initial = WorkspaceDigest(Sha256::digest(b"uniclipboard-workspace-initial/v1").into());
        let c = instance(0x0c);
        let old = instance(0x0e);
        let rejoined = instance(0x0f);
        let first = admission_change("lineage", 0, c, *initial.as_bytes());
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(first.clone()), 2)
            .unwrap();
        let second = admission_change("lineage", 1, old, compute_change_digest(&state.changes[0]));
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(second), 3)
            .unwrap();
        let removal = removal_change(
            "lineage",
            2,
            &[old],
            compute_change_digest(&state.changes[1]),
        );
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(removal), 4)
            .unwrap();
        let rejoin = admission_change(
            "lineage",
            3,
            rejoined,
            compute_change_digest(&state.changes[2]),
        );
        state
            .apply(WorkspaceConvergenceEvent::CommittedChange(rejoin), 5)
            .unwrap();
        let historical = removal_intent("lineage", 2, &[c, old], c, old);
        let to_remove = state.record_removal_intent(&historical, 6).unwrap();
        assert!(
            to_remove.is_empty(),
            "the removed old instance is not current; the new instance must be untouched"
        );
        assert!(
            !state.removed,
            "the rejoined member must not observe a stale removal"
        );
        assert_eq!(state.effective_members(), BTreeSet::from([c, rejoined]));
    }
}
