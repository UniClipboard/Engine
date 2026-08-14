//! Unified workspace convergence owner (ADR-016).
//!
//! `WorkspaceConvergence` is the single application-layer owner of member
//! joining, removal, rejoining, device discovery, material handoff,
//! confirmation, restart recovery, and published state. It saves every
//! verified change and the local security effect in one encrypted commit,
//! arranges handoffs, verifies receptions, and only reports completion when
//! every current effective member has saved the same digest and continuous
//! security state.
//!
//! The old fragmented owners (candidate convergence, shared-device refresh,
//! member-removal coordinator) do not exist here: this module owns the whole
//! flow and exposes only member operations, a query, and a change event.
//! The ordinary member channel carries only validated removal intents and
//! their bounded acknowledgements (plus workspace confirmations); recovery
//! material handoff uses the restricted `workspace-recovery/1` channel.
//! Removed instances keep only the restricted late-submission and
//! removal-notice entries.

pub(crate) mod assembly;
pub mod discovery;
pub(crate) mod group_update_delivery;
pub(crate) mod legacy_upgrade;
pub(crate) mod membership_connectivity;
pub(crate) mod network_recovery;
pub(crate) mod reachability;
mod runtime;

#[cfg(test)]
pub(crate) mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use sha2::Digest;
use tokio::sync::broadcast;
use tracing::info;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignaturePort, CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort,
    LegacyPeerProbePort, MemberRepositoryPort, MembershipDecision, MembershipEvent,
    MembershipEventId, MembershipEventsRequest, MembershipEventsResponse, MembershipHistoryAck,
    MembershipHistoryExchangeEndpointPort, MembershipHistoryExchangeError,
    MembershipHistoryExchangePort, MembershipHistoryHello, MembershipHistoryMessage,
    MembershipHistoryRelationship, MembershipOperation, MembershipReconciliationOutcome,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort, RemovalDecision,
    SpaceProtectionStatusPort, WorkspaceConvergenceEvent, WorkspaceConvergenceRepositoryError,
    WorkspaceConvergenceRepositoryPort, WorkspaceConvergenceState, WorkspaceMergeOutcome,
    WorkspacePhase, WorkspaceSnapshot,
};
use uc_core::ports::{
    ClockPort, DeviceIdentityPort, PeerAddressRepositoryPort, PresencePort, ReachabilityState,
};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;

pub(crate) use runtime::WorkspaceConvergenceActivity;
pub use runtime::WorkspaceConvergenceRuntime;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceConvergenceError {
    #[error("workspace convergence state is locked")]
    Locked,
    #[error("workspace convergence state could not be persisted: {0}")]
    Repository(#[from] WorkspaceConvergenceRepositoryError),
    #[error("workspace convergence security update failed: {0}")]
    SecurityUpdate(#[from] MembershipSecurityUpdateError),
    #[error("current membership identity is unavailable")]
    NotAMember,
    #[error("the target member does not exist in the current workspace")]
    UnknownTarget,
    #[error("the local member cannot remove itself")]
    SelfTarget,
    #[error("the local member has observed its own removal")]
    OwnInstanceRemoved,
    #[error("workspace convergence cannot proceed until a manual recovery")]
    RecoveryRequired,
    #[error("workspace convergence confirmation is invalid")]
    InvalidConfirmation,
    #[error("workspace convergence handoff is invalid")]
    InvalidHandoff,
    #[error("workspace convergence state is inconsistent: {0}")]
    Inconsistent(String),
    #[error("workspace convergence admission storage failed: {0}")]
    AdmissionStorage(String),
    #[error("workspace convergence admission generation advanced")]
    AdmissionGenerationAdvanced,
    #[error("workspace convergence is unavailable")]
    Unavailable,
}

pub struct WorkspaceConvergenceDeps {
    pub repository: Arc<dyn WorkspaceConvergenceRepositoryPort>,
    pub member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub membership_identity: Arc<dyn CurrentMembershipIdentityPort>,
    pub announcement_material: Arc<dyn CurrentMembershipAnnouncementPort>,
    pub security_updates: Arc<dyn MembershipSecurityUpdatePort>,
    pub clock: Arc<dyn ClockPort>,
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    /// The sole authenticated peer channel for bounded member-history
    /// reconciliation.
    pub membership_history_exchange: Arc<dyn MembershipHistoryExchangePort>,
    /// Positive-only legacy endpoint probe. It is used only after the current
    /// member-history endpoint failed to confirm the peer.
    pub legacy_peer_probe: Arc<dyn LegacyPeerProbePort>,
    /// Member roster persistence: admission commits write the admitted
    /// member facts here in the same save boundary.
    pub trusted_peer_repo: Arc<dyn TrustedPeerRepositoryPort>,
    pub peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    pub presence: Arc<dyn PresencePort>,
    pub space_protection: Arc<dyn SpaceProtectionStatusPort>,
    pub own_device: DeviceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceMembership {
    Active,
    Removed,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupRelationship {
    Consistent,
    PendingLocalDecision,
    Diverged,
    Unverifiable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceCompatibility {
    Compatible,
    UpgradeRequired,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRelationship {
    Usable,
    WaitingForLocalDecision,
    PausedGroupDiverged,
    PausedUpgradeRequired,
    PausedUnverifiable,
    RemovedLocalDevice,
    RemovedPeerDevice,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTrustChoice {
    ApplyChange,
    KeepCurrentDeviceGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTrustAction {
    ApplyCurrentChange,
    KeepCurrentDeviceGroup,
    ConfirmApplyRemovesLocalDevice,
    RejoinDeviceGroup,
    UpdateThisDevice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionUnavailableReason {
    NoCurrentChange,
    ChangeNoLongerCurrent,
    LocalDeviceConfirmationRequired,
    LocalDeviceRemoved,
    RecoveryNotAvailableInThisVersion,
    PeerUpgradeRequired,
    DeviceFactsUnverifiable,
    EngineUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAvailability {
    NotAvailableInThisVersion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceTrustDecisionResult {
    Applied {
        change_id: MembershipEventId,
        snapshot: DeviceTrustSnapshot,
    },
    KeptCurrentDeviceGroup {
        change_id: MembershipEventId,
        snapshot: DeviceTrustSnapshot,
    },
    AlreadyCompleted {
        change_id: MembershipEventId,
        completed_choice: DeviceTrustChoice,
        snapshot: DeviceTrustSnapshot,
    },
    StateChanged {
        current_change_id: Option<MembershipEventId>,
        snapshot: DeviceTrustSnapshot,
    },
    LocalDeviceConfirmationRequired {
        change_id: MembershipEventId,
        snapshot: DeviceTrustSnapshot,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustImpact {
    pub usable_device_ids: Vec<DeviceId>,
    pub paused_device_ids: Vec<DeviceId>,
    pub local_device_outcome: DeviceMembership,
    pub requires_rejoin_device_ids: Vec<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustChange {
    pub change_id: MembershipEventId,
    pub proposed_by_device_id: DeviceId,
    pub target_device_ids: Vec<DeviceId>,
    pub includes_local_device: bool,
    pub apply_impact: DeviceTrustImpact,
    pub keep_current_impact: DeviceTrustImpact,
    pub allowed_choices: Vec<DeviceTrustChoice>,
    pub blocked_reason: Option<ActionUnavailableReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustRelationship {
    pub device_id: DeviceId,
    pub display_name: String,
    pub is_local: bool,
    pub reachability: ReachabilityState,
    pub membership: DeviceMembership,
    pub group_relationship: GroupRelationship,
    pub compatibility: DeviceCompatibility,
    pub sync_relationship: SyncRelationship,
    pub available_actions: Vec<DeviceTrustAction>,
    pub blocked_reason: Option<ActionUnavailableReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceTrustSnapshot {
    pub revision: u64,
    pub local_device_id: DeviceId,
    pub local_membership: DeviceMembership,
    pub current_change: Option<DeviceTrustChange>,
    pub devices: Vec<DeviceTrustRelationship>,
    pub recovery: RecoveryAvailability,
    pub allowed_actions: Vec<DeviceTrustAction>,
    pub blocked_reason: Option<ActionUnavailableReason>,
    pub updated_at_ms: i64,
}

/// The unified workspace convergence owner.
pub struct WorkspaceConvergence {
    deps: WorkspaceConvergenceDeps,
    state_lock: tokio::sync::Mutex<()>,
    device_trust_decision_lock: tokio::sync::Mutex<()>,
    peer_reconciliation_locks: tokio::sync::Mutex<BTreeMap<DeviceId, Arc<tokio::sync::Mutex<()>>>>,
    wake: Arc<tokio::sync::Notify>,
    events: broadcast::Sender<WorkspaceSnapshot>,
}

#[derive(Clone, Copy)]
enum ReconciliationPeerRole {
    AuthenticatedSponsor,
    RuntimePeer,
}

impl WorkspaceConvergence {
    pub fn new(deps: WorkspaceConvergenceDeps) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            deps,
            state_lock: tokio::sync::Mutex::new(()),
            device_trust_decision_lock: tokio::sync::Mutex::new(()),
            peer_reconciliation_locks: tokio::sync::Mutex::new(BTreeMap::new()),
            wake: Arc::new(tokio::sync::Notify::new()),
            events,
        })
    }

    fn admission_generation(state: &WorkspaceConvergenceState) -> u64 {
        state
            .membership_reconciliation
            .as_ref()
            .map_or(0, |history| history.known_event_count() as u64)
    }

    pub(crate) async fn admission_decision_for_joiner(
        &self,
        invitation_generation: u64,
        joiner_device_id: &DeviceId,
    ) -> uc_core::membership::MembershipAdmissionDecision {
        let state = match self.load_state().await {
            Ok(state) => state,
            Err(_) => return uc_core::membership::MembershipAdmissionDecision::Unavailable,
        };
        let decision = Self::admission_decision_for_state(&state, invitation_generation);
        if decision != uc_core::membership::MembershipAdmissionDecision::Allowed {
            return decision;
        }
        if state.latest_instance_for_device(joiner_device_id).is_some() {
            return uc_core::membership::MembershipAdmissionDecision::Unavailable;
        }
        uc_core::membership::MembershipAdmissionDecision::Allowed
    }

    fn admission_decision_for_state(
        state: &WorkspaceConvergenceState,
        invitation_generation: u64,
    ) -> uc_core::membership::MembershipAdmissionDecision {
        if state.phase == WorkspacePhase::RecoveryRequired {
            return uc_core::membership::MembershipAdmissionDecision::RecoveryRequired;
        }
        if state.removed {
            return uc_core::membership::MembershipAdmissionDecision::Unavailable;
        }
        if invitation_generation < Self::admission_generation(state) {
            return uc_core::membership::MembershipAdmissionDecision::SupersededInvitation;
        }
        uc_core::membership::MembershipAdmissionDecision::Allowed
    }

    pub fn wake_handle(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.wake)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkspaceSnapshot> {
        self.events.subscribe()
    }

    fn notify(&self) {
        self.wake.notify_waiters();
    }

    async fn load_state(&self) -> Result<WorkspaceConvergenceState, WorkspaceConvergenceError> {
        let lineage = self
            .deps
            .membership_identity
            .current_membership_identity()
            .await
            .map(|identity| identity.space_id.as_ref().to_owned())
            .unwrap_or_default();
        let mut state = self.deps.repository.load_state().await?.unwrap_or_else(|| {
            WorkspaceConvergenceState::fresh(lineage.clone(), self.deps.clock.now_ms())
        });
        if state.space_lineage.is_empty() {
            state.space_lineage = lineage;
        }
        Ok(state)
    }

    async fn persist(
        &self,
        state: &WorkspaceConvergenceState,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.deps.repository.save_state(state).await?;
        Ok(())
    }

    fn enqueue_applied_membership_effects(
        state: &mut WorkspaceConvergenceState,
        events: &[MembershipEvent],
    ) {
        for event in events {
            let event_id = event.event_id();
            if state
                .pending_applied_membership_effects
                .iter()
                .any(|effect| effect.event_id == event_id)
            {
                continue;
            }
            state.pending_applied_membership_effects.push(
                uc_core::membership::PendingAppliedMembershipEffect {
                    event_id,
                    member_facts_completed: !matches!(
                        event.operation,
                        MembershipOperation::AddDevice { .. }
                    ),
                    security_update_completed: event.security_update_payload.is_empty(),
                },
            );
        }
    }

    async fn execute_pending_membership_effects(
        &self,
        state: &mut WorkspaceConvergenceState,
        now_ms: i64,
    ) -> Result<(), WorkspaceConvergenceError> {
        for index in 0..state.pending_applied_membership_effects.len() {
            let effect = state.pending_applied_membership_effects[index].clone();
            let event = state
                .membership_reconciliation
                .as_ref()
                .and_then(|history| history.event(effect.event_id))
                .cloned()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "pending membership effect references an unknown event".to_owned(),
                    )
                })?;

            if !effect.member_facts_completed {
                if let MembershipOperation::AddDevice { admission } = &event.operation {
                    self.save_member_facts(admission, now_ms).await?;
                }
                state.pending_applied_membership_effects[index].member_facts_completed = true;
                self.persist(state).await?;
            }
            if !effect.security_update_completed {
                self.deps
                    .security_updates
                    .apply_group_epoch_update(&event.security_update_payload)
                    .await?;
                state.pending_applied_membership_effects[index].security_update_completed = true;
                self.persist(state).await?;
            }
        }
        state.pending_applied_membership_effects.clear();
        self.persist(state).await?;
        Ok(())
    }

    pub(crate) async fn recover_pending_membership_effects(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let mut state = self.load_state().await?;
        if state.pending_applied_membership_effects.is_empty() {
            return Ok(());
        }
        self.execute_pending_membership_effects(&mut state, self.deps.clock.now_ms())
            .await?;
        self.publish(&state);
        self.notify();
        Ok(())
    }

    pub(crate) async fn deliver_pending_membership_decisions(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let pending = {
            let _guard = self.state_lock.lock().await;
            self.load_state()
                .await?
                .pending_membership_decision_deliveries
        };
        for delivery in pending {
            let delivered = self
                .deps
                .membership_history_exchange
                .exchange_membership_history(
                    &delivery.recipient,
                    MembershipHistoryMessage::Decision(delivery.decision.clone()),
                )
                .await
                .is_ok();
            if !delivered {
                continue;
            }
            let _guard = self.state_lock.lock().await;
            let mut state = self.load_state().await?;
            state.pending_membership_decision_deliveries.retain(|item| {
                item.recipient != delivery.recipient || item.decision != delivery.decision
            });
            self.persist(&state).await?;
        }
        Ok(())
    }

    fn publish(&self, state: &WorkspaceConvergenceState) {
        let _ = self.events.send(state.snapshot());
    }

    /// Whether the local device may currently drive content sends.
    pub async fn locally_removed(&self, device_id: &DeviceId) -> bool {
        let in_current_scope = uc_core::membership::CurrentWorkspacePeerScopePort::snapshot(self)
            .await
            .map(|scope| scope.peer_device_ids.contains(device_id))
            .unwrap_or(false);
        if !in_current_scope {
            return true;
        }
        let state = match self.load_state().await {
            Ok(state) => state,
            Err(_) => return true,
        };
        state.removed
            || state.is_device_removed(device_id)
            || !state.allows_normal_exchange(device_id)
    }

    /// Whether the local member instance has observed its own removal.
    pub async fn own_instance_removed(&self) -> bool {
        self.load_state().await.map_or(true, |state| state.removed)
    }

    pub async fn query_device_trust(
        &self,
    ) -> Result<DeviceTrustSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let state = self.load_state().await?;
        let local_device_id = self.deps.own_device.clone();
        let workspace_unverifiable = state.failure_category.is_some();
        let local_membership = if state.own_instance.is_none() {
            match uc_core::membership::CurrentWorkspacePeerScopePort::snapshot(self).await {
                Ok(scope) => match scope.local_membership {
                    uc_core::membership::CurrentWorkspaceLocalMembership::Active => {
                        DeviceMembership::Active
                    }
                    uc_core::membership::CurrentWorkspaceLocalMembership::Removed => {
                        DeviceMembership::Removed
                    }
                },
                Err(_) => DeviceMembership::Unavailable,
            }
        } else if state.removed {
            DeviceMembership::Removed
        } else {
            DeviceMembership::Active
        };
        let history = state.membership_reconciliation.as_ref();
        let pending_facts = (!workspace_unverifiable)
            .then(|| history.and_then(|history| history.pending_removal_facts()))
            .flatten();
        let includes_local_device = pending_facts.as_ref().is_some_and(|facts| {
            state
                .own_instance
                .is_some_and(|member| facts.includes_member(member))
        });

        let mut names = BTreeMap::new();
        if let Some(history) = history {
            for admission in history.admitted_device_facts() {
                names.insert(admission.device_id, admission.device_name);
            }
        }
        let roster = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        for member in roster {
            names.insert(member.device_id, member.device_name);
        }
        names.entry(local_device_id.clone()).or_default();

        let mut devices = Vec::with_capacity(names.len());
        for (device_id, display_name) in names {
            let is_local = device_id == local_device_id;
            let reachability = if is_local {
                ReachabilityState::Online
            } else {
                self.deps.presence.current_state(&device_id).await
            };
            let membership = if is_local {
                local_membership
            } else if history.is_some_and(|history| history.is_device_effective(&device_id)) {
                DeviceMembership::Active
            } else if history.is_some_and(|history| history.has_admitted_device(&device_id)) {
                DeviceMembership::Removed
            } else {
                DeviceMembership::Unknown
            };
            let relationship = state.peer_history_relationships.get(&device_id);
            let group_relationship = if workspace_unverifiable {
                GroupRelationship::Unverifiable
            } else {
                match relationship {
                    Some(MembershipHistoryRelationship::Consistent) => {
                        GroupRelationship::Consistent
                    }
                    Some(MembershipHistoryRelationship::PendingRemovalDecision) => {
                        GroupRelationship::PendingLocalDecision
                    }
                    Some(MembershipHistoryRelationship::Diverged) => GroupRelationship::Diverged,
                    Some(MembershipHistoryRelationship::Invalid) => GroupRelationship::Unverifiable,
                    Some(MembershipHistoryRelationship::Unknown)
                    | Some(MembershipHistoryRelationship::UpgradeRequired)
                    | None => GroupRelationship::Unknown,
                }
            };
            let compatibility = match relationship {
                Some(MembershipHistoryRelationship::UpgradeRequired) => {
                    DeviceCompatibility::UpgradeRequired
                }
                Some(MembershipHistoryRelationship::Invalid)
                | Some(MembershipHistoryRelationship::Unknown)
                | None
                    if !is_local =>
                {
                    DeviceCompatibility::Unknown
                }
                _ => DeviceCompatibility::Compatible,
            };
            let sync_relationship = if local_membership == DeviceMembership::Removed {
                SyncRelationship::RemovedLocalDevice
            } else if membership == DeviceMembership::Removed {
                SyncRelationship::RemovedPeerDevice
            } else {
                match (group_relationship, compatibility) {
                    (GroupRelationship::Unverifiable, _) => SyncRelationship::PausedUnverifiable,
                    (GroupRelationship::PendingLocalDecision, _) => {
                        SyncRelationship::WaitingForLocalDecision
                    }
                    (GroupRelationship::Diverged, _) => SyncRelationship::PausedGroupDiverged,
                    (_, DeviceCompatibility::UpgradeRequired) => {
                        SyncRelationship::PausedUpgradeRequired
                    }
                    (GroupRelationship::Consistent, DeviceCompatibility::Compatible) => {
                        SyncRelationship::Usable
                    }
                    _ if is_local => SyncRelationship::Usable,
                    _ => SyncRelationship::Unknown,
                }
            };
            let (available_actions, blocked_reason) = match sync_relationship {
                SyncRelationship::RemovedLocalDevice => (
                    vec![DeviceTrustAction::RejoinDeviceGroup],
                    Some(ActionUnavailableReason::LocalDeviceRemoved),
                ),
                SyncRelationship::PausedUpgradeRequired if is_local => {
                    (vec![DeviceTrustAction::UpdateThisDevice], None)
                }
                SyncRelationship::PausedUpgradeRequired => (
                    Vec::new(),
                    Some(ActionUnavailableReason::PeerUpgradeRequired),
                ),
                SyncRelationship::PausedUnverifiable => (
                    Vec::new(),
                    Some(ActionUnavailableReason::DeviceFactsUnverifiable),
                ),
                _ => (Vec::new(), None),
            };
            devices.push(DeviceTrustRelationship {
                device_id,
                display_name,
                is_local,
                reachability,
                membership,
                group_relationship,
                compatibility,
                sync_relationship,
                available_actions,
                blocked_reason,
            });
        }

        let current_change = pending_facts.map(|facts| {
            let mut apply_usable = devices
                .iter()
                .filter(|device| {
                    device.membership == DeviceMembership::Active
                        && !facts.target_device_ids.contains(&device.device_id)
                })
                .map(|device| device.device_id.clone())
                .collect::<Vec<_>>();
            apply_usable.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            let mut keep_usable = devices
                .iter()
                .filter(|device| device.membership == DeviceMembership::Active)
                .map(|device| device.device_id.clone())
                .collect::<Vec<_>>();
            keep_usable.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            let mut paused = vec![facts.proposed_by_device_id.clone()];
            paused.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            DeviceTrustChange {
                change_id: facts.removal_event_id,
                proposed_by_device_id: facts.proposed_by_device_id,
                target_device_ids: facts.target_device_ids.clone(),
                includes_local_device,
                apply_impact: DeviceTrustImpact {
                    usable_device_ids: apply_usable,
                    paused_device_ids: Vec::new(),
                    local_device_outcome: if includes_local_device {
                        DeviceMembership::Removed
                    } else {
                        local_membership
                    },
                    requires_rejoin_device_ids: facts.target_device_ids,
                },
                keep_current_impact: DeviceTrustImpact {
                    usable_device_ids: keep_usable,
                    paused_device_ids: paused,
                    local_device_outcome: local_membership,
                    requires_rejoin_device_ids: Vec::new(),
                },
                allowed_choices: vec![
                    DeviceTrustChoice::ApplyChange,
                    DeviceTrustChoice::KeepCurrentDeviceGroup,
                ],
                blocked_reason: includes_local_device
                    .then_some(ActionUnavailableReason::LocalDeviceConfirmationRequired),
            }
        });
        let (allowed_actions, blocked_reason) = if workspace_unverifiable {
            (
                Vec::new(),
                Some(ActionUnavailableReason::DeviceFactsUnverifiable),
            )
        } else {
            match &current_change {
                Some(change) if change.includes_local_device => (
                    vec![
                        DeviceTrustAction::KeepCurrentDeviceGroup,
                        DeviceTrustAction::ConfirmApplyRemovesLocalDevice,
                    ],
                    Some(ActionUnavailableReason::LocalDeviceConfirmationRequired),
                ),
                Some(_) => (
                    vec![
                        DeviceTrustAction::ApplyCurrentChange,
                        DeviceTrustAction::KeepCurrentDeviceGroup,
                    ],
                    None,
                ),
                None if local_membership == DeviceMembership::Removed => (
                    vec![DeviceTrustAction::RejoinDeviceGroup],
                    Some(ActionUnavailableReason::LocalDeviceRemoved),
                ),
                None => (Vec::new(), Some(ActionUnavailableReason::NoCurrentChange)),
            }
        };
        Ok(DeviceTrustSnapshot {
            revision: state.revision,
            local_device_id,
            local_membership,
            current_change,
            devices,
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions,
            blocked_reason,
            updated_at_ms: state.updated_at_ms,
        })
    }

    /// Load the current workspace snapshot without changing any state.
    pub async fn query(&self) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let state = self.load_state().await?;
        Ok(state.snapshot())
    }

    /// Receive one bounded membership-history message from an already
    /// authenticated member connection. This owner persists the verified
    /// result before returning an acknowledgement, so callers never compose
    /// history application with a separate persistence step.
    pub async fn handle_membership_history(
        &self,
        source_device_id: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if matches!(
            state.peer_history_relationships.get(source_device_id),
            Some(MembershipHistoryRelationship::Diverged)
        ) && matches!(
            message,
            MembershipHistoryMessage::Hello(_)
                | MembershipHistoryMessage::EventsRequest(_)
                | MembershipHistoryMessage::EventsResponse(_)
        ) {
            return Ok(MembershipHistoryMessage::Ack(
                MembershipHistoryAck::Diverged,
            ));
        }
        let response = match message {
            MembershipHistoryMessage::EventsResponse(response) => {
                self.receive_membership_history_events(
                    &mut state,
                    source_device_id,
                    response,
                    now_ms,
                )
                .await?
            }
            MembershipHistoryMessage::EventsRequest(request) => {
                self.respond_to_membership_history_request(&state, request)
            }
            MembershipHistoryMessage::Hello(hello) => {
                self.respond_to_membership_history_hello(
                    &mut state,
                    source_device_id,
                    hello,
                    now_ms,
                )
                .await?
            }
            MembershipHistoryMessage::Decision(decision) => {
                self.receive_membership_history_decision(
                    &mut state,
                    source_device_id,
                    decision,
                    now_ms,
                )
                .await?
            }
            MembershipHistoryMessage::Ack(ack) => MembershipHistoryMessage::Ack(ack),
        };
        Ok(response)
    }

    /// Start one bounded reconciliation exchange after a peer becomes
    /// reachable. The caller supplies only the authenticated peer identity;
    /// this owner builds every protocol message and persists every reply.
    pub async fn reconcile_membership_history_with_peer(
        &self,
        peer: &DeviceId,
    ) -> Result<(), WorkspaceConvergenceError> {
        let peer_lock = {
            let mut locks = self.peer_reconciliation_locks.lock().await;
            Arc::clone(
                locks
                    .entry(peer.clone())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _peer_guard = peer_lock.lock().await;
        match self
            .reconcile_membership_history(peer, ReconciliationPeerRole::RuntimePeer)
            .await
        {
            Ok(()) => {
                self.clear_upgrade_required_after_current_confirmation(peer)
                    .await?;
                Ok(())
            }
            // A legacy probe is only version evidence when the current 1.1
            // endpoint could not be reached. A current endpoint rejection
            // instead means the authenticated exchange could not proceed and
            // must never be presented as an upgrade requirement.
            Err(current_error @ WorkspaceConvergenceError::Unavailable) => {
                match self.deps.legacy_peer_probe.probe_legacy_peer(peer).await {
                    Ok(()) => self.mark_peer_upgrade_required(peer).await,
                    Err(_) => Err(current_error),
                }
            }
            Err(current_error) => Err(current_error),
        }
    }

    async fn mark_peer_upgrade_required(
        &self,
        peer: &DeviceId,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        self.update_peer_history_relationship(
            &mut state,
            peer.clone(),
            MembershipHistoryRelationship::UpgradeRequired,
            now_ms,
        )?;
        self.persist(&state).await?;
        self.publish(&state);
        self.notify();
        Ok(())
    }

    async fn clear_upgrade_required_after_current_confirmation(
        &self,
        peer: &DeviceId,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.peer_history_relationships.get(peer)
            == Some(&MembershipHistoryRelationship::UpgradeRequired)
        {
            self.update_peer_history_relationship(
                &mut state,
                peer.clone(),
                MembershipHistoryRelationship::Consistent,
                now_ms,
            )?;
            self.persist(&state).await?;
            self.publish(&state);
            self.notify();
        }
        Ok(())
    }

    async fn reconcile_membership_history_with_sponsor(
        &self,
        sponsor: &DeviceId,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.reconcile_membership_history(sponsor, ReconciliationPeerRole::AuthenticatedSponsor)
            .await
    }

    async fn reconcile_membership_history(
        &self,
        peer: &DeviceId,
        peer_role: ReconciliationPeerRole,
    ) -> Result<(), WorkspaceConvergenceError> {
        let initial = {
            let _guard = self.state_lock.lock().await;
            let state = self.load_state().await?;
            if state.removed
                || matches!(
                    state.peer_history_relationships.get(peer),
                    Some(
                        MembershipHistoryRelationship::PendingRemovalDecision
                            | MembershipHistoryRelationship::Diverged
                            | MembershipHistoryRelationship::Invalid
                    )
                )
            {
                return Ok(());
            }
            let Some(history) = state.membership_reconciliation.as_ref() else {
                return Err(WorkspaceConvergenceError::Unavailable);
            };
            let Some(member_instance_id) = state.own_instance else {
                return Err(WorkspaceConvergenceError::Unavailable);
            };
            if matches!(peer_role, ReconciliationPeerRole::RuntimePeer)
                && state
                    .peer_history_relationships
                    .get(peer)
                    .is_none_or(|relationship| {
                        *relationship == MembershipHistoryRelationship::Unknown
                    })
            {
                MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
                    lineage_id: state.space_lineage.clone(),
                    after_event_id: None,
                    events: history.events_after(
                        None,
                        uc_core::membership::MAX_MEMBERSHIP_HISTORY_EVENTS_PER_PAGE,
                    ),
                })
            } else {
                let admission = self.local_admission_facts(Some(member_instance_id)).await?;
                MembershipHistoryMessage::Hello(MembershipHistoryHello {
                    lineage_id: state.space_lineage.clone(),
                    member_instance_id,
                    admission,
                    known_head: history.known_head(),
                    applied_head: history.applied_head(),
                    applied_members_digest: history.applied_members_digest(),
                })
            }
        };

        let mut outgoing = initial;
        for _ in 0..3 {
            let reply = self
                .deps
                .membership_history_exchange
                .exchange_membership_history(peer, outgoing)
                .await
                .map_err(|error| match error {
                    MembershipHistoryExchangeError::Offline
                    | MembershipHistoryExchangeError::Transport => {
                        WorkspaceConvergenceError::Unavailable
                    }
                    MembershipHistoryExchangeError::Rejected => {
                        WorkspaceConvergenceError::Inconsistent(
                            "membership history exchange rejected".to_owned(),
                        )
                    }
                })?;
            if let MembershipHistoryMessage::Ack(ack) = reply {
                tracing::debug!(?ack, "membership history exchange completed");
                return Ok(());
            }
            outgoing = self.handle_membership_history(peer, reply).await?;
            if let MembershipHistoryMessage::Ack(ack) = outgoing {
                tracing::debug!(?ack, "membership history exchange completed locally");
                return Ok(());
            }
        }
        Err(WorkspaceConvergenceError::Inconsistent(
            "membership history exchange exceeded its round limit".to_owned(),
        ))
    }

    /// Persist the local user's decision for one pending remote removal.
    /// The only caller-controlled facts are the opaque pending identifier and
    /// accept/reject choice; this owner derives and signs every other field.
    pub async fn decide_membership_removal(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _decision_guard = self.device_trust_decision_lock.lock().await;
        self.decide_membership_removal_locked(removal_event_id, decision)
            .await
    }

    async fn decide_membership_removal_locked(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let (snapshot, recipients, signed_decision) = {
            let _guard = self.state_lock.lock().await;
            let now_ms = self.deps.clock.now_ms();
            let mut state = self.load_state().await?;
            let own_member_instance_id = state
                .own_instance
                .ok_or(WorkspaceConvergenceError::NotAMember)?;
            let history = state
                .membership_reconciliation
                .as_ref()
                .ok_or(WorkspaceConvergenceError::NotAMember)?;
            if let Some(completed) = history.local_removal_decision(removal_event_id) {
                return if completed == decision {
                    Ok(state.snapshot())
                } else {
                    Err(WorkspaceConvergenceError::Inconsistent(
                        "membership removal was completed with a different decision".to_owned(),
                    ))
                };
            }
            if history.pending_removal_decision() != Some(removal_event_id) {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "membership removal is no longer pending".to_owned(),
                ));
            }
            let removal =
                history
                    .event(removal_event_id)
                    .ok_or(WorkspaceConvergenceError::Inconsistent(
                        "membership removal is unknown".to_owned(),
                    ))?;
            let removal_author_device_id = history
                .device_for_member_before(removal_event_id, &removal.author_member_instance_id)
                .ok_or(WorkspaceConvergenceError::Inconsistent(
                    "membership removal author is unknown".to_owned(),
                ))?;
            let resulting_members_digest = match decision {
                RemovalDecision::Accept => removal.resulting_members_digest,
                RemovalDecision::Reject => history.applied_members_digest().ok_or(
                    WorkspaceConvergenceError::Inconsistent(
                        "membership removal has no applied parent".to_owned(),
                    ),
                )?,
            };
            let mut recipients = history
                .effective_members()
                .into_iter()
                .filter_map(|member| history.device_for_member(&member))
                .filter(|device| *device != self.deps.own_device)
                .collect::<Vec<_>>();
            recipients.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            recipients.dedup();
            let unsigned = MembershipDecision::new(
                state.space_lineage.clone(),
                removal_event_id,
                own_member_instance_id,
                decision,
                history.applied_head(),
                resulting_members_digest,
                uuid::Uuid::new_v4().into_bytes(),
                Vec::new(),
            );
            let signature = self
                .deps
                .member_signatures
                .sign_current_member_payload(&unsigned.signing_payload())
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            let signed_decision = MembershipDecision::new(
                unsigned.lineage_id,
                unsigned.removal_event_id,
                unsigned.decided_by_member_instance_id,
                unsigned.decision,
                unsigned.observed_applied_head,
                unsigned.resulting_members_digest,
                unsigned.decision_nonce,
                signature,
            );
            let applied_events = {
                let history = state
                    .membership_reconciliation
                    .as_mut()
                    .ok_or(WorkspaceConvergenceError::NotAMember)?;
                let previous_applied_head = history.applied_head();
                history
                    .record_decision(signed_decision.clone())
                    .map_err(|_| {
                        WorkspaceConvergenceError::Inconsistent(
                            "membership removal decision was rejected".to_owned(),
                        )
                    })?;
                history.newly_applied_events_after(previous_applied_head)
            };
            for recipient in &recipients {
                state.pending_membership_decision_deliveries.push(
                    uc_core::membership::PendingMembershipDecisionDelivery {
                        recipient: recipient.clone(),
                        decision: signed_decision.clone(),
                    },
                );
            }
            if decision == RemovalDecision::Accept {
                Self::enqueue_applied_membership_effects(&mut state, &applied_events);
                self.persist(&state).await?;
                self.execute_pending_membership_effects(&mut state, now_ms)
                    .await?;
            }
            let relationship = match decision {
                RemovalDecision::Accept => MembershipHistoryRelationship::Consistent,
                RemovalDecision::Reject => MembershipHistoryRelationship::Diverged,
            };
            self.update_peer_history_relationship(
                &mut state,
                removal_author_device_id.clone(),
                relationship,
                now_ms,
            )?;
            self.persist(&state).await?;
            self.publish(&state);
            self.notify();
            (state.snapshot(), recipients, signed_decision)
        };

        let _ = recipients;
        let _ = signed_decision;
        self.deliver_pending_membership_decisions().await?;
        Ok(snapshot)
    }

    pub async fn decide_device_trust_change(
        &self,
        change_id: MembershipEventId,
        choice: DeviceTrustChoice,
        confirm_local_removal: bool,
    ) -> Result<DeviceTrustDecisionResult, WorkspaceConvergenceError> {
        let _decision_guard = self.device_trust_decision_lock.lock().await;
        let state = self.load_state().await?;
        let history = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        if let Some(completed) = history.local_removal_decision(change_id) {
            let completed_choice = match completed {
                RemovalDecision::Accept => DeviceTrustChoice::ApplyChange,
                RemovalDecision::Reject => DeviceTrustChoice::KeepCurrentDeviceGroup,
            };
            let snapshot = self.query_device_trust().await?;
            return if completed_choice == choice {
                Ok(DeviceTrustDecisionResult::AlreadyCompleted {
                    change_id,
                    completed_choice,
                    snapshot,
                })
            } else {
                Ok(DeviceTrustDecisionResult::StateChanged {
                    current_change_id: snapshot
                        .current_change
                        .as_ref()
                        .map(|change| change.change_id),
                    snapshot,
                })
            };
        }
        let pending = history.pending_removal_facts();
        if pending.as_ref().map(|facts| facts.removal_event_id) != Some(change_id) {
            let snapshot = self.query_device_trust().await?;
            return Ok(DeviceTrustDecisionResult::StateChanged {
                current_change_id: snapshot
                    .current_change
                    .as_ref()
                    .map(|change| change.change_id),
                snapshot,
            });
        }
        let removes_local = pending.is_some_and(|facts| {
            state
                .own_instance
                .is_some_and(|member| facts.includes_member(member))
        });
        if choice == DeviceTrustChoice::ApplyChange && removes_local && !confirm_local_removal {
            return Ok(DeviceTrustDecisionResult::LocalDeviceConfirmationRequired {
                change_id,
                snapshot: self.query_device_trust().await?,
            });
        }
        let decision = match choice {
            DeviceTrustChoice::ApplyChange => RemovalDecision::Accept,
            DeviceTrustChoice::KeepCurrentDeviceGroup => RemovalDecision::Reject,
        };
        self.decide_membership_removal_locked(change_id, decision)
            .await?;
        let snapshot = self.query_device_trust().await?;
        Ok(match choice {
            DeviceTrustChoice::ApplyChange => DeviceTrustDecisionResult::Applied {
                change_id,
                snapshot,
            },
            DeviceTrustChoice::KeepCurrentDeviceGroup => {
                DeviceTrustDecisionResult::KeptCurrentDeviceGroup {
                    change_id,
                    snapshot,
                }
            }
        })
    }

    fn respond_to_membership_history_request(
        &self,
        state: &WorkspaceConvergenceState,
        request: MembershipEventsRequest,
    ) -> MembershipHistoryMessage {
        if request.validate().is_err() || request.lineage_id != state.space_lineage {
            return MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid);
        }
        let Some(history) = state.membership_reconciliation.as_ref() else {
            return MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid);
        };
        let events = history.events_after(request.after_event_id, usize::from(request.max_events));
        if events.is_empty() {
            return MembershipHistoryMessage::Ack(MembershipHistoryAck::Consistent);
        }
        MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
            lineage_id: state.space_lineage.clone(),
            after_event_id: request.after_event_id,
            events,
        })
    }

    async fn respond_to_membership_history_hello(
        &self,
        state: &mut WorkspaceConvergenceState,
        source_device_id: &DeviceId,
        hello: MembershipHistoryHello,
        now_ms: i64,
    ) -> Result<MembershipHistoryMessage, WorkspaceConvergenceError> {
        let lineage_matches = hello.lineage_id == state.space_lineage;
        let device_matches = hello.admission.device_id == *source_device_id;
        let instance_matches = hello.admission.member_instance == hello.member_instance_id;
        let signature_valid = self
            .deps
            .member_signatures
            .verify_member_instance_payload(
                source_device_id,
                hello.member_instance_id,
                &hello.admission.signing_payload(),
                &hello.admission.identity_signature,
            )
            .await
            .unwrap_or(false);
        if !lineage_matches || !device_matches || !instance_matches || !signature_valid {
            tracing::debug!(
                lineage_matches,
                device_matches,
                instance_matches,
                signature_valid,
                "membership history hello was invalid"
            );
            return Ok(MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid));
        }
        let upgrade_requirement_cleared = state.peer_history_relationships.get(source_device_id)
            == Some(&MembershipHistoryRelationship::UpgradeRequired);
        if upgrade_requirement_cleared {
            self.update_peer_history_relationship(
                state,
                source_device_id.clone(),
                MembershipHistoryRelationship::Consistent,
                now_ms,
            )?;
        }
        let Some(history) = state.membership_reconciliation.as_ref() else {
            return Ok(MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid));
        };
        if history.known_event_count() > 0
            && !history.has_admitted_device(source_device_id)
            && state.own_instance.is_some()
        {
            let security_state = self.deps.security_updates.current_state().await?;
            let mut digest = sha2::Sha256::new();
            digest.update(b"uniclipboard-membership-security/v1\0");
            digest.update(security_state.space_id.as_ref().as_bytes());
            digest.update(security_state.group_epoch.to_be_bytes());
            self.record_local_admission_history(
                state,
                &hello.admission,
                digest.finalize().into(),
                Vec::new(),
            )
            .await?;
            self.save_member_facts(&hello.admission, now_ms).await?;
            self.persist(state).await?;
        } else if upgrade_requirement_cleared {
            self.persist(state).await?;
        }
        if upgrade_requirement_cleared {
            self.publish(state);
            self.notify();
        }
        let history = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        if hello.known_head == history.known_head() && hello.applied_head == history.applied_head()
        {
            return Ok(MembershipHistoryMessage::Ack(
                MembershipHistoryAck::Consistent,
            ));
        }
        let events = history.events_after(
            hello.known_head,
            uc_core::membership::MAX_MEMBERSHIP_HISTORY_EVENTS_PER_PAGE,
        );
        if events.is_empty() {
            return Ok(MembershipHistoryMessage::EventsRequest(
                MembershipEventsRequest {
                    lineage_id: state.space_lineage.clone(),
                    after_event_id: history.known_head(),
                    max_events: uc_core::membership::MAX_MEMBERSHIP_HISTORY_EVENTS_PER_PAGE as u16,
                },
            ));
        }
        Ok(MembershipHistoryMessage::EventsResponse(
            MembershipEventsResponse {
                lineage_id: state.space_lineage.clone(),
                after_event_id: hello.known_head,
                events,
            },
        ))
    }

    async fn receive_membership_history_events(
        &self,
        state: &mut WorkspaceConvergenceState,
        source_device_id: &DeviceId,
        response: MembershipEventsResponse,
        now_ms: i64,
    ) -> Result<MembershipHistoryMessage, WorkspaceConvergenceError> {
        if response.validate().is_err() || response.lineage_id != state.space_lineage {
            self.update_peer_history_relationship(
                state,
                source_device_id.clone(),
                MembershipHistoryRelationship::Invalid,
                now_ms,
            )?;
            self.persist(state).await?;
            self.publish(state);
            return Ok(MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid));
        }

        let mut outcome = MembershipReconciliationOutcome::UpdatesApplied;
        for event in response.events {
            if !self
                .verify_membership_history_event(state, source_device_id, &event)
                .await?
            {
                tracing::debug!(
                    parent_depth = event.parent_depth,
                    "membership history event signature was invalid"
                );
                self.update_peer_history_relationship(
                    state,
                    source_device_id.clone(),
                    MembershipHistoryRelationship::Invalid,
                    now_ms,
                )?;
                self.persist(state).await?;
                self.publish(state);
                return Ok(MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid));
            }
            let applied_events = {
                let Some(history) = state.membership_reconciliation.as_mut() else {
                    return Err(WorkspaceConvergenceError::NotAMember);
                };
                let previous_applied_head = history.applied_head();
                outcome = match history.receive_verified(event) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        tracing::debug!(
                            ?error,
                            "membership history event failed structural validation"
                        );
                        self.update_peer_history_relationship(
                            state,
                            source_device_id.clone(),
                            MembershipHistoryRelationship::Invalid,
                            now_ms,
                        )?;
                        self.persist(state).await?;
                        self.publish(state);
                        return Ok(MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid));
                    }
                };
                history.newly_applied_events_after(previous_applied_head)
            };
            Self::enqueue_applied_membership_effects(state, &applied_events);
            self.persist(state).await?;
            self.execute_pending_membership_effects(state, now_ms)
                .await?;
            if matches!(outcome, MembershipReconciliationOutcome::Diverged) {
                break;
            }
        }

        let (relationship, ack) = match outcome {
            MembershipReconciliationOutcome::UpdatesApplied => (
                MembershipHistoryRelationship::Consistent,
                MembershipHistoryAck::UpdatesApplied,
            ),
            MembershipReconciliationOutcome::RemovalDecisionRequired { removal_event_id } => (
                MembershipHistoryRelationship::PendingRemovalDecision,
                MembershipHistoryAck::RemovalDecisionRequired { removal_event_id },
            ),
            MembershipReconciliationOutcome::RemovalAccepted { removal_event_id } => (
                MembershipHistoryRelationship::Consistent,
                MembershipHistoryAck::RemovalAccepted { removal_event_id },
            ),
            MembershipReconciliationOutcome::RemovalRejected { removal_event_id } => (
                MembershipHistoryRelationship::Diverged,
                MembershipHistoryAck::RemovalRejected { removal_event_id },
            ),
            MembershipReconciliationOutcome::Diverged => (
                MembershipHistoryRelationship::Diverged,
                MembershipHistoryAck::Diverged,
            ),
        };
        self.update_peer_history_relationship(
            state,
            source_device_id.clone(),
            relationship,
            now_ms,
        )?;
        self.persist(state).await?;
        self.publish(state);
        Ok(MembershipHistoryMessage::Ack(ack))
    }

    async fn receive_membership_history_decision(
        &self,
        state: &mut WorkspaceConvergenceState,
        source_device_id: &DeviceId,
        decision: MembershipDecision,
        now_ms: i64,
    ) -> Result<MembershipHistoryMessage, WorkspaceConvergenceError> {
        let Some(history) = state.membership_reconciliation.as_ref() else {
            return Ok(MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid));
        };
        let valid_binding = decision.lineage_id == state.space_lineage
            && history.event(decision.removal_event_id).is_some()
            && history.device_for_member_before(
                decision.removal_event_id,
                &decision.decided_by_member_instance_id,
            ) == Some(source_device_id.clone());
        let valid_signature = if valid_binding {
            self.deps
                .member_signatures
                .verify_member_instance_payload(
                    source_device_id,
                    decision.decided_by_member_instance_id,
                    &decision.signing_payload(),
                    &decision.signature,
                )
                .await
                .unwrap_or(false)
        } else {
            false
        };
        if !valid_signature {
            self.update_peer_history_relationship(
                state,
                source_device_id.clone(),
                MembershipHistoryRelationship::Invalid,
                now_ms,
            )?;
            self.persist(state).await?;
            self.publish(state);
            return Ok(MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid));
        }
        let local_decision = state
            .membership_reconciliation
            .as_ref()
            .and_then(|history| {
                state
                    .own_instance
                    .and_then(|own| history.decision_for(decision.removal_event_id, own))
            })
            .map(|decision| decision.decision);
        let outcome = state
            .membership_reconciliation
            .as_mut()
            .ok_or(WorkspaceConvergenceError::NotAMember)?
            .record_peer_decision(decision.clone());
        let (relationship, ack) = match outcome {
            Ok(MembershipReconciliationOutcome::RemovalAccepted { removal_event_id }) => (
                if local_decision.is_none_or(|local| local == decision.decision) {
                    MembershipHistoryRelationship::Consistent
                } else {
                    MembershipHistoryRelationship::Diverged
                },
                MembershipHistoryAck::RemovalAccepted { removal_event_id },
            ),
            Ok(MembershipReconciliationOutcome::RemovalRejected { removal_event_id }) => (
                if local_decision == Some(decision.decision) {
                    MembershipHistoryRelationship::Consistent
                } else {
                    MembershipHistoryRelationship::Diverged
                },
                MembershipHistoryAck::RemovalRejected { removal_event_id },
            ),
            Ok(MembershipReconciliationOutcome::Diverged) => (
                MembershipHistoryRelationship::Diverged,
                MembershipHistoryAck::Diverged,
            ),
            _ => {
                self.update_peer_history_relationship(
                    state,
                    source_device_id.clone(),
                    MembershipHistoryRelationship::Invalid,
                    now_ms,
                )?;
                self.persist(state).await?;
                self.publish(state);
                return Ok(MembershipHistoryMessage::Ack(MembershipHistoryAck::Invalid));
            }
        };
        self.update_peer_history_relationship(
            state,
            source_device_id.clone(),
            relationship,
            now_ms,
        )?;
        self.persist(state).await?;
        self.publish(state);
        Ok(MembershipHistoryMessage::Ack(ack))
    }

    async fn verify_membership_history_event(
        &self,
        state: &WorkspaceConvergenceState,
        _source_device_id: &DeviceId,
        event: &MembershipEvent,
    ) -> Result<bool, WorkspaceConvergenceError> {
        let Some(history) = state.membership_reconciliation.as_ref() else {
            return Ok(false);
        };
        let author_device = match event.parent_event_id {
            Some(_) => history.device_for_member(&event.author_member_instance_id),
            None => match &event.operation {
                MembershipOperation::AddDevice { admission }
                    if admission.member_instance == event.author_member_instance_id =>
                {
                    Some(admission.device_id.clone())
                }
                _ => None,
            },
        };
        let Some(author_device) = author_device else {
            return Ok(false);
        };
        let valid_author = self
            .deps
            .member_signatures
            .verify_member_instance_payload(
                &author_device,
                event.author_member_instance_id,
                &event.signing_payload(),
                &event.signature,
            )
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        Ok(valid_author)
    }

    fn update_peer_history_relationship(
        &self,
        state: &mut WorkspaceConvergenceState,
        peer: DeviceId,
        relationship: MembershipHistoryRelationship,
        now_ms: i64,
    ) -> Result<(), WorkspaceConvergenceError> {
        state
            .apply(
                WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated { peer, relationship },
                now_ms,
            )
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("history relationship rejected".to_owned())
            })?;
        Ok(())
    }

    /// Submit a local member removal by appending one signed history event.
    /// Other devices receive the event through history reconciliation and
    /// decide for themselves; this path never emits an auto-applied intent.
    pub async fn submit_removal(
        &self,
        target: &DeviceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let mut state = self.load_state().await?;
        if state.removed {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        let own = state
            .own_instance
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let target_member = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::NotAMember)?
            .effective_members()
            .into_iter()
            .find(|member| {
                state
                    .membership_reconciliation
                    .as_ref()
                    .and_then(|history| history.device_for_member(member))
                    .as_ref()
                    == Some(target)
            })
            .ok_or(WorkspaceConvergenceError::UnknownTarget)?;
        if target_member == own {
            return Err(WorkspaceConvergenceError::SelfTarget);
        }
        let security_state_digest = state
            .current_digest()
            .map(|digest| *digest.as_bytes())
            .unwrap_or([0; 32]);
        self.record_local_removal_history(&mut state, target_member, security_state_digest)
            .await?;
        self.persist(&state).await?;
        self.publish(&state);
        self.notify();
        Ok(state.snapshot())
    }

    /// Build the locally signed facts that a joiner returns after its group
    /// session is active. The facts remain inside the pairing exchange until
    /// the sponsor commits the admission chain.
    ///
    /// `member_instance` overrides the security-view resolution: a joining
    /// device must identify itself by the instance derived from this
    /// admission's freshly generated credential. The security view can still
    /// carry a stale instance of the same device from an earlier admission
    /// (a removed device cannot receive group updates), so the view must not
    /// be the source of truth for a fresh admission.
    pub async fn local_admission_facts(
        &self,
        member_instance: Option<uc_core::membership::MemberInstanceId>,
    ) -> Result<uc_core::membership::AdmissionChangeFacts, WorkspaceConvergenceError> {
        let material = self
            .deps
            .announcement_material
            .current_announcement_material()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let member_instance = match member_instance {
            Some(instance) => instance,
            None => self
                .load_state()
                .await?
                .own_instance
                .ok_or(WorkspaceConvergenceError::NotAMember)?,
        };
        let mut facts = uc_core::membership::AdmissionChangeFacts {
            member_instance,
            device_id: material.device_id,
            device_name: material.device_name,
            identity_fingerprint: material.identity_fingerprint,
            transport_public_key: material.transport_public_key,
            transport_address_blob: material.transport_address_blob,
            identity_signature: Vec::new(),
        };
        facts.identity_signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&facts.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        Ok(facts)
    }

    /// Save the sponsor's in-flight admission record before it starts
    /// waiting for the joiner's readiness. Survives restarts so the sponsor
    /// re-awaits the same joiner's readiness instead of saving a second
    /// member instance or a duplicated change. Idempotent for the same
    /// session and joiner.
    pub async fn begin_admission(
        &self,
        session: &uc_core::ports::pairing::PairingSessionId,
        joiner_device_id: &DeviceId,
        invitation_generation: u64,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if let Some(persisted_instance) = state.own_instance {
            let current_instance = self
                .deps
                .member_signatures
                .current_member_instance(&self.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            if current_instance != persisted_instance {
                state
                    .apply(
                        WorkspaceConvergenceEvent::IntegrityFailure(
                            uc_core::membership::WorkspaceFailureCategory::IdentityMismatch,
                        ),
                        now_ms,
                    )
                    .map_err(|_| {
                        WorkspaceConvergenceError::Inconsistent(
                            "current member identity mismatch could not be recorded".to_owned(),
                        )
                    })?;
                self.persist(&state).await?;
                self.publish(&state);
                self.notify();
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "current member identity does not match persisted membership history"
                        .to_owned(),
                ));
            }
        }
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::AdmissionBegan {
                    session: session.clone(),
                    joiner_device_id: joiner_device_id.clone(),
                    invitation_generation,
                },
                now_ms,
            )
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("admission begin rejected".to_owned())
            })?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        self.notify();
        Ok(state.snapshot())
    }

    /// The sponsor's saved in-flight admission record for a pairing session,
    /// if any. Used after a restart to re-await the same joiner's readiness.
    pub async fn pending_admission(
        &self,
        session: &uc_core::ports::pairing::PairingSessionId,
    ) -> Result<Option<uc_core::membership::PendingAdmissionRecord>, WorkspaceConvergenceError>
    {
        let state = self.load_state().await?;
        Ok(state.pending_admissions.get(session).cloned())
    }

    /// Commit the readiness-confirmed admission in the single owner. On the
    /// first pairing the sponsor's already active member instance is seeded
    /// together with the joining instance in one repository save. The
    /// admission change, the joiner's pending handoff facts and the
    /// confirmation material are saved in the same commit; the in-flight
    /// admission record is cleared there as well. The returned confirmation
    /// is sent back to the joiner over the pairing channel.
    pub async fn commit_joiner_admission(
        &self,
        session: &uc_core::ports::pairing::PairingSessionId,
        joiner: uc_core::membership::AdmissionChangeFacts,
        security_update_payload: Vec<u8>,
    ) -> Result<uc_core::membership::AdmissionSavedFacts, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.removed {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        if let Some(record) = state.pending_admissions.get(session) {
            if record.joiner_device_id != joiner.device_id {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "joiner readiness does not match the in-flight admission".to_owned(),
                ));
            }
            if record.invitation_generation < Self::admission_generation(&state) {
                // The admission generation advanced after the invitation was
                // bound; an old invitation cannot recover its old authority.
                return Err(WorkspaceConvergenceError::AdmissionGenerationAdvanced);
            }
        }
        let own_instance = match state.own_instance {
            Some(instance) => instance,
            None => self
                .deps
                .member_signatures
                .current_member_instance(&self.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?,
        };
        let own = self.local_admission_facts(Some(own_instance)).await?;
        let mut additions = Vec::new();
        if !state.effective_members().contains(&own.member_instance) {
            additions.push(own.clone());
        }
        if !state.effective_members().contains(&joiner.member_instance) {
            additions.push(joiner.clone());
        }
        if additions.is_empty() {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "admission unchanged".to_owned(),
            ));
        }
        // The local instance owns the admission event signatures. Establish
        // its durable history before the first event is appended.
        if state.own_instance.is_none() {
            state
                .apply(
                    WorkspaceConvergenceEvent::LocalAdmissionReady {
                        own_instance: own.member_instance,
                    },
                    now_ms,
                )
                .map_err(|_| {
                    WorkspaceConvergenceError::Inconsistent("own instance rejected".to_owned())
                })?;
        }
        // The roster persistence failures abort the commit before any
        // workspace change is persisted, keeping the save boundary intact.
        self.save_member_facts(&joiner, now_ms).await?;
        let security_state_digest = sha2::Sha256::digest(&security_update_payload).into();
        for facts in &additions {
            let event_security_update = (facts.member_instance == joiner.member_instance)
                .then_some(security_update_payload.clone())
                .unwrap_or_default();
            self.record_local_admission_history(
                &mut state,
                facts,
                security_state_digest,
                event_security_update,
            )
            .await?;
        }
        let _ = state.apply(
            WorkspaceConvergenceEvent::AdmissionCleared {
                session: session.clone(),
            },
            now_ms,
        );
        self.persist(&state).await?;
        self.publish(&state);
        self.notify();
        info!(joiner_device_id = %joiner.device_id.as_str(), "workspace admission change recorded");
        let history = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let history_digest = history.applied_members_digest().ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent(
                "admission produced no history digest".to_owned(),
            )
        })?;
        Ok(uc_core::membership::AdmissionSavedFacts {
            history_digest,
            history_event_count: history.known_event_count() as u64,
            sponsor_facts: own,
        })
    }

    /// Complete the joiner's admission only after it has saved the sponsor's
    /// member facts and recovered the exact history progress the sponsor
    /// committed. The sponsor is already authenticated by the pairing
    /// channel, so it may relay earlier members' individually signed events;
    /// every event is still verified against its actual author before it is
    /// persisted.
    pub async fn record_admission_saved(
        &self,
        confirmation: uc_core::membership::AdmissionSavedFacts,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        self.save_member_facts(&confirmation.sponsor_facts, now_ms)
            .await?;
        self.reconcile_membership_history_with_sponsor(&confirmation.sponsor_facts.device_id)
            .await?;

        let _guard = self.state_lock.lock().await;
        let state = self.load_state().await?;
        let history = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        if history.known_event_count() as u64 != confirmation.history_event_count
            || history.applied_members_digest() != Some(confirmation.history_digest)
        {
            tracing::debug!(
                local_history_event_count = history.known_event_count(),
                sponsor_history_event_count = confirmation.history_event_count,
                digest_matches =
                    history.applied_members_digest() == Some(confirmation.history_digest),
                "sponsor admission history did not match the saved confirmation"
            );
            return Err(WorkspaceConvergenceError::Inconsistent(
                "sponsor admission history is incomplete or mismatched".to_owned(),
            ));
        }
        Ok(state.snapshot())
    }

    /// Persist the admitted member's roster facts (member instance, trust
    /// record and transport address) as part of the admission save boundary.
    async fn save_member_facts(
        &self,
        facts: &uc_core::membership::AdmissionChangeFacts,
        now_ms: i64,
    ) -> Result<(), WorkspaceConvergenceError> {
        let joined_at = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
            .ok_or_else(|| WorkspaceConvergenceError::AdmissionStorage("clock".to_owned()))?;
        let sync_preferences = match self.deps.member_repo.get(&facts.device_id).await {
            Ok(Some(existing)) => existing.sync_preferences,
            _ => uc_core::MemberSyncPreferences::default(),
        };
        let member = uc_core::SpaceMember {
            device_id: facts.device_id.clone(),
            device_name: facts.device_name.clone(),
            identity_fingerprint: facts.identity_fingerprint.clone(),
            joined_at,
            sync_preferences,
        };
        self.deps
            .member_repo
            .save(&member)
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let peer = uc_core::trusted_peer::TrustedPeer {
            local_device_id: self.deps.device_identity.current_device_id(),
            peer_device_id: facts.device_id.clone(),
            peer_fingerprint: facts.identity_fingerprint.clone(),
            trusted_at: joined_at,
        };
        self.deps
            .trusted_peer_repo
            .save(&peer)
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        if !facts.transport_address_blob.is_empty() {
            self.deps
                .peer_addr_repo
                .upsert(&uc_core::ports::PeerAddressRecord {
                    device_id: facts.device_id.clone(),
                    addr_blob: facts.transport_address_blob.clone(),
                    observed_at: joined_at,
                })
                .await
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        }
        Ok(())
    }

    async fn record_local_admission_history(
        &self,
        state: &mut WorkspaceConvergenceState,
        facts: &uc_core::membership::AdmissionChangeFacts,
        security_state_digest: [u8; 32],
        security_update_payload: Vec<u8>,
    ) -> Result<(), WorkspaceConvergenceError> {
        use sha2::{Digest, Sha256};

        let own_instance = state
            .own_instance
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let (parent_event_id, parent_depth, mut members) = {
            let history = state.membership_reconciliation.get_or_insert_with(|| {
                uc_core::membership::MembershipReconciliation::new(
                    state.space_lineage.clone(),
                    own_instance,
                )
            });
            let (parent_event_id, parent_depth) = history.next_event_position();
            (parent_event_id, parent_depth, history.effective_members())
        };
        members.insert(facts.member_instance);
        let mut members_hasher = Sha256::new();
        members_hasher.update(b"uniclipboard-membership-members/v1\0");
        members_hasher.update(state.space_lineage.as_bytes());
        for member in members {
            members_hasher.update(member.as_bytes());
        }
        let resulting_members_digest = members_hasher.finalize().into();
        let admission_bundle_digest = Some(Sha256::digest(facts.signing_payload()).into());
        let operation_id = uuid::Uuid::new_v4().into_bytes();
        let unsigned = uc_core::membership::MembershipEvent::new(
            state.space_lineage.clone(),
            parent_event_id,
            parent_depth,
            operation_id,
            own_instance,
            uc_core::membership::MembershipOperation::AddDevice {
                admission: facts.clone(),
            },
            resulting_members_digest,
            security_state_digest,
            security_update_payload,
            admission_bundle_digest,
            Vec::new(),
        );
        let signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&unsigned.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let event = uc_core::membership::MembershipEvent::new(
            unsigned.lineage_id,
            unsigned.parent_event_id,
            unsigned.parent_depth,
            unsigned.operation_id,
            unsigned.author_member_instance_id,
            unsigned.operation,
            unsigned.resulting_members_digest,
            unsigned.security_state_digest,
            unsigned.security_update_payload,
            unsigned.admission_bundle_digest,
            signature,
        );
        state
            .membership_reconciliation
            .as_mut()
            .ok_or(WorkspaceConvergenceError::NotAMember)?
            .receive_verified(event)
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("admission history rejected".to_owned())
            })?;
        Ok(())
    }

    async fn record_local_removal_history(
        &self,
        state: &mut WorkspaceConvergenceState,
        removed_member: uc_core::membership::MemberInstanceId,
        security_state_digest: [u8; 32],
    ) -> Result<(), WorkspaceConvergenceError> {
        use sha2::{Digest, Sha256};

        let own_instance = state
            .own_instance
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let (parent_event_id, parent_depth, mut members) = {
            let history = state
                .membership_reconciliation
                .as_ref()
                .ok_or(WorkspaceConvergenceError::NotAMember)?;
            let (parent_event_id, parent_depth) = history.next_event_position();
            (parent_event_id, parent_depth, history.effective_members())
        };
        if !members.remove(&removed_member) {
            return Err(WorkspaceConvergenceError::UnknownTarget);
        }
        let mut members_hasher = Sha256::new();
        members_hasher.update(b"uniclipboard-membership-members/v1\0");
        members_hasher.update(state.space_lineage.as_bytes());
        for member in members {
            members_hasher.update(member.as_bytes());
        }
        let resulting_members_digest = members_hasher.finalize().into();
        let operation_id = uuid::Uuid::new_v4().into_bytes();
        let unsigned = uc_core::membership::MembershipEvent::new(
            state.space_lineage.clone(),
            parent_event_id,
            parent_depth,
            operation_id,
            own_instance,
            uc_core::membership::MembershipOperation::RemoveDevice {
                member: removed_member,
            },
            resulting_members_digest,
            security_state_digest,
            Vec::new(),
            None,
            Vec::new(),
        );
        let signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&unsigned.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let event = uc_core::membership::MembershipEvent::new(
            unsigned.lineage_id,
            unsigned.parent_event_id,
            unsigned.parent_depth,
            unsigned.operation_id,
            unsigned.author_member_instance_id,
            unsigned.operation,
            unsigned.resulting_members_digest,
            unsigned.security_state_digest,
            unsigned.security_update_payload,
            unsigned.admission_bundle_digest,
            signature,
        );
        state
            .membership_reconciliation
            .as_mut()
            .ok_or(WorkspaceConvergenceError::NotAMember)?
            .receive_verified(event)
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("removal history rejected".to_owned())
            })?;
        Ok(())
    }

    /// Record the local member instance and its readiness record after a
    /// successful admission (the joiner's local readiness; the sponsor
    /// records the admission change only after this readiness).
    ///
    /// A re-admission with a new member instance discards the previous
    /// instance's local chain, confirmations and removal facts: the old
    /// instance's history must not constrain the new one (ADR-015 new
    /// instance rule). The lineage is preserved.
    pub async fn record_local_readiness(
        &self,
        own_instance: uc_core::membership::MemberInstanceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.removed
            || state
                .own_instance
                .is_some_and(|previous| previous != own_instance)
        {
            let lineage = state.space_lineage.clone();
            state = WorkspaceConvergenceState::fresh(lineage, now_ms);
            self.persist(&state).await?;
        }
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::LocalAdmissionReady { own_instance },
                now_ms,
            )
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("readiness rejected".to_owned())
            })?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        Ok(state.snapshot())
    }

    /// Establish the single current-history starting point after a retained
    /// pre-1.1 space has completed its shared protection upgrade.
    pub async fn initialize_upgraded_legacy_space(
        &self,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_facts = self.local_admission_facts(Some(own_instance)).await?;
        let members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let is_stable_initializer = members
            .iter()
            .map(|member| &member.device_id)
            .chain(std::iter::once(&self.deps.own_device))
            .min_by(|left, right| left.as_str().cmp(right.as_str()))
            == Some(&self.deps.own_device);
        let mut protection_member_ids = members
            .iter()
            .map(|member| member.device_id)
            .collect::<Vec<_>>();
        protection_member_ids.push(self.deps.own_device);
        protection_member_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        protection_member_ids.dedup();
        let resumes_owned_legacy_bootstrap = self
            .deps
            .space_protection
            .query_space_protection(&protection_member_ids)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?
            .legacy_bootstrap
            .is_some_and(|item| {
                item.status == uc_core::membership::LegacyBootstrapStatus::AwaitingReadmission
            });
        let is_initializer = is_stable_initializer || resumes_owned_legacy_bootstrap;
        let security_state = self.deps.security_updates.current_state().await?;
        let mut digest = sha2::Sha256::new();
        digest.update(b"uniclipboard-membership-security/v1\0");
        digest.update(security_state.space_id.as_ref().as_bytes());
        digest.update(security_state.group_epoch.to_be_bytes());

        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.own_instance.is_none() {
            state
                .apply(
                    WorkspaceConvergenceEvent::LocalAdmissionReady { own_instance },
                    now_ms,
                )
                .map_err(|_| {
                    WorkspaceConvergenceError::Inconsistent(
                        "legacy upgrade readiness rejected".to_owned(),
                    )
                })?;
        }
        let history_empty = state
            .membership_reconciliation
            .as_ref()
            .is_none_or(|history| history.known_event_count() == 0);
        if is_initializer && history_empty {
            self.record_local_admission_history(
                &mut state,
                &own_facts,
                digest.finalize().into(),
                Vec::new(),
            )
            .await?;
        }
        if state
            .membership_reconciliation
            .as_ref()
            .is_some_and(|history| history.applied_head().is_some())
        {
            state.migrated_from_pre_adr_020 = false;
        }
        self.persist(&state).await?;
        self.publish(&state);
        self.notify();
        Ok(state.snapshot())
    }

    /// Complete a retained legacy member's protection-group join by fetching
    /// the sponsor's authoritative current membership history before normal
    /// peer reconciliation resumes.
    pub async fn complete_upgraded_legacy_join(
        &self,
        sponsor: &DeviceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        self.initialize_upgraded_legacy_space().await?;
        self.reconcile_membership_history_with_sponsor(sponsor)
            .await?;
        self.query().await
    }

    /// Reconcile the local member history with every applied peer before an
    /// admission is committed. The bounded exchange is the same one used by
    /// the runtime when a peer becomes reachable, so admission cannot revive
    /// the superseded recovery channel or use a second membership source.
    pub async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError> {
        let history_candidates = {
            let _guard = self.state_lock.lock().await;
            let state = self.load_state().await?;
            if state.removed {
                return Ok(());
            }
            state.membership_reconciliation.as_ref().map(|history| {
                history
                    .effective_members()
                    .into_iter()
                    .filter_map(|member| history.device_for_member(&member))
                    .filter(|device| *device != self.deps.own_device)
                    .collect::<Vec<_>>()
            })
        };
        let mut candidates = if let Some(candidates) = history_candidates {
            candidates
        } else {
            self.deps
                .peer_addr_repo
                .list()
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?
                .into_iter()
                .map(|record| record.device_id)
                .filter(|device| *device != self.deps.own_device)
                .collect::<Vec<_>>()
        };
        candidates.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        candidates.dedup();
        for device in candidates {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                self.reconcile_membership_history_with_peer(&device),
            )
            .await;
        }
        Ok(())
    }
}

#[async_trait]
impl uc_core::membership::ContentExchangeGatePort for WorkspaceConvergence {
    async fn is_locally_removed(&self, device_id: &DeviceId) -> bool {
        self.locally_removed(device_id).await
    }
}

#[async_trait]
impl uc_core::membership::CurrentWorkspacePeerScopePort for WorkspaceConvergence {
    async fn snapshot(
        &self,
    ) -> Result<
        uc_core::membership::CurrentWorkspacePeerSnapshot,
        uc_core::membership::CurrentWorkspacePeerScopeError,
    > {
        use uc_core::membership::{
            CurrentWorkspaceLocalMembership, CurrentWorkspacePeerScopeError,
            CurrentWorkspacePeerScopeSource, CurrentWorkspacePeerSnapshot,
        };

        let state = self.load_state().await.map_err(|error| match error {
            WorkspaceConvergenceError::Repository(
                uc_core::membership::WorkspaceConvergenceRepositoryError::Locked,
            ) => CurrentWorkspacePeerScopeError::Locked,
            WorkspaceConvergenceError::Repository(
                uc_core::membership::WorkspaceConvergenceRepositoryError::Corrupt,
            ) => CurrentWorkspacePeerScopeError::Corrupt,
            _ => CurrentWorkspacePeerScopeError::Unavailable,
        })?;
        let history = state
            .membership_reconciliation
            .as_ref()
            .filter(|history| history.applied_head().is_some());
        let Some(history) = history else {
            let members = self
                .deps
                .member_repo
                .list()
                .await
                .map_err(|_| CurrentWorkspacePeerScopeError::Unavailable)?;
            let member_ids = members
                .iter()
                .map(|member| member.device_id)
                .collect::<Vec<_>>();
            let mut protection_member_ids = member_ids.clone();
            protection_member_ids.push(self.deps.own_device);
            protection_member_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            protection_member_ids.dedup();
            let protection = self
                .deps
                .space_protection
                .query_space_protection(&protection_member_ids)
                .await
                .map_err(|error| match error {
                    uc_core::membership::SpaceProtectionError::Corrupted => {
                        CurrentWorkspacePeerScopeError::Corrupt
                    }
                    _ => CurrentWorkspacePeerScopeError::Unavailable,
                })?;
            let active_legacy_bootstrap =
                protection.legacy_bootstrap.as_ref().is_some_and(|item| {
                    item.status == uc_core::membership::LegacyBootstrapStatus::AwaitingReadmission
                });
            if protection.mode != uc_core::membership::SpaceProtectionMode::Legacy
                && !state.migrated_from_pre_adr_020
                && !active_legacy_bootstrap
            {
                return Err(CurrentWorkspacePeerScopeError::Unavailable);
            }
            let local_is_member = protection.mode
                == uc_core::membership::SpaceProtectionMode::Legacy
                || protection.members.iter().any(|member| {
                    member.device_id == self.deps.own_device
                        && member.status == uc_core::membership::MemberProtectionStatus::Protected
                });
            let mut peer_device_ids = if local_is_member {
                member_ids
                    .into_iter()
                    .filter(|device| *device != self.deps.own_device)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            peer_device_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            peer_device_ids.dedup();
            return Ok(CurrentWorkspacePeerSnapshot {
                revision: state.revision,
                source: CurrentWorkspacePeerScopeSource::Legacy,
                local_membership: if local_is_member {
                    CurrentWorkspaceLocalMembership::Active
                } else {
                    CurrentWorkspaceLocalMembership::Removed
                },
                peer_device_ids,
            });
        };
        let local_membership = if state.removed
            || state
                .own_instance
                .is_none_or(|instance| !history.effective_members().contains(&instance))
        {
            CurrentWorkspaceLocalMembership::Removed
        } else {
            CurrentWorkspaceLocalMembership::Active
        };
        let pending_additions = state
            .pending_applied_membership_effects
            .iter()
            .filter_map(|effect| history.event(effect.event_id))
            .filter_map(|event| match &event.operation {
                MembershipOperation::AddDevice { admission } => Some(admission.device_id.clone()),
                MembershipOperation::RemoveDevice { .. } => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        let mut peer_device_ids = if local_membership == CurrentWorkspaceLocalMembership::Active {
            history
                .effective_members()
                .into_iter()
                .filter_map(|member| history.device_for_member(&member))
                .filter(|device| *device != self.deps.own_device)
                .filter(|device| !pending_additions.contains(device))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        peer_device_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        peer_device_ids.dedup();

        Ok(CurrentWorkspacePeerSnapshot {
            revision: state.revision,
            source: CurrentWorkspacePeerScopeSource::CurrentHistory,
            local_membership,
            peer_device_ids,
        })
    }
}

#[async_trait]
impl uc_core::membership::MembershipAdmissionGatePort for WorkspaceConvergence {
    async fn admission_decision(
        &self,
        invitation_generation: u64,
    ) -> uc_core::membership::MembershipAdmissionDecision {
        let state = match self.load_state().await {
            Ok(state) => state,
            Err(_) => return uc_core::membership::MembershipAdmissionDecision::Unavailable,
        };
        Self::admission_decision_for_state(&state, invitation_generation)
    }

    async fn invitation_generation(
        &self,
    ) -> Result<u64, uc_core::membership::MembershipAdmissionDecision> {
        let state = self
            .load_state()
            .await
            .map_err(|_| uc_core::membership::MembershipAdmissionDecision::Unavailable)?;
        Ok(Self::admission_generation(&state))
    }
}

#[async_trait]
impl MembershipHistoryExchangeEndpointPort for WorkspaceConvergence {
    async fn handle_membership_history_exchange(
        &self,
        source_device_id: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, MembershipHistoryExchangeError> {
        self.handle_membership_history(source_device_id, message)
            .await
            .map_err(|_| MembershipHistoryExchangeError::Rejected)
    }
}
