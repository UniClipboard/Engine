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

pub(crate) mod admission;
pub(crate) mod assembly;
pub(crate) mod connectivity;
pub mod discovery;
pub(crate) mod membership;
pub(crate) mod network_recovery;
pub(crate) mod projection;
mod runtime;

#[cfg(test)]
#[path = "testing/mod.rs"]
pub(crate) mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use sha2::Digest;
use tokio::sync::broadcast;
use tracing::info;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignaturePort, CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort,
    MemberRepositoryPort, MembershipDecision, MembershipDecisionV2, MembershipEvent,
    MembershipEventId, MembershipEventV2, MembershipHistoryExchangeEndpointPort,
    MembershipHistoryExchangeError, MembershipHistoryExchangePort, MembershipHistoryMessage,
    MembershipHistoryRelationship, MembershipOperation, MembershipOperationV2,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort, RemovalDecision,
    SpaceProtectionStatusPort, WorkspaceConvergenceEvent, WorkspaceConvergenceRepositoryError,
    WorkspaceConvergenceRepositoryPort, WorkspaceConvergenceState, WorkspaceMergeOutcome,
    WorkspacePhase, WorkspaceSnapshot, MEMBERSHIP_DECISION_FORMAT_V2, MEMBERSHIP_EVENT_FORMAT_V2,
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
    #[error("unreadable source history requires explicit confirmation")]
    UnreadableHistoryRequiresConfirmation,
    #[error("another workspace admission is already in progress")]
    AdmissionInProgress,
    #[error("the previous local join cannot be superseded")]
    PreviousJoinCannotBeSuperseded,
    #[error("the admission conflicts with the current membership history")]
    AdmissionConflict,
    #[error("local join was not found")]
    JoinNotFound,
    #[error("workspace convergence is unavailable")]
    Unavailable,
}

pub struct WorkspaceConvergenceDeps {
    pub initial_state_origin: WorkspaceConvergenceStateOrigin,
    pub repository: Arc<dyn WorkspaceConvergenceRepositoryPort>,
    pub admission_attempts: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    pub historical_membership_signatures:
        Arc<dyn uc_core::membership::HistoricalMembershipSignatureVerifier>,
    pub admission_security_transition:
        Arc<dyn uc_core::membership::AdmissionSecurityTransitionPort>,
    pub prepare_sponsor_admission_security:
        Arc<dyn uc_core::membership::PrepareSponsorAdmissionSecurityPort>,
    pub activate_sponsor_admission_security:
        Arc<dyn uc_core::membership::ActivateSponsorAdmissionSecurityPort>,
    pub activate_completion_helper_admission_security:
        Arc<dyn uc_core::membership::ActivateCompletionHelperAdmissionSecurityPort>,
    pub admission_space_transition: Arc<dyn uc_core::membership::AdmissionSpaceTransitionPort>,
    pub admission_outbox_delivery: Arc<dyn uc_core::membership::AdmissionOutboxDeliveryPort>,
    pub admission_completion_recovery:
        Arc<dyn uc_core::membership::AdmissionCompletionRecoveryPort>,
    pub legacy_migration_recovery: Arc<dyn uc_core::ports::setup::LegacyMigrationRecoveryPort>,
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
    /// Member roster persistence: admission commits write the admitted
    /// member facts here in the same save boundary.
    pub trusted_peer_repo: Arc<dyn TrustedPeerRepositoryPort>,
    pub peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    pub presence: Arc<dyn PresencePort>,
    pub space_protection: Arc<dyn SpaceProtectionStatusPort>,
    pub group_bootstrap: Arc<dyn uc_core::membership::GroupBootstrapPort>,
    pub own_device: DeviceId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkspaceConvergenceStateOrigin {
    CurrentInstallation,
    UpgradeWithoutConvergenceState,
}

impl WorkspaceConvergenceStateOrigin {
    pub fn from_version_transition(_previous: Option<&str>, _current: &str) -> Self {
        Self::CurrentInstallation
    }
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
    pub current_join: Option<CurrentJoinStatus>,
    pub pending_inbound_member: Option<PendingInboundMember>,
    pub devices: Vec<DeviceTrustRelationship>,
    pub recovery: RecoveryAvailability,
    pub allowed_actions: Vec<DeviceTrustAction>,
    pub blocked_reason: Option<ActionUnavailableReason>,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInboundMember {
    pub device_id: DeviceId,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinedSpace {
    pub sponsor_device_id: DeviceId,
    pub sponsor_identity_fingerprint: uc_core::security::IdentityFingerprint,
    pub space_id: String,
    pub self_device_id: DeviceId,
    pub self_identity_fingerprint: uc_core::security::IdentityFingerprint,
    pub migrated_records: Option<u64>,
    pub preserved_unreadable_records: Option<u64>,
}

pub struct PendingJoinerCompleteAck {
    pub sponsor_device_id: DeviceId,
    pub frame: uc_core::pairing::DurableAdmissionFrame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CurrentJoinStatus {
    Active {
        join_id: [u8; 16],
        joined_space: JoinedSpace,
    },
    Pending {
        join_id: [u8; 16],
        target_space_id: Option<String>,
        sponsor_device_id: Option<DeviceId>,
        sponsor_identity_fingerprint: Option<uc_core::security::IdentityFingerprint>,
        cancel_requested: bool,
    },
    Rejected {
        join_id: [u8; 16],
        reason: uc_core::membership::AdmissionRejectionReasonV1,
    },
}

/// The unified workspace convergence owner.
pub struct WorkspaceConvergence {
    deps: WorkspaceConvergenceDeps,
    admission: admission::DurableAdmissionTransaction,
    state_lock: tokio::sync::Mutex<()>,
    device_trust_decision_lock: tokio::sync::Mutex<()>,
    peer_reconciliation_locks: tokio::sync::Mutex<BTreeMap<DeviceId, Arc<tokio::sync::Mutex<()>>>>,
    wake: Arc<tokio::sync::Notify>,
    events: broadcast::Sender<WorkspaceSnapshot>,
}

pub struct ProfileWorkspaceConvergence {
    admission: admission::DurableAdmissionProjection,
    admission_attempts: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    own_device: DeviceId,
    clock: Arc<dyn ClockPort>,
    active: tokio::sync::RwLock<Option<Arc<WorkspaceConvergence>>>,
    active_event_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    events: broadcast::Sender<u64>,
}

#[async_trait]
pub trait SpaceTransitionRecoveryPort: Send + Sync {
    async fn requires_session_transition(&self) -> Result<bool, WorkspaceConvergenceError>;

    async fn recover_after_session_drain(&self) -> Result<usize, WorkspaceConvergenceError>;
}

#[derive(Clone, Copy)]
enum ReconciliationPeerRole {
    AuthenticatedSponsor,
    RuntimePeer,
    RestrictedDecisionDelivery,
}

impl WorkspaceConvergence {
    pub fn new(deps: WorkspaceConvergenceDeps) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        let admission = admission::DurableAdmissionTransaction::new(
            Arc::clone(&deps.admission_attempts),
            Arc::clone(&deps.historical_membership_signatures),
            Arc::clone(&deps.admission_security_transition),
            Arc::clone(&deps.admission_space_transition),
        );
        Arc::new(Self {
            deps,
            admission,
            state_lock: tokio::sync::Mutex::new(()),
            device_trust_decision_lock: tokio::sync::Mutex::new(()),
            peer_reconciliation_locks: tokio::sync::Mutex::new(BTreeMap::new()),
            wake: Arc::new(tokio::sync::Notify::new()),
            events,
        })
    }

    pub async fn current_join(
        &self,
    ) -> Result<Option<CurrentJoinStatus>, WorkspaceConvergenceError> {
        self.admission.current_local_join().await
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
        let mut state = match self.deps.repository.load_state().await? {
            Some(state) => state,
            None => {
                let mut state =
                    WorkspaceConvergenceState::fresh(lineage.clone(), self.deps.clock.now_ms());
                state.migrated_from_pre_adr_020 = matches!(
                    self.deps.initial_state_origin,
                    WorkspaceConvergenceStateOrigin::UpgradeWithoutConvergenceState
                );
                state
            }
        };
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

    fn publish(&self, state: &WorkspaceConvergenceState) {
        let _ = self.events.send(state.snapshot());
    }
}
