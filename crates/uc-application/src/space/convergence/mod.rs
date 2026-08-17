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

pub(crate) mod admission_transaction;
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
    LegacyPeerProbePort, MemberRepositoryPort, MembershipDecision, MembershipDecisionV2,
    MembershipEvent, MembershipEventId, MembershipEventV2, MembershipHistoryExchangeEndpointPort,
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

pub(crate) use admission_transaction::DurableAdmissionTransaction;
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
    pub admission_space_transition: Arc<dyn uc_core::membership::AdmissionSpaceTransitionPort>,
    pub admission_outbox_delivery: Arc<dyn uc_core::membership::AdmissionOutboxDeliveryPort>,
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
    /// Positive-only legacy endpoint probe. It is used only after the current
    /// member-history endpoint failed to confirm the peer.
    pub legacy_peer_probe: Arc<dyn LegacyPeerProbePort>,
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
    pub fn from_version_transition(previous: Option<&str>, current: &str) -> Self {
        let Some(previous) = previous.and_then(|value| semver::Version::parse(value).ok()) else {
            return Self::CurrentInstallation;
        };
        let Ok(current) = semver::Version::parse(current) else {
            return Self::CurrentInstallation;
        };
        if previous < current {
            Self::UpgradeWithoutConvergenceState
        } else {
            Self::CurrentInstallation
        }
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
    admission: DurableAdmissionTransaction,
    state_lock: tokio::sync::Mutex<()>,
    device_trust_decision_lock: tokio::sync::Mutex<()>,
    peer_reconciliation_locks: tokio::sync::Mutex<BTreeMap<DeviceId, Arc<tokio::sync::Mutex<()>>>>,
    wake: Arc<tokio::sync::Notify>,
    events: broadcast::Sender<WorkspaceSnapshot>,
}

pub struct ProfileWorkspaceConvergence {
    admission: admission_transaction::DurableAdmissionProjection,
    admission_attempts: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    own_device: DeviceId,
    clock: Arc<dyn ClockPort>,
    active: tokio::sync::RwLock<Option<Arc<WorkspaceConvergence>>>,
    active_event_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    events: broadcast::Sender<u64>,
}

impl ProfileWorkspaceConvergence {
    pub fn new(
        admission_attempts: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
        own_device: DeviceId,
        clock: Arc<dyn ClockPort>,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            admission: admission_transaction::DurableAdmissionProjection::new(Arc::clone(
                &admission_attempts,
            )),
            admission_attempts,
            own_device,
            clock,
            active: tokio::sync::RwLock::new(None),
            active_event_task: tokio::sync::Mutex::new(None),
            events,
        })
    }

    pub async fn attach_active(self: &Arc<Self>, active: Option<Arc<WorkspaceConvergence>>) {
        *self.active.write().await = active.clone();
        if let Some(task) = self.active_event_task.lock().await.take() {
            task.abort();
        }
        if let Some(active) = active {
            let mut changes = active.subscribe();
            let events = self.events.clone();
            let admission_attempts = Arc::clone(&self.admission_attempts);
            *self.active_event_task.lock().await = Some(tokio::spawn(async move {
                while let Ok(snapshot) = changes.recv().await {
                    let revision = admission_attempts
                        .profile_metadata()
                        .await
                        .map(|metadata| metadata.device_trust_revision.max(snapshot.revision))
                        .unwrap_or(snapshot.revision);
                    let _ = events.send(revision);
                }
            }));
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<u64> {
        self.events.subscribe()
    }

    pub async fn current_join(
        &self,
    ) -> Result<Option<CurrentJoinStatus>, WorkspaceConvergenceError> {
        self.admission.current_local_join().await
    }

    pub async fn pending_joiner_complete_ack(
        &self,
    ) -> Result<Option<PendingJoinerCompleteAck>, WorkspaceConvergenceError> {
        use sha2::Digest as _;
        use uc_core::membership::{AdmissionIdentityBindingV1, AdmissionTerminalResultV1};

        let Some(projection) = self
            .admission_attempts
            .project_current_local_join()
            .await
            .map_err(admission_transaction::map_repository_error)?
        else {
            return Ok(None);
        };
        if projection.terminal_result != Some(AdmissionTerminalResultV1::Active) {
            return Ok(None);
        }
        let terminal = self
            .admission_attempts
            .load_terminal(projection.attempt_id)
            .await
            .map_err(admission_transaction::map_repository_error)?
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "active local join terminal is missing".to_owned(),
                )
            })?;
        let binding = AdmissionIdentityBindingV1::decode(
            terminal.identity_binding.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "active local join identity is missing".to_owned(),
                )
            })?,
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let completion_digest: [u8; 32] = sha2::Sha256::digest(&terminal.replay_result).into();
        let acknowledgment = terminal
            .acknowledgment_rebuild
            .iter()
            .find(|record| record.payload_digest == completion_digest)
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "active local join completion acknowledgment is missing".to_owned(),
                )
            })?;
        let payload = postcard::to_stdvec(acknowledgment)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        Ok(Some(PendingJoinerCompleteAck {
            sponsor_device_id: binding.sponsor_device_id,
            frame: complete_ack_frame(projection.attempt_id, acknowledgment.message_id, payload),
        }))
    }

    pub async fn cancel_join_space(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, WorkspaceConvergenceError> {
        let result = self.admission.cancel_local_join(join_id).await?;
        let revision = self
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission_transaction::map_repository_error)?
            .device_trust_revision;
        let _ = self.events.send(revision);
        Ok(result)
    }

    pub async fn prepare_reset_space(&self) -> Result<(), WorkspaceConvergenceError> {
        let metadata = self.admission.reset_join_projection_if_quiet().await?;
        let _ = self.events.send(metadata.device_trust_revision);
        Ok(())
    }

    pub async fn query_device_trust(
        &self,
    ) -> Result<DeviceTrustSnapshot, WorkspaceConvergenceError> {
        if let Some(active) = self.active.read().await.clone() {
            return match active.query_device_trust().await {
                Ok(snapshot) => Ok(snapshot),
                Err(WorkspaceConvergenceError::Locked)
                | Err(WorkspaceConvergenceError::Repository(
                    WorkspaceConvergenceRepositoryError::Locked,
                )) => Ok(self.unavailable_device_trust_snapshot()),
                Err(error) => Err(error),
            };
        }
        let metadata = self
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission_transaction::map_repository_error)?;
        Ok(DeviceTrustSnapshot {
            revision: metadata.device_trust_revision,
            local_device_id: self.own_device.clone(),
            local_membership: DeviceMembership::Unavailable,
            current_change: None,
            current_join: self.admission.current_local_join().await?,
            pending_inbound_member: None,
            devices: Vec::new(),
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions: Vec::new(),
            blocked_reason: None,
            updated_at_ms: self.clock.now_ms(),
        })
    }

    fn unavailable_device_trust_snapshot(&self) -> DeviceTrustSnapshot {
        DeviceTrustSnapshot {
            revision: 0,
            local_device_id: self.own_device.clone(),
            local_membership: DeviceMembership::Unavailable,
            current_change: None,
            current_join: None,
            pending_inbound_member: None,
            devices: Vec::new(),
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions: Vec::new(),
            blocked_reason: Some(ActionUnavailableReason::EngineUnavailable),
            updated_at_ms: self.clock.now_ms(),
        }
    }
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
        let admission = DurableAdmissionTransaction::new(
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

    pub(crate) async fn validate_join_request(
        &self,
        request: &uc_core::pairing::JoinerRequest,
    ) -> Result<(), WorkspaceConvergenceError> {
        request
            .validate_durable_identity()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_owned()))?;
        let verified = self
            .deps
            .historical_membership_signatures
            .verify(
                request.membership_credential.signature_algorithm_version,
                &request.membership_credential.public_key,
                &request.admission.signing_payload(),
                &request.admission.identity_signature,
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if !verified {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        Ok(())
    }

    pub(crate) async fn verified_admission_base_history(
        &self,
    ) -> Result<uc_core::membership::VersionedMembershipHistory, WorkspaceConvergenceError> {
        if let Some(encoded) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission_transaction::map_repository_error)?
        {
            let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                &encoded,
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            let state = self.load_state().await?;
            let own_instance = self
                .deps
                .member_signatures
                .current_member_instance(&self.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            if history.lineage_id() != state.space_lineage
                || !history.active_members().contains(&own_instance)
            {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            return Ok(history);
        }

        let state = self.load_state().await?;
        if state.removed {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        let own_instance = state
            .own_instance
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        let current_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        if current_instance != own_instance {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let remote_members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
            .into_iter()
            .any(|member| member.device_id != self.deps.own_device);
        let legacy = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        let head = legacy
            .applied_head()
            .filter(|head| legacy.known_head() == Some(*head))
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        let event = legacy
            .event(head)
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        if remote_members
            || legacy.known_event_count() != 1
            || legacy.effective_members() != [own_instance].into()
            || event.author_member_instance_id != own_instance
            || !matches!(
                &event.operation,
                uc_core::membership::MembershipOperation::AddDevice { admission }
                    if admission.member_instance == own_instance
                        && admission.device_id == self.deps.own_device
            )
        {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        if credential.member_instance_id(&self.deps.own_device) != own_instance
            || !self
                .deps
                .member_signatures
                .verify_current_member_payload(
                    &self.deps.own_device,
                    &event.signing_payload(),
                    &event.signature,
                )
                .await
                .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?
        {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let own_facts = self.local_admission_facts(Some(own_instance)).await?;
        uc_core::membership::VersionedMembershipHistory::from_activation_baseline(
            uc_core::membership::MembershipActivationBaselineV2::FullyVerifiedMigration {
                lineage_id: state.space_lineage,
                head_event_id: head,
                head_depth: event.parent_depth,
                current_members: vec![(own_facts, credential)],
            },
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))
    }

    pub(crate) async fn prepare_sponsor_candidate(
        &self,
        request: &uc_core::pairing::JoinerRequest,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionIdentityBindingV1, AdmissionOutboxPurposeV1,
            MembershipAdmissionV2, MembershipEventV2, MembershipOperationV2,
            SponsorAdmissionSecurityRecipient, SponsorAdmissionSecurityRequest,
            MEMBERSHIP_EVENT_FORMAT_V2,
        };

        let _guard = self.state_lock.lock().await;
        self.validate_join_request(request).await?;
        let attempt_id = AdmissionAttemptId::from_bytes(request.attempt_id);
        let invitation_digest = admission_invitation_digest(request.invitation_code.as_str());
        let stable_request_binding = crate::space::admission::adapter::stable_join_request_binding(
            &request.device_id,
            &request.identity_fingerprint,
        );
        let request_message = admission_transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::JoinRequest,
            request.invitation_code.as_str().as_bytes(),
            None,
            &stable_request_binding,
        );
        if request_message.message_id != request.request_message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }

        if let Some(existing) = self.admission.load(attempt_id).await? {
            if existing.invitation_claim.as_deref() != Some(invitation_digest.as_slice())
                || existing.candidate_key_package.as_deref() != Some(request.key_package.as_slice())
                || existing
                    .resume_public_key
                    .as_deref()
                    .is_some_and(|key| key != request.resume_public_key)
            {
                return Err(WorkspaceConvergenceError::AdmissionInProgress);
            }
            let candidate_message = existing
                .outboxes
                .iter()
                .find(|message| message.purpose == AdmissionOutboxPurposeV1::Candidate)
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "persisted sponsor admission has no candidate".to_owned(),
                    )
                })?;
            let payload = admission_transaction::DurableAdmissionCandidatePayloadV1::decode(
                &candidate_message.payload,
            )?;
            validate_candidate_request(&payload.candidate, request)?;
            return candidate_frame(attempt_id, candidate_message);
        }

        let base_history = self.verified_admission_base_history().await?;
        if base_history.active_members() != base_history.effective_members() {
            return Err(WorkspaceConvergenceError::AdmissionInProgress);
        }
        let base_position = base_history
            .current_position()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_instance = own_credential.member_instance_id(&self.deps.own_device);
        if !base_history.active_members().contains(&own_instance)
            || base_history.credential_for(own_instance) != Some(&own_credential)
        {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }

        let sponsor_facts = self.local_admission_facts(Some(own_instance)).await?;
        if sponsor_facts.device_id != self.deps.own_device {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let mut target_relationships = Vec::new();
        let mut existing_recipients = Vec::new();
        for member in base_history.active_members() {
            let credential = base_history
                .credential_for(member)
                .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
            let facts = if member == own_instance {
                sponsor_facts.clone()
            } else {
                base_history
                    .admission_facts_for(member)
                    .cloned()
                    .ok_or(WorkspaceConvergenceError::RecoveryRequired)?
            };
            if credential.member_instance_id(&facts.device_id) != member {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            if member != own_instance {
                existing_recipients.push(SponsorAdmissionSecurityRecipient {
                    device_id: facts.device_id.clone(),
                    credential_id: credential.credential_id,
                });
            }
            target_relationships.push(facts);
        }
        if target_relationships.iter().any(|facts| {
            facts.device_id == request.device_id || facts.member_instance == request.member_instance
        }) {
            return Err(WorkspaceConvergenceError::AdmissionConflict);
        }
        target_relationships.push(request.admission.clone());
        target_relationships.sort_by_key(|facts| facts.member_instance);

        let resume_public_key_digest =
            admission_resume_public_key_digest(&request.resume_public_key);
        let operation_id = admission_operation_id(attempt_id);
        let provisional_operation = MembershipOperationV2::AddDevice {
            admission: MembershipAdmissionV2 {
                facts: request.admission.clone(),
                membership_credential: request.membership_credential.clone(),
                resume_public_key_digest,
                security_commitment_id: [0; 32],
            },
        };
        let resulting_members_digest = base_history
            .expected_resulting_members_digest(base_position.event_id, &provisional_operation)
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let provisional_event = MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            base_history.lineage_id().to_owned(),
            base_position.event_id,
            base_position.depth.saturating_add(1),
            operation_id,
            own_instance,
            own_credential.credential_id,
            own_credential.signature_algorithm_version,
            provisional_operation,
            resulting_members_digest,
            [0; 32],
            Vec::new(),
            None,
            Vec::new(),
        );
        let candidate_core_digest = provisional_event
            .admission_candidate_core_digest(request.attempt_id, &request.key_package)
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let prepared_security = self
            .deps
            .prepare_sponsor_admission_security
            .prepare_sponsor_admission_security(SponsorAdmissionSecurityRequest {
                space_id: uc_core::ids::SpaceId::from_string(base_history.lineage_id().to_owned()),
                attempt_id: request.attempt_id,
                base_history_position: base_position.clone(),
                candidate_core_digest,
                candidate_identity: request.device_id.as_str().as_bytes().to_vec(),
                candidate_key_package: request.key_package.clone(),
                existing_recipients,
            })
            .await
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if prepared_security.public_commitment.attempt_id != request.attempt_id
            || prepared_security.public_commitment.lineage_id != base_history.lineage_id()
            || prepared_security.public_commitment.base_history_position != base_position
            || prepared_security.public_commitment.candidate_core_digest != candidate_core_digest
        {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "prepared sponsor security result does not match the candidate".to_owned(),
            ));
        }
        let security_update_payload =
            common_existing_member_delivery_payload(&prepared_security.existing_member_deliveries)?;
        let operation = MembershipOperationV2::AddDevice {
            admission: MembershipAdmissionV2 {
                facts: request.admission.clone(),
                membership_credential: request.membership_credential.clone(),
                resume_public_key_digest,
                security_commitment_id: prepared_security.public_commitment.security_commitment_id,
            },
        };
        let mut candidate_event = MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            base_history.lineage_id().to_owned(),
            base_position.event_id,
            base_position.depth.saturating_add(1),
            operation_id,
            own_instance,
            own_credential.credential_id,
            own_credential.signature_algorithm_version,
            operation,
            resulting_members_digest,
            prepared_security.public_commitment.group_context_digest,
            security_update_payload,
            Some(prepared_security.public_commitment.admission_bundle_digest),
            Vec::new(),
        );
        if candidate_event
            .admission_candidate_core_digest(request.attempt_id, &request.key_package)
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
            != candidate_core_digest
        {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "candidate core changed after security preparation".to_owned(),
            ));
        }
        candidate_event.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&candidate_event.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let candidate_event_id = candidate_event.event_id();
        let identity_binding = AdmissionIdentityBindingV1::new(
            base_history.lineage_id().to_owned(),
            candidate_event_id,
            &sponsor_facts,
            &request.admission,
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        .encode()
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let encoded_base_history = base_history
            .encode_persisted_v2()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let candidate = admission_transaction::DurableAdmissionCandidateV1 {
            lineage_id: base_history.lineage_id().to_owned(),
            base_history_position: postcard::to_stdvec(&base_position)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?,
            candidate_event: postcard::to_stdvec(&candidate_event)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?,
            candidate_event_id: *candidate_event_id.as_bytes(),
            candidate_key_package: request.key_package.clone(),
            resume_public_key: request.resume_public_key.clone(),
            target_members_digest: resulting_members_digest,
            security_commitment: postcard::to_stdvec(&prepared_security.public_commitment)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?,
            security_commit: prepared_security.commit,
            security_welcome: prepared_security.welcome,
            target_protection_group_id: prepared_security.target_protection_group_id,
            target_key_catalog: prepared_security
                .target_key_catalog
                .encode()
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?,
            target_relationships,
            existing_member_deliveries: prepared_security.existing_member_deliveries,
            staged_security_state: prepared_security.staged_state,
            identity_binding,
        };
        let payload = admission_transaction::DurableAdmissionCandidatePayloadV1::new(
            encoded_base_history,
            candidate.clone(),
        )
        .encode()?;
        let candidate_message = self
            .admission
            .sponsor_accept_and_offer(
                attempt_id,
                invitation_digest,
                &request_message,
                candidate,
                base_history,
                &candidate_event,
                &prepared_security.public_commitment,
                request.device_id.as_str().as_bytes(),
                &payload,
            )
            .await?;
        candidate_frame(attempt_id, &candidate_message)
    }

    pub(crate) async fn prepare_joiner_candidate(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        proof_signer: &(dyn uc_core::ports::space::GroupAdmissionPort + Send + Sync),
        target_access: &(dyn uc_core::ports::space::PrepareAdmissionTargetAccessPort + Send + Sync),
        passphrase: &uc_core::crypto::domain::Passphrase,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1,
            MembershipEventV2, MembershipOperationV2, VersionedMembershipHistory,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Candidate {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let payload =
            admission_transaction::DurableAdmissionCandidatePayloadV1::decode(&frame.payload)?;
        let candidate_message = admission_transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Candidate,
            self.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if candidate_message.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let base_history = VersionedMembershipHistory::decode_persisted_v2(
            &payload.base_membership_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let candidate_event: MembershipEventV2 =
            postcard::from_bytes(&payload.candidate.candidate_event)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let sponsor_commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(&payload.candidate.security_commitment)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let computed_core = candidate_event
            .admission_candidate_core_digest(
                frame.attempt_id,
                &payload.candidate.candidate_key_package,
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if computed_core != sponsor_commitment.candidate_core_digest {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let target_access_state = target_access
            .prepare_target_access(
                &uc_core::ids::SpaceId::from_string(payload.candidate.lineage_id.clone()),
                passphrase,
            )
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
            .into_bytes();
        let sponsor_device_id = payload
            .candidate
            .target_relationships
            .iter()
            .find(|facts| facts.member_instance == candidate_event.author_member_instance_id)
            .map(|facts| facts.device_id.clone())
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        if admission.facts.device_id != self.deps.own_device {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let prepared = self
            .admission
            .joiner_verify_and_prepare(
                attempt_id,
                &candidate_message,
                payload.candidate,
                base_history,
                &candidate_event,
                &sponsor_commitment,
                &target_access_state,
                &[],
                Some(proof_signer),
                sponsor_device_id.as_str().as_bytes(),
                &[],
            )
            .await?;
        durable_frame_from_outbox(
            attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Prepared,
            AdmissionOutboxPurposeV1::Prepared,
            &prepared,
        )
    }

    pub(crate) async fn commit_sponsor_prepared(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1,
            MembershipEventV2, MembershipOperationV2, PreparedAdmissionProofV1,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Prepared {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let attempt = self
            .admission
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let prepared = admission_transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Prepared,
            self.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if prepared.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let proof: PreparedAdmissionProofV1 = postcard::from_bytes(&frame.payload)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let candidate_event: MembershipEventV2 =
            postcard::from_bytes(attempt.candidate_event.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("candidate event is missing".to_owned())
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(attempt.security_commitment.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "candidate security commitment is missing".to_owned(),
                )
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        if proof.proof_format_version != uc_core::membership::PREPARED_ADMISSION_PROOF_FORMAT_V1
            || proof.attempt_id != frame.attempt_id
            || proof.lineage_id != candidate_event.lineage_id
            || proof.base_history_position != commitment.base_history_position
            || proof.candidate_event_id != candidate_event.event_id()
            || proof.target_members_digest != candidate_event.resulting_members_digest
            || proof.security_commitment_id != commitment.security_commitment_id
            || proof.joiner_member_instance_id != admission.facts.member_instance
            || proof.joiner_credential_id != admission.membership_credential.credential_id
            || !self
                .deps
                .historical_membership_signatures
                .verify(
                    admission.membership_credential.signature_algorithm_version,
                    &admission.membership_credential.public_key,
                    &proof.signing_payload(),
                    &proof.signature,
                )
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let commit_payload = admission_transaction::DurableAdmissionCommitPayloadV1 {
            format_version: admission_transaction::DurableAdmissionCommitPayloadV1::FORMAT_V1,
            candidate_event_id: *candidate_event.event_id().as_bytes(),
            security_commitment_id: commitment.security_commitment_id,
            prepared_proof: frame.payload.clone(),
            resume_public_key: attempt.resume_public_key.clone().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("resume public key is missing".to_owned())
            })?,
            existing_member_deliveries: attempt
                .existing_member_security_deliveries
                .clone()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "existing-member security deliveries are missing".to_owned(),
                    )
                })?,
        }
        .encode()?;
        let commit = self
            .admission
            .sponsor_commit(
                attempt_id,
                &prepared,
                &frame.payload,
                admission.facts.device_id.as_str().as_bytes(),
                &commit_payload,
            )
            .await?;
        durable_frame_from_outbox(
            attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Commit,
            AdmissionOutboxPurposeV1::Commit,
            &commit,
        )
    }

    pub(crate) async fn apply_joiner_commit(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        receipt_signer: &(dyn uc_core::ports::space::GroupAdmissionPort + Send + Sync),
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionActivationReceipt, AdmissionAttemptId, AdmissionOutboxPurposeV1,
            AdmissionSecurityCommitmentV1, AdmissionSecurityTransitionInput, MembershipEventV2,
            MembershipOperationV2,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Commit {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let attempt = self
            .admission
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let commit = admission_transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Commit,
            self.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if commit.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let commit_payload =
            admission_transaction::DurableAdmissionCommitPayloadV1::decode(&frame.payload)?;
        let candidate_event: MembershipEventV2 =
            postcard::from_bytes(attempt.candidate_event.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("candidate event is missing".to_owned())
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(attempt.security_commitment.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "candidate security commitment is missing".to_owned(),
                )
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        if commit_payload.candidate_event_id != *candidate_event.event_id().as_bytes()
            || commit_payload.security_commitment_id != commitment.security_commitment_id
            || attempt.prepared_proof.as_deref() != Some(commit_payload.prepared_proof.as_slice())
            || attempt.resume_public_key.as_deref()
                != Some(commit_payload.resume_public_key.as_slice())
            || attempt.existing_member_security_deliveries.as_deref()
                != Some(commit_payload.existing_member_deliveries.as_slice())
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let transition_input = AdmissionSecurityTransitionInput {
            attempt_id: frame.attempt_id,
            base_history_position: commitment.base_history_position.clone(),
            candidate_core_digest: commitment.candidate_core_digest,
            key_catalog_digest: commitment.key_catalog_digest,
            admission_bundle_digest: commitment.admission_bundle_digest,
        };
        let rederived = self
            .deps
            .admission_security_transition
            .derive_public_commitment(
                attempt.staged_security_state.as_deref().ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "joiner staged security state is missing".to_owned(),
                    )
                })?,
                attempt.security_commit.as_deref().ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent("security commit is missing".to_owned())
                })?,
                &transition_input,
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if rederived != commitment {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        let mut receipt = AdmissionActivationReceipt::new(
            1,
            frame.attempt_id,
            candidate_event.event_id(),
            candidate_event.resulting_members_digest,
            commitment.security_commitment_id,
            admission.facts.member_instance,
            Vec::new(),
        );
        let prepared_join = uc_core::space_access::PreparedGroupJoin::new(
            attempt.candidate_key_package.clone().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "candidate key package is missing".to_owned(),
                )
            })?,
            attempt
                .joiner_pending_security_state
                .clone()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "joiner signing state is missing".to_owned(),
                    )
                })?,
        )
        .with_member_instance(admission.facts.member_instance);
        receipt.signature = receipt_signer
            .sign_prepared_join_payload(&prepared_join, &receipt.signing_payload())
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let applied_payload = postcard::to_stdvec(&receipt)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let sponsor_device_id = attempt
            .target_relationships
            .as_deref()
            .and_then(|relationships| {
                relationships.iter().find(|facts| {
                    facts.member_instance == candidate_event.author_member_instance_id
                })
            })
            .map(|facts| facts.device_id.clone())
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let applied = self
            .admission
            .joiner_apply(
                attempt_id,
                &commit,
                &receipt,
                sponsor_device_id.as_str().as_bytes(),
                &applied_payload,
            )
            .await?;
        durable_frame_from_outbox(
            attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Applied,
            AdmissionOutboxPurposeV1::Applied,
            &applied,
        )
    }

    pub(crate) async fn complete_sponsor_applied(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionActivationReceipt, AdmissionAttemptId, AdmissionCompletionV1,
            AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1, MembershipEventV2,
            MembershipOperationV2, VersionedMembershipHistory,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Applied {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let attempt = self
            .admission
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let applied = admission_transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Applied,
            self.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if applied.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let receipt: AdmissionActivationReceipt = postcard::from_bytes(&frame.payload)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let candidate_event: MembershipEventV2 =
            postcard::from_bytes(attempt.candidate_event.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("candidate event is missing".to_owned())
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(attempt.security_commitment.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "candidate security commitment is missing".to_owned(),
                )
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let mut completed_history = VersionedMembershipHistory::decode_persisted_v2(
            &self
                .deps
                .admission_attempts
                .load_membership_history_v2()
                .await
                .map_err(admission_transaction::map_repository_error)?
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "committed membership history is missing".to_owned(),
                    )
                })?,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        completed_history
            .verify_and_record_activation_receipt(
                receipt.clone(),
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
        self.admission
            .sponsor_prepare_security_activation(attempt_id, &receipt)
            .await?;
        self.deps
            .activate_sponsor_admission_security
            .activate_sponsor_admission_security(
                uc_core::membership::ActivateSponsorAdmissionSecurityRequest {
                    space_id: uc_core::ids::SpaceId::from_string(
                        candidate_event.lineage_id.clone(),
                    ),
                    staged_state: attempt.staged_security_state.clone().ok_or_else(|| {
                        WorkspaceConvergenceError::Inconsistent(
                            "sponsor staged security state is missing".to_owned(),
                        )
                    })?,
                    commit: attempt.security_commit.clone().ok_or_else(|| {
                        WorkspaceConvergenceError::Inconsistent(
                            "sponsor security commit is missing".to_owned(),
                        )
                    })?,
                    expected_commitment: commitment.clone(),
                },
            )
            .await
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let completed_position = completed_history
            .current_position()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_instance = own_credential.member_instance_id(&self.deps.own_device);
        if !completed_history.active_members().contains(&own_instance) {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        let receipt_bytes = postcard::to_stdvec(&receipt)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let mut completion = AdmissionCompletionV1::new(
            frame.attempt_id,
            candidate_event.event_id(),
            sha2::Sha256::digest(&receipt_bytes).into(),
            commitment.security_commitment_id,
            own_instance,
            own_credential.credential_id,
            completed_position,
            Vec::new(),
        );
        completion.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&completion.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let completion_bytes = postcard::to_stdvec(&completion)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        // Completion is not externally visible until the sponsor has also
        // installed the admitted member's roster, trust and address facts.
        // This is idempotent, so recovery can repeat it after any later save
        // or send failure without creating a second relationship.
        self.save_member_facts(&admission.facts, self.deps.clock.now_ms())
            .await?;
        let complete = self
            .admission
            .sponsor_complete(
                attempt_id,
                &applied,
                &receipt,
                &completion_bytes,
                admission.facts.device_id.as_str().as_bytes(),
                &completion_bytes,
            )
            .await?;
        durable_frame_from_outbox(
            attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Complete,
            AdmissionOutboxPurposeV1::Complete,
            &complete,
        )
    }

    pub(crate) async fn activate_joiner_complete(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<crate::space::admission::adapter::DurableJoinerCompletion, WorkspaceConvergenceError>
    {
        use uc_core::membership::{
            AdmissionActivationReceipt, AdmissionAttemptId, AdmissionCompletionV1,
            AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1, MembershipEventV2,
            VersionedMembershipHistory,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Complete {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let attempt = self
            .admission
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let complete = admission_transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Complete,
            self.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if complete.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let completion: AdmissionCompletionV1 = postcard::from_bytes(&frame.payload)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let candidate_event: MembershipEventV2 =
            postcard::from_bytes(attempt.candidate_event.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("candidate event is missing".to_owned())
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(attempt.security_commitment.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "candidate security commitment is missing".to_owned(),
                )
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let receipt_bytes = attempt.activation_receipt.as_deref().ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent("activation receipt is missing".to_owned())
        })?;
        let _: AdmissionActivationReceipt = postcard::from_bytes(receipt_bytes)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let history = VersionedMembershipHistory::decode_persisted_v2(
            attempt
                .verified_membership_history
                .as_deref()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "verified membership history is missing".to_owned(),
                    )
                })?,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let completer_credential = history
            .credential_for(completion.completed_by_member_instance_id)
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let receipt_digest: [u8; 32] = sha2::Sha256::digest(receipt_bytes).into();
        if completion.completion_format_version
            != uc_core::membership::ADMISSION_COMPLETION_FORMAT_V1
            || completion.attempt_id != frame.attempt_id
            || completion.event_id != candidate_event.event_id()
            || completion.activation_receipt_digest != receipt_digest
            || completion.security_commitment_id != commitment.security_commitment_id
            || completion.completed_by_credential_id != completer_credential.credential_id
            || completion.completed_history_position
                != history
                    .current_position()
                    .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
            || !history
                .active_members()
                .contains(&completion.completed_by_member_instance_id)
            || !self
                .deps
                .historical_membership_signatures
                .verify(
                    completer_credential.signature_algorithm_version,
                    &completer_credential.public_key,
                    &completion.signing_payload(),
                    &completion.signature,
                )
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let acknowledgment = match self
            .admission
            .joiner_activate(attempt_id, &complete, &frame.payload)
            .await?
        {
            admission_transaction::JoinerActivationOutcomeV1::Active(acknowledgment) => {
                self.admission.compact_if_settled(attempt_id).await?;
                acknowledgment
            }
            admission_transaction::JoinerActivationOutcomeV1::SpaceTransitionRequired => {
                return Ok(crate::space::admission::adapter::DurableJoinerCompletion::SpaceTransitionRequired);
            }
        };
        let payload = postcard::to_stdvec(&acknowledgment)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        Ok(
            crate::space::admission::adapter::DurableJoinerCompletion::Active(complete_ack_frame(
                attempt_id,
                frame.message_id,
                payload,
            )),
        )
    }

    pub(crate) async fn confirm_sponsor_complete_ack(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::CompleteAck {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes(frame.attempt_id);
        if complete_ack_frame(
            attempt_id,
            frame.predecessor_message_id.unwrap_or([0; 32]),
            frame.payload.clone(),
        )
        .message_id
            != frame.message_id
            || frame.predecessor_message_id.is_none()
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let acknowledgment: uc_core::membership::AdmissionInboxRecordV1 =
            postcard::from_bytes(&frame.payload)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        self.admission
            .sponsor_confirm_active(attempt_id, &acknowledgment)
            .await
    }

    pub async fn cancel_join_space(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, WorkspaceConvergenceError> {
        self.admission.cancel_local_join(join_id).await
    }

    pub(crate) async fn preflight_local_join_source(
        &self,
        preserve_unreadable_history: bool,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.admission
            .preflight_join_source(preserve_unreadable_history)
            .await
    }

    pub(crate) async fn prepare_local_join_before_network(
        &self,
        preparation: &(dyn uc_core::ports::space::GroupAdmissionPort + Send + Sync),
        local_device_id: &DeviceId,
        sponsor: &[u8],
        stable_request_binding: &[u8],
        preserve_unreadable_history: bool,
    ) -> Result<
        crate::space::admission::adapter::DurableLocalJoinPreparation,
        WorkspaceConvergenceError,
    > {
        let _guard = self.state_lock.lock().await;
        let start = self
            .admission
            .prepare_join_before_network(
                preparation,
                local_device_id,
                sponsor,
                stable_request_binding,
                preserve_unreadable_history,
            )
            .await?;
        let join_id = start.attempt.join_id.ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent("local join id is missing".into())
        })?;
        Ok(
            crate::space::admission::adapter::DurableLocalJoinPreparation {
                attempt_id: *start.attempt.attempt_id.as_bytes(),
                join_id,
                request_message_id: start.request_message_id()?,
                resume_public_key: self
                    .admission
                    .load_join_recovery_material(start.attempt.attempt_id)
                    .await?
                    .resume_public_key,
                prepared_group_join: start.prepared_group_join,
            },
        )
    }

    pub(crate) async fn reject_local_join_before_candidate(
        &self,
        attempt_id: [u8; 32],
        reason: uc_core::membership::AdmissionRejectionReasonV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        self.admission
            .joiner_reject_before_candidate(
                uc_core::membership::AdmissionAttemptId::from_bytes(attempt_id),
                reason,
            )
            .await
    }

    async fn recover_pending_admissions(&self) -> Result<usize, WorkspaceConvergenceError> {
        self.deps
            .legacy_migration_recovery
            .recover()
            .await
            .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        let recoverable = self.admission.recoverable().await?;
        for attempt in recoverable {
            if !matches!(
                attempt.role_state,
                uc_core::membership::AdmissionAttemptRoleStateV1::Sponsor(
                    uc_core::membership::SponsorAdmissionStateV1 {
                        stage: uc_core::membership::SponsorAdmissionStageV1::Committed,
                    },
                )
            ) {
                continue;
            }
            let Some(receipt_payload) = attempt.write_ahead_recovery.clone() else {
                continue;
            };
            let commit_id = attempt
                .outboxes
                .iter()
                .find(|message| {
                    message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::Commit
                })
                .map(|message| message.message_id)
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "recoverable sponsor activation has no Commit".to_owned(),
                    )
                })?;
            let applied = admission_transaction::durable_admission_message(
                attempt.attempt_id,
                uc_core::membership::AdmissionOutboxPurposeV1::Applied,
                self.deps.own_device.as_str().as_bytes(),
                Some(commit_id),
                &receipt_payload,
            );
            let frame = uc_core::pairing::DurableAdmissionFrame {
                attempt_id: *attempt.attempt_id.as_bytes(),
                kind: uc_core::pairing::DurableAdmissionMessageKind::Applied,
                message_id: applied.message_id,
                predecessor_message_id: applied.predecessor_message_id,
                payload: receipt_payload,
            };
            self.complete_sponsor_applied(&frame).await?;
        }
        let report = self
            .admission
            .recover_with(self.deps.admission_outbox_delivery.as_ref())
            .await?;
        if report.deliveries_confirmed > 0 || report.attempts_compacted > 0 {
            self.notify();
        }
        Ok(report.deliveries_attempted)
    }

    pub async fn requires_session_transition(&self) -> Result<bool, WorkspaceConvergenceError> {
        self.admission.requires_session_transition().await
    }

    pub async fn recover_space_transition_after_session_drain(
        &self,
    ) -> Result<usize, WorkspaceConvergenceError> {
        let finished = self
            .admission
            .recover_space_transitions_after_session_drain()
            .await?;
        if finished > 0 {
            self.notify();
        }
        Ok(finished)
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

    pub(crate) async fn recover_legacy_migration_marker(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let mut state = self.load_state().await?;
        if Self::clear_legacy_migration_marker_if_current_history_exists(&mut state) {
            self.persist(&state).await?;
        }
        Ok(())
    }

    pub(crate) async fn deliver_pending_membership_decisions(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.deliver_persisted_v2_removal_decisions().await
    }

    async fn deliver_persisted_v2_removal_decisions(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let Some(encoded) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission_transaction::map_repository_error)?
        else {
            return Ok(());
        };
        let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &encoded,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let own = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let recipients = history.removal_decision_recipients_for(own);
        if recipients.is_empty() {
            return Ok(());
        }
        let mut candidate_devices = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
            .into_iter()
            .map(|member| member.device_id)
            .collect::<Vec<_>>();
        candidate_devices.push(self.deps.own_device.clone());
        for recipient_member in recipients {
            let Some(recipient) = history.device_for_member(&recipient_member, &candidate_devices)
            else {
                continue;
            };
            if recipient == self.deps.own_device {
                continue;
            }
            if let Err(error) = self
                .reconcile_membership_history_serialized(
                    &recipient,
                    ReconciliationPeerRole::RestrictedDecisionDelivery,
                )
                .await
            {
                tracing::debug!(
                    recipient = %recipient.as_str(),
                    error = %error,
                    "restricted membership decision delivery deferred"
                );
            }
        }
        Ok(())
    }

    fn publish(&self, state: &WorkspaceConvergenceState) {
        let _ = self.events.send(state.snapshot());
    }

    /// Whether the local device may currently drive content sends.
    pub async fn locally_removed(&self, device_id: &DeviceId) -> bool {
        let scope = match uc_core::membership::CurrentWorkspacePeerScopePort::snapshot(self).await {
            Ok(scope) => scope,
            Err(error) => {
                tracing::warn!(
                    peer = %device_id.as_str(),
                    error = ?error,
                    "content exchange denied because current member scope is unavailable"
                );
                return true;
            }
        };
        if !scope.peer_device_ids.contains(device_id) {
            tracing::warn!(
                peer = %device_id.as_str(),
                source = ?scope.source,
                "content exchange denied because peer is outside current member scope"
            );
            return true;
        }
        match self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
        {
            Ok(Some(_)) => {
                return self
                    .load_state()
                    .await
                    .map_or(true, |state| !state.allows_normal_exchange(device_id));
            }
            Ok(None) => {}
            Err(_) => return true,
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
        let roster = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let mut candidate_devices = roster
            .iter()
            .map(|member| member.device_id.clone())
            .collect::<Vec<_>>();
        candidate_devices.push(local_device_id.clone());
        candidate_devices.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        candidate_devices.dedup();
        let v2_history = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission_transaction::map_repository_error)?
            .map(|encoded| {
                uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                    &encoded,
                    self.deps.historical_membership_signatures.as_ref(),
                )
            })
            .transpose()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let v2_own_instance = if v2_history.is_some() {
            Some(
                self.deps
                    .member_signatures
                    .current_member_instance(&local_device_id)
                    .await
                    .map_err(|_| WorkspaceConvergenceError::Unavailable)?,
            )
        } else {
            None
        };
        let local_membership =
            if let (Some(history), Some(own_instance)) = (v2_history.as_ref(), v2_own_instance) {
                if history.active_members().contains(&own_instance) {
                    DeviceMembership::Active
                } else {
                    DeviceMembership::Removed
                }
            } else if state.own_instance.is_none() {
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
        let pending_facts = if workspace_unverifiable {
            None
        } else if let (Some(history), Some(own_instance)) = (v2_history.as_ref(), v2_own_instance) {
            history
                .pending_removal_decision(own_instance)
                .and_then(|removal_event_id| {
                    let event = history.event(removal_event_id)?;
                    let MembershipOperationV2::RemoveDevice { member } = event.operation else {
                        return None;
                    };
                    let proposed_by_device_id = history
                        .device_for_member(&event.author_member_instance_id, &candidate_devices)?;
                    let target_device_id =
                        history.device_for_member(&member, &candidate_devices)?;
                    Some(uc_core::membership::PendingRemovalFacts::new(
                        removal_event_id,
                        proposed_by_device_id,
                        vec![target_device_id],
                        [member].into(),
                    ))
                })
        } else {
            history.and_then(|history| history.pending_removal_facts())
        };
        let includes_local_device = pending_facts.as_ref().is_some_and(|facts| {
            v2_own_instance
                .or(state.own_instance)
                .is_some_and(|member| facts.includes_member(member))
        });

        let mut names = BTreeMap::new();
        if let Some(history) = history {
            for admission in history.admitted_device_facts() {
                names.insert(admission.device_id, admission.device_name);
            }
        }
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
            } else if v2_history.as_ref().is_some_and(|history| {
                history.effective_members().iter().any(|member| {
                    history
                        .device_for_member(member, &candidate_devices)
                        .as_ref()
                        == Some(&device_id)
                })
            }) || history.is_some_and(|history| history.is_device_effective(&device_id))
            {
                DeviceMembership::Active
            } else if v2_history
                .as_ref()
                .is_some_and(|history| history.has_admitted_device(&device_id, &candidate_devices))
                || history.is_some_and(|history| history.has_admitted_device(&device_id))
            {
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
        let current_join = self.admission.current_local_join().await?;
        let pending_inbound_member = self
            .admission
            .pending_inbound_member(&state.space_lineage)
            .await?;
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
        let admission_revision = self
            .deps
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission_transaction::map_repository_error)?
            .device_trust_revision;
        Ok(DeviceTrustSnapshot {
            revision: state.revision.max(admission_revision),
            local_device_id,
            local_membership,
            current_change,
            current_join,
            pending_inbound_member,
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
        let mut snapshot = state.snapshot();
        let Some(scope) = self
            .v2_current_peer_snapshot(&state)
            .await
            .map_err(|error| {
                WorkspaceConvergenceError::Inconsistent(format!(
                    "current V2 member scope is unavailable: {error:?}"
                ))
            })?
        else {
            return Ok(snapshot);
        };
        let encoded_history = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission_transaction::map_repository_error)?
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "current V2 member history disappeared during query".to_owned(),
                )
            })?;
        let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &encoded_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| {
            WorkspaceConvergenceError::Inconsistent(format!(
                "current V2 member history is invalid: {error}"
            ))
        })?;
        let position = history.current_position().map_err(|error| {
            WorkspaceConvergenceError::Inconsistent(format!(
                "current V2 member history position is invalid: {error}"
            ))
        })?;
        let metadata = self
            .deps
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission_transaction::map_repository_error)?;
        snapshot.revision = snapshot.revision.max(metadata.device_trust_revision);
        snapshot.history_event_count =
            usize::try_from(position.depth.saturating_add(1)).unwrap_or(usize::MAX);
        snapshot.effective_member_count = history.active_members().len();
        snapshot.convergence_digest = Some(uc_core::membership::WorkspaceDigest::from_bytes(
            position.history_digest,
        ));
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        snapshot.pending_removal_decision_event_id = history.pending_removal_decision(own_instance);
        snapshot.removed =
            scope.local_membership == uc_core::membership::CurrentWorkspaceLocalMembership::Removed;
        Ok(snapshot)
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
        let response = match message {
            MembershipHistoryMessage::HistoryPageV2(page) => {
                self.receive_membership_history_v2(&mut state, source_device_id, page, now_ms)
                    .await?
            }
            MembershipHistoryMessage::AckV2(ack) => MembershipHistoryMessage::AckV2(ack),
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
        match self
            .reconcile_membership_history_serialized(peer, ReconciliationPeerRole::RuntimePeer)
            .await
        {
            Ok(()) => Ok(()),
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

    async fn reconcile_membership_history_serialized(
        &self,
        peer: &DeviceId,
        peer_role: ReconciliationPeerRole,
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
        self.reconcile_membership_history(peer, peer_role).await
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

    async fn record_current_peer_confirmation(
        &self,
        peer: &DeviceId,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if matches!(
            state.peer_history_relationships.get(peer),
            None | Some(MembershipHistoryRelationship::Unknown)
                | Some(MembershipHistoryRelationship::UpgradeRequired)
        ) {
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
        let pages = {
            let _guard = self.state_lock.lock().await;
            let state = self.load_state().await?;
            let restricted_decision_delivery = matches!(
                peer_role,
                ReconciliationPeerRole::RestrictedDecisionDelivery
            );
            if !restricted_decision_delivery
                && (state.removed
                    || matches!(
                        state.peer_history_relationships.get(peer),
                        Some(
                            MembershipHistoryRelationship::PendingRemovalDecision
                                | MembershipHistoryRelationship::Diverged
                                | MembershipHistoryRelationship::Invalid
                        )
                    ))
            {
                return Ok(());
            }
            let Some(encoded) = self
                .deps
                .admission_attempts
                .load_membership_history_v2()
                .await
                .map_err(admission_transaction::map_repository_error)?
            else {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            };
            let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                &encoded,
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            if history.lineage_id() != state.space_lineage {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            let own_admission = self
                .local_admission_facts(Some(
                    self.deps
                        .member_signatures
                        .current_member_instance(&self.deps.own_device)
                        .await
                        .map_err(|_| WorkspaceConvergenceError::Unavailable)?,
                ))
                .await?;
            history
                .export_reconciliation_pages_v2(own_admission)
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        };
        let transfer_id = pages
            .first()
            .map(|page| page.transfer_id())
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "membership history page export was empty".to_owned(),
                )
            })?;
        let mut next_page_index = 0u32;
        for _ in 0..=pages.len() {
            let page = pages
                .get(next_page_index as usize)
                .cloned()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "membership history requested an invalid page".to_owned(),
                    )
                })?;
            let reply = self
                .deps
                .membership_history_exchange
                .exchange_membership_history(peer, MembershipHistoryMessage::HistoryPageV2(page))
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
            let MembershipHistoryMessage::AckV2(ack) = reply else {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "membership history exchange returned an invalid response".to_owned(),
                ));
            };
            match ack {
                uc_core::membership::MembershipHistoryV2Ack::Continue {
                    transfer_id: acknowledged_transfer,
                    next_page_index: requested_page,
                } if acknowledged_transfer == transfer_id
                    && (requested_page as usize) < pages.len() =>
                {
                    next_page_index = requested_page;
                }
                uc_core::membership::MembershipHistoryV2Ack::Consistent
                | uc_core::membership::MembershipHistoryV2Ack::UpdatesApplied
                    if next_page_index as usize + 1 == pages.len() =>
                {
                    self.record_current_peer_confirmation(peer).await?;
                    return Ok(());
                }
                uc_core::membership::MembershipHistoryV2Ack::Diverged
                | uc_core::membership::MembershipHistoryV2Ack::Invalid => return Ok(()),
                _ => {
                    return Err(WorkspaceConvergenceError::Inconsistent(
                        "membership history acknowledgement is inconsistent".to_owned(),
                    ))
                }
            }
        }
        Err(WorkspaceConvergenceError::Inconsistent(
            "membership history paging did not advance".to_owned(),
        ))
    }

    async fn receive_membership_history_v2(
        &self,
        state: &mut WorkspaceConvergenceState,
        source_device_id: &DeviceId,
        page: uc_core::membership::MembershipHistoryPageV2,
        now_ms: i64,
    ) -> Result<MembershipHistoryMessage, WorkspaceConvergenceError> {
        use uc_core::membership::{
            MembershipHistoryV2Ack, PendingMembershipHistoryTransferV2, VersionedMembershipHistory,
        };

        let transfer_id = page.transfer_id();
        let page_count = page.page_count();
        let page_index = page.page_index();
        if page.validate_envelope().is_err() {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let pending = state
            .pending_membership_history_transfers
            .entry(source_device_id.clone())
            .or_insert_with(|| PendingMembershipHistoryTransferV2 {
                transfer_id,
                page_count,
                pages: Vec::new(),
            });
        if pending.transfer_id != transfer_id || pending.page_count != page_count {
            state
                .pending_membership_history_transfers
                .remove(source_device_id);
            self.persist(state).await?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let expected_index = u32::try_from(pending.pages.len()).map_err(|_| {
            WorkspaceConvergenceError::Inconsistent(
                "membership history page count exceeds the protocol range".to_owned(),
            )
        })?;
        if page_index < expected_index {
            if pending.pages.get(page_index as usize) != Some(&page) {
                state
                    .pending_membership_history_transfers
                    .remove(source_device_id);
                self.persist(state).await?;
                return Ok(MembershipHistoryMessage::AckV2(
                    MembershipHistoryV2Ack::Invalid,
                ));
            }
        } else if page_index == expected_index {
            pending.pages.push(page);
        } else {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Continue {
                    transfer_id,
                    next_page_index: expected_index,
                },
            ));
        }
        self.persist(state).await?;
        let received_count = state
            .pending_membership_history_transfers
            .get(source_device_id)
            .map(|transfer| transfer.pages.len())
            .unwrap_or_default();
        if received_count < page_count as usize {
            let next_page_index = u32::try_from(received_count).map_err(|_| {
                WorkspaceConvergenceError::Inconsistent(
                    "membership history page count exceeds the protocol range".to_owned(),
                )
            })?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Continue {
                    transfer_id,
                    next_page_index,
                },
            ));
        }
        let pages = state
            .pending_membership_history_transfers
            .get(source_device_id)
            .map(|transfer| transfer.pages.clone())
            .unwrap_or_default();
        let incoming = match VersionedMembershipHistory::import_exchange_pages_v2(
            &pages,
            self.deps.historical_membership_signatures.as_ref(),
        ) {
            Ok(history) if history.lineage_id() == state.space_lineage => history,
            _ => {
                state
                    .pending_membership_history_transfers
                    .remove(source_device_id);
                self.persist(state).await?;
                return Ok(MembershipHistoryMessage::AckV2(
                    MembershipHistoryV2Ack::Invalid,
                ));
            }
        };
        let candidates = [source_device_id.clone()];
        let Some(source_member) = incoming.member_for_device(source_device_id, &candidates) else {
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        };

        let current_encoded = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission_transaction::map_repository_error)?;
        if current_encoded.as_deref()
            == Some(
                incoming
                    .encode_persisted_v2()
                    .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
                    .as_slice(),
            )
        {
            state
                .pending_membership_history_transfers
                .remove(source_device_id);
            self.update_peer_history_relationship(
                state,
                source_device_id.clone(),
                MembershipHistoryRelationship::Consistent,
                now_ms,
            )?;
            self.persist(state).await?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Consistent,
            ));
        }

        let current = current_encoded
            .as_deref()
            .map(|encoded| {
                VersionedMembershipHistory::decode_persisted_v2(
                    encoded,
                    self.deps.historical_membership_signatures.as_ref(),
                )
            })
            .transpose()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let source_is_allowed = current.as_ref().map_or_else(
            || incoming.active_members().contains(&source_member),
            |current| {
                incoming.active_members().contains(&source_member)
                    && incoming.is_authorized_active_member_extension_of(current, source_member)
                    || incoming.is_authorized_decision_delivery_of(current, source_member)
            },
        );
        if !source_is_allowed {
            state
                .pending_membership_history_transfers
                .remove(source_device_id);
            self.persist(state).await?;
            return Ok(MembershipHistoryMessage::AckV2(
                MembershipHistoryV2Ack::Invalid,
            ));
        }
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let mut merged = match current {
            Some(current) => current,
            None => incoming.clone(),
        };
        let changed = if current_encoded.is_some() {
            merged
                .merge_remote_history(
                    &incoming,
                    own_instance,
                    self.deps.historical_membership_signatures.as_ref(),
                )
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        } else {
            true
        };
        let replacement = merged
            .encode_persisted_v2()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        self.deps
            .admission_attempts
            .compare_and_replace_membership_history_v2(current_encoded.as_deref(), &replacement)
            .await
            .map_err(admission_transaction::map_repository_error)?;
        for member in merged.active_members() {
            if let Some(facts) = merged.admission_facts_for(member) {
                self.save_member_facts(facts, now_ms).await?;
            }
        }
        let relationship = if merged.pending_removal_decision(own_instance).is_some() {
            MembershipHistoryRelationship::PendingRemovalDecision
        } else if merged.removal_choices_diverge(own_instance, source_member) {
            MembershipHistoryRelationship::Diverged
        } else {
            MembershipHistoryRelationship::Consistent
        };
        self.update_peer_history_relationship(
            state,
            source_device_id.clone(),
            relationship,
            now_ms,
        )?;
        state
            .pending_membership_history_transfers
            .remove(source_device_id);
        self.persist(state).await?;
        self.publish(state);
        self.notify();
        Ok(MembershipHistoryMessage::AckV2(if changed {
            MembershipHistoryV2Ack::UpdatesApplied
        } else {
            MembershipHistoryV2Ack::Consistent
        }))
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

    async fn decide_membership_removal_v2(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
    ) -> Result<Option<WorkspaceSnapshot>, WorkspaceConvergenceError> {
        let Some(encoded_history) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission_transaction::map_repository_error)?
        else {
            return Ok(None);
        };
        let mut history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &encoded_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let Some(removal) = history.event(removal_event_id).cloned() else {
            return Ok(None);
        };
        if !matches!(
            removal.operation,
            MembershipOperationV2::RemoveDevice { .. }
        ) {
            return Ok(None);
        }
        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own = own_credential.member_instance_id(&self.deps.own_device);
        if let Some(completed) = history.decision_for(removal_event_id, own) {
            if completed.decision != decision {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "membership removal was completed with a different decision".to_owned(),
                ));
            }
            return self.query().await.map(Some);
        }
        if history.pending_removal_decision(own) != Some(removal_event_id) {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "membership removal is no longer pending".to_owned(),
            ));
        }
        let parent = removal.parent_event_id.ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent("membership removal has no parent".to_owned())
        })?;
        let resulting_members_digest = match decision {
            RemovalDecision::Accept => removal.resulting_members_digest,
            RemovalDecision::Reject => history.members_digest_at(parent).ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "membership removal parent is unavailable".to_owned(),
                )
            })?,
        };
        let mut signed_decision = MembershipDecisionV2::new(
            MEMBERSHIP_DECISION_FORMAT_V2,
            history.lineage_id().to_owned(),
            removal_event_id,
            own,
            own_credential.credential_id,
            own_credential.signature_algorithm_version,
            decision,
            Some(parent),
            resulting_members_digest,
            uuid::Uuid::new_v4().into_bytes(),
            Vec::new(),
        );
        signed_decision.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&signed_decision.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        history
            .verify_and_record_local_decision(
                signed_decision,
                own,
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let replacement = history
            .encode_persisted_v2()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;

        let _guard = self.state_lock.lock().await;
        let mut state = self.load_state().await?;
        self.deps
            .admission_attempts
            .compare_and_replace_membership_history_v2(Some(&encoded_history), &replacement)
            .await
            .map_err(admission_transaction::map_repository_error)?;
        let mut candidate_devices = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
            .into_iter()
            .map(|member| member.device_id)
            .collect::<Vec<_>>();
        candidate_devices.push(self.deps.own_device.clone());
        let decision_author =
            history.device_for_member(&removal.author_member_instance_id, &candidate_devices);
        if let Some(author) = decision_author.as_ref() {
            self.update_peer_history_relationship(
                &mut state,
                author.clone(),
                if decision == RemovalDecision::Accept {
                    MembershipHistoryRelationship::Consistent
                } else {
                    MembershipHistoryRelationship::Diverged
                },
                self.deps.clock.now_ms(),
            )?;
            self.persist(&state).await?;
        }
        self.publish(&state);
        self.notify();
        drop(_guard);
        self.deliver_pending_membership_decisions().await?;
        self.query().await.map(Some)
    }

    async fn decide_membership_removal_locked(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        if let Some(snapshot) = self
            .decide_membership_removal_v2(removal_event_id, decision)
            .await?
        {
            return Ok(snapshot);
        }
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
        if let Some(encoded_history) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission_transaction::map_repository_error)?
        {
            let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                &encoded_history,
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            let own = self
                .deps
                .member_signatures
                .current_member_instance(&self.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            if let Some(completed) = history.decision_for(change_id, own) {
                let completed_choice = match completed.decision {
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
            if history.pending_removal_decision(own) != Some(change_id) {
                let snapshot = self.query_device_trust().await?;
                return Ok(DeviceTrustDecisionResult::StateChanged {
                    current_change_id: snapshot
                        .current_change
                        .as_ref()
                        .map(|change| change.change_id),
                    snapshot,
                });
            }
            let removes_local = history.event(change_id).is_some_and(|event| {
                matches!(
                    event.operation,
                    MembershipOperationV2::RemoveDevice { member } if member == own
                )
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
            self.decide_membership_removal_v2(change_id, decision)
                .await?
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "current V2 removal disappeared".to_owned(),
                    )
                })?;
            let snapshot = self.query_device_trust().await?;
            return Ok(match choice {
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
            });
        }
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
        if let Some(encoded_history) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission_transaction::map_repository_error)?
        {
            let mut history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                &encoded_history,
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            let own_credential = self
                .deps
                .member_signatures
                .current_membership_credential(&self.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            let own = own_credential.member_instance_id(&self.deps.own_device);
            if !history.active_members().contains(&own) {
                return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
            }
            let mut candidate_devices = self
                .deps
                .member_repo
                .list()
                .await
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
                .into_iter()
                .map(|member| member.device_id)
                .collect::<Vec<_>>();
            candidate_devices.push(self.deps.own_device.clone());
            candidate_devices.push(target.clone());
            candidate_devices.sort_by(|left, right| left.as_str().cmp(right.as_str()));
            candidate_devices.dedup();
            let removal_recipients = history
                .active_members()
                .iter()
                .filter(|member| **member != own)
                .filter_map(|member| history.device_for_member(member, &candidate_devices))
                .collect::<Vec<_>>();
            let target_member = history
                .effective_members()
                .into_iter()
                .find(|member| {
                    history
                        .device_for_member(member, &candidate_devices)
                        .as_ref()
                        == Some(target)
                })
                .ok_or(WorkspaceConvergenceError::UnknownTarget)?;
            if target_member == own {
                return Err(WorkspaceConvergenceError::SelfTarget);
            }
            let operation = MembershipOperationV2::RemoveDevice {
                member: target_member,
            };
            let position = history
                .current_position()
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            let resulting_members_digest = history
                .expected_resulting_members_digest(position.event_id, &operation)
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            let mut event = MembershipEventV2::new(
                MEMBERSHIP_EVENT_FORMAT_V2,
                history.lineage_id().to_owned(),
                position.event_id,
                position.depth.saturating_add(1),
                uuid::Uuid::new_v4().into_bytes(),
                own,
                own_credential.credential_id,
                own_credential.signature_algorithm_version,
                operation,
                resulting_members_digest,
                state
                    .current_digest()
                    .map(|digest| *digest.as_bytes())
                    .unwrap_or([0; 32]),
                Vec::new(),
                None,
                Vec::new(),
            );
            event.signature = self
                .deps
                .member_signatures
                .sign_current_member_payload(&event.signing_payload())
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            history
                .verify_and_receive_event(
                    event,
                    self.deps.historical_membership_signatures.as_ref(),
                )
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            let replacement = history
                .encode_persisted_v2()
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            self.deps
                .admission_attempts
                .compare_and_replace_membership_history_v2(Some(&encoded_history), &replacement)
                .await
                .map_err(admission_transaction::map_repository_error)?;
            self.publish(&state);
            self.notify();
            drop(_guard);
            for recipient in removal_recipients {
                let _ = self
                    .reconcile_membership_history_with_peer(&recipient)
                    .await;
            }
            return self.query().await;
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

    fn clear_legacy_migration_marker_if_current_history_exists(
        state: &mut WorkspaceConvergenceState,
    ) -> bool {
        if !state.migrated_from_pre_adr_020 {
            return false;
        }
        if state
            .membership_reconciliation
            .as_ref()
            .is_some_and(|history| history.applied_head().is_some())
        {
            state.migrated_from_pre_adr_020 = false;
            return true;
        }
        false
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
        let protection = self
            .deps
            .space_protection
            .query_space_protection(&protection_member_ids)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let resumes_owned_legacy_bootstrap =
            protection.legacy_bootstrap.as_ref().is_some_and(|item| {
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
        Self::clear_legacy_migration_marker_if_current_history_exists(&mut state);
        self.persist(&state).await?;
        self.publish(&state);
        self.notify();
        Ok(state.snapshot())
    }

    /// Finish the membership baseline for a newly-created Space before A1 is
    /// allowed to report success.
    pub(crate) async fn initialize_new_space_membership(
        &self,
    ) -> Result<(), WorkspaceConvergenceError> {
        let result = self
            .deps
            .group_bootstrap
            .bootstrap_legacy_space(&self.deps.own_device, &[], self.deps.clock.now_ms())
            .await
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if !matches!(
            result,
            uc_core::membership::GroupBootstrapResult::Complete { .. }
        ) {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "new space protection group did not complete".to_owned(),
            ));
        }
        self.initialize_upgraded_legacy_space().await?;
        Ok(())
    }

    /// Complete a retained legacy member's protection-group join by fetching
    /// the sponsor's authoritative current membership history before normal
    /// peer reconciliation resumes.
    pub async fn complete_upgraded_legacy_join(
        &self,
        sponsor: &DeviceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        self.record_local_readiness(own_instance).await?;
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
impl SpaceTransitionRecoveryPort for WorkspaceConvergence {
    async fn requires_session_transition(&self) -> Result<bool, WorkspaceConvergenceError> {
        WorkspaceConvergence::requires_session_transition(self).await
    }

    async fn recover_after_session_drain(&self) -> Result<usize, WorkspaceConvergenceError> {
        self.recover_space_transition_after_session_drain().await
    }
}

#[async_trait]
impl uc_core::membership::ContentExchangeGatePort for WorkspaceConvergence {
    async fn is_locally_removed(&self, device_id: &DeviceId) -> bool {
        self.locally_removed(device_id).await
    }
}

impl WorkspaceConvergence {
    async fn v2_current_peer_snapshot(
        &self,
        state: &WorkspaceConvergenceState,
    ) -> Result<
        Option<uc_core::membership::CurrentWorkspacePeerSnapshot>,
        uc_core::membership::CurrentWorkspacePeerScopeError,
    > {
        use uc_core::membership::{
            AdmissionAttemptRepositoryError, AdmissionTerminalResultV1,
            CurrentWorkspaceLocalMembership, CurrentWorkspacePeerScopeError,
            CurrentWorkspacePeerScopeSource, CurrentWorkspacePeerSnapshot,
            MembershipHistoryV2Error, VersionedMembershipHistory,
        };

        let map_repository_error = |error| match error {
            AdmissionAttemptRepositoryError::Locked => CurrentWorkspacePeerScopeError::Locked,
            AdmissionAttemptRepositoryError::Corrupt => CurrentWorkspacePeerScopeError::Corrupt,
            _ => CurrentWorkspacePeerScopeError::Unavailable,
        };
        let Some(encoded_history) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(map_repository_error)?
        else {
            return Ok(None);
        };
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &encoded_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| match error {
            MembershipHistoryV2Error::UpgradeRequired => {
                CurrentWorkspacePeerScopeError::Unavailable
            }
            _ => CurrentWorkspacePeerScopeError::Corrupt,
        })?;
        let local_join = self
            .deps
            .admission_attempts
            .project_current_local_join()
            .await
            .map_err(map_repository_error)?;
        if history.lineage_id() != state.space_lineage {
            if let Some(join) = &local_join {
                if join.terminal_result.is_none() {
                    let attempt = self
                        .deps
                        .admission_attempts
                        .load(join.attempt_id)
                        .await
                        .map_err(map_repository_error)?
                        .ok_or(CurrentWorkspacePeerScopeError::Corrupt)?;
                    let transition = attempt
                        .space_transition
                        .as_deref()
                        .and_then(uc_core::membership::AdmissionSpaceTransitionV2::decode);
                    if transition.as_ref().is_some_and(|transition| {
                        matches!(
                            transition,
                            uc_core::membership::AdmissionSpaceTransitionV2::CrossSpace(item)
                                if transition.attempt_id() == join.attempt_id
                                    && item.source_space_id == state.space_lineage
                                    && item.target_space_id == history.lineage_id()
                                    && transition.phase_rank()
                                        < transition.activation_started_rank()
                        )
                    }) {
                        return Ok(None);
                    }
                } else if join.terminal_result == Some(AdmissionTerminalResultV1::Rejected) {
                    let terminal = self
                        .deps
                        .admission_attempts
                        .load_terminal(join.attempt_id)
                        .await
                        .map_err(map_repository_error)?
                        .ok_or(CurrentWorkspacePeerScopeError::Corrupt)?;
                    if terminal
                        .candidate_event_id
                        .is_some_and(|event_id| history.contains_event_id(&event_id))
                    {
                        return Ok(None);
                    }
                }
            }
            return Err(CurrentWorkspacePeerScopeError::Corrupt);
        }

        let members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|_| CurrentWorkspacePeerScopeError::Unavailable)?;
        let mut candidate_devices = members
            .into_iter()
            .map(|member| member.device_id)
            .collect::<Vec<_>>();
        candidate_devices.push(self.deps.own_device.clone());
        candidate_devices.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        candidate_devices.dedup();

        let active_devices = history
            .active_members()
            .iter()
            .map(|member| {
                history
                    .device_for_member(member, &candidate_devices)
                    .ok_or(CurrentWorkspacePeerScopeError::Unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let local_join_does_not_block_current_history =
            local_join.is_none_or(|join| join.terminal_result.is_some());
        let local_membership = if active_devices.contains(&self.deps.own_device)
            && local_join_does_not_block_current_history
        {
            CurrentWorkspaceLocalMembership::Active
        } else {
            CurrentWorkspaceLocalMembership::Removed
        };
        let mut peer_device_ids = if local_membership == CurrentWorkspaceLocalMembership::Active {
            active_devices
                .into_iter()
                .filter(|device| *device != self.deps.own_device)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        peer_device_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        peer_device_ids.dedup();

        Ok(Some(CurrentWorkspacePeerSnapshot {
            revision: state.revision,
            source: CurrentWorkspacePeerScopeSource::CurrentHistory,
            local_membership,
            peer_device_ids,
        }))
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
        if let Some(snapshot) = self.v2_current_peer_snapshot(&state).await? {
            return Ok(snapshot);
        }
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

fn admission_invitation_digest(invitation: &str) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"uniclipboard/admission-invitation-claim/v1\0");
    hasher.update((invitation.len() as u64).to_be_bytes());
    hasher.update(invitation.as_bytes());
    hasher.finalize().into()
}

fn admission_resume_public_key_digest(public_key: &[u8]) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"uniclipboard/admission-resume-public-key/v1\0");
    hasher.update((public_key.len() as u64).to_be_bytes());
    hasher.update(public_key);
    hasher.finalize().into()
}

fn admission_operation_id(attempt_id: uc_core::membership::AdmissionAttemptId) -> [u8; 16] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"uniclipboard/admission-operation/v1\0");
    hasher.update(attempt_id.as_bytes());
    let digest = hasher.finalize();
    let mut operation_id = [0; 16];
    operation_id.copy_from_slice(&digest[..16]);
    operation_id
}

fn common_existing_member_delivery_payload(
    deliveries: &[uc_core::membership::SponsorAdmissionSecurityDelivery],
) -> Result<Vec<u8>, WorkspaceConvergenceError> {
    let Some(first) = deliveries.first() else {
        return Ok(Vec::new());
    };
    if first.payload.is_empty()
        || deliveries
            .iter()
            .skip(1)
            .any(|delivery| delivery.payload != first.payload)
    {
        return Err(WorkspaceConvergenceError::Inconsistent(
            "existing members received incompatible security updates".to_owned(),
        ));
    }
    Ok(first.payload.clone())
}

fn validate_candidate_request(
    candidate: &admission_transaction::DurableAdmissionCandidateV1,
    request: &uc_core::pairing::JoinerRequest,
) -> Result<(), WorkspaceConvergenceError> {
    let event: uc_core::membership::MembershipEventV2 =
        postcard::from_bytes(&candidate.candidate_event)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
    let uc_core::membership::MembershipOperationV2::AddDevice { admission } = event.operation
    else {
        return Err(WorkspaceConvergenceError::InvalidConfirmation);
    };
    if admission.facts != request.admission
        || admission.membership_credential != request.membership_credential
        || admission.resume_public_key_digest
            != admission_resume_public_key_digest(&request.resume_public_key)
        || candidate.candidate_key_package != request.key_package
        || candidate.resume_public_key != request.resume_public_key
    {
        return Err(WorkspaceConvergenceError::InvalidConfirmation);
    }
    Ok(())
}

fn candidate_frame(
    attempt_id: uc_core::membership::AdmissionAttemptId,
    message: &uc_core::membership::AdmissionOutboxMessageV1,
) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
    if message.purpose != uc_core::membership::AdmissionOutboxPurposeV1::Candidate {
        return Err(WorkspaceConvergenceError::Inconsistent(
            "candidate outbox has the wrong purpose".to_owned(),
        ));
    }
    Ok(uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *attempt_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Candidate,
        message_id: message.message_id,
        predecessor_message_id: message.predecessor_message_id,
        payload: message.payload.clone(),
    })
}

fn durable_frame_from_outbox(
    attempt_id: uc_core::membership::AdmissionAttemptId,
    kind: uc_core::pairing::DurableAdmissionMessageKind,
    purpose: uc_core::membership::AdmissionOutboxPurposeV1,
    message: &uc_core::membership::AdmissionOutboxMessageV1,
) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
    if message.purpose != purpose {
        return Err(WorkspaceConvergenceError::Inconsistent(
            "durable admission outbox has the wrong purpose".to_owned(),
        ));
    }
    Ok(uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *attempt_id.as_bytes(),
        kind,
        message_id: message.message_id,
        predecessor_message_id: message.predecessor_message_id,
        payload: message.payload.clone(),
    })
}

fn complete_ack_frame(
    attempt_id: uc_core::membership::AdmissionAttemptId,
    complete_message_id: [u8; 32],
    payload: Vec<u8>,
) -> uc_core::pairing::DurableAdmissionFrame {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"uniclipboard/admission-complete-ack/v1\0");
    hasher.update(attempt_id.as_bytes());
    hasher.update(complete_message_id);
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(&payload);
    uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *attempt_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::CompleteAck,
        message_id: hasher.finalize().into(),
        predecessor_message_id: Some(complete_message_id),
        payload,
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
