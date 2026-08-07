use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{broadcast, Notify};
use tracing::{info, warn};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    CandidateFailure, CandidateMergeError, CandidateMergeOutcome, CandidateStatus,
    CurrentMemberSignaturePort, CurrentMembershipAnnouncementPort, CurrentMembershipIdentityError,
    DeviceAnnouncement, MemberRepositoryPort, MembershipAnnouncementRepositoryError,
    MembershipAnnouncementRepositoryPort, MembershipAttestationPort,
    MembershipCandidateRepositoryError, MembershipCandidateRepositoryPort, MembershipEvent,
    MembershipEventBatch, MembershipGossipBoundsError, MembershipGossipEndpointError,
    MembershipGossipEndpointPort, MembershipGossipMessage, MembershipGossipTransportPort,
    MembershipOutboxRepositoryError, MembershipOutboxRepositoryPort, MembershipSecurityUpdateError,
    MembershipSecurityUpdatePort, MembershipSharedDevicePage, MembershipSharedDevicePageRequest,
    PendingGroupUpdate, PendingMembershipBatch, RelayedSecurityUpdate, SpaceMembershipCandidate,
    SponsorCandidateSeed, VerifiedPeerPromotionPort,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{ClockPort, ContentHashPort, DeviceIdentityPort, PeerAddressRepositoryPort};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;
use uuid::Uuid;

mod candidates;
mod exchange;
mod runtime;
mod shared_devices;

pub use runtime::{MembershipConvergenceActivity, MembershipConvergenceRuntime};

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
    shared_device_refresh: tokio::sync::Mutex<Option<ActiveSharedDeviceRefresh>>,
    shared_device_refresh_events: broadcast::Sender<SharedDeviceRefreshStatus>,
}

pub fn build_membership_convergence(deps: MembershipConvergenceDeps) -> Arc<MembershipConvergence> {
    Arc::new(MembershipConvergence::new(deps))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct MembershipGossipPassOutcome {
    pub delivered_batches: usize,
    pub confirmed_candidates: usize,
    pub synchronized_members: usize,
    pub deferred_items: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipConvergenceState {
    Complete,
    Converging,
    WaitingForUpgrade,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipConvergenceStatus {
    pub state: MembershipConvergenceState,
    pub pending_count: usize,
    pub waiting_for_peer_count: usize,
    pub waiting_for_update_count: usize,
    pub version_incompatible_count: usize,
    pub blocked_count: usize,
    pub rejected_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedDeviceRefreshPhase {
    Started,
    Discovering,
    Connecting,
    RoundCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedDeviceRefreshDeviceState {
    Discovered,
    Connecting,
    Connected,
    AlreadyPresent,
    WaitingForPeer,
    WaitingForUpdate,
    VersionIncompatible,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedDeviceRefreshDevice {
    pub device_id: uc_core::ids::DeviceId,
    pub device_name: String,
    pub state: SharedDeviceRefreshDeviceState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedDeviceRefreshStarted {
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedDeviceRefreshStatus {
    pub request_id: String,
    pub phase: SharedDeviceRefreshPhase,
    pub devices: Vec<SharedDeviceRefreshDevice>,
    pub total_count: usize,
    pub discovered_count: usize,
    pub connecting_count: usize,
    pub connected_count: usize,
    pub already_present_count: usize,
    pub waiting_for_peer_count: usize,
    pub waiting_for_update_count: usize,
    pub version_incompatible_count: usize,
    pub rejected_count: usize,
    pub unavailable_source_count: usize,
}

struct ActiveSharedDeviceRefresh {
    space_id: SpaceId,
    initial_round_active: bool,
    status: SharedDeviceRefreshStatus,
}

impl SharedDeviceRefreshStatus {
    fn new(request_id: String) -> Self {
        Self {
            request_id,
            phase: SharedDeviceRefreshPhase::Started,
            devices: Vec::new(),
            total_count: 0,
            discovered_count: 0,
            connecting_count: 0,
            connected_count: 0,
            already_present_count: 0,
            waiting_for_peer_count: 0,
            waiting_for_update_count: 0,
            version_incompatible_count: 0,
            rejected_count: 0,
            unavailable_source_count: 0,
        }
    }

    fn recount(&mut self) {
        self.total_count = self.devices.len();
        self.discovered_count = 0;
        self.connecting_count = 0;
        self.connected_count = 0;
        self.already_present_count = 0;
        self.waiting_for_peer_count = 0;
        self.waiting_for_update_count = 0;
        self.version_incompatible_count = 0;
        self.rejected_count = 0;
        for device in &self.devices {
            match device.state {
                SharedDeviceRefreshDeviceState::Discovered => {
                    self.discovered_count = self.discovered_count.saturating_add(1)
                }
                SharedDeviceRefreshDeviceState::Connecting => {
                    self.connecting_count = self.connecting_count.saturating_add(1)
                }
                SharedDeviceRefreshDeviceState::Connected => {
                    self.connected_count = self.connected_count.saturating_add(1)
                }
                SharedDeviceRefreshDeviceState::AlreadyPresent => {
                    self.already_present_count = self.already_present_count.saturating_add(1)
                }
                SharedDeviceRefreshDeviceState::WaitingForPeer => {
                    self.waiting_for_peer_count = self.waiting_for_peer_count.saturating_add(1)
                }
                SharedDeviceRefreshDeviceState::WaitingForUpdate => {
                    self.waiting_for_update_count = self.waiting_for_update_count.saturating_add(1)
                }
                SharedDeviceRefreshDeviceState::VersionIncompatible => {
                    self.version_incompatible_count =
                        self.version_incompatible_count.saturating_add(1)
                }
                SharedDeviceRefreshDeviceState::Rejected => {
                    self.rejected_count = self.rejected_count.saturating_add(1)
                }
            }
        }
    }
}

pub struct SponsorSeedBatchContext {
    pub space_id: SpaceId,
    pub sponsor_device_id: uc_core::ids::DeviceId,
    pub sponsor_transport_address_blob: Vec<u8>,
    pub joiner_device_id: uc_core::ids::DeviceId,
    pub joiner_device_name: String,
    pub joiner_identity_fingerprint: uc_core::security::IdentityFingerprint,
    pub joiner_transport_address_blob: Vec<u8>,
    pub group_epoch: u64,
    pub existing_member_updates: Vec<PendingGroupUpdate>,
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

#[async_trait]
pub trait PairingMembershipConvergencePort: Send + Sync {
    async fn prepare_sponsor_membership(
        &self,
        context: SponsorSeedBatchContext,
    ) -> Result<(), MembershipConvergenceError>;

    fn notify_pending_delivery(&self);
}

impl MembershipConvergence {
    fn new(deps: MembershipConvergenceDeps) -> Self {
        let (shared_device_refresh_events, _) = broadcast::channel(64);
        Self {
            deps,
            candidate_attempt_lock: tokio::sync::Mutex::new(()),
            wake: Arc::new(Notify::new()),
            shared_device_refresh: tokio::sync::Mutex::new(None),
            shared_device_refresh_events,
        }
    }

    async fn reconcile_once(
        &self,
    ) -> Result<MembershipGossipPassOutcome, MembershipConvergenceError> {
        let state = self.deps.security_updates.current_state().await?;
        let now_ms = self.deps.clock.now_ms();
        let delivered_batches = self.deliver_pending(&state.space_id, now_ms).await?;
        let mut outcome = MembershipGossipPassOutcome {
            delivered_batches,
            ..MembershipGossipPassOutcome::default()
        };

        let mut candidates = self.load_pending(&state.space_id).await?;
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
                    self.mark_shared_device_refresh_candidate_connected(
                        &state.space_id,
                        candidate.device_id(),
                    )
                    .await;
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

    async fn convergence_status(
        &self,
        space_id: &SpaceId,
    ) -> Result<MembershipConvergenceStatus, MembershipConvergenceError> {
        let candidates = self.deps.candidate_repo.list(space_id).await?;
        let outbox = self.deps.outbox_repo.list_pending(space_id).await?;
        let outbox_count = outbox.len();
        let waiting_for_peer_count = candidates
            .iter()
            .filter(|candidate| candidate.status() == CandidateStatus::WaitingForPeer)
            .count();
        let waiting_for_update_count = candidates
            .iter()
            .filter(|candidate| candidate.status() == CandidateStatus::WaitingForUpdate)
            .count();
        let version_incompatible_count = candidates
            .iter()
            .filter(|candidate| {
                candidate.last_failure() == Some(CandidateFailure::VersionIncompatible)
            })
            .count()
            .saturating_add(
                outbox
                    .iter()
                    .filter(|pending| {
                        pending.last_failure() == Some(CandidateFailure::VersionIncompatible)
                    })
                    .count(),
            );
        let rejected_count = candidates
            .iter()
            .filter(|candidate| candidate.status() == CandidateStatus::Rejected)
            .count();
        let blocked_count = candidates
            .iter()
            .filter(|candidate| {
                candidate.status() == CandidateStatus::Blocked
                    && candidate.last_failure() != Some(CandidateFailure::VersionIncompatible)
            })
            .count();
        let pending_candidates = candidates
            .iter()
            .filter(|candidate| candidate.status() != CandidateStatus::Ready)
            .count();
        let pending_count = pending_candidates.saturating_add(outbox_count);
        let state = if blocked_count > 0 || rejected_count > 0 {
            MembershipConvergenceState::Blocked
        } else if version_incompatible_count > 0 {
            MembershipConvergenceState::WaitingForUpgrade
        } else if pending_count > 0 {
            MembershipConvergenceState::Converging
        } else {
            MembershipConvergenceState::Complete
        };
        Ok(MembershipConvergenceStatus {
            state,
            pending_count,
            waiting_for_peer_count,
            waiting_for_update_count,
            version_incompatible_count,
            blocked_count,
            rejected_count,
        })
    }

    pub(crate) async fn current_convergence_status(
        &self,
    ) -> Result<MembershipConvergenceStatus, MembershipConvergenceError> {
        let material = self
            .deps
            .announcement_material
            .current_announcement_material()
            .await?;
        self.convergence_status(&material.space_id).await
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

fn should_persist_merge(outcome: CandidateMergeOutcome) -> bool {
    matches!(
        outcome,
        CandidateMergeOutcome::Updated
            | CandidateMergeOutcome::IdentityConflict
            | CandidateMergeOutcome::AnnouncementConflict
            | CandidateMergeOutcome::SecurityHistoryConflict
    )
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
mod testing;
