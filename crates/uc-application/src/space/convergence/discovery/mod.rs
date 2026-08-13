use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use tracing::info;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    CandidateEvent, CandidateFailure, CandidateMergeError, CandidateMergeOutcome, CandidateStatus,
    CurrentMemberSignaturePort, CurrentMembershipAnnouncementPort, CurrentMembershipIdentityError,
    DeviceAnnouncement, MemberRepositoryPort, MembershipAnnouncementRepositoryError,
    MembershipAnnouncementRepositoryPort, MembershipAppliedSecurityUpdateRepositoryError,
    MembershipAppliedSecurityUpdateRepositoryPort, MembershipAttestationPort,
    MembershipCandidateRepositoryError, MembershipCandidateRepositoryPort, MembershipEventBatch,
    MembershipGossipBoundsError, MembershipGossipEndpointError, MembershipGossipEndpointPort,
    MembershipGossipEvent, MembershipGossipMessage, MembershipGossipTransportPort,
    MembershipOutboxRepositoryError, MembershipOutboxRepositoryPort, MembershipSecurityUpdateError,
    MembershipSecurityUpdatePort, MembershipSharedDevicePage, MembershipSharedDevicePageRequest,
    PendingMembershipBatch, SpaceMembershipCandidate, SponsorCandidateSeed,
    VerifiedPeerPromotionPort,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{ClockPort, ContentHashPort, DeviceIdentityPort, PeerAddressRepositoryPort};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;

use crate::space::convergence::group_update_delivery::GroupUpdateDeliveryPort;

mod candidates;
mod exchange;
mod runtime;

pub use runtime::{MembershipConvergenceActivity, MembershipConvergenceRuntime};

#[async_trait]
pub(crate) trait MembershipConvergenceActivityPort: Send + Sync {
    async fn pause(&self) -> Result<(), String>;
    async fn resume(&self) -> Result<(), String>;
}

const INITIAL_RETRY_DELAY_MS: i64 = 30_000;
const MAX_RETRY_DELAY_MS: i64 = 5 * 60 * 1_000;
const DIRECT_ATTESTATION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const ANNOUNCEMENT_REFRESH_LEAD_MS: i64 = 24 * 60 * 60 * 1_000;
const GOSSIP_RECONCILE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const GOSSIP_RECONCILE_JITTER_WINDOW: Duration = Duration::from_secs(60);
const MIN_SCHEDULED_RECONCILE_DELAY: Duration = Duration::from_millis(100);

pub struct MembershipConvergenceDeps {
    pub candidate_repo: Arc<dyn MembershipCandidateRepositoryPort>,
    pub announcement_repo: Arc<dyn MembershipAnnouncementRepositoryPort>,
    pub outbox_repo: Arc<dyn MembershipOutboxRepositoryPort>,
    pub security_updates: Arc<dyn MembershipSecurityUpdatePort>,
    pub applied_security_updates: Arc<dyn MembershipAppliedSecurityUpdateRepositoryPort>,
    pub transport: Arc<dyn MembershipGossipTransportPort>,
    pub clock: Arc<dyn ClockPort>,
    pub device_identity: Arc<dyn DeviceIdentityPort>,
    pub announcement_material: Arc<dyn CurrentMembershipAnnouncementPort>,
    pub member_signatures: Arc<dyn CurrentMemberSignaturePort>,
    pub fingerprint_factory: Arc<dyn IdentityFingerprintFactoryPort>,
    pub attestation: Arc<dyn MembershipAttestationPort>,
    pub verified_peer_promotion: Arc<dyn VerifiedPeerPromotionPort>,
    pub member_repo: Arc<dyn MemberRepositoryPort>,
    pub trusted_peer_repo: Arc<dyn TrustedPeerRepositoryPort>,
    pub peer_address_repo: Arc<dyn PeerAddressRepositoryPort>,
    pub hash: Arc<dyn ContentHashPort>,
}

pub struct MembershipConvergence {
    deps: MembershipConvergenceDeps,
    candidate_attempt_lock: tokio::sync::Mutex<()>,
    wake: Arc<Notify>,
    group_update_delivery: std::sync::Mutex<Option<Arc<dyn GroupUpdateDeliveryPort>>>,
}

pub fn build_membership_convergence(deps: MembershipConvergenceDeps) -> Arc<MembershipConvergence> {
    Arc::new(MembershipConvergence::new(deps))
}

impl MembershipConvergence {
    pub fn install_group_update_delivery(&self, delivery: Arc<dyn GroupUpdateDeliveryPort>) {
        *self
            .group_update_delivery
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(delivery);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MembershipGossipPassOutcome {
    pub delivered_batches: usize,
    pub confirmed_candidates: usize,
    pub synchronized_members: usize,
    pub deferred_items: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum MembershipConvergenceError {
    #[error("membership candidate was invalid: {0}")]
    InvalidCandidate(#[from] CandidateMergeError),
    #[error("membership candidate storage failed: {0}")]
    Storage(#[from] MembershipCandidateRepositoryError),
    #[error("membership announcement storage failed: {0}")]
    AnnouncementStorage(#[from] MembershipAnnouncementRepositoryError),
    #[error("membership outbox storage failed: {0}")]
    Outbox(#[from] MembershipOutboxRepositoryError),
    #[error("membership delivery batch is invalid: {0}")]
    DeliveryBatch(#[from] MembershipGossipBoundsError),
    #[error("membership security update failed: {0}")]
    SecurityUpdate(#[from] MembershipSecurityUpdateError),
    #[error("membership applied update storage failed: {0}")]
    AppliedSecurityUpdate(#[from] MembershipAppliedSecurityUpdateRepositoryError),
    #[error("membership candidate was not found")]
    CandidateNotFound,
    #[error("membership peer is unavailable")]
    PeerUnavailable,
    #[error("membership peer is waiting for a security update")]
    WaitingForUpdate,
    #[error("membership verification was rejected")]
    VerificationRejected,
    #[error("membership relationship persistence failed: {0}")]
    Relationship(String),
    #[error("current membership identity is unavailable")]
    CurrentIdentity(#[from] CurrentMembershipIdentityError),
}

impl MembershipConvergence {
    fn new(deps: MembershipConvergenceDeps) -> Self {
        Self {
            deps,
            candidate_attempt_lock: tokio::sync::Mutex::new(()),
            wake: Arc::new(Notify::new()),
            group_update_delivery: std::sync::Mutex::new(None),
        }
    }

    async fn reconcile_once(
        &self,
    ) -> Result<MembershipGossipPassOutcome, MembershipConvergenceError> {
        let group_update_delivery = self
            .group_update_delivery
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(delivery) = group_update_delivery {
            if let Err(error) = delivery.deliver_pending(self.deps.clock.now_ms()).await {
                tracing::warn!(error = %error, "pending group updates could not be delivered during membership reconciliation");
            }
        }
        let state = self.deps.security_updates.current_state().await?;
        let now_ms = self.deps.clock.now_ms();
        let delivered_batches = self.deliver_pending(&state.space_id, now_ms).await?;
        let mut outcome = MembershipGossipPassOutcome {
            delivered_batches,
            ..MembershipGossipPassOutcome::default()
        };

        let mut candidates = self.load_pending(&state.space_id).await?;
        let waiting_for_update = candidates
            .iter()
            .any(|candidate| candidate.status() == CandidateStatus::WaitingForUpdate);
        if waiting_for_update {
            self.pull_updates_from_connected_members(&mut outcome)
                .await?;
            candidates = self.load_pending(&state.space_id).await?;
        }
        candidates.sort_by(|left, right| left.device_id().as_str().cmp(right.device_id().as_str()));
        for candidate in candidates {
            let deferred = match candidate.next_attempt_at_ms() {
                Some(next_attempt) => next_attempt > now_ms,
                None => candidate.status() != CandidateStatus::Verifying,
            };
            if deferred {
                outcome.deferred_items = outcome.deferred_items.saturating_add(1);
                continue;
            }
            match self
                .confirm_candidate(&state.space_id, candidate.device_id())
                .await
            {
                Ok(()) => {
                    outcome.confirmed_candidates = outcome.confirmed_candidates.saturating_add(1);
                }
                Err(
                    MembershipConvergenceError::PeerUnavailable
                    | MembershipConvergenceError::WaitingForUpdate
                    | MembershipConvergenceError::VerificationRejected,
                ) => {
                    outcome.deferred_items = outcome.deferred_items.saturating_add(1);
                }
                Err(error) => return Err(error),
            }
        }

        let local_device_id = self.deps.device_identity.current_device_id();
        let mut members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        members.sort_by(|left, right| left.device_id.as_str().cmp(right.device_id.as_str()));
        for member in members
            .into_iter()
            .filter(|member| member.device_id != local_device_id)
        {
            match self.synchronize_member(&member.device_id).await {
                Ok(()) => {
                    outcome.synchronized_members = outcome.synchronized_members.saturating_add(1);
                }
                Err(
                    MembershipConvergenceError::PeerUnavailable
                    | MembershipConvergenceError::VerificationRejected,
                ) => {
                    outcome.deferred_items = outcome.deferred_items.saturating_add(1);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(outcome)
    }

    /// Ask every connected member for security updates the local device is
    /// still missing, so `WaitingForUpdate` candidates can resume without
    /// depending on the provider's own push schedule.
    async fn pull_updates_from_connected_members(
        &self,
        outcome: &mut MembershipGossipPassOutcome,
    ) -> Result<(), MembershipConvergenceError> {
        let local_device_id = self.deps.device_identity.current_device_id();
        let mut members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| MembershipConvergenceError::Relationship(error.to_string()))?;
        members.sort_by(|left, right| left.device_id.as_str().cmp(right.device_id.as_str()));
        for member in members
            .into_iter()
            .filter(|member| member.device_id != local_device_id)
        {
            match self.pull_security_updates(&member.device_id).await {
                Ok(applied) if applied > 0 => {
                    info!(
                        provider_device_id = %member.device_id.as_str(),
                        applied_updates = applied,
                        "membership security updates pulled from connected member"
                    );
                }
                Ok(_) => {}
                Err(
                    MembershipConvergenceError::PeerUnavailable
                    | MembershipConvergenceError::VerificationRejected,
                ) => {
                    outcome.deferred_items = outcome.deferred_items.saturating_add(1);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    async fn next_reconcile_delay(&self) -> Duration {
        let fallback = gossip_reconcile_delay(&self.deps.device_identity.current_device_id());
        let state = match self.deps.security_updates.current_state().await {
            Ok(state) => state,
            Err(_) => return fallback,
        };
        let candidates = match self.deps.candidate_repo.list(&state.space_id).await {
            Ok(candidates) => candidates,
            Err(_) => return fallback,
        };
        let outbox = match self.deps.outbox_repo.list_pending(&state.space_id).await {
            Ok(outbox) => outbox,
            Err(_) => return fallback,
        };
        let next_attempt_at_ms = candidates
            .iter()
            .filter(|candidate| is_pending(candidate.status()))
            .filter_map(|candidate| {
                candidate.next_attempt_at_ms().or_else(|| {
                    (candidate.status() == CandidateStatus::Verifying)
                        .then_some(self.deps.clock.now_ms())
                })
            })
            .chain(
                outbox
                    .iter()
                    .map(PendingMembershipBatch::next_attempt_at_ms),
            )
            .min();
        let Some(next_attempt_at_ms) = next_attempt_at_ms else {
            return fallback;
        };
        let remaining_ms = next_attempt_at_ms.saturating_sub(self.deps.clock.now_ms());
        let scheduled = Duration::from_millis(u64::try_from(remaining_ms).unwrap_or(u64::MAX));
        scheduled.max(MIN_SCHEDULED_RECONCILE_DELAY).min(fallback)
    }
}

fn gossip_reconcile_delay(device_id: &uc_core::ids::DeviceId) -> Duration {
    let jitter_window_ms = GOSSIP_RECONCILE_JITTER_WINDOW.as_millis() as u64;
    let jitter_seed = device_id.as_str().bytes().fold(0u64, |sum, byte| {
        sum.wrapping_mul(31).wrapping_add(u64::from(byte))
    });
    GOSSIP_RECONCILE_INTERVAL
        .saturating_add(Duration::from_millis(jitter_seed % jitter_window_ms.max(1)))
}

fn is_pending(status: CandidateStatus) -> bool {
    matches!(
        status,
        CandidateStatus::Pending
            | CandidateStatus::WaitingForPeer
            | CandidateStatus::WaitingForUpdate
            | CandidateStatus::Verifying
    )
}

#[cfg(test)]
pub(crate) mod testing;
