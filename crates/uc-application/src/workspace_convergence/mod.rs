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

mod runtime;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tracing::info;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignaturePort, CurrentMembershipIdentityPort, MemberRepositoryPort,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort, RemovalIntentContent,
    RemovalIntentVerificationError, RemovalIntentVerificationPort, RemovalRecoveryError,
    RemovalRecoveryPort, SignedRemovalIntent, WorkspaceChange, WorkspaceChangeKind,
    WorkspaceConfirmation, WorkspaceConvergenceEvent, WorkspaceConvergenceRepositoryError,
    WorkspaceConvergenceRepositoryPort, WorkspaceConvergenceState, WorkspaceMergeOutcome,
    WorkspacePhase, WorkspaceSnapshot,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort};

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
    pub security_updates: Arc<dyn MembershipSecurityUpdatePort>,
    pub clock: Arc<dyn ClockPort>,
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    pub own_device: DeviceId,
}

/// The unified workspace convergence owner.
pub struct WorkspaceConvergence {
    deps: WorkspaceConvergenceDeps,
    state_lock: tokio::sync::Mutex<()>,
    wake: Arc<tokio::sync::Notify>,
    events: broadcast::Sender<WorkspaceSnapshot>,
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
        state.member_devices.iter().any(|(instance, device)| {
            device == device_id && !state.effective_members().contains(instance)
        })
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

        let to_remove = state
            .record_removal_intent(&intent, now_ms)
            .map_err(|_| WorkspaceConvergenceError::Inconsistent("intent rejected".to_owned()))?;
        if to_remove.is_empty() {
            // The intent did not change the effective membership (target
            // already covered by an earlier intent); keep the saved state.
            self.persist(&state).await?;
            self.notify();
            return Ok(state.snapshot());
        }
        let removed_instances = to_remove.iter().copied().collect::<Vec<_>>();
        let change = self.build_removal_change(&state, &removed_instances, now_ms)?;
        self.apply_and_publish_change(&mut state, change, now_ms)
            .await?;
        self.notify();
        info!(
            removed_count = removed_instances.len(),
            "workspace removal change recorded"
        );
        Ok(state.snapshot())
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

    /// One reconcile pass: save pending handoff records for members that
    /// have not yet confirmed the current digest. Actual network transfer
    /// is performed by the recovery transport; this pass only bookkeeps.
    pub async fn reconcile(&self) -> Result<(), WorkspaceConvergenceError> {
        let now_ms = self.deps.clock.now_ms();
        let mut state = self.load_state().await?;
        if state.phase.is_terminal() {
            return Ok(());
        }
        let digest = match state.current_digest() {
            Some(digest) => digest,
            None => return Ok(()),
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
        if changed {
            self.persist(&state).await?;
        }
        Ok(())
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
