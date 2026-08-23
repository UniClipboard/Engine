//! Workspace membership owner.
//!
//! `WorkspaceMembership` is the single application-layer owner of membership
//! history, verified admission effects, removal, decisions, reconciliation,
//! current member scope, and published state. It saves every
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

pub mod discovery;
pub(crate) mod membership;
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

use crate::space::admission::durable as admission;
use crate::space::membership_state::{
    SpaceMembershipStateRepositoryError, SpaceMembershipStateRepositoryPort,
};
pub(crate) use crate::space::query_space_membership_status::{
    build_active_space_status, ActiveSpaceStatusFacts, ActiveSpaceStatusResult, DeviceMembership,
    SpaceMembershipChangeChoice, SpaceMembershipChangeDecisionResult,
};
#[cfg(test)]
pub(crate) use crate::space::query_space_membership_status::{
    ActionUnavailableReason, DeviceCompatibility, GroupRelationship, PendingSpaceMembershipChange,
    RecoveryAvailability, SpaceMemberRelationship, SpaceMembershipAction,
    SpaceMembershipChangeImpact, SpaceMembershipStatus, SyncRelationship,
};

use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignaturePort, CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort,
    MemberRepositoryPort, MembershipDecision, MembershipDecisionV2, MembershipEvent,
    MembershipEventId, MembershipEventV2, MembershipHistoryExchangeEndpointPort,
    MembershipHistoryExchangeError, MembershipHistoryExchangePort, MembershipHistoryMessage,
    MembershipHistoryRelationship, MembershipOperation, MembershipOperationV2,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort, RemovalDecision,
    SpaceMembershipState, SpaceProtectionStatusPort, WorkspaceConvergenceEvent,
    WorkspaceMergeOutcome, WorkspacePhase, WorkspaceSnapshot, MEMBERSHIP_DECISION_FORMAT_V2,
    MEMBERSHIP_EVENT_FORMAT_V2,
};
#[cfg(test)]
use uc_core::ports::ReachabilityState;
use uc_core::ports::{ClockPort, DeviceIdentityPort, PeerAddressRepositoryPort, PresencePort};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;

pub(crate) use runtime::WorkspaceMembershipActivity;
pub use runtime::WorkspaceMembershipRuntime;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceConvergenceError {
    #[error("workspace convergence state is locked")]
    Locked,
    #[error("workspace convergence state could not be persisted: {0}")]
    Repository(#[from] SpaceMembershipStateRepositoryError),
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

impl WorkspaceConvergenceError {
    pub(crate) fn is_locked(&self) -> bool {
        matches!(
            self,
            Self::Locked | Self::Repository(SpaceMembershipStateRepositoryError::Locked)
        )
    }

    pub(crate) fn is_corrupt(&self) -> bool {
        matches!(
            self,
            Self::Repository(SpaceMembershipStateRepositoryError::Corrupt)
        )
    }
}

pub struct WorkspaceMembershipDeps {
    pub repository: Arc<dyn SpaceMembershipStateRepositoryPort>,
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

/// The unified workspace convergence owner.
pub struct WorkspaceMembership {
    pub(in crate::space) deps: WorkspaceMembershipDeps,
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
    RestrictedDecisionDelivery,
}

impl WorkspaceMembership {
    pub fn new(deps: WorkspaceMembershipDeps) -> Arc<Self> {
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

    pub(in crate::space) fn admission_generation(state: &SpaceMembershipState) -> u64 {
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

    pub(in crate::space) fn admission_decision_for_state(
        state: &SpaceMembershipState,
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

    async fn load_state_with_presence(
        &self,
    ) -> Result<(SpaceMembershipState, bool), WorkspaceConvergenceError> {
        let lineage = self
            .deps
            .membership_identity
            .current_membership_identity()
            .await
            .map(|identity| identity.space_id.as_ref().to_owned())
            .unwrap_or_default();
        let persisted = self.deps.repository.load_state().await?;
        let was_persisted = persisted.is_some();
        let mut state = match persisted {
            Some(state) => state,
            None => SpaceMembershipState::fresh(lineage.clone(), self.deps.clock.now_ms()),
        };
        if state.space_lineage.is_empty() {
            state.space_lineage = lineage;
        }
        Ok((state, was_persisted))
    }

    async fn load_state(&self) -> Result<SpaceMembershipState, WorkspaceConvergenceError> {
        Ok(self.load_state_with_presence().await?.0)
    }

    async fn persist(&self, state: &SpaceMembershipState) -> Result<(), WorkspaceConvergenceError> {
        self.deps.repository.save_state(state).await?;
        Ok(())
    }

    fn publish(&self, state: &SpaceMembershipState) {
        let _ = self.events.send(state.snapshot());
    }
}
