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

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tracing::info;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignaturePort, CurrentMembershipAnnouncementPort, CurrentMembershipIdentityPort,
    MemberRepositoryPort, MembershipSecurityUpdateError, MembershipSecurityUpdatePort,
    RemovalExchangeEndpointPort, RemovalExchangeError, RemovalExchangeMessage, RemovalExchangePort,
    RemovalIntentContent, RemovalIntentVerificationError, RemovalIntentVerificationPort,
    RemovalLateAcceptance, RemovalLateRejectionReason, RemovalLateSubmission,
    RemovalLateSubmissionEndpointPort, RemovalLateSubmissionError, RemovalLateSubmissionPort,
    RemovalNotice, RemovalNoticeAcceptance, RemovalNoticeEndpointPort, RemovalNoticeError,
    RemovalNoticePort, RemovalNoticeRejectionReason, RemovalNoticeVerificationPort,
    RemovalRecoveryError, RemovalRecoveryPort, SignedRemovalIntent, WorkspaceChange,
    WorkspaceChangeKind, WorkspaceConfirmation, WorkspaceConvergenceEvent,
    WorkspaceConvergenceRepositoryError, WorkspaceConvergenceRepositoryPort,
    WorkspaceConvergenceState, WorkspaceMergeOutcome, WorkspacePhase, WorkspaceSnapshot,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort, PeerAddressRepositoryPort};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;

pub use runtime::WorkspaceConvergenceRuntime;

const MAX_HANDOFF_BATCH_CHANGES: usize = 64;

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceConvergenceError {
    #[error("workspace convergence state is locked")]
    Locked,
    #[error("workspace convergence state could not be persisted: {0}")]
    Repository(#[from] WorkspaceConvergenceRepositoryError),
    #[error("workspace convergence security update failed: {0}")]
    SecurityUpdate(#[from] MembershipSecurityUpdateError),
    #[error("workspace convergence recovery state is unavailable: {0}")]
    Recovery(#[from] RemovalRecoveryError),
    #[error("workspace convergence intent verification failed: {0}")]
    IntentVerification(#[from] RemovalIntentVerificationError),
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
    #[error("workspace convergence is unavailable")]
    Unavailable,
}

pub struct WorkspaceConvergenceDeps {
    pub repository: Arc<dyn WorkspaceConvergenceRepositoryPort>,
    pub verification: Arc<dyn RemovalIntentVerificationPort>,
    pub recovery: Arc<dyn RemovalRecoveryPort>,
    pub member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub membership_identity: Arc<dyn CurrentMembershipIdentityPort>,
    pub announcement_material: Arc<dyn CurrentMembershipAnnouncementPort>,
    pub security_updates: Arc<dyn MembershipSecurityUpdatePort>,
    pub clock: Arc<dyn ClockPort>,
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    /// Ordinary member channel: validated intent exchange and workspace
    /// confirmation delivery.
    pub exchange: Arc<dyn RemovalExchangePort>,
    /// Restricted entry used by a removed instance to submit historical
    /// intents to current members.
    pub late_submission: Arc<dyn RemovalLateSubmissionPort>,
    /// Restricted entry used to notify a removed target device.
    pub notice: Arc<dyn RemovalNoticePort>,
    pub notice_verification: Arc<dyn RemovalNoticeVerificationPort>,
    /// Member roster persistence: admission commits write the admitted
    /// member facts here in the same save boundary.
    pub trusted_peer_repo: Arc<dyn TrustedPeerRepositoryPort>,
    pub peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    pub own_device: DeviceId,
}

/// The unified workspace convergence owner.
pub struct WorkspaceConvergence {
    deps: WorkspaceConvergenceDeps,
    state_lock: tokio::sync::Mutex<()>,
    wake: Arc<tokio::sync::Notify>,
    events: broadcast::Sender<WorkspaceSnapshot>,
}

/// One network action planned under the state lock, sent outside it.
enum Outgoing {
    Exchange {
        recipient: DeviceId,
        message: RemovalExchangeMessage,
    },
    Late {
        recipient: DeviceId,
        intent: SignedRemovalIntent,
    },
    Notice {
        recipient: DeviceId,
        notice: RemovalNotice,
    },
}

impl WorkspaceConvergence {
    pub fn new(deps: WorkspaceConvergenceDeps) -> Arc<Self> {
        let (events, _) = broadcast::channel(64);
        Arc::new(Self {
            deps,
            state_lock: tokio::sync::Mutex::new(()),
            wake: Arc::new(tokio::sync::Notify::new()),
            events,
        })
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

    fn publish(&self, state: &WorkspaceConvergenceState) {
        let _ = self.events.send(state.snapshot());
    }

    /// Whether the local device may currently drive content sends.
    pub async fn locally_removed(&self, device_id: &DeviceId) -> bool {
        let state = match self.load_state().await {
            Ok(state) => state,
            Err(_) => return true,
        };
        if state.removed {
            return true;
        }
        state.is_device_removed(device_id)
    }

    /// Whether the local member instance has observed its own removal.
    pub async fn own_instance_removed(&self) -> bool {
        self.load_state().await.map_or(true, |state| state.removed)
    }

    /// Load the current workspace snapshot without changing any state.
    pub async fn query(&self) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let state = self.load_state().await?;
        Ok(state.snapshot())
    }

    /// Submit a member removal: create and verify the removal intent, merge
    /// it into the validated intent set, form the next continuous workspace
    /// change, and save intent + change + local restriction in one commit.
    pub async fn submit_removal(
        &self,
        target: &DeviceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.removed {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        let view = self.deps.recovery.current_view().await?;
        let own = self
            .deps
            .recovery
            .own_instance()
            .await?
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
        let target_member = view
            .members
            .iter()
            .find(|member| member.device_id == *target)
            .ok_or(WorkspaceConvergenceError::UnknownTarget)?;
        if target_member.instance == own {
            return Err(WorkspaceConvergenceError::SelfTarget);
        }
        if state.effective_members().is_empty() {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }

        let mut view_members = view
            .members
            .iter()
            .map(|member| member.instance)
            .collect::<Vec<_>>();
        view_members.sort_unstable();
        view_members.dedup();
        let content = RemovalIntentContent {
            space_lineage: state.space_lineage.clone(),
            view_epoch: view.epoch,
            view_members,
            initiator: own,
            target: target_member.instance,
        };
        content.validate().map_err(|_| {
            WorkspaceConvergenceError::Inconsistent("invalid removal intent content".to_owned())
        })?;
        let payload = content.canonical_bytes();
        let signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&payload)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let intent = SignedRemovalIntent::new(content, signature, view.causal_proof.clone());
        self.deps.verification.verify_intent(&intent).await?;

        self.accept_intent(&mut state, intent, now_ms).await?;
        Ok(state.snapshot())
    }

    /// Verify a validated intent, merge it into the intent set, and form the
    /// next continuous removal change in one save commit. Idempotent for a
    /// known intent. Shared by local submission, the ordinary-member-channel
    /// endpoint, and the restricted late-submission entry.
    async fn accept_intent(
        &self,
        state: &mut WorkspaceConvergenceState,
        intent: SignedRemovalIntent,
        now_ms: i64,
    ) -> Result<uc_core::membership::RemovalIntentId, WorkspaceConvergenceError> {
        if intent.content.space_lineage != state.space_lineage {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "intent space mismatch".to_owned(),
            ));
        }
        self.deps.verification.verify_intent(&intent).await?;
        let to_remove = state
            .record_removal_intent(&intent, now_ms)
            .map_err(|_| WorkspaceConvergenceError::Inconsistent("intent rejected".to_owned()))?;
        if to_remove.is_empty() {
            // The intent did not change the effective membership (target
            // already covered by an earlier intent); keep the saved state.
            self.persist(state).await?;
            self.notify();
            return Ok(intent.intent_id);
        }
        let removed_instances = to_remove.iter().copied().collect::<Vec<_>>();
        let change = self.build_removal_change(state, &removed_instances, now_ms)?;
        self.apply_and_publish_change(state, change, now_ms).await?;
        self.notify();
        info!(
            removed_count = removed_instances.len(),
            "workspace removal change recorded"
        );
        Ok(intent.intent_id)
    }

    /// Record the sponsor's admission change after the joiner's readiness
    /// was confirmed, in one save commit with the pending handoff facts.
    pub async fn record_admission_change(
        &self,
        change: WorkspaceChange,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.removed {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        self.apply_and_publish_change(&mut state, change, now_ms)
            .await?;
        self.notify();
        Ok(state.snapshot())
    }

    /// Build the locally signed facts that a joiner returns after its group
    /// session is active. The facts remain inside the pairing exchange until
    /// the sponsor commits the admission chain.
    pub async fn local_admission_facts(
        &self,
    ) -> Result<uc_core::membership::AdmissionChangeFacts, WorkspaceConvergenceError> {
        let material = self
            .deps
            .announcement_material
            .current_announcement_material()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let member_instance = self
            .deps
            .recovery
            .own_instance()
            .await?
            .ok_or(WorkspaceConvergenceError::NotAMember)?;
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
    ) -> Result<uc_core::membership::AdmissionCommittedFacts, WorkspaceConvergenceError> {
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
            if record.invitation_generation < state.removal_intents.len() as u64 {
                // The admission generation advanced after the invitation was
                // bound; an old invitation cannot recover its old authority.
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "admission generation advanced".to_owned(),
                ));
            }
        }
        let own = self.local_admission_facts().await?;
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
        // The roster persistence failures abort the commit before any
        // workspace change is persisted, keeping the save boundary intact.
        self.save_member_facts(&joiner, now_ms).await?;
        for facts in &additions {
            let change = self.build_admission_change(&state, facts.clone(), now_ms);
            let (outcome, effect) = state
                .apply(WorkspaceConvergenceEvent::CommittedChange(change), now_ms)
                .map_err(|_| {
                    WorkspaceConvergenceError::Inconsistent("admission rejected".to_owned())
                })?;
            if !matches!(outcome, WorkspaceMergeOutcome::Updated) || !effect.persist {
                return Err(WorkspaceConvergenceError::Inconsistent(
                    "admission unchanged".to_owned(),
                ));
            }
        }
        // Pending handoff facts for the joiner: it must receive the whole
        // continuous chain from its own saved state (no change yet).
        let Some(digest) = state.current_digest() else {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "admission produced no digest".to_owned(),
            ));
        };
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::PendingHandoffCreated {
                    recipient: joiner.member_instance,
                    recipient_device: joiner.device_id,
                    confirmed_epoch: 0,
                    target_digest: *digest.as_bytes(),
                    has_more: false,
                },
                now_ms,
            )
            .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
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
        Ok(uc_core::membership::AdmissionCommittedFacts {
            change_digest: *digest.as_bytes(),
            change_count: state.changes.len() as u64,
            sponsor_facts: own,
        })
    }

    /// The joiner durably records the sponsor's admission-saved
    /// confirmation. Until then it stays locally ready and must not take
    /// part in ordinary content exchange. The sponsor's member facts carried
    /// by the confirmation are persisted in the same save boundary.
    pub async fn record_admission_committed(
        &self,
        confirmation: uc_core::membership::AdmissionCommittedFacts,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        self.save_member_facts(&confirmation.sponsor_facts, now_ms)
            .await?;
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::LocalAdmissionCommitted(confirmation),
                now_ms,
            )
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("admission committed rejected".to_owned())
            })?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        self.notify();
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

    fn build_admission_change(
        &self,
        state: &WorkspaceConvergenceState,
        facts: uc_core::membership::AdmissionChangeFacts,
        now_ms: i64,
    ) -> WorkspaceChange {
        let previous_epoch = state.current_epoch();
        let previous_digest = state
            .changes
            .last()
            .map(uc_core::membership::compute_change_digest)
            .unwrap_or_else(initial_digest);
        let mut change = WorkspaceChange {
            space_lineage: state.space_lineage.clone(),
            kind: WorkspaceChangeKind::Admission,
            previous_epoch,
            next_epoch: previous_epoch.saturating_add(1),
            previous_digest,
            digest: [0; 32],
            security_updates: Vec::new(),
            admission: Some(facts),
            removal: None,
            created_at_ms: now_ms,
        };
        change.digest = uc_core::membership::compute_change_digest(&change);
        change
    }

    async fn apply_and_publish_change(
        &self,
        state: &mut WorkspaceConvergenceState,
        change: WorkspaceChange,
        now_ms: i64,
    ) -> Result<(), WorkspaceConvergenceError> {
        let (outcome, effect) = state
            .apply(WorkspaceConvergenceEvent::CommittedChange(change), now_ms)
            .map_err(|_| WorkspaceConvergenceError::Inconsistent("change rejected".to_owned()))?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(state).await?;
        }
        if effect.publish {
            self.publish(state);
        }
        Ok(())
    }

    fn build_removal_change(
        &self,
        state: &WorkspaceConvergenceState,
        removed_instances: &[uc_core::membership::MemberInstanceId],
        now_ms: i64,
    ) -> Result<WorkspaceChange, WorkspaceConvergenceError> {
        let previous_epoch = state.current_epoch();
        let previous_digest = state
            .changes
            .last()
            .map(uc_core::membership::compute_change_digest)
            .unwrap_or_else(initial_digest);
        let mut change = WorkspaceChange {
            space_lineage: state.space_lineage.clone(),
            kind: WorkspaceChangeKind::Removal,
            previous_epoch,
            next_epoch: previous_epoch.saturating_add(1),
            previous_digest,
            digest: [0; 32],
            security_updates: Vec::new(),
            admission: None,
            removal: Some(uc_core::membership::RemovalChangeFacts {
                removed_instances: removed_instances.to_vec(),
            }),
            created_at_ms: now_ms,
        };
        change.digest = uc_core::membership::compute_change_digest(&change);
        Ok(change)
    }

    /// Apply a verified continuous change delivered by a handoff device.
    pub async fn ingest_handoff_change(
        &self,
        change: WorkspaceChange,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        self.apply_and_publish_change(&mut state, change, now_ms)
            .await?;
        self.notify();
        Ok(state.snapshot())
    }

    /// Record the local member instance and its readiness record after a
    /// successful admission (the joiner's local readiness; the sponsor
    /// records the admission change only after this readiness).
    pub async fn record_local_readiness(
        &self,
        own_instance: uc_core::membership::MemberInstanceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
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

    /// Record a member's confirmation of the current digest.
    pub async fn record_confirmation(
        &self,
        confirmation: WorkspaceConfirmation,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        let device = state
            .member_devices
            .get(&confirmation.member_instance)
            .cloned()
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let valid = self
            .deps
            .member_signatures
            .verify_current_member_payload(
                &device,
                &confirmation.signing_payload(),
                &confirmation.signature,
            )
            .await
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        if !valid {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::ConfirmationReceived(confirmation),
                now_ms,
            )
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        if effect.publish {
            self.publish(&state);
        }
        if state.phase == WorkspacePhase::Complete {
            info!("workspace convergence complete");
        }
        self.notify();
        Ok(state.snapshot())
    }

    /// Mark a known effective member temporarily unreachable.
    pub async fn mark_member_unreachable(
        &self,
        member: uc_core::membership::MemberInstanceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        let (outcome, effect) = state
            .apply(WorkspaceConvergenceEvent::MemberUnreachable(member), now_ms)
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("member state rejected".to_owned())
            })?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        if effect.publish {
            self.publish(&state);
        }
        Ok(state.snapshot())
    }

    /// Mark a previously unreachable known effective member back online.
    pub async fn mark_member_reachable(
        &self,
        member: uc_core::membership::MemberInstanceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        let (outcome, effect) = state
            .apply(WorkspaceConvergenceEvent::MemberReachable(member), now_ms)
            .map_err(|_| {
                WorkspaceConvergenceError::Inconsistent("member state rejected".to_owned())
            })?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        if effect.publish {
            self.publish(&state);
        }
        self.notify();
        Ok(state.snapshot())
    }

    /// Continuous changes from `from_epoch` (exclusive) up to the current
    /// target, bounded to `MAX_HANDOFF_BATCH_CHANGES` per batch.
    pub fn handoff_batch(
        &self,
        state: &WorkspaceConvergenceState,
        from_epoch: u64,
    ) -> Vec<WorkspaceChange> {
        state
            .changes
            .iter()
            .filter(|change| change.next_epoch > from_epoch)
            .take(MAX_HANDOFF_BATCH_CHANGES)
            .cloned()
            .collect()
    }

    /// Whether a handoff batch to `member` must continue after the returned
    /// batch: the current target digest is not yet included.
    pub fn has_more_after(
        &self,
        state: &WorkspaceConvergenceState,
        batch: &[WorkspaceChange],
    ) -> bool {
        state.changes.last().is_some_and(|last| {
            batch
                .last()
                .is_none_or(|last_batch| last_batch.change_id() != last.change_id())
        })
    }

    /// Update handoff progress after a recipient's durable acknowledgement.
    pub async fn apply_handoff_progress(
        &self,
        recipient: uc_core::membership::MemberInstanceId,
        confirmed_epoch: u64,
        target_digest: [u8; 32],
        has_more: bool,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::HandoffAdvanced {
                    recipient,
                    confirmed_epoch,
                    target_digest,
                    has_more,
                },
                now_ms,
            )
            .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        if effect.publish {
            self.publish(&state);
        }
        self.notify();
        Ok(state.snapshot())
    }

    /// Create a pending handoff record for a recipient.
    pub async fn create_pending_handoff(
        &self,
        recipient: uc_core::membership::MemberInstanceId,
        recipient_device: &DeviceId,
        confirmed_epoch: u64,
        target_digest: [u8; 32],
        has_more: bool,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::PendingHandoffCreated {
                    recipient,
                    recipient_device: *recipient_device,
                    confirmed_epoch,
                    target_digest,
                    has_more,
                },
                now_ms,
            )
            .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state).await?;
        }
        if effect.publish {
            self.publish(&state);
        }
        Ok(state.snapshot())
    }

    /// Effective members that have not yet confirmed the current digest,
    /// with their confirmed handoff epochs.
    pub async fn pending_confirmations(
        &self,
    ) -> Result<
        Vec<(uc_core::membership::MemberInstanceId, DeviceId, u64)>,
        WorkspaceConvergenceError,
    > {
        let state = self.load_state().await?;
        let confirmed = state.confirmed_members();
        let mut pending = Vec::new();
        for member in state.effective_members() {
            if confirmed.contains(&member) {
                continue;
            }
            if let Some(device) = state.member_devices.get(&member) {
                let epoch = state
                    .pending_handoffs
                    .get(&member)
                    .map_or(0, |record| record.confirmed_epoch);
                pending.push((member, *device, epoch));
            }
        }
        Ok(pending)
    }

    /// One reconcile pass: propagate validated intents, notify removed
    /// targets, submit historical intents when the local instance is
    /// removed, and save pending handoff records for members that have not
    /// yet confirmed the current digest. Network exchanges happen outside
    /// the state lock; responses are applied back inside it.
    pub async fn reconcile(&self) -> Result<(), WorkspaceConvergenceError> {
        loop {
            let outgoing = {
                let _guard = self.state_lock.lock().await;
                self.reconcile_plan().await?
            };
            if outgoing.is_empty() {
                return Ok(());
            }
            let mut responses = Vec::with_capacity(outgoing.len());
            for item in outgoing {
                let outcome: Result<RemovalExchangeMessage, RemovalExchangeError> = match &item {
                    Outgoing::Exchange { recipient, message } => {
                        self.deps
                            .exchange
                            .exchange(recipient, message.clone())
                            .await
                    }
                    Outgoing::Late { recipient, intent } => self
                        .deps
                        .late_submission
                        .submit_late(
                            recipient,
                            RemovalLateSubmission::Intent(Box::new(intent.clone())),
                        )
                        .await
                        .map(|_| RemovalExchangeMessage::IntentAck(intent.intent_id))
                        .map_err(|error| match error {
                            uc_core::membership::RemovalLateSubmissionTransportError::Transport => {
                                RemovalExchangeError::Transport
                            }
                            uc_core::membership::RemovalLateSubmissionTransportError::Offline => {
                                RemovalExchangeError::Offline
                            }
                        }),
                    Outgoing::Notice { recipient, notice } => self
                        .deps
                        .notice
                        .send_notice(recipient, notice.clone())
                        .await
                        .map(|_| RemovalExchangeMessage::IntentAck(notice.intent_id))
                        .map_err(|error| match error {
                            uc_core::membership::RemovalNoticeTransportError::Transport => {
                                RemovalExchangeError::Transport
                            }
                            uc_core::membership::RemovalNoticeTransportError::Offline => {
                                RemovalExchangeError::Offline
                            }
                        }),
                };
                responses.push((item, outcome));
            }
            let progressed = {
                let _guard = self.state_lock.lock().await;
                self.apply_exchange_responses(responses).await?
            };
            if !progressed {
                return Ok(());
            }
        }
    }

    /// Lock-held decision: what to send this pass, plus pending-handoff
    /// bookkeeping. No network happens here.
    async fn reconcile_plan(&self) -> Result<Vec<Outgoing>, WorkspaceConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.phase == WorkspacePhase::RecoveryRequired {
            return Ok(Vec::new());
        }
        let own_device = self.deps.device_identity.current_device_id();
        let mut outgoing = Vec::new();

        if state.removed {
            // The local instance observed its own removal: the only network
            // action is submitting historical intents through the restricted
            // late entry. No intent exchange, notice or handoff bookkeeping.
            for intent in &state.removal_intent_records {
                let intent_id = intent.intent_id;
                for member in state.effective_members() {
                    let Some(device) = state.member_devices.get(&member).cloned() else {
                        continue;
                    };
                    if device == own_device {
                        continue;
                    }
                    if state
                        .peer_intent_acks
                        .contains_key(&(device.clone(), intent_id))
                    {
                        continue;
                    }
                    outgoing.push(Outgoing::Late {
                        recipient: device,
                        intent: intent.clone(),
                    });
                }
            }
            if !outgoing.is_empty() {
                state.updated_at_ms = now_ms;
                self.persist(&state).await?;
            }
            return Ok(outgoing);
        }

        // Intent propagation on the ordinary member channel: every validated
        // intent is sent to every effective member that has not acknowledged
        // it yet (idempotent, retriable).
        for intent in &state.removal_intent_records {
            let intent_id = intent.intent_id;
            for member in state.effective_members() {
                let Some(device) = state.member_devices.get(&member).cloned() else {
                    continue;
                };
                if device == own_device {
                    continue;
                }
                if state
                    .peer_intent_acks
                    .contains_key(&(device.clone(), intent_id))
                {
                    continue;
                }
                outgoing.push(Outgoing::Exchange {
                    recipient: device,
                    message: RemovalExchangeMessage::Intent(Box::new(intent.clone())),
                });
            }
        }

        // Removal notices: any accepted intent whose target is no longer
        // effective is notified to the target device (best effort, not a
        // completion condition).
        for intent in &state.removal_intent_records {
            if state.notified_removals.contains(&intent.intent_id) {
                continue;
            }
            if state.effective_members().contains(&intent.content.target) {
                continue;
            }
            let Some(target_device) = state.member_devices.get(&intent.content.target).cloned()
            else {
                continue;
            };
            if target_device == own_device {
                continue;
            }
            let Ok(own) = self.deps.recovery.own_instance().await else {
                continue;
            };
            let Some(own) = own else {
                continue;
            };
            let mut notice = RemovalNotice {
                space_lineage_fingerprint: RemovalNotice::space_lineage_fingerprint(
                    &state.space_lineage,
                ),
                intent_id: intent.intent_id,
                target_instance: intent.content.target,
                target_device_id: target_device.clone(),
                issuer_instance: own,
                signature: Vec::new(),
            };
            let payload = notice.signing_payload();
            let signature = self
                .deps
                .member_signatures
                .sign_current_member_payload(&payload)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            notice.signature = signature;
            outgoing.push(Outgoing::Notice {
                recipient: target_device,
                notice,
            });
        }

        // Pending-handoff bookkeeping for members that have not yet
        // confirmed the current digest. Actual network transfer is performed
        // by the recovery transport; this pass only records the facts.
        let digest = match state.current_digest() {
            Some(digest) => digest,
            None => {
                if !outgoing.is_empty() {
                    state.updated_at_ms = now_ms;
                    self.persist(&state).await?;
                }
                return Ok(outgoing);
            }
        };
        let confirmed = state.confirmed_members();
        let mut changed = false;
        for member in state.effective_members() {
            if confirmed.contains(&member) {
                continue;
            }
            let Some(device) = state.member_devices.get(&member).cloned() else {
                continue;
            };
            let confirmed_epoch = state
                .pending_handoffs
                .get(&member)
                .map_or(0, |record| record.confirmed_epoch);
            let batch = self.handoff_batch(&state, confirmed_epoch);
            let has_more = self.has_more_after(&state, &batch);
            let (outcome, effect) = state
                .apply(
                    uc_core::membership::WorkspaceConvergenceEvent::PendingHandoffCreated {
                        recipient: member,
                        recipient_device: device,
                        confirmed_epoch,
                        target_digest: *digest.as_bytes(),
                        has_more,
                    },
                    now_ms,
                )
                .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
            if matches!(outcome, uc_core::membership::WorkspaceMergeOutcome::Updated) {
                changed = true;
            }
            if effect.publish {
                self.publish(&state);
            }
        }
        if changed || !outgoing.is_empty() {
            self.persist(&state).await?;
        }
        Ok(outgoing)
    }

    /// Lock-held: apply the bounded responses of the sent messages.
    async fn apply_exchange_responses(
        &self,
        responses: Vec<(
            Outgoing,
            Result<RemovalExchangeMessage, RemovalExchangeError>,
        )>,
    ) -> Result<bool, WorkspaceConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        let mut progressed = false;
        for (item, outcome) in responses {
            let Ok(reply) = outcome else {
                // Transport failure: keep the pending fact for the next pass.
                continue;
            };
            match item {
                Outgoing::Exchange { recipient, message } => match message {
                    RemovalExchangeMessage::Intent(intent) => {
                        if let RemovalExchangeMessage::IntentAck(_) = reply {
                            let (result, effect) = state
                                .apply(
                                    WorkspaceConvergenceEvent::IntentAcknowledged {
                                        peer: recipient,
                                        intent_id: intent.intent_id,
                                    },
                                    now_ms,
                                )
                                .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
                            progressed |= matches!(result, WorkspaceMergeOutcome::Updated);
                            if effect.publish {
                                self.publish(&state);
                            }
                        }
                    }
                    RemovalExchangeMessage::IntentAck(_) => {}
                },
                Outgoing::Late { recipient, intent } => {
                    // Any bounded response counts as delivered.
                    let (result, effect) = state
                        .apply(
                            WorkspaceConvergenceEvent::IntentAcknowledged {
                                peer: recipient,
                                intent_id: intent.intent_id,
                            },
                            now_ms,
                        )
                        .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
                    progressed |= matches!(result, WorkspaceMergeOutcome::Updated);
                    if effect.publish {
                        self.publish(&state);
                    }
                }
                Outgoing::Notice {
                    recipient: _,
                    notice,
                } => {
                    let (result, effect) = state
                        .apply(
                            WorkspaceConvergenceEvent::RemovalNoticeDelivered(notice.intent_id),
                            now_ms,
                        )
                        .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
                    progressed |= matches!(result, WorkspaceMergeOutcome::Updated);
                    if effect.publish {
                        self.publish(&state);
                    }
                }
            }
        }
        if progressed {
            self.persist(&state).await?;
        }
        Ok(progressed)
    }
}

fn initial_digest() -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b"uniclipboard-workspace-initial/v1").into()
}

#[async_trait]
impl uc_core::membership::RemovalTargetGatePort for WorkspaceConvergence {
    async fn is_locally_removed(&self, device_id: &DeviceId) -> bool {
        self.locally_removed(device_id).await
    }
}

#[async_trait]
impl uc_core::membership::RemovalAdmissionGatePort for WorkspaceConvergence {
    async fn admission_decision(
        &self,
        invitation_generation: u64,
    ) -> uc_core::membership::RemovalAdmissionDecision {
        let state = match self.load_state().await {
            Ok(state) => state,
            Err(_) => return uc_core::membership::RemovalAdmissionDecision::Unavailable,
        };
        if state.phase == WorkspacePhase::RecoveryRequired {
            return uc_core::membership::RemovalAdmissionDecision::RecoveryRequired;
        }
        if state.removed {
            return uc_core::membership::RemovalAdmissionDecision::Unavailable;
        }
        if state.removal_intents.is_empty() {
            return uc_core::membership::RemovalAdmissionDecision::Allowed;
        }
        if invitation_generation < state.removal_intents.len() as u64 {
            return uc_core::membership::RemovalAdmissionDecision::SupersededInvitation;
        }
        uc_core::membership::RemovalAdmissionDecision::Allowed
    }

    async fn invitation_generation(
        &self,
    ) -> Result<u64, uc_core::membership::RemovalAdmissionDecision> {
        let state = self
            .load_state()
            .await
            .map_err(|_| uc_core::membership::RemovalAdmissionDecision::Unavailable)?;
        Ok(state.removal_intents.len() as u64)
    }
}

/// The ordinary-member-channel intent exchange endpoint: accepts a verified
/// intent from a current effective member, merges it, forms the removal
/// change, and acknowledges. `IntentAck` records the peer's acknowledgement.
#[async_trait]
impl RemovalExchangeEndpointPort for WorkspaceConvergence {
    async fn handle_exchange(
        &self,
        source_device_id: &DeviceId,
        message: RemovalExchangeMessage,
    ) -> Result<RemovalExchangeMessage, RemovalExchangeError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        match message {
            RemovalExchangeMessage::Intent(intent) => {
                let mut state = self
                    .load_state()
                    .await
                    .map_err(|_| RemovalExchangeError::Rejected)?;
                if state.removed {
                    return Err(RemovalExchangeError::Rejected);
                }
                let source_effective = state
                    .effective_members()
                    .iter()
                    .any(|member| state.member_devices.get(member) == Some(source_device_id));
                if !source_effective {
                    // Only current effective members may submit new intents;
                    // a removed instance keeps only the restricted entries.
                    return Err(RemovalExchangeError::Rejected);
                }
                let latest = self
                    .accept_intent(&mut state, *intent, now_ms)
                    .await
                    .map_err(|_| RemovalExchangeError::Rejected)?;
                Ok(RemovalExchangeMessage::IntentAck(latest))
            }
            RemovalExchangeMessage::IntentAck(intent_id) => {
                let mut state = self
                    .load_state()
                    .await
                    .map_err(|_| RemovalExchangeError::Rejected)?;
                let (outcome, effect) = state
                    .apply(
                        WorkspaceConvergenceEvent::IntentAcknowledged {
                            peer: source_device_id.clone(),
                            intent_id,
                        },
                        now_ms,
                    )
                    .map_err(|_| RemovalExchangeError::Rejected)?;
                if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
                    self.persist(&state)
                        .await
                        .map_err(|_| RemovalExchangeError::Rejected)?;
                }
                Ok(RemovalExchangeMessage::IntentAck(intent_id))
            }
        }
    }
}

/// The restricted late-submission entry: a removed initiator submits a
/// historical intent; the bounded response never discloses members, digest,
/// generation, keys or content.
#[async_trait]
impl RemovalLateSubmissionEndpointPort for WorkspaceConvergence {
    async fn handle_late_submission(
        &self,
        submission: RemovalLateSubmission,
    ) -> Result<RemovalLateAcceptance, RemovalLateSubmissionError> {
        let RemovalLateSubmission::Intent(intent) = submission;
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self
            .load_state()
            .await
            .map_err(|_| RemovalLateSubmissionError::Unavailable)?;
        if state.space_lineage.is_empty() || intent.content.space_lineage != state.space_lineage {
            return Ok(RemovalLateAcceptance::Rejected {
                reason: RemovalLateRejectionReason::InvalidSpaceLineage,
            });
        }
        if state
            .removal_intent_records
            .iter()
            .any(|known| known.intent_id == intent.intent_id)
        {
            return Ok(RemovalLateAcceptance::AlreadyKnown {
                intent_id: intent.intent_id,
            });
        }
        match self.accept_intent(&mut state, *intent, now_ms).await {
            Ok(intent_id) => Ok(RemovalLateAcceptance::Accepted { intent_id }),
            Err(_) => Ok(RemovalLateAcceptance::Rejected {
                reason: RemovalLateRejectionReason::Invalid,
            }),
        }
    }
}

/// The restricted removal-notice entry: the receiver verifies the space
/// fingerprint, the issuer signature against its saved view material, and
/// that the notice targets its own device before accepting (fail closed).
#[async_trait]
impl RemovalNoticeEndpointPort for WorkspaceConvergence {
    async fn handle_notice(
        &self,
        notice: RemovalNotice,
    ) -> Result<RemovalNoticeAcceptance, RemovalNoticeError> {
        let _guard = self.state_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut state = self
            .load_state()
            .await
            .map_err(|_| RemovalNoticeError::Unavailable)?;
        if state.space_lineage.is_empty()
            || RemovalNotice::space_lineage_fingerprint(&state.space_lineage)
                != notice.space_lineage_fingerprint
        {
            return Ok(RemovalNoticeAcceptance::Rejected {
                reason: RemovalNoticeRejectionReason::SpaceMismatch,
            });
        }
        if state
            .removal_intent_records
            .iter()
            .any(|known| known.intent_id == notice.intent_id)
            || state.accepted_removal_notices.contains(&notice.intent_id)
        {
            // The intent or this notice was already accepted; idempotent.
            return Ok(RemovalNoticeAcceptance::AlreadyKnown {
                intent_id: notice.intent_id,
            });
        }
        let Some(issuer_key) = self.lookup_issuer_key(&notice).await else {
            return Ok(RemovalNoticeAcceptance::Rejected {
                reason: RemovalNoticeRejectionReason::Invalid,
            });
        };
        if self
            .deps
            .notice_verification
            .verify_notice_signature(&notice, &issuer_key)
            .await
            .is_err()
        {
            return Ok(RemovalNoticeAcceptance::Rejected {
                reason: RemovalNoticeRejectionReason::Invalid,
            });
        }
        if !self.notice_targets_own_device(&notice).await {
            return Ok(RemovalNoticeAcceptance::Rejected {
                reason: RemovalNoticeRejectionReason::Invalid,
            });
        }
        let (outcome, effect) = state
            .apply(
                WorkspaceConvergenceEvent::RemovalNoticeAccepted(notice.intent_id),
                now_ms,
            )
            .map_err(|_| RemovalNoticeError::Rejected)?;
        if matches!(outcome, WorkspaceMergeOutcome::Updated) && effect.persist {
            self.persist(&state)
                .await
                .map_err(|_| RemovalNoticeError::Persistence)?;
        }
        self.notify();
        info!(intent = %notice.intent_id, "removal notice accepted");
        Ok(RemovalNoticeAcceptance::Accepted {
            intent_id: notice.intent_id,
        })
    }
}

impl WorkspaceConvergence {
    /// Issuer public signing material: the current view's public material
    /// for the issuer instance (fail closed on any mismatch).
    async fn lookup_issuer_key(&self, notice: &RemovalNotice) -> Option<Vec<u8>> {
        self.deps
            .recovery
            .current_view()
            .await
            .ok()
            .and_then(|view| {
                view.members
                    .into_iter()
                    .find(|member| member.instance == notice.issuer_instance)
                    .map(|member| member.signing_public_key)
            })
    }

    /// The notice target must be the local device: the current instance is
    /// the target instance; when the local instance no longer exists, the
    /// target device must match the local device (fail closed).
    async fn notice_targets_own_device(&self, notice: &RemovalNotice) -> bool {
        match self.deps.recovery.own_instance().await {
            Ok(Some(own)) => own == notice.target_instance,
            _ => notice.target_device_id == self.deps.own_device,
        }
    }
}

/// The restricted recovery channel endpoint. The owner verifies the
/// connection identity, the request proof, the requester's member instance
/// in its declared predecessor state, and whether that instance is still a
/// current effective member before any offer is released. A removed or
/// unknown instance receives only a stable rejection.
#[async_trait]
impl uc_core::membership::RecoveryTransportEndpointPort for WorkspaceConvergence {
    async fn handle_recovery(
        &self,
        source_device: &DeviceId,
        message: uc_core::membership::RecoveryChannelMessage,
    ) -> Result<
        uc_core::membership::RecoveryChannelMessage,
        uc_core::membership::RecoveryTransportError,
    > {
        match message {
            uc_core::membership::RecoveryChannelMessage::Request(request) => {
                self.handle_recovery_request(source_device, request).await
            }
            uc_core::membership::RecoveryChannelMessage::Ack(ack) => {
                self.handle_recovery_ack(source_device, ack).await
            }
            _ => Err(uc_core::membership::RecoveryTransportError::Rejected(
                uc_core::membership::RecoveryRejection::Unauthorized,
            )),
        }
    }
}

impl WorkspaceConvergence {
    async fn handle_recovery_request(
        &self,
        source_device: &DeviceId,
        request: uc_core::membership::RecoveryRequest,
    ) -> Result<
        uc_core::membership::RecoveryChannelMessage,
        uc_core::membership::RecoveryTransportError,
    > {
        if request.validate_transfer_bounds().is_err() {
            return Ok(uc_core::membership::RecoveryChannelMessage::Reject(
                uc_core::membership::RecoveryReject {
                    space_lineage_fingerprint: request.space_lineage_fingerprint,
                    request_number: request.request_number,
                    reply_number: 0,
                    reason: uc_core::membership::RecoveryRejection::RangeOutOfBounds,
                },
            ));
        }
        let state = self.load_state().await.map_err(|_| {
            uc_core::membership::RecoveryTransportError::Rejected(
                uc_core::membership::RecoveryRejection::Unauthorized,
            )
        })?;
        let requester =
            uc_core::membership::MemberInstanceId::from_bytes(request.requester_instance);
        if !state.effective_members().contains(&requester) {
            // A removed or unknown instance receives no offer and no
            // distinguishing details.
            return Ok(uc_core::membership::RecoveryChannelMessage::Reject(
                uc_core::membership::RecoveryReject {
                    space_lineage_fingerprint: request.space_lineage_fingerprint,
                    request_number: request.request_number,
                    reply_number: 0,
                    reason: uc_core::membership::RecoveryRejection::Unauthorized,
                },
            ));
        }
        if state.member_devices.get(&requester) != Some(source_device) {
            return Ok(uc_core::membership::RecoveryChannelMessage::Reject(
                uc_core::membership::RecoveryReject {
                    space_lineage_fingerprint: request.space_lineage_fingerprint,
                    request_number: request.request_number,
                    reply_number: 0,
                    reason: uc_core::membership::RecoveryRejection::IdentityMismatch,
                },
            ));
        }
        if request.from_epoch >= state.current_epoch() {
            return Ok(uc_core::membership::RecoveryChannelMessage::Reject(
                uc_core::membership::RecoveryReject {
                    space_lineage_fingerprint: request.space_lineage_fingerprint,
                    request_number: request.request_number,
                    reply_number: 0,
                    reason: uc_core::membership::RecoveryRejection::RangeOutOfBounds,
                },
            ));
        }
        let batch = self.handoff_batch(&state, request.from_epoch);
        if batch.is_empty() {
            return Ok(uc_core::membership::RecoveryChannelMessage::Reject(
                uc_core::membership::RecoveryReject {
                    space_lineage_fingerprint: request.space_lineage_fingerprint,
                    request_number: request.request_number,
                    reply_number: 0,
                    reason: uc_core::membership::RecoveryRejection::ContinuityMissing,
                },
            ));
        }
        let digest = state.current_digest().ok_or_else(|| {
            uc_core::membership::RecoveryTransportError::Rejected(
                uc_core::membership::RecoveryRejection::DigestConflict,
            )
        })?;
        let from_epoch = request.from_epoch;
        let to_epoch = batch.last().map_or(from_epoch, |last| last.next_epoch);
        let has_more = self.has_more_after(&state, &batch);
        Ok(uc_core::membership::RecoveryChannelMessage::Offer(
            uc_core::membership::RecoveryOffer {
                space_lineage_fingerprint: request.space_lineage_fingerprint,
                request_number: request.request_number,
                reply_number: 1,
                from_epoch,
                to_epoch,
                has_more,
                target_digest: *digest.as_bytes(),
                changes: batch,
            },
        ))
    }

    async fn handle_recovery_ack(
        &self,
        source_device: &DeviceId,
        ack: uc_core::membership::RecoveryAck,
    ) -> Result<
        uc_core::membership::RecoveryChannelMessage,
        uc_core::membership::RecoveryTransportError,
    > {
        let state = self.load_state().await.map_err(|_| {
            uc_core::membership::RecoveryTransportError::Rejected(
                uc_core::membership::RecoveryRejection::Unauthorized,
            )
        })?;
        let recipient = state
            .member_devices
            .iter()
            .find_map(|(instance, device)| (device == source_device).then_some(*instance))
            .ok_or_else(|| {
                uc_core::membership::RecoveryTransportError::Rejected(
                    uc_core::membership::RecoveryRejection::Unauthorized,
                )
            })?;
        self.apply_handoff_progress(
            recipient,
            ack.confirmed_epoch,
            ack.target_digest,
            ack.has_more,
        )
        .await
        .map_err(|_| {
            uc_core::membership::RecoveryTransportError::Rejected(
                uc_core::membership::RecoveryRejection::DigestConflict,
            )
        })?;
        Ok(uc_core::membership::RecoveryChannelMessage::Ack(ack))
    }
}
