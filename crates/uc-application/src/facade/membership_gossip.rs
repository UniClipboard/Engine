use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{TimeZone, Utc};
use tokio::sync::{broadcast, mpsc, oneshot, Notify};
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    validate_sponsor_candidate_seed_batch, CandidateFailure, CandidateMergeError,
    CandidateMergeOutcome, CandidateStatus, CurrentMemberSignaturePort,
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentityError, DeviceAnnouncement,
    MemberRepositoryPort, MemberSyncPreferences, MembershipAnnouncementRepositoryError,
    MembershipAnnouncementRepositoryPort, MembershipAttestationEndpointError,
    MembershipAttestationEndpointPort, MembershipAttestationError, MembershipAttestationPort,
    MembershipCandidateRepositoryError, MembershipCandidateRepositoryPort, MembershipEvent,
    MembershipEventBatch, MembershipGossipEndpointError, MembershipGossipEndpointPort,
    MembershipGossipMessage, MembershipGossipTransportPort, MembershipOutboxRepositoryError,
    MembershipOutboxRepositoryPort, MembershipSecurityUpdateError, MembershipSecurityUpdatePort,
    PendingGroupUpdate, PendingMembershipBatch, RelayedSecurityUpdate, SpaceMember,
    SpaceMembershipCandidate, SponsorCandidateSeed, VerifiedMembershipPeer,
    VerifiedPeerPromotionPort,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{
    ClockPort, ContentHashPort, DeviceIdentityPort, PeerAddressRecord, PeerAddressRepositoryPort,
};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;
use uc_core::TrustedPeer;

const INITIAL_RETRY_DELAY_MS: i64 = 30_000;
const MAX_RETRY_DELAY_MS: i64 = 5 * 60 * 1_000;
const DIRECT_ATTESTATION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;
const ANNOUNCEMENT_REFRESH_LEAD_MS: i64 = 24 * 60 * 60 * 1_000;
const GOSSIP_RECONCILE_INTERVAL: Duration = Duration::from_secs(5 * 60);
const GOSSIP_RECONCILE_JITTER_WINDOW: Duration = Duration::from_secs(60);
const MIN_SCHEDULED_RECONCILE_DELAY: Duration = Duration::from_millis(100);

pub struct SpaceMembershipGossipDeps {
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

pub struct SpaceMembershipGossip {
    deps: SpaceMembershipGossipDeps,
    candidate_attempt_lock: tokio::sync::Mutex<()>,
    wake: Arc<Notify>,
}

pub fn build_space_membership_gossip(
    deps: SpaceMembershipGossipDeps,
) -> Arc<SpaceMembershipGossip> {
    Arc::new(SpaceMembershipGossip::new(deps))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MembershipGossipPassOutcome {
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

enum MembershipGossipRuntimeCommand {
    Pause(oneshot::Sender<()>),
    Resume(oneshot::Sender<()>),
    Shutdown(oneshot::Sender<()>),
}

#[derive(Debug, thiserror::Error)]
pub enum MembershipGossipRuntimeError {
    #[error("membership gossip runtime is stopped")]
    Stopped,
}

pub struct SpaceMembershipGossipRuntime {
    activity: SpaceMembershipGossipActivity,
    task: Option<JoinHandle<()>>,
}

#[derive(Clone)]
pub struct SpaceMembershipGossipActivity {
    commands: mpsc::UnboundedSender<MembershipGossipRuntimeCommand>,
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
pub enum SpaceMembershipGossipError {
    #[error("membership candidate was invalid: {0}")]
    InvalidCandidate(#[from] CandidateMergeError),
    #[error("membership candidate storage failed: {0}")]
    Storage(#[from] MembershipCandidateRepositoryError),
    #[error("membership announcement storage failed: {0}")]
    AnnouncementStorage(#[from] MembershipAnnouncementRepositoryError),
    #[error("membership outbox storage failed: {0}")]
    Outbox(#[from] MembershipOutboxRepositoryError),
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
pub trait PairingMembershipGossipPort: Send + Sync {
    async fn prepare_sponsor_membership(
        &self,
        context: SponsorSeedBatchContext,
    ) -> Result<Vec<SponsorCandidateSeed>, SpaceMembershipGossipError>;

    async fn accept_sponsor_seed_batch(
        &self,
        seeds: Vec<SponsorCandidateSeed>,
    ) -> Result<(), SpaceMembershipGossipError>;
}

impl SpaceMembershipGossip {
    pub fn new(deps: SpaceMembershipGossipDeps) -> Self {
        Self {
            deps,
            candidate_attempt_lock: tokio::sync::Mutex::new(()),
            wake: Arc::new(Notify::new()),
        }
    }

    pub async fn accept_sponsor_seed(
        &self,
        seed: SponsorCandidateSeed,
    ) -> Result<CandidateMergeOutcome, SpaceMembershipGossipError> {
        let now_ms = self.deps.clock.now_ms();
        self.deps
            .candidate_repo
            .purge_expired(&seed.space_id, now_ms)
            .await?;

        let formal_member = self
            .deps
            .member_repo
            .get(&seed.device_id)
            .await
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
        if let Some(member) = formal_member.as_ref() {
            if member.identity_fingerprint != seed.identity_fingerprint_hint {
                return Err(SpaceMembershipGossipError::VerificationRejected);
            }
        }

        let existing = self
            .deps
            .candidate_repo
            .get(&seed.space_id, &seed.device_id)
            .await?;
        if let Some(member) = formal_member {
            return match existing {
                Some(mut candidate) => {
                    if candidate.identity_fingerprint_hint() != &member.identity_fingerprint {
                        return Err(SpaceMembershipGossipError::VerificationRejected);
                    }
                    let outcome = candidate.merge_sponsor_seed(seed, now_ms)?;
                    candidate.mark_ready(now_ms);
                    self.deps.candidate_repo.save(&candidate).await?;
                    Ok(outcome)
                }
                None => {
                    SpaceMembershipCandidate::from_sponsor_seed(seed, now_ms)?;
                    Ok(CandidateMergeOutcome::Unchanged)
                }
            };
        }
        match existing {
            Some(mut candidate) => {
                let outcome = candidate.merge_sponsor_seed(seed, now_ms)?;
                if should_persist_merge(outcome) {
                    self.deps.candidate_repo.save(&candidate).await?;
                }
                self.wake.notify_one();
                Ok(outcome)
            }
            None => {
                let candidate = SpaceMembershipCandidate::from_sponsor_seed(seed, now_ms)?;
                self.deps.candidate_repo.save(&candidate).await?;
                self.wake.notify_one();
                Ok(CandidateMergeOutcome::Updated)
            }
        }
    }

    pub async fn refresh_local_announcement(
        &self,
    ) -> Result<DeviceAnnouncement, SpaceMembershipGossipError> {
        let material = self
            .deps
            .announcement_material
            .current_announcement_material()
            .await
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
        let state = self.deps.security_updates.current_state().await?;
        if material.space_id != state.space_id
            || material.device_id != self.deps.device_identity.current_device_id()
        {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }
        let derived_fingerprint = self
            .deps
            .fingerprint_factory
            .from_public_key(&material.transport_public_key)
            .map_err(|_| SpaceMembershipGossipError::VerificationRejected)?;
        if derived_fingerprint != material.identity_fingerprint {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }
        let now_ms = self.deps.clock.now_ms();
        let existing = self
            .deps
            .announcement_repo
            .get(&state.space_id, &material.device_id)
            .await?;
        if let Some(existing) = existing.as_ref() {
            let unchanged = existing.device_name == material.device_name
                && existing.identity_fingerprint == material.identity_fingerprint
                && existing.transport_public_key == material.transport_public_key
                && existing.transport_address_blob == material.transport_address_blob
                && existing.group_epoch == state.group_epoch;
            if unchanged
                && existing.expires_at_ms > now_ms.saturating_add(ANNOUNCEMENT_REFRESH_LEAD_MS)
            {
                return Ok(existing.clone());
            }
        }
        let sequence = existing
            .as_ref()
            .map(|announcement| announcement.sequence.saturating_add(1))
            .unwrap_or(1);
        let mut announcement = DeviceAnnouncement {
            space_id: state.space_id,
            device_id: material.device_id,
            device_name: material.device_name,
            identity_fingerprint: material.identity_fingerprint,
            transport_public_key: material.transport_public_key,
            transport_address_blob: material.transport_address_blob,
            sequence,
            group_epoch: state.group_epoch,
            expires_at_ms: now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS),
            content_digest: [0; 32],
            signature: Vec::new(),
        };
        announcement.content_digest = self
            .deps
            .hash
            .hash_bytes(&announcement.content_bytes())
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?
            .bytes;
        announcement.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&announcement.signing_payload())
            .await
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
        MembershipEventBatch {
            space_id: announcement.space_id.clone(),
            batch_id: announcement.content_digest,
            events: vec![MembershipEvent::Announcement(announcement.clone())],
        }
        .validate_transfer_bounds()
        .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
        self.deps.announcement_repo.save(&announcement).await?;
        Ok(announcement)
    }

    pub(crate) async fn accept_sponsor_seed_batch(
        &self,
        seeds: Vec<SponsorCandidateSeed>,
    ) -> Result<(), SpaceMembershipGossipError> {
        validate_sponsor_candidate_seed_batch(&seeds)?;
        for seed in seeds {
            self.accept_sponsor_seed(seed).await?;
        }
        Ok(())
    }

    #[cfg(test)]
    async fn build_sponsor_seed_batch(
        &self,
        context: SponsorSeedBatchContext,
    ) -> Result<Vec<SponsorCandidateSeed>, SpaceMembershipGossipError> {
        self.build_sponsor_seed_batch_inner(&context).await
    }

    async fn build_sponsor_seed_batch_inner(
        &self,
        context: &SponsorSeedBatchContext,
    ) -> Result<Vec<SponsorCandidateSeed>, SpaceMembershipGossipError> {
        let previous_epoch = context.group_epoch.checked_sub(1).ok_or_else(|| {
            SpaceMembershipGossipError::Relationship("invalid group epoch".into())
        })?;
        let now_ms = self.deps.clock.now_ms();
        let expires_at_ms = now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS);
        let mut members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
        members.retain(|member| member.device_id != context.joiner_device_id);
        members.sort_by(|left, right| left.device_id.as_str().cmp(right.device_id.as_str()));

        let mut seeds = Vec::with_capacity(members.len());
        for member in members {
            let (transport_address_blob, address_observed_at_ms) = if member.device_id
                == context.sponsor_device_id
            {
                if context.sponsor_transport_address_blob.is_empty() {
                    continue;
                }
                (context.sponsor_transport_address_blob.clone(), now_ms)
            } else {
                let address = self
                    .deps
                    .peer_address_repo
                    .get(&member.device_id)
                    .await
                    .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?
                    .ok_or_else(|| {
                        SpaceMembershipGossipError::Relationship(
                            "current member address is unavailable".into(),
                        )
                    })?;
                (address.addr_blob, address.observed_at.timestamp_millis())
            };
            let security_updates = context
                .existing_member_updates
                .iter()
                .filter(|update| update.recipient() == &member.device_id)
                .map(|update| {
                    let digest = self
                        .deps
                        .hash
                        .hash_bytes(update.payload())
                        .map_err(|error| {
                            SpaceMembershipGossipError::Relationship(error.to_string())
                        })?;
                    Ok(RelayedSecurityUpdate {
                        previous_epoch,
                        next_epoch: context.group_epoch,
                        payload: update.payload().to_vec(),
                        digest: digest.bytes,
                    })
                })
                .collect::<Result<Vec<_>, SpaceMembershipGossipError>>()?;
            seeds.push(SponsorCandidateSeed {
                space_id: context.space_id.clone(),
                device_id: member.device_id,
                device_name_hint: member.device_name,
                identity_fingerprint_hint: member.identity_fingerprint,
                transport_address_blob,
                address_observed_at_ms,
                source_device_id: context.sponsor_device_id,
                security_updates,
                expires_at_ms,
            });
        }
        validate_sponsor_candidate_seed_batch(&seeds)?;
        Ok(seeds)
    }

    pub(crate) async fn prepare_sponsor_membership(
        &self,
        context: SponsorSeedBatchContext,
    ) -> Result<Vec<SponsorCandidateSeed>, SpaceMembershipGossipError> {
        let seeds = self.build_sponsor_seed_batch_inner(&context).await?;
        let now_ms = self.deps.clock.now_ms();
        let expires_at_ms = now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS);
        let mut persisted_outbox_count = 0usize;
        let mut failed_outbox_count = 0usize;
        for recipient in seeds
            .iter()
            .filter(|seed| seed.device_id != context.sponsor_device_id)
        {
            let joiner_seed = SponsorCandidateSeed {
                space_id: context.space_id.clone(),
                device_id: context.joiner_device_id,
                device_name_hint: context.joiner_device_name.clone(),
                identity_fingerprint_hint: context.joiner_identity_fingerprint.clone(),
                transport_address_blob: context.joiner_transport_address_blob.clone(),
                address_observed_at_ms: now_ms,
                source_device_id: context.sponsor_device_id,
                security_updates: recipient.security_updates.clone(),
                expires_at_ms,
            };
            let event = MembershipEvent::SponsorSeed(joiner_seed);
            let batch_id_input = serde_json::to_vec(&(recipient.device_id.as_str(), &event))
                .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
            let batch_id = self
                .deps
                .hash
                .hash_bytes(&batch_id_input)
                .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?
                .bytes;
            let pending = PendingMembershipBatch::new(
                recipient.device_id,
                MembershipEventBatch {
                    space_id: context.space_id.clone(),
                    batch_id,
                    events: vec![event],
                },
                now_ms,
            )
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
            match self.deps.outbox_repo.save(&pending).await {
                Ok(()) => {
                    persisted_outbox_count = persisted_outbox_count.saturating_add(1);
                }
                Err(_) => {
                    // The joiner receives the same existing-member seeds in Confirm and
                    // can therefore drive convergence even when this redundant sponsor
                    // delivery path is temporarily unavailable. Failing the pairing here
                    // would be worse: the group admission is already durable.
                    failed_outbox_count = failed_outbox_count.saturating_add(1);
                }
            }
        }
        if persisted_outbox_count > 0 {
            self.wake.notify_one();
        }
        if failed_outbox_count > 0 {
            warn!(
                failed_outbox_count,
                retry_via_joiner = true,
                "membership sponsor delivery could not be persisted"
            );
        }
        Ok(seeds)
    }

    pub async fn load_pending(
        &self,
        space_id: &SpaceId,
    ) -> Result<Vec<SpaceMembershipCandidate>, SpaceMembershipGossipError> {
        let now_ms = self.deps.clock.now_ms();
        self.deps
            .candidate_repo
            .purge_expired(space_id, now_ms)
            .await?;
        Ok(self
            .deps
            .candidate_repo
            .list(space_id)
            .await?
            .into_iter()
            .filter(|candidate| is_pending(candidate.status()))
            .collect())
    }

    async fn accept_verified_announcement(
        &self,
        announcement: DeviceAnnouncement,
    ) -> Result<CandidateMergeOutcome, SpaceMembershipGossipError> {
        let state = self.deps.security_updates.current_state().await?;
        if state.space_id != announcement.space_id || state.group_epoch != announcement.group_epoch
        {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }
        let digest = self
            .deps
            .hash
            .hash_bytes(&announcement.content_bytes())
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
        if digest.bytes != announcement.content_digest {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }
        let fingerprint = self
            .deps
            .fingerprint_factory
            .from_public_key(&announcement.transport_public_key)
            .map_err(|_| SpaceMembershipGossipError::VerificationRejected)?;
        if fingerprint != announcement.identity_fingerprint {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }
        let valid = self
            .deps
            .member_signatures
            .verify_current_member_payload(
                &announcement.device_id,
                &announcement.signing_payload(),
                &announcement.signature,
            )
            .await
            .map_err(|_| SpaceMembershipGossipError::VerificationRejected)?;
        if !valid {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }

        let now_ms = self.deps.clock.now_ms();
        let formal_member = self
            .deps
            .member_repo
            .get(&announcement.device_id)
            .await
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
        if let Some(member) = formal_member.as_ref() {
            if member.identity_fingerprint != announcement.identity_fingerprint {
                return Err(SpaceMembershipGossipError::VerificationRejected);
            }
        }
        let existing_announcement = self
            .deps
            .announcement_repo
            .get(&announcement.space_id, &announcement.device_id)
            .await?;
        if let Some(existing) = existing_announcement {
            if announcement.sequence < existing.sequence {
                return Ok(CandidateMergeOutcome::Stale);
            }
            if announcement.sequence == existing.sequence {
                if announcement.content_digest == existing.content_digest {
                    return Ok(CandidateMergeOutcome::Unchanged);
                }
                return Err(SpaceMembershipGossipError::VerificationRejected);
            }
        }

        let existing_candidate = self
            .deps
            .candidate_repo
            .get(&announcement.space_id, &announcement.device_id)
            .await?;
        if let Some(member) = formal_member {
            return match existing_candidate {
                Some(mut candidate) => {
                    if candidate.identity_fingerprint_hint() != &member.identity_fingerprint {
                        return Err(SpaceMembershipGossipError::VerificationRejected);
                    }
                    let outcome =
                        candidate.merge_verified_announcement(announcement.clone(), now_ms)?;
                    candidate.mark_ready(now_ms);
                    if should_persist_merge(outcome) {
                        self.deps.announcement_repo.save(&announcement).await?;
                    }
                    self.deps.candidate_repo.save(&candidate).await?;
                    Ok(outcome)
                }
                None => {
                    self.deps.announcement_repo.save(&announcement).await?;
                    Ok(CandidateMergeOutcome::Updated)
                }
            };
        }
        let (candidate, outcome) = match existing_candidate {
            Some(mut candidate) => {
                let outcome =
                    candidate.merge_verified_announcement(announcement.clone(), now_ms)?;
                (candidate, outcome)
            }
            None => (
                SpaceMembershipCandidate::from_verified_announcement(announcement.clone(), now_ms)?,
                CandidateMergeOutcome::Updated,
            ),
        };
        if should_persist_merge(outcome) {
            self.deps.announcement_repo.save(&announcement).await?;
            self.deps.candidate_repo.save(&candidate).await?;
        }
        Ok(outcome)
    }

    async fn request_for_digest(
        &self,
        digest: uc_core::membership::MembershipDigest,
    ) -> Result<MembershipGossipMessage, MembershipGossipEndpointError> {
        let state = self
            .deps
            .security_updates
            .current_state()
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        if state.space_id != digest.space_id {
            return Err(MembershipGossipEndpointError::Rejected);
        }
        let local = self
            .deps
            .announcement_repo
            .list(&digest.space_id)
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        let mut requested = digest
            .announcements
            .into_iter()
            .filter(|remote| {
                local
                    .iter()
                    .find(|known| known.device_id == remote.device_id)
                    .map(|known| {
                        remote.sequence > known.sequence
                            || (remote.sequence == known.sequence
                                && remote.content_digest != known.content_digest)
                    })
                    .unwrap_or(true)
            })
            .map(|remote| remote.device_id)
            .collect::<Vec<_>>();
        requested.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        requested.dedup();
        Ok(MembershipGossipMessage::RequestMissing(
            uc_core::membership::MembershipRequestMissing {
                space_id: digest.space_id,
                announcement_devices: requested,
                security_updates_after_epoch: (digest.group_epoch > state.group_epoch)
                    .then_some(state.group_epoch),
            },
        ))
    }

    async fn events_for_request(
        &self,
        request: uc_core::membership::MembershipRequestMissing,
    ) -> Result<MembershipGossipMessage, MembershipGossipEndpointError> {
        let state = self
            .deps
            .security_updates
            .current_state()
            .await
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        if state.space_id != request.space_id {
            return Err(MembershipGossipEndpointError::Rejected);
        }
        let mut events = Vec::new();
        for device_id in request.announcement_devices {
            if let Some(announcement) = self
                .deps
                .announcement_repo
                .get(&request.space_id, &device_id)
                .await
                .map_err(|_| MembershipGossipEndpointError::Persistence)?
            {
                events.push(MembershipEvent::Announcement(announcement));
            }
        }
        if let Some(epoch) = request.security_updates_after_epoch {
            let mut updates = self
                .deps
                .candidate_repo
                .list(&request.space_id)
                .await
                .map_err(|_| MembershipGossipEndpointError::Persistence)?
                .into_iter()
                .flat_map(|candidate| candidate.security_updates().to_vec())
                .filter(|update| update.previous_epoch >= epoch)
                .collect::<Vec<_>>();
            updates.sort_by_key(|update| update.previous_epoch);
            updates.dedup_by_key(|update| update.digest);
            events.extend(updates.into_iter().map(MembershipEvent::SecurityUpdate));
        }
        let batch_input = serde_json::to_vec(&(&request.space_id, &events))
            .map_err(|_| MembershipGossipEndpointError::Persistence)?;
        let batch_id = self
            .deps
            .hash
            .hash_bytes(&batch_input)
            .map_err(|_| MembershipGossipEndpointError::Persistence)?
            .bytes;
        let batch = MembershipEventBatch {
            space_id: request.space_id,
            batch_id,
            events,
        };
        batch
            .validate_transfer_bounds()
            .map_err(|_| MembershipGossipEndpointError::Rejected)?;
        Ok(MembershipGossipMessage::EventBatch(batch))
    }

    pub(crate) async fn apply_relayed_security_updates(
        &self,
        space_id: &SpaceId,
        updates: &[RelayedSecurityUpdate],
    ) -> Result<u64, SpaceMembershipGossipError> {
        let mut state = self.deps.security_updates.current_state().await?;
        if &state.space_id != space_id {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }
        for update in updates {
            let digest = self
                .deps
                .hash
                .hash_bytes(&update.payload)
                .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
            if digest.bytes != update.digest
                || update.next_epoch != update.previous_epoch.saturating_add(1)
            {
                return Err(SpaceMembershipGossipError::VerificationRejected);
            }
            if update.next_epoch <= state.group_epoch {
                continue;
            }
            if update.previous_epoch != state.group_epoch {
                return Err(SpaceMembershipGossipError::WaitingForUpdate);
            }
            let applied_epoch = self
                .deps
                .security_updates
                .apply_group_epoch_update(&update.payload)
                .await?;
            if applied_epoch != update.next_epoch {
                return Err(SpaceMembershipGossipError::VerificationRejected);
            }
            state.group_epoch = applied_epoch;
        }
        Ok(state.group_epoch)
    }

    pub async fn deliver_pending(
        &self,
        space_id: &SpaceId,
        now_ms: i64,
    ) -> Result<usize, SpaceMembershipGossipError> {
        let pending = self.deps.outbox_repo.list_pending(space_id).await?;
        let mut delivered = 0usize;
        for mut item in pending
            .into_iter()
            .filter(|item| item.next_attempt_at_ms() <= now_ms)
        {
            let response = self
                .deps
                .transport
                .exchange(
                    item.recipient_device_id(),
                    MembershipGossipMessage::EventBatch(item.batch().clone()),
                )
                .await;
            let acknowledged = matches!(
                response,
                Ok(MembershipGossipMessage::Ack(ref ack))
                    if ack.space_id == item.batch().space_id
                        && ack.batch_id == item.batch().batch_id
            );
            if acknowledged {
                if self
                    .deps
                    .outbox_repo
                    .remove(
                        &item.batch().space_id,
                        item.recipient_device_id(),
                        &item.batch().batch_id,
                    )
                    .await?
                {
                    delivered = delivered.saturating_add(1);
                }
            } else {
                let next_attempt_at_ms = next_membership_retry_at(&item, now_ms);
                if matches!(
                    response,
                    Err(uc_core::membership::MembershipGossipTransportError::VersionIncompatible)
                ) {
                    item.mark_retry_after(
                        CandidateFailure::VersionIncompatible,
                        next_attempt_at_ms,
                        now_ms,
                    );
                } else {
                    item.mark_retry(next_attempt_at_ms, now_ms);
                }
                self.deps.outbox_repo.save(&item).await?;
            }
        }
        Ok(delivered)
    }

    pub async fn synchronize_member(
        &self,
        recipient: &uc_core::ids::DeviceId,
    ) -> Result<(), SpaceMembershipGossipError> {
        self.refresh_local_announcement().await?;
        let state = self.deps.security_updates.current_state().await?;
        let mut announcements = self
            .deps
            .announcement_repo
            .list(&state.space_id)
            .await?
            .into_iter()
            .map(
                |announcement| uc_core::membership::MembershipAnnouncementVersion {
                    device_id: announcement.device_id,
                    sequence: announcement.sequence,
                    content_digest: announcement.content_digest,
                },
            )
            .collect::<Vec<_>>();
        announcements.sort_by(|left, right| left.device_id.as_str().cmp(right.device_id.as_str()));
        let group_update_head_digest = self
            .deps
            .candidate_repo
            .list(&state.space_id)
            .await?
            .into_iter()
            .flat_map(|candidate| candidate.security_updates().to_vec())
            .max_by_key(|update| update.next_epoch)
            .map(|update| update.digest);
        let response = self
            .deps
            .transport
            .exchange(
                recipient,
                MembershipGossipMessage::Digest(uc_core::membership::MembershipDigest {
                    space_id: state.space_id.clone(),
                    group_epoch: state.group_epoch,
                    group_update_head_digest,
                    announcements,
                }),
            )
            .await
            .map_err(map_gossip_transport_error)?;
        let MembershipGossipMessage::RequestMissing(request) = response else {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        };
        if request.space_id != state.space_id {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }
        let events = self
            .events_for_request(request)
            .await
            .map_err(|error| match error {
                MembershipGossipEndpointError::Rejected => {
                    SpaceMembershipGossipError::VerificationRejected
                }
                MembershipGossipEndpointError::Persistence => {
                    SpaceMembershipGossipError::Relationship(
                        "membership event batch could not be built".into(),
                    )
                }
            })?;
        let MembershipGossipMessage::EventBatch(batch) = events else {
            return Err(SpaceMembershipGossipError::VerificationRejected);
        };
        let response = self
            .deps
            .transport
            .exchange(
                recipient,
                MembershipGossipMessage::EventBatch(batch.clone()),
            )
            .await
            .map_err(map_gossip_transport_error)?;
        match response {
            MembershipGossipMessage::Ack(ack)
                if ack.space_id == batch.space_id && ack.batch_id == batch.batch_id =>
            {
                Ok(())
            }
            _ => Err(SpaceMembershipGossipError::VerificationRejected),
        }
    }

    pub async fn reconcile_once(
        &self,
    ) -> Result<MembershipGossipPassOutcome, SpaceMembershipGossipError> {
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
                }
                Err(
                    SpaceMembershipGossipError::PeerUnavailable
                    | SpaceMembershipGossipError::WaitingForUpdate
                    | SpaceMembershipGossipError::VerificationRejected,
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
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
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
                    SpaceMembershipGossipError::PeerUnavailable
                    | SpaceMembershipGossipError::VerificationRejected,
                ) => {
                    outcome.deferred_items = outcome.deferred_items.saturating_add(1);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(outcome)
    }

    pub async fn convergence_status(
        &self,
        space_id: &SpaceId,
    ) -> Result<MembershipConvergenceStatus, SpaceMembershipGossipError> {
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

    pub async fn current_convergence_status(
        &self,
    ) -> Result<MembershipConvergenceStatus, SpaceMembershipGossipError> {
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

    pub fn start(
        self: Arc<Self>,
        mut presence_events: broadcast::Receiver<uc_core::ports::PresenceEvent>,
    ) -> SpaceMembershipGossipRuntime {
        let (commands, mut command_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut paused = false;
            let mut presence_open = true;
            let mut announcement_changes_open = true;
            let mut run_now = true;
            loop {
                let mut pass_failed = false;
                if run_now && !paused {
                    run_now = false;
                    let mut pass = Box::pin(self.reconcile_once());
                    let (completed_pass, pause_completed) = loop {
                        tokio::select! {
                            result = &mut pass => break (Some(result), None),
                            command = command_rx.recv() => match command {
                                Some(MembershipGossipRuntimeCommand::Pause(completed)) => {
                                    paused = true;
                                    run_now = true;
                                    break (None, Some(completed));
                                }
                                Some(MembershipGossipRuntimeCommand::Resume(completed)) => {
                                    let _ = completed.send(());
                                }
                                Some(MembershipGossipRuntimeCommand::Shutdown(completed)) => {
                                    let _ = completed.send(());
                                    return;
                                }
                                None => return,
                            }
                        }
                    };
                    // Dropping the pass first guarantees that a completed pause
                    // cannot leave an in-flight network exchange running.
                    drop(pass);
                    if let Some(completed) = pause_completed {
                        let _ = completed.send(());
                    }
                    match completed_pass {
                        Some(Ok(outcome)) => {
                            debug!(
                                delivered_batches = outcome.delivered_batches,
                                confirmed_candidates = outcome.confirmed_candidates,
                                synchronized_members = outcome.synchronized_members,
                                deferred_items = outcome.deferred_items,
                                "membership gossip pass completed"
                            );
                        }
                        Some(Err(_)) => {
                            pass_failed = true;
                            warn!(
                                error_kind = "membership_gossip_reconcile",
                                retryable = true,
                                "membership gossip pass deferred"
                            );
                        }
                        None => {}
                    }
                }

                let reconcile_delay = if paused {
                    gossip_reconcile_delay(&self.deps.device_identity.current_device_id())
                } else if pass_failed {
                    Duration::from_millis(INITIAL_RETRY_DELAY_MS as u64)
                } else {
                    self.next_reconcile_delay().await
                };
                let timer = tokio::time::sleep(reconcile_delay);
                tokio::pin!(timer);
                let announcement_change = self
                    .deps
                    .announcement_material
                    .wait_for_announcement_change();
                tokio::pin!(announcement_change);
                tokio::select! {
                    command = command_rx.recv() => match command {
                        Some(MembershipGossipRuntimeCommand::Pause(completed)) => {
                            paused = true;
                            let _ = completed.send(());
                        }
                        Some(MembershipGossipRuntimeCommand::Resume(completed)) => {
                            paused = false;
                            run_now = true;
                            let _ = completed.send(());
                        }
                        Some(MembershipGossipRuntimeCommand::Shutdown(completed)) => {
                            let _ = completed.send(());
                            break;
                        }
                        None => break,
                    },
                    _ = self.wake.notified(), if !paused => {
                        run_now = true;
                    }
                    event = presence_events.recv(), if !paused && presence_open => match event {
                        Ok(event) if event.state == uc_core::ports::ReachabilityState::Online => {
                            run_now = true;
                        }
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            run_now = true;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            presence_open = false;
                        }
                    },
                    change = &mut announcement_change, if !paused && announcement_changes_open => {
                        match change {
                            Ok(()) => run_now = true,
                            Err(_) => announcement_changes_open = false,
                        }
                    },
                    _ = &mut timer, if !paused => {
                        run_now = true;
                    }
                }
            }
        });
        SpaceMembershipGossipRuntime {
            activity: SpaceMembershipGossipActivity { commands },
            task: Some(task),
        }
    }

    pub async fn confirm_candidate(
        &self,
        space_id: &SpaceId,
        device_id: &uc_core::ids::DeviceId,
    ) -> Result<(), SpaceMembershipGossipError> {
        let _attempt = self.candidate_attempt_lock.lock().await;
        let now_ms = self.deps.clock.now_ms();
        let mut candidate = self
            .deps
            .candidate_repo
            .get(space_id, device_id)
            .await?
            .ok_or(SpaceMembershipGossipError::CandidateNotFound)?;
        candidate.mark_verifying(now_ms);
        self.deps.candidate_repo.save(&candidate).await?;

        let verified = match self.deps.attestation.attest_candidate(&candidate).await {
            Ok(verified) => verified,
            Err(
                error @ (MembershipAttestationError::Offline
                | MembershipAttestationError::Transport),
            ) => {
                let failure = if matches!(error, MembershipAttestationError::Offline) {
                    CandidateFailure::PeerOffline
                } else {
                    CandidateFailure::Transport
                };
                candidate.mark_waiting_for_peer(
                    failure,
                    next_candidate_retry_at(&candidate, now_ms),
                    now_ms,
                );
                self.deps.candidate_repo.save(&candidate).await?;
                return Err(SpaceMembershipGossipError::PeerUnavailable);
            }
            Err(MembershipAttestationError::MissingSecurityUpdate) => {
                candidate.mark_waiting_for_update(now_ms);
                self.deps.candidate_repo.save(&candidate).await?;
                return Err(SpaceMembershipGossipError::WaitingForUpdate);
            }
            Err(MembershipAttestationError::VersionIncompatible) => {
                candidate.mark_waiting_for_peer(
                    CandidateFailure::VersionIncompatible,
                    next_candidate_retry_at(&candidate, now_ms),
                    now_ms,
                );
                self.deps.candidate_repo.save(&candidate).await?;
                return Err(SpaceMembershipGossipError::PeerUnavailable);
            }
            Err(MembershipAttestationError::Rejected) => {
                candidate.mark_rejected(CandidateFailure::InvalidProof, now_ms);
                self.deps.candidate_repo.save(&candidate).await?;
                return Err(SpaceMembershipGossipError::VerificationRejected);
            }
        };

        let merge = candidate.apply_verified_peer(&verified, now_ms);
        if !matches!(merge, Ok(CandidateMergeOutcome::Updated)) {
            candidate.mark_rejected(CandidateFailure::InvalidProof, now_ms);
            self.deps.candidate_repo.save(&candidate).await?;
            return Err(SpaceMembershipGossipError::VerificationRejected);
        }
        self.promote_verified_peer(&mut candidate, verified, now_ms)
            .await
    }

    pub async fn accept_verified_peer(
        &self,
        verified: VerifiedMembershipPeer,
    ) -> Result<(), SpaceMembershipGossipError> {
        let now_ms = self.deps.clock.now_ms();
        let existing = self
            .deps
            .candidate_repo
            .get(&verified.space_id, &verified.device_id)
            .await?;
        let mut candidate = match existing {
            Some(mut candidate) => {
                let merge = candidate.apply_verified_peer(&verified, now_ms)?;
                if merge != CandidateMergeOutcome::Updated {
                    return Err(SpaceMembershipGossipError::VerificationRejected);
                }
                candidate.mark_verifying(now_ms);
                candidate
            }
            None => SpaceMembershipCandidate::from_verified_peer(
                &verified,
                now_ms.saturating_add(DIRECT_ATTESTATION_TTL_MS),
                now_ms,
            )?,
        };
        self.deps.candidate_repo.save(&candidate).await?;
        self.promote_verified_peer(&mut candidate, verified, now_ms)
            .await
    }

    async fn promote_verified_peer(
        &self,
        candidate: &mut SpaceMembershipCandidate,
        verified: VerifiedMembershipPeer,
        now_ms: i64,
    ) -> Result<(), SpaceMembershipGossipError> {
        let observed_at = Utc
            .timestamp_millis_opt(now_ms)
            .single()
            .ok_or_else(|| SpaceMembershipGossipError::Relationship("invalid clock".into()))?;
        let address = PeerAddressRecord {
            device_id: verified.device_id,
            addr_blob: verified.transport_address_blob,
            observed_at,
        };
        let trusted_peer = TrustedPeer {
            local_device_id: self.deps.device_identity.current_device_id(),
            peer_device_id: verified.device_id,
            peer_fingerprint: verified.identity_fingerprint.clone(),
            trusted_at: observed_at,
        };
        let member = SpaceMember {
            device_id: verified.device_id,
            device_name: verified.device_name,
            identity_fingerprint: verified.identity_fingerprint,
            joined_at: observed_at,
            sync_preferences: MemberSyncPreferences::default(),
        };
        candidate.mark_ready(now_ms);
        self.deps
            .verified_peer_promotion
            .promote_verified_peer(&member, &trusted_peer, &address, candidate)
            .await
            .map_err(|error| SpaceMembershipGossipError::Relationship(error.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl PairingMembershipGossipPort for SpaceMembershipGossip {
    async fn prepare_sponsor_membership(
        &self,
        context: SponsorSeedBatchContext,
    ) -> Result<Vec<SponsorCandidateSeed>, SpaceMembershipGossipError> {
        SpaceMembershipGossip::prepare_sponsor_membership(self, context).await
    }

    async fn accept_sponsor_seed_batch(
        &self,
        seeds: Vec<SponsorCandidateSeed>,
    ) -> Result<(), SpaceMembershipGossipError> {
        SpaceMembershipGossip::accept_sponsor_seed_batch(self, seeds).await
    }
}

#[async_trait]
impl MembershipAttestationEndpointPort for SpaceMembershipGossip {
    async fn apply_relayed_security_updates(
        &self,
        space_id: &SpaceId,
        updates: &[RelayedSecurityUpdate],
    ) -> Result<u64, MembershipAttestationEndpointError> {
        SpaceMembershipGossip::apply_relayed_security_updates(self, space_id, updates)
            .await
            .map_err(|error| match error {
                SpaceMembershipGossipError::WaitingForUpdate => {
                    MembershipAttestationEndpointError::MissingSecurityUpdate
                }
                SpaceMembershipGossipError::VerificationRejected
                | SpaceMembershipGossipError::InvalidCandidate(_)
                | SpaceMembershipGossipError::CandidateNotFound => {
                    MembershipAttestationEndpointError::Rejected
                }
                _ => MembershipAttestationEndpointError::Persistence,
            })
    }

    async fn accept_verified_peer(
        &self,
        peer: VerifiedMembershipPeer,
    ) -> Result<(), MembershipAttestationEndpointError> {
        SpaceMembershipGossip::accept_verified_peer(self, peer)
            .await
            .map_err(|error| match error {
                SpaceMembershipGossipError::VerificationRejected
                | SpaceMembershipGossipError::InvalidCandidate(_)
                | SpaceMembershipGossipError::CandidateNotFound => {
                    MembershipAttestationEndpointError::Rejected
                }
                _ => MembershipAttestationEndpointError::Persistence,
            })
    }
}

#[async_trait]
impl MembershipGossipEndpointPort for SpaceMembershipGossip {
    async fn handle_message(
        &self,
        source_device_id: &uc_core::ids::DeviceId,
        message: MembershipGossipMessage,
    ) -> Result<MembershipGossipMessage, MembershipGossipEndpointError> {
        message
            .validate_transfer_bounds()
            .map_err(|_| MembershipGossipEndpointError::Rejected)?;
        match message {
            MembershipGossipMessage::EventBatch(batch) => {
                let state = self
                    .deps
                    .security_updates
                    .current_state()
                    .await
                    .map_err(|_| MembershipGossipEndpointError::Persistence)?;
                if state.space_id != batch.space_id {
                    return Err(MembershipGossipEndpointError::Rejected);
                }
                for event in &batch.events {
                    match event {
                        MembershipEvent::SponsorSeed(seed)
                            if &seed.source_device_id == source_device_id
                                && seed.space_id == batch.space_id =>
                        {
                            self.accept_sponsor_seed(seed.clone())
                                .await
                                .map_err(|error| match error {
                                    SpaceMembershipGossipError::InvalidCandidate(_)
                                    | SpaceMembershipGossipError::VerificationRejected => {
                                        MembershipGossipEndpointError::Rejected
                                    }
                                    _ => MembershipGossipEndpointError::Persistence,
                                })?;
                        }
                        MembershipEvent::SecurityUpdate(update) => {
                            self.apply_relayed_security_updates(
                                &batch.space_id,
                                std::slice::from_ref(update),
                            )
                            .await
                            .map_err(|error| match error {
                                SpaceMembershipGossipError::VerificationRejected
                                | SpaceMembershipGossipError::WaitingForUpdate => {
                                    MembershipGossipEndpointError::Rejected
                                }
                                _ => MembershipGossipEndpointError::Persistence,
                            })?;
                        }
                        MembershipEvent::Announcement(announcement) => {
                            self.accept_verified_announcement(announcement.clone())
                                .await
                                .map_err(|error| match error {
                                    SpaceMembershipGossipError::VerificationRejected
                                    | SpaceMembershipGossipError::InvalidCandidate(_) => {
                                        MembershipGossipEndpointError::Rejected
                                    }
                                    _ => MembershipGossipEndpointError::Persistence,
                                })?;
                        }
                        MembershipEvent::SponsorSeed(_) => {
                            return Err(MembershipGossipEndpointError::Rejected);
                        }
                    }
                }
                Ok(MembershipGossipMessage::Ack(
                    uc_core::membership::MembershipAck {
                        space_id: batch.space_id,
                        batch_id: batch.batch_id,
                    },
                ))
            }
            MembershipGossipMessage::Digest(digest) => self.request_for_digest(digest).await,
            MembershipGossipMessage::RequestMissing(request) => {
                self.events_for_request(request).await
            }
            MembershipGossipMessage::Ack(_) => Err(MembershipGossipEndpointError::Rejected),
        }
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

fn map_gossip_transport_error(
    error: uc_core::membership::MembershipGossipTransportError,
) -> SpaceMembershipGossipError {
    match error {
        uc_core::membership::MembershipGossipTransportError::Offline
        | uc_core::membership::MembershipGossipTransportError::Transport => {
            SpaceMembershipGossipError::PeerUnavailable
        }
        uc_core::membership::MembershipGossipTransportError::Rejected
        | uc_core::membership::MembershipGossipTransportError::VersionIncompatible => {
            SpaceMembershipGossipError::VerificationRejected
        }
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

impl SpaceMembershipGossipRuntime {
    pub fn activity(&self) -> SpaceMembershipGossipActivity {
        self.activity.clone()
    }

    pub async fn pause(&self) -> Result<(), MembershipGossipRuntimeError> {
        self.activity.pause().await
    }

    pub async fn resume(&self) -> Result<(), MembershipGossipRuntimeError> {
        self.activity.resume().await
    }

    pub async fn shutdown(mut self) {
        let (completed, response) = oneshot::channel();
        if self
            .activity
            .commands
            .send(MembershipGossipRuntimeCommand::Shutdown(completed))
            .is_ok()
        {
            let _ = response.await;
        }
        if let Some(task) = self.task.take() {
            if let Err(error) = task.await {
                if !error.is_cancelled() {
                    warn!(
                        error_kind = "membership_gossip_runtime_panic",
                        "membership gossip runtime stopped unexpectedly"
                    );
                }
            }
        }
    }
}

impl SpaceMembershipGossipActivity {
    pub async fn pause(&self) -> Result<(), MembershipGossipRuntimeError> {
        let (completed, response) = oneshot::channel();
        self.commands
            .send(MembershipGossipRuntimeCommand::Pause(completed))
            .map_err(|_| MembershipGossipRuntimeError::Stopped)?;
        response
            .await
            .map_err(|_| MembershipGossipRuntimeError::Stopped)
    }

    pub async fn resume(&self) -> Result<(), MembershipGossipRuntimeError> {
        let (completed, response) = oneshot::channel();
        self.commands
            .send(MembershipGossipRuntimeCommand::Resume(completed))
            .map_err(|_| MembershipGossipRuntimeError::Stopped)?;
        response
            .await
            .map_err(|_| MembershipGossipRuntimeError::Stopped)
    }
}

#[async_trait]
impl crate::facade::space_session::MembershipActivityPort for SpaceMembershipGossipActivity {
    async fn pause(&self) -> Result<(), String> {
        self.pause().await.map_err(|error| error.to_string())
    }

    async fn resume(&self) -> Result<(), String> {
        self.resume().await.map_err(|error| error.to_string())
    }
}

impl Drop for SpaceMembershipGossipRuntime {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
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

fn next_membership_retry_at(pending: &PendingMembershipBatch, now_ms: i64) -> i64 {
    let multiplier = 1i64 << pending.attempt_count().min(4);
    let base = INITIAL_RETRY_DELAY_MS
        .saturating_mul(multiplier)
        .min(MAX_RETRY_DELAY_MS);
    let jitter_window = (base / 5).max(1);
    let jitter_seed =
        u16::from_be_bytes([pending.batch().batch_id[0], pending.batch().batch_id[1]]);
    let jitter = i64::from(jitter_seed) % jitter_window;
    now_ms.saturating_add(base).saturating_add(jitter)
}

fn next_candidate_retry_at(candidate: &SpaceMembershipCandidate, now_ms: i64) -> i64 {
    let multiplier = 1i64 << candidate.attempt_count().min(4);
    let base = INITIAL_RETRY_DELAY_MS
        .saturating_mul(multiplier)
        .min(MAX_RETRY_DELAY_MS);
    let jitter_window = (base / 5).max(1);
    let jitter_seed = candidate
        .device_id()
        .as_str()
        .bytes()
        .fold(u64::from(candidate.attempt_count()), |sum, byte| {
            sum.wrapping_mul(31).wrapping_add(u64::from(byte))
        });
    let jitter = (jitter_seed % jitter_window as u64) as i64;
    now_ms.saturating_add(base).saturating_add(jitter)
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use async_trait::async_trait;
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        CandidateFailure, CandidateMergeOutcome, CandidateStatus, CurrentMemberSignatureError,
        CurrentMemberSignaturePort, CurrentMembershipAnnouncementMaterial,
        CurrentMembershipAnnouncementPort, CurrentMembershipIdentityError, DeviceAnnouncement,
        MemberRepositoryPort, MembershipAnnouncementRepositoryError,
        MembershipAnnouncementRepositoryPort, MembershipAnnouncementVersion,
        MembershipAttestationError, MembershipAttestationPort, MembershipCandidateRepositoryError,
        MembershipCandidateRepositoryPort, MembershipDigest, MembershipError, MembershipEvent,
        MembershipEventBatch, MembershipGossipEndpointPort, MembershipGossipMessage,
        MembershipGossipTransportError, MembershipGossipTransportPort,
        MembershipOutboxRepositoryError, MembershipOutboxRepositoryPort, MembershipRequestMissing,
        MembershipSecurityState, MembershipSecurityUpdateError, MembershipSecurityUpdatePort,
        PendingMembershipBatch, RelayedSecurityUpdate, SpaceMember, SpaceMembershipCandidate,
        SponsorCandidateSeed, VerifiedMembershipPeer, VerifiedPeerPromotionError,
        VerifiedPeerPromotionPort,
    };
    use uc_core::ports::security::IdentityFingerprintFactoryPort;
    use uc_core::ports::{
        ClockPort, ContentHashPort, DeviceIdentityPort, PeerAddressError, PeerAddressRecord,
        PeerAddressRepositoryPort,
    };
    use uc_core::security::IdentityFingerprint;
    use uc_core::trusted_peer::{TrustedPeer, TrustedPeerError, TrustedPeerRepositoryPort};

    use super::{
        next_candidate_retry_at, MembershipConvergenceState, SpaceMembershipGossip,
        SpaceMembershipGossipDeps, SpaceMembershipGossipError, SponsorSeedBatchContext,
    };

    #[derive(Default)]
    struct InMemoryCandidateRepository {
        candidates: Mutex<HashMap<(String, DeviceId), SpaceMembershipCandidate>>,
        save_count: Mutex<usize>,
    }

    #[derive(Default)]
    struct InMemoryMembershipOutbox(Mutex<Vec<PendingMembershipBatch>>);

    struct FailingMembershipOutbox;

    #[derive(Default)]
    struct InMemoryAnnouncementRepository(Mutex<Vec<DeviceAnnouncement>>);

    #[async_trait]
    impl MembershipAnnouncementRepositoryPort for InMemoryAnnouncementRepository {
        async fn get(
            &self,
            space_id: &SpaceId,
            device_id: &DeviceId,
        ) -> Result<Option<DeviceAnnouncement>, MembershipAnnouncementRepositoryError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|announcement| {
                    &announcement.space_id == space_id && &announcement.device_id == device_id
                })
                .cloned())
        }

        async fn list(
            &self,
            space_id: &SpaceId,
        ) -> Result<Vec<DeviceAnnouncement>, MembershipAnnouncementRepositoryError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|announcement| &announcement.space_id == space_id)
                .cloned()
                .collect())
        }

        async fn save(
            &self,
            announcement: &DeviceAnnouncement,
        ) -> Result<(), MembershipAnnouncementRepositoryError> {
            let mut announcements = self.0.lock().unwrap();
            announcements.retain(|known| {
                known.space_id != announcement.space_id || known.device_id != announcement.device_id
            });
            announcements.push(announcement.clone());
            Ok(())
        }

        async fn remove(
            &self,
            space_id: &SpaceId,
            device_id: &DeviceId,
        ) -> Result<bool, MembershipAnnouncementRepositoryError> {
            let mut announcements = self.0.lock().unwrap();
            let before = announcements.len();
            announcements
                .retain(|known| &known.space_id != space_id || &known.device_id != device_id);
            Ok(before != announcements.len())
        }
    }

    struct AcceptingMemberSignatures;

    #[async_trait]
    impl CurrentMemberSignaturePort for AcceptingMemberSignatures {
        async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
            Ok(4)
        }

        async fn sign_current_member_payload(
            &self,
            _payload: &[u8],
        ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
            Ok(b"valid-signature".to_vec())
        }

        async fn verify_current_member_payload(
            &self,
            _member: &DeviceId,
            _payload: &[u8],
            _signature: &[u8],
        ) -> Result<bool, CurrentMemberSignatureError> {
            Ok(true)
        }
    }

    struct FixedFingerprintFactory;

    impl IdentityFingerprintFactoryPort for FixedFingerprintFactory {
        fn from_public_key(&self, _public_key: &[u8]) -> anyhow::Result<IdentityFingerprint> {
            Ok(fingerprint("ANNOUNCEMENTFP01"))
        }
    }

    struct FixedAnnouncementMaterial;

    #[async_trait]
    impl CurrentMembershipAnnouncementPort for FixedAnnouncementMaterial {
        async fn current_announcement_material(
            &self,
        ) -> Result<
            CurrentMembershipAnnouncementMaterial,
            uc_core::membership::CurrentMembershipIdentityError,
        > {
            Ok(CurrentMembershipAnnouncementMaterial {
                space_id: SpaceId::from("space-a"),
                device_id: DeviceId::new("device-a"),
                device_name: "Device A".to_owned(),
                identity_fingerprint: fingerprint("ANNOUNCEMENTFP01"),
                transport_public_key: b"key-a".to_vec(),
                transport_address_blob: b"address-a".to_vec(),
            })
        }

        async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
            std::future::pending::<()>().await;
            Ok(())
        }
    }

    struct NotifyingAnnouncementMaterial {
        changed: AtomicBool,
        change: tokio::sync::Notify,
    }

    impl NotifyingAnnouncementMaterial {
        fn new() -> Self {
            Self {
                changed: AtomicBool::new(false),
                change: tokio::sync::Notify::new(),
            }
        }

        fn change_address(&self) {
            self.changed.store(true, Ordering::SeqCst);
            self.change.notify_one();
        }
    }

    #[async_trait]
    impl CurrentMembershipAnnouncementPort for NotifyingAnnouncementMaterial {
        async fn current_announcement_material(
            &self,
        ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
            let transport_address_blob = if self.changed.load(Ordering::SeqCst) {
                b"address-a-updated".to_vec()
            } else {
                b"address-a".to_vec()
            };
            Ok(CurrentMembershipAnnouncementMaterial {
                space_id: SpaceId::from("space-a"),
                device_id: DeviceId::new("device-a"),
                device_name: "Device A".to_owned(),
                identity_fingerprint: fingerprint("ANNOUNCEMENTFP01"),
                transport_public_key: b"key-a".to_vec(),
                transport_address_blob,
            })
        }

        async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
            self.change.notified().await;
            Ok(())
        }
    }

    #[async_trait]
    impl MembershipOutboxRepositoryPort for InMemoryMembershipOutbox {
        async fn get(
            &self,
            space_id: &SpaceId,
            recipient_device_id: &DeviceId,
            batch_id: &[u8; 32],
        ) -> Result<Option<PendingMembershipBatch>, MembershipOutboxRepositoryError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|pending| {
                    &pending.batch().space_id == space_id
                        && pending.recipient_device_id() == recipient_device_id
                        && &pending.batch().batch_id == batch_id
                })
                .cloned())
        }

        async fn list_pending(
            &self,
            space_id: &SpaceId,
        ) -> Result<Vec<PendingMembershipBatch>, MembershipOutboxRepositoryError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|pending| &pending.batch().space_id == space_id)
                .cloned()
                .collect())
        }

        async fn save(
            &self,
            pending: &PendingMembershipBatch,
        ) -> Result<(), MembershipOutboxRepositoryError> {
            let mut rows = self.0.lock().unwrap();
            rows.retain(|known| {
                known.batch().space_id != pending.batch().space_id
                    || known.recipient_device_id() != pending.recipient_device_id()
                    || known.batch().batch_id != pending.batch().batch_id
            });
            rows.push(pending.clone());
            Ok(())
        }

        async fn remove(
            &self,
            space_id: &SpaceId,
            recipient_device_id: &DeviceId,
            batch_id: &[u8; 32],
        ) -> Result<bool, MembershipOutboxRepositoryError> {
            let mut rows = self.0.lock().unwrap();
            let before = rows.len();
            rows.retain(|pending| {
                &pending.batch().space_id != space_id
                    || pending.recipient_device_id() != recipient_device_id
                    || &pending.batch().batch_id != batch_id
            });
            Ok(rows.len() != before)
        }
    }

    #[async_trait]
    impl MembershipOutboxRepositoryPort for FailingMembershipOutbox {
        async fn get(
            &self,
            _space_id: &SpaceId,
            _recipient_device_id: &DeviceId,
            _batch_id: &[u8; 32],
        ) -> Result<Option<PendingMembershipBatch>, MembershipOutboxRepositoryError> {
            Ok(None)
        }

        async fn list_pending(
            &self,
            _space_id: &SpaceId,
        ) -> Result<Vec<PendingMembershipBatch>, MembershipOutboxRepositoryError> {
            Ok(Vec::new())
        }

        async fn save(
            &self,
            _pending: &PendingMembershipBatch,
        ) -> Result<(), MembershipOutboxRepositoryError> {
            Err(MembershipOutboxRepositoryError::Repository(
                "injected outbox failure".to_owned(),
            ))
        }

        async fn remove(
            &self,
            _space_id: &SpaceId,
            _recipient_device_id: &DeviceId,
            _batch_id: &[u8; 32],
        ) -> Result<bool, MembershipOutboxRepositoryError> {
            Ok(false)
        }
    }

    impl InMemoryCandidateRepository {
        fn save_count(&self) -> usize {
            *self.save_count.lock().unwrap()
        }
    }

    #[async_trait]
    impl MembershipCandidateRepositoryPort for InMemoryCandidateRepository {
        async fn get(
            &self,
            space_id: &SpaceId,
            device_id: &DeviceId,
        ) -> Result<Option<SpaceMembershipCandidate>, MembershipCandidateRepositoryError> {
            Ok(self
                .candidates
                .lock()
                .unwrap()
                .get(&(space_id.as_ref().to_owned(), *device_id))
                .cloned())
        }

        async fn list(
            &self,
            space_id: &SpaceId,
        ) -> Result<Vec<SpaceMembershipCandidate>, MembershipCandidateRepositoryError> {
            Ok(self
                .candidates
                .lock()
                .unwrap()
                .values()
                .filter(|candidate| candidate.space_id() == space_id)
                .cloned()
                .collect())
        }

        async fn save(
            &self,
            candidate: &SpaceMembershipCandidate,
        ) -> Result<(), MembershipCandidateRepositoryError> {
            self.candidates.lock().unwrap().insert(
                (
                    candidate.space_id().as_ref().to_owned(),
                    *candidate.device_id(),
                ),
                candidate.clone(),
            );
            *self.save_count.lock().unwrap() += 1;
            Ok(())
        }

        async fn remove(
            &self,
            space_id: &SpaceId,
            device_id: &DeviceId,
        ) -> Result<bool, MembershipCandidateRepositoryError> {
            Ok(self
                .candidates
                .lock()
                .unwrap()
                .remove(&(space_id.as_ref().to_owned(), *device_id))
                .is_some())
        }

        async fn purge_expired(
            &self,
            space_id: &SpaceId,
            now_ms: i64,
        ) -> Result<usize, MembershipCandidateRepositoryError> {
            let mut candidates = self.candidates.lock().unwrap();
            let before = candidates.len();
            candidates.retain(|(candidate_space_id, _), candidate| {
                candidate_space_id != space_id.as_ref() || candidate.expires_at_ms() > now_ms
            });
            Ok(before - candidates.len())
        }
    }

    struct FixedClock(i64);

    impl ClockPort for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }
    }

    struct ManualClock(AtomicI64);

    impl ManualClock {
        fn new(now_ms: i64) -> Self {
            Self(AtomicI64::new(now_ms))
        }

        fn set(&self, now_ms: i64) {
            self.0.store(now_ms, Ordering::SeqCst);
        }
    }

    impl ClockPort for ManualClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct FixedDeviceIdentity(DeviceId);

    impl DeviceIdentityPort for FixedDeviceIdentity {
        fn current_device_id(&self) -> DeviceId {
            self.0
        }
    }

    struct FixedHasher;

    impl ContentHashPort for FixedHasher {
        fn hash_bytes(&self, _bytes: &[u8]) -> anyhow::Result<uc_core::ContentHash> {
            Ok(uc_core::ContentHash {
                alg: uc_core::HashAlgorithm::Blake3V1,
                bytes: [9; 32],
            })
        }
    }

    struct Blake3Hasher;

    impl ContentHashPort for Blake3Hasher {
        fn hash_bytes(&self, bytes: &[u8]) -> anyhow::Result<uc_core::ContentHash> {
            Ok(uc_core::ContentHash {
                alg: uc_core::HashAlgorithm::Blake3V1,
                bytes: *blake3::hash(bytes).as_bytes(),
            })
        }
    }

    struct RejectingMemberSignatures;

    #[async_trait]
    impl CurrentMemberSignaturePort for RejectingMemberSignatures {
        async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
            Ok(4)
        }

        async fn sign_current_member_payload(
            &self,
            _payload: &[u8],
        ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
            Ok(b"invalid-signature".to_vec())
        }

        async fn verify_current_member_payload(
            &self,
            _member: &DeviceId,
            _payload: &[u8],
            _signature: &[u8],
        ) -> Result<bool, CurrentMemberSignatureError> {
            Ok(false)
        }
    }

    struct InMemoryMembershipSecurity {
        state: Mutex<MembershipSecurityState>,
        applied: Mutex<Vec<Vec<u8>>>,
    }

    struct FixedMembershipTransport(
        Mutex<Result<uc_core::membership::MembershipGossipMessage, MembershipGossipTransportError>>,
    );

    #[async_trait]
    impl MembershipGossipTransportPort for FixedMembershipTransport {
        async fn exchange(
            &self,
            _recipient: &DeviceId,
            _message: uc_core::membership::MembershipGossipMessage,
        ) -> Result<uc_core::membership::MembershipGossipMessage, MembershipGossipTransportError>
        {
            self.0.lock().unwrap().clone()
        }
    }

    struct ScriptedMembershipTransport {
        responses: Mutex<VecDeque<Result<MembershipGossipMessage, MembershipGossipTransportError>>>,
        sent: Mutex<Vec<MembershipGossipMessage>>,
    }

    struct BlockingMembershipTransport {
        started: Arc<tokio::sync::Notify>,
        active: Arc<AtomicBool>,
    }

    struct ActiveExchangeGuard {
        active: Arc<AtomicBool>,
    }

    impl Drop for ActiveExchangeGuard {
        fn drop(&mut self) {
            self.active.store(false, Ordering::Release);
        }
    }

    #[async_trait]
    impl MembershipGossipTransportPort for ScriptedMembershipTransport {
        async fn exchange(
            &self,
            _recipient: &DeviceId,
            message: MembershipGossipMessage,
        ) -> Result<MembershipGossipMessage, MembershipGossipTransportError> {
            self.sent.lock().unwrap().push(message);
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(MembershipGossipTransportError::Transport))
        }
    }

    #[async_trait]
    impl MembershipGossipTransportPort for BlockingMembershipTransport {
        async fn exchange(
            &self,
            _recipient: &DeviceId,
            _message: MembershipGossipMessage,
        ) -> Result<MembershipGossipMessage, MembershipGossipTransportError> {
            self.active.store(true, Ordering::Release);
            let _active = ActiveExchangeGuard {
                active: Arc::clone(&self.active),
            };
            self.started.notify_one();
            std::future::pending().await
        }
    }

    fn membership_transport() -> Arc<FixedMembershipTransport> {
        Arc::new(FixedMembershipTransport(Mutex::new(Err(
            MembershipGossipTransportError::Offline,
        ))))
    }

    #[async_trait]
    impl MembershipSecurityUpdatePort for InMemoryMembershipSecurity {
        async fn current_state(
            &self,
        ) -> Result<MembershipSecurityState, MembershipSecurityUpdateError> {
            Ok(self.state.lock().unwrap().clone())
        }

        async fn apply_group_epoch_update(
            &self,
            payload: &[u8],
        ) -> Result<u64, MembershipSecurityUpdateError> {
            self.applied.lock().unwrap().push(payload.to_vec());
            let mut state = self.state.lock().unwrap();
            state.group_epoch += 1;
            Ok(state.group_epoch)
        }
    }

    fn membership_security(group_epoch: u64) -> Arc<InMemoryMembershipSecurity> {
        Arc::new(InMemoryMembershipSecurity {
            state: Mutex::new(MembershipSecurityState {
                space_id: SpaceId::from("space-a"),
                group_epoch,
            }),
            applied: Mutex::new(Vec::new()),
        })
    }

    #[derive(Default)]
    struct InMemoryMemberRepository(Mutex<HashMap<DeviceId, SpaceMember>>);

    #[async_trait]
    impl MemberRepositoryPort for InMemoryMemberRepository {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
            Ok(self.0.lock().unwrap().get(device_id).cloned())
        }

        async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }

        async fn save(&self, member: &SpaceMember) -> Result<(), MembershipError> {
            self.0
                .lock()
                .unwrap()
                .insert(member.device_id, member.clone());
            Ok(())
        }

        async fn remove(&self, device_id: &DeviceId) -> Result<bool, MembershipError> {
            Ok(self.0.lock().unwrap().remove(device_id).is_some())
        }
    }

    #[derive(Default)]
    struct InMemoryTrustedPeerRepository(Mutex<HashMap<DeviceId, TrustedPeer>>);

    #[async_trait]
    impl TrustedPeerRepositoryPort for InMemoryTrustedPeerRepository {
        async fn get(&self, device_id: &DeviceId) -> Result<Option<TrustedPeer>, TrustedPeerError> {
            Ok(self.0.lock().unwrap().get(device_id).cloned())
        }

        async fn list(&self) -> Result<Vec<TrustedPeer>, TrustedPeerError> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }

        async fn save(&self, peer: &TrustedPeer) -> Result<(), TrustedPeerError> {
            self.0
                .lock()
                .unwrap()
                .insert(peer.peer_device_id, peer.clone());
            Ok(())
        }

        async fn remove(&self, device_id: &DeviceId) -> Result<bool, TrustedPeerError> {
            Ok(self.0.lock().unwrap().remove(device_id).is_some())
        }
    }

    #[derive(Default)]
    struct InMemoryPeerAddressRepository(Mutex<HashMap<DeviceId, PeerAddressRecord>>);

    #[async_trait]
    impl PeerAddressRepositoryPort for InMemoryPeerAddressRepository {
        async fn get(
            &self,
            device: &DeviceId,
        ) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
            Ok(self.0.lock().unwrap().get(device).cloned())
        }

        async fn upsert(&self, record: &PeerAddressRecord) -> Result<(), PeerAddressError> {
            self.0
                .lock()
                .unwrap()
                .insert(record.device_id, record.clone());
            Ok(())
        }

        async fn list(&self) -> Result<Vec<PeerAddressRecord>, PeerAddressError> {
            Ok(self.0.lock().unwrap().values().cloned().collect())
        }

        async fn remove(&self, device: &DeviceId) -> Result<(), PeerAddressError> {
            self.0.lock().unwrap().remove(device);
            Ok(())
        }
    }

    struct InMemoryVerifiedPeerPromotion {
        candidates: Arc<InMemoryCandidateRepository>,
        members: Arc<InMemoryMemberRepository>,
        trusted: Arc<InMemoryTrustedPeerRepository>,
        addresses: Arc<InMemoryPeerAddressRepository>,
    }

    #[async_trait]
    impl VerifiedPeerPromotionPort for InMemoryVerifiedPeerPromotion {
        async fn promote_verified_peer(
            &self,
            member: &SpaceMember,
            trusted_peer: &TrustedPeer,
            peer_address: &PeerAddressRecord,
            ready_candidate: &SpaceMembershipCandidate,
        ) -> Result<(), VerifiedPeerPromotionError> {
            let mut candidates = self.candidates.candidates.lock().unwrap();
            let mut members = self.members.0.lock().unwrap();
            let mut trusted = self.trusted.0.lock().unwrap();
            let mut addresses = self.addresses.0.lock().unwrap();
            candidates.insert(
                (
                    ready_candidate.space_id().as_ref().to_owned(),
                    *ready_candidate.device_id(),
                ),
                ready_candidate.clone(),
            );
            members.insert(member.device_id, member.clone());
            trusted.insert(trusted_peer.peer_device_id, trusted_peer.clone());
            addresses.insert(peer_address.device_id, peer_address.clone());
            *self.candidates.save_count.lock().unwrap() += 1;
            Ok(())
        }
    }

    fn in_memory_promotion(
        candidates: Arc<InMemoryCandidateRepository>,
        members: Arc<InMemoryMemberRepository>,
        trusted: Arc<InMemoryTrustedPeerRepository>,
        addresses: Arc<InMemoryPeerAddressRepository>,
    ) -> Arc<dyn VerifiedPeerPromotionPort> {
        Arc::new(InMemoryVerifiedPeerPromotion {
            candidates,
            members,
            trusted,
            addresses,
        })
    }

    struct NoopVerifiedPeerPromotion;

    #[async_trait]
    impl VerifiedPeerPromotionPort for NoopVerifiedPeerPromotion {
        async fn promote_verified_peer(
            &self,
            _member: &SpaceMember,
            _trusted_peer: &TrustedPeer,
            _peer_address: &PeerAddressRecord,
            _ready_candidate: &SpaceMembershipCandidate,
        ) -> Result<(), VerifiedPeerPromotionError> {
            Ok(())
        }
    }

    struct FixedAttestation(Result<VerifiedMembershipPeer, MembershipAttestationError>);

    struct ScriptedAttestation(
        Mutex<VecDeque<Result<VerifiedMembershipPeer, MembershipAttestationError>>>,
    );

    #[async_trait]
    impl MembershipAttestationPort for FixedAttestation {
        async fn attest_candidate(
            &self,
            _candidate: &SpaceMembershipCandidate,
        ) -> Result<VerifiedMembershipPeer, MembershipAttestationError> {
            self.0.clone()
        }
    }

    #[async_trait]
    impl MembershipAttestationPort for ScriptedAttestation {
        async fn attest_candidate(
            &self,
            _candidate: &SpaceMembershipCandidate,
        ) -> Result<VerifiedMembershipPeer, MembershipAttestationError> {
            self.0
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Err(MembershipAttestationError::Offline))
        }
    }

    struct FailingVerifiedPeerPromotion;

    #[async_trait]
    impl VerifiedPeerPromotionPort for FailingVerifiedPeerPromotion {
        async fn promote_verified_peer(
            &self,
            _member: &SpaceMember,
            _trusted_peer: &TrustedPeer,
            _peer_address: &PeerAddressRecord,
            _ready_candidate: &SpaceMembershipCandidate,
        ) -> Result<(), VerifiedPeerPromotionError> {
            Err(VerifiedPeerPromotionError::Repository(
                "forced promotion failure".to_string(),
            ))
        }
    }

    fn fingerprint(raw: &str) -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string(raw).unwrap()
    }

    fn seed(observed_at_ms: i64) -> SponsorCandidateSeed {
        SponsorCandidateSeed {
            space_id: SpaceId::from("space-a"),
            device_id: DeviceId::new("device-c"),
            device_name_hint: "Device C".to_owned(),
            identity_fingerprint_hint: fingerprint("CANDIDATEFP00001"),
            transport_address_blob: b"address-c".to_vec(),
            address_observed_at_ms: observed_at_ms,
            source_device_id: DeviceId::new("device-b"),
            security_updates: vec![RelayedSecurityUpdate {
                previous_epoch: 4,
                next_epoch: 5,
                payload: b"update-4-to-5".to_vec(),
                digest: [4; 32],
            }],
            expires_at_ms: 10_000,
        }
    }

    fn gossip(repo: Arc<InMemoryCandidateRepository>, now_ms: i64) -> SpaceMembershipGossip {
        SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: repo,
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(now_ms)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: Arc::new(InMemoryMemberRepository::default()),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        })
    }

    fn announcement_gossip(
        candidates: Arc<InMemoryCandidateRepository>,
        announcements: Arc<InMemoryAnnouncementRepository>,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
    ) -> SpaceMembershipGossip {
        announcement_gossip_with_members(
            candidates,
            announcements,
            signatures,
            Arc::new(InMemoryMemberRepository::default()),
        )
    }

    fn announcement_gossip_with_members(
        candidates: Arc<InMemoryCandidateRepository>,
        announcements: Arc<InMemoryAnnouncementRepository>,
        signatures: Arc<dyn CurrentMemberSignaturePort>,
        members: Arc<InMemoryMemberRepository>,
    ) -> SpaceMembershipGossip {
        SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates,
            announcement_repo: announcements,
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: signatures,
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(Blake3Hasher),
        })
    }

    fn signed_announcement(sequence: u64, device_name: &str) -> DeviceAnnouncement {
        let mut announcement = DeviceAnnouncement {
            space_id: SpaceId::from("space-a"),
            device_id: DeviceId::new("device-c"),
            device_name: device_name.to_owned(),
            identity_fingerprint: fingerprint("ANNOUNCEMENTFP01"),
            transport_public_key: b"key-c".to_vec(),
            transport_address_blob: b"address-c".to_vec(),
            sequence,
            group_epoch: 4,
            expires_at_ms: 10_000,
            content_digest: [0; 32],
            signature: b"valid-signature".to_vec(),
        };
        announcement.content_digest = *blake3::hash(&announcement.content_bytes()).as_bytes();
        announcement
    }

    fn verified_peer() -> VerifiedMembershipPeer {
        VerifiedMembershipPeer {
            space_id: SpaceId::from("space-a"),
            device_id: DeviceId::new("device-c"),
            device_name: "Device C verified".to_owned(),
            identity_fingerprint: fingerprint("CANDIDATEFP00001"),
            transport_public_key: b"transport-key-c".to_vec(),
            transport_address_blob: b"address-c-verified".to_vec(),
        }
    }

    #[tokio::test]
    async fn sponsor_seed_for_formal_member_does_not_create_pending_candidate() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        members
            .save(&SpaceMember {
                device_id: DeviceId::new("device-c"),
                device_name: "Device C".to_owned(),
                identity_fingerprint: fingerprint("CANDIDATEFP00001"),
                joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });

        assert_eq!(
            gossip.accept_sponsor_seed(seed(100)).await.unwrap(),
            CandidateMergeOutcome::Unchanged
        );
        assert!(candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            gossip
                .convergence_status(&SpaceId::from("space-a"))
                .await
                .unwrap()
                .state,
            MembershipConvergenceState::Complete
        );
    }

    #[tokio::test]
    async fn sponsor_seed_cannot_replace_formal_member_identity() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        members
            .save(&SpaceMember {
                device_id: DeviceId::new("device-c"),
                device_name: "Device C".to_owned(),
                identity_fingerprint: fingerprint("FORMALMEMBERFP01"),
                joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });

        assert!(matches!(
            gossip.accept_sponsor_seed(seed(100)).await,
            Err(SpaceMembershipGossipError::VerificationRejected)
        ));
        assert!(candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn newer_announcement_for_formal_member_keeps_candidate_ready() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let announcements = Arc::new(InMemoryAnnouncementRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        members
            .save(&SpaceMember {
                device_id: DeviceId::new("device-c"),
                device_name: "Device C".to_owned(),
                identity_fingerprint: fingerprint("ANNOUNCEMENTFP01"),
                joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let mut candidate = SpaceMembershipCandidate::from_verified_announcement(
            signed_announcement(1, "Device C"),
            900,
        )
        .unwrap();
        candidate.mark_ready(950);
        candidates.save(&candidate).await.unwrap();
        let gossip = announcement_gossip_with_members(
            candidates.clone(),
            announcements.clone(),
            Arc::new(AcceptingMemberSignatures),
            members,
        );

        assert_eq!(
            gossip
                .accept_verified_announcement(signed_announcement(2, "Device C updated"))
                .await
                .unwrap(),
            CandidateMergeOutcome::Updated
        );
        assert_eq!(
            candidates
                .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
                .await
                .unwrap()
                .unwrap()
                .status(),
            CandidateStatus::Ready
        );
        assert_eq!(
            announcements
                .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
                .await
                .unwrap()
                .unwrap()
                .sequence,
            2
        );
    }

    #[tokio::test]
    async fn announcement_cannot_replace_formal_member_identity() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let announcements = Arc::new(InMemoryAnnouncementRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        members
            .save(&SpaceMember {
                device_id: DeviceId::new("device-c"),
                device_name: "Device C".to_owned(),
                identity_fingerprint: fingerprint("FORMALMEMBERFP01"),
                joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let gossip = announcement_gossip_with_members(
            candidates.clone(),
            announcements.clone(),
            Arc::new(AcceptingMemberSignatures),
            members,
        );

        assert!(matches!(
            gossip
                .accept_verified_announcement(signed_announcement(1, "Device C"))
                .await,
            Err(SpaceMembershipGossipError::VerificationRejected)
        ));
        assert!(announcements
            .list(&SpaceId::from("space-a"))
            .await
            .unwrap()
            .is_empty());
        assert!(candidates
            .list(&SpaceId::from("space-a"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn sponsor_seed_batch_contains_current_members_and_recipient_updates() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let member = |device_id: &str, name: &str, fingerprint_raw: &str| SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: name.to_owned(),
            identity_fingerprint: fingerprint(fingerprint_raw),
            joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
            sync_preferences: uc_core::MemberSyncPreferences::default(),
        };
        for current in [
            member("device-a", "Device A", "AAAAAAAAAAAAAAAA"),
            member("device-b", "Device B", "BBBBBBBBBBBBBBBB"),
            member("device-c", "Device C", "CCCCCCCCCCCCCCCC"),
        ] {
            members.save(&current).await.unwrap();
        }
        addresses
            .upsert(&PeerAddressRecord {
                device_id: DeviceId::new("device-a"),
                addr_blob: b"address-a".to_vec(),
                observed_at: chrono::DateTime::from_timestamp_millis(700).unwrap(),
            })
            .await
            .unwrap();
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates,
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(6),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-b"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: addresses,
            hash: Arc::new(FixedHasher),
        });

        let seeds = gossip
            .build_sponsor_seed_batch(SponsorSeedBatchContext {
                space_id: SpaceId::from("space-a"),
                sponsor_device_id: DeviceId::new("device-b"),
                sponsor_transport_address_blob: b"address-b".to_vec(),
                joiner_device_id: DeviceId::new("device-c"),
                joiner_device_name: "Device C".to_owned(),
                joiner_identity_fingerprint: fingerprint("CCCCCCCCCCCCCCCC"),
                joiner_transport_address_blob: b"address-c".to_vec(),
                group_epoch: 7,
                existing_member_updates: vec![uc_core::membership::PendingGroupUpdate::persistent(
                    DeviceId::new("device-a"),
                    b"epoch-6-to-7".to_vec(),
                )],
            })
            .await
            .unwrap();

        assert_eq!(
            seeds
                .iter()
                .map(|seed| seed.device_id.as_str())
                .collect::<Vec<_>>(),
            vec!["device-a", "device-b"]
        );
        assert_eq!(seeds[0].security_updates.len(), 1);
        assert_eq!(seeds[0].security_updates[0].previous_epoch, 6);
        assert_eq!(seeds[0].security_updates[0].next_epoch, 7);
        assert!(seeds[1].security_updates.is_empty());
    }

    #[tokio::test]
    async fn sponsor_preparation_persists_joiner_seed_for_existing_member() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let outbox = Arc::new(InMemoryMembershipOutbox::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let member = |device_id: &str, name: &str, fingerprint_raw: &str| SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: name.to_owned(),
            identity_fingerprint: fingerprint(fingerprint_raw),
            joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
            sync_preferences: uc_core::MemberSyncPreferences::default(),
        };
        for current in [
            member("device-a", "Device A", "AAAAAAAAAAAAAAAA"),
            member("device-b", "Device B", "BBBBBBBBBBBBBBBB"),
            member("device-c", "Device C", "CCCCCCCCCCCCCCCC"),
        ] {
            members.save(&current).await.unwrap();
        }
        addresses
            .upsert(&PeerAddressRecord {
                device_id: DeviceId::new("device-a"),
                addr_blob: b"address-a".to_vec(),
                observed_at: chrono::DateTime::from_timestamp_millis(700).unwrap(),
            })
            .await
            .unwrap();
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates,
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: outbox.clone(),
            security_updates: membership_security(6),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-b"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: addresses,
            hash: Arc::new(FixedHasher),
        });

        let seeds = gossip
            .prepare_sponsor_membership(SponsorSeedBatchContext {
                space_id: SpaceId::from("space-a"),
                sponsor_device_id: DeviceId::new("device-b"),
                sponsor_transport_address_blob: b"address-b".to_vec(),
                joiner_device_id: DeviceId::new("device-c"),
                joiner_device_name: "Device C".to_owned(),
                joiner_identity_fingerprint: fingerprint("CCCCCCCCCCCCCCCC"),
                joiner_transport_address_blob: b"address-c".to_vec(),
                group_epoch: 7,
                existing_member_updates: vec![uc_core::membership::PendingGroupUpdate::persistent(
                    DeviceId::new("device-a"),
                    b"epoch-6-to-7".to_vec(),
                )],
            })
            .await
            .unwrap();

        assert_eq!(seeds.len(), 2);
        let pending = outbox
            .list_pending(&SpaceId::from("space-a"))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].recipient_device_id(), &DeviceId::new("device-a"));
        assert_eq!(pending[0].batch().batch_id, [9; 32]);
        assert_eq!(pending[0].batch().events.len(), 1);
        let MembershipEvent::SponsorSeed(joiner) = &pending[0].batch().events[0] else {
            panic!("expected sponsor seed event");
        };
        assert_eq!(joiner.device_id, DeviceId::new("device-c"));
        assert_eq!(joiner.source_device_id, DeviceId::new("device-b"));
        assert_eq!(joiner.security_updates.len(), 1);
        assert_eq!(joiner.security_updates[0].previous_epoch, 6);
        assert_eq!(joiner.security_updates[0].next_epoch, 7);
    }

    #[tokio::test]
    async fn sponsor_outbox_failure_still_returns_joiner_recovery_seeds() {
        let members = Arc::new(InMemoryMemberRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let member = |device_id: &str, name: &str, fingerprint_raw: &str| SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: name.to_owned(),
            identity_fingerprint: fingerprint(fingerprint_raw),
            joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
            sync_preferences: uc_core::MemberSyncPreferences::default(),
        };
        for current in [
            member("device-a", "Device A", "AAAAAAAAAAAAAAAA"),
            member("device-b", "Device B", "BBBBBBBBBBBBBBBB"),
            member("device-c", "Device C", "CCCCCCCCCCCCCCCC"),
        ] {
            members.save(&current).await.unwrap();
        }
        addresses
            .upsert(&PeerAddressRecord {
                device_id: DeviceId::new("device-a"),
                addr_blob: b"address-a".to_vec(),
                observed_at: chrono::DateTime::from_timestamp_millis(700).unwrap(),
            })
            .await
            .unwrap();
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(FailingMembershipOutbox),
            security_updates: membership_security(6),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-b"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: addresses,
            hash: Arc::new(FixedHasher),
        });

        let seeds = gossip
            .prepare_sponsor_membership(SponsorSeedBatchContext {
                space_id: SpaceId::from("space-a"),
                sponsor_device_id: DeviceId::new("device-b"),
                sponsor_transport_address_blob: b"address-b".to_vec(),
                joiner_device_id: DeviceId::new("device-c"),
                joiner_device_name: "Device C".to_owned(),
                joiner_identity_fingerprint: fingerprint("CCCCCCCCCCCCCCCC"),
                joiner_transport_address_blob: b"address-c".to_vec(),
                group_epoch: 7,
                existing_member_updates: vec![uc_core::membership::PendingGroupUpdate::persistent(
                    DeviceId::new("device-a"),
                    b"epoch-6-to-7".to_vec(),
                )],
            })
            .await
            .unwrap();

        assert_eq!(
            seeds
                .iter()
                .map(|seed| seed.device_id.as_str())
                .collect::<Vec<_>>(),
            vec!["device-a", "device-b"]
        );
        assert_eq!(seeds[0].security_updates.len(), 1);
    }

    #[tokio::test]
    async fn relayed_security_updates_are_applied_contiguously() {
        let security = Arc::new(InMemoryMembershipSecurity {
            state: Mutex::new(MembershipSecurityState {
                space_id: SpaceId::from("space-a"),
                group_epoch: 4,
            }),
            applied: Mutex::new(Vec::new()),
        });
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: security.clone(),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: Arc::new(InMemoryMemberRepository::default()),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });
        let updates = vec![
            RelayedSecurityUpdate {
                previous_epoch: 4,
                next_epoch: 5,
                payload: b"epoch-4-to-5".to_vec(),
                digest: [9; 32],
            },
            RelayedSecurityUpdate {
                previous_epoch: 5,
                next_epoch: 6,
                payload: b"epoch-5-to-6".to_vec(),
                digest: [9; 32],
            },
        ];

        let epoch = gossip
            .apply_relayed_security_updates(&SpaceId::from("space-a"), &updates)
            .await
            .unwrap();

        assert_eq!(epoch, 6);
        assert_eq!(
            *security.applied.lock().unwrap(),
            vec![b"epoch-4-to-5".to_vec(), b"epoch-5-to-6".to_vec()]
        );

        let repeated_epoch = gossip
            .apply_relayed_security_updates(&SpaceId::from("space-a"), &updates)
            .await
            .unwrap();
        assert_eq!(repeated_epoch, 6);
        assert_eq!(security.applied.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn lost_response_keeps_membership_batch_until_a_matching_ack_arrives() {
        let outbox = Arc::new(InMemoryMembershipOutbox::default());
        let pending = PendingMembershipBatch::new(
            DeviceId::new("device-a"),
            uc_core::membership::MembershipEventBatch {
                space_id: SpaceId::from("space-a"),
                batch_id: [3; 32],
                events: vec![MembershipEvent::SponsorSeed(seed(100))],
            },
            1_000,
        )
        .unwrap();
        outbox.save(&pending).await.unwrap();
        let transport = Arc::new(FixedMembershipTransport(Mutex::new(Err(
            MembershipGossipTransportError::Transport,
        ))));
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: outbox.clone(),
            security_updates: membership_security(4),
            transport: transport.clone(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-b"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: Arc::new(InMemoryMemberRepository::default()),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });

        assert_eq!(
            gossip
                .deliver_pending(&SpaceId::from("space-a"), 1_000)
                .await
                .unwrap(),
            0
        );
        let pending = outbox
            .list_pending(&SpaceId::from("space-a"))
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].attempt_count(), 1);

        *transport.0.lock().unwrap() = Ok(uc_core::membership::MembershipGossipMessage::Ack(
            uc_core::membership::MembershipAck {
                space_id: SpaceId::from("space-a"),
                batch_id: [3; 32],
            },
        ));
        let delivered = gossip
            .deliver_pending(&SpaceId::from("space-a"), pending[0].next_attempt_at_ms())
            .await
            .unwrap();

        assert_eq!(delivered, 1);
        assert!(outbox
            .list_pending(&SpaceId::from("space-a"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn incompatible_existing_member_keeps_convergence_waiting_for_upgrade() {
        let outbox = Arc::new(InMemoryMembershipOutbox::default());
        outbox
            .save(
                &PendingMembershipBatch::new(
                    DeviceId::new("legacy-member"),
                    MembershipEventBatch {
                        space_id: SpaceId::from("space-a"),
                        batch_id: [4; 32],
                        events: Vec::new(),
                    },
                    1_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let transport = Arc::new(FixedMembershipTransport(Mutex::new(Err(
            MembershipGossipTransportError::VersionIncompatible,
        ))));
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: outbox.clone(),
            security_updates: membership_security(4),
            transport: transport.clone(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-b"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: Arc::new(InMemoryMemberRepository::default()),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });

        assert_eq!(
            gossip
                .deliver_pending(&SpaceId::from("space-a"), 1_000)
                .await
                .unwrap(),
            0
        );

        let pending = outbox
            .list_pending(&SpaceId::from("space-a"))
            .await
            .unwrap();
        assert_eq!(
            pending[0].last_failure(),
            Some(CandidateFailure::VersionIncompatible)
        );
        let status = gossip
            .convergence_status(&SpaceId::from("space-a"))
            .await
            .unwrap();
        assert_eq!(status.state, MembershipConvergenceState::WaitingForUpgrade);
        assert_eq!(status.version_incompatible_count, 1);

        *transport.0.lock().unwrap() = Ok(MembershipGossipMessage::Ack(
            uc_core::membership::MembershipAck {
                space_id: SpaceId::from("space-a"),
                batch_id: [4; 32],
            },
        ));
        assert_eq!(
            gossip
                .deliver_pending(&SpaceId::from("space-a"), pending[0].next_attempt_at_ms())
                .await
                .unwrap(),
            1
        );
        let status = gossip
            .convergence_status(&SpaceId::from("space-a"))
            .await
            .unwrap();
        assert_eq!(status.state, MembershipConvergenceState::Complete);
        assert_eq!(status.version_incompatible_count, 0);
    }

    #[tokio::test]
    async fn inbound_event_batch_persists_candidate_and_returns_matching_ack() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members.clone(),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });
        let batch = uc_core::membership::MembershipEventBatch {
            space_id: SpaceId::from("space-a"),
            batch_id: [5; 32],
            events: vec![MembershipEvent::SponsorSeed(seed(100))],
        };

        let response = MembershipGossipEndpointPort::handle_message(
            &gossip,
            &DeviceId::new("device-b"),
            uc_core::membership::MembershipGossipMessage::EventBatch(batch),
        )
        .await
        .unwrap();

        assert_eq!(
            response,
            uc_core::membership::MembershipGossipMessage::Ack(uc_core::membership::MembershipAck {
                space_id: SpaceId::from("space-a"),
                batch_id: [5; 32],
            })
        );
        assert!(candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_some());
        assert!(members.list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn new_sponsor_seed_is_saved_as_pending() {
        let repo = Arc::new(InMemoryCandidateRepository::default());
        let gossip = gossip(repo.clone(), 1_000);

        let outcome = gossip.accept_sponsor_seed(seed(100)).await.unwrap();

        assert_eq!(outcome, CandidateMergeOutcome::Updated);
        let stored = repo
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status(), CandidateStatus::Pending);
    }

    #[tokio::test]
    async fn sponsor_seed_batch_saves_every_candidate() {
        let repo = Arc::new(InMemoryCandidateRepository::default());
        let gossip = gossip(repo.clone(), 1_000);
        let first = seed(100);
        let mut second = seed(200);
        second.device_id = DeviceId::new("device-d");
        second.device_name_hint = "Device D".to_owned();
        second.identity_fingerprint_hint = fingerprint("CANDIDATEFP00002");

        gossip
            .accept_sponsor_seed_batch(vec![first, second])
            .await
            .unwrap();

        assert_eq!(repo.save_count(), 2);
        assert!(repo
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_some());
        assert!(repo
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-d"))
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn repeated_seed_is_idempotent() {
        let repo = Arc::new(InMemoryCandidateRepository::default());
        let gossip = gossip(repo.clone(), 1_000);

        gossip.accept_sponsor_seed(seed(100)).await.unwrap();
        let outcome = gossip.accept_sponsor_seed(seed(100)).await.unwrap();

        assert_eq!(outcome, CandidateMergeOutcome::Unchanged);
        assert_eq!(repo.save_count(), 1);
    }

    #[tokio::test]
    async fn identity_conflict_is_persisted_as_blocked() {
        let repo = Arc::new(InMemoryCandidateRepository::default());
        let gossip = gossip(repo.clone(), 1_000);
        gossip.accept_sponsor_seed(seed(100)).await.unwrap();
        let mut conflicting = seed(200);
        conflicting.identity_fingerprint_hint = fingerprint("CONFLICTFP000001");

        let outcome = gossip.accept_sponsor_seed(conflicting).await.unwrap();

        assert_eq!(outcome, CandidateMergeOutcome::IdentityConflict);
        let stored = repo
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status(), CandidateStatus::Blocked);
    }

    #[tokio::test]
    async fn loading_pending_candidates_removes_expired_records() {
        let repo = Arc::new(InMemoryCandidateRepository::default());
        gossip(repo.clone(), 1_000)
            .accept_sponsor_seed(seed(100))
            .await
            .unwrap();

        let pending = gossip(repo.clone(), 10_000)
            .load_pending(&SpaceId::from("space-a"))
            .await
            .unwrap();

        assert!(pending.is_empty());
        assert!(repo
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn a_new_gossip_instance_recovers_persisted_pending_work() {
        let repo = Arc::new(InMemoryCandidateRepository::default());
        gossip(repo.clone(), 1_000)
            .accept_sponsor_seed(seed(100))
            .await
            .unwrap();

        let recovered = gossip(repo, 2_000)
            .load_pending(&SpaceId::from("space-a"))
            .await
            .unwrap();

        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].device_id(), &DeviceId::new("device-c"));
        assert_eq!(recovered[0].status(), CandidateStatus::Pending);
    }

    #[tokio::test]
    async fn successful_attestation_promotes_all_formal_relationships() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let trusted = Arc::new(InMemoryTrustedPeerRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: in_memory_promotion(
                candidates.clone(),
                members.clone(),
                trusted.clone(),
                addresses.clone(),
            ),
            member_repo: members.clone(),
            trusted_peer_repo: trusted.clone(),
            peer_address_repo: addresses.clone(),
            hash: Arc::new(FixedHasher),
        });
        gossip.accept_sponsor_seed(seed(100)).await.unwrap();

        gossip
            .confirm_candidate(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap();
        gossip
            .confirm_candidate(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap();

        assert_eq!(members.list().await.unwrap().len(), 1);
        assert_eq!(trusted.list().await.unwrap().len(), 1);
        assert_eq!(addresses.list().await.unwrap().len(), 1);
        assert!(members
            .get(&DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_some());
        assert!(trusted
            .get(&DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_some());
        assert!(addresses
            .get(&DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_some());
        let candidate = candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.status(), CandidateStatus::Ready);
    }

    #[tokio::test]
    async fn failed_atomic_promotion_keeps_formal_relationships_hidden() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(FailingVerifiedPeerPromotion),
            member_repo: members.clone(),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });
        gossip.accept_sponsor_seed(seed(100)).await.unwrap();

        let result = gossip
            .confirm_candidate(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await;

        assert!(matches!(
            result,
            Err(SpaceMembershipGossipError::Relationship(_))
        ));
        assert!(members.list().await.unwrap().is_empty());
        let candidate = candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.status(), CandidateStatus::Verifying);
    }

    #[tokio::test]
    async fn reconcile_retries_a_verifying_candidate_after_interruption() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let trusted = Arc::new(InMemoryTrustedPeerRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let mut candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(100), 1_000).unwrap();
        candidate.mark_verifying(1_000);
        candidates.save(&candidate).await.unwrap();
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: in_memory_promotion(
                candidates.clone(),
                members.clone(),
                trusted.clone(),
                addresses.clone(),
            ),
            member_repo: members,
            trusted_peer_repo: trusted,
            peer_address_repo: addresses,
            hash: Arc::new(FixedHasher),
        });

        let outcome = gossip.reconcile_once().await.unwrap();

        assert_eq!(outcome.confirmed_candidates, 1);
        assert_eq!(
            candidates
                .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
                .await
                .unwrap()
                .unwrap()
                .status(),
            CandidateStatus::Ready
        );
    }

    #[tokio::test]
    async fn offline_attestation_keeps_candidate_retryable_without_formal_membership() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Err(MembershipAttestationError::Offline))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members.clone(),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });
        gossip.accept_sponsor_seed(seed(100)).await.unwrap();

        let error = gossip
            .confirm_candidate(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            super::SpaceMembershipGossipError::PeerUnavailable
        ));
        assert!(members.list().await.unwrap().is_empty());
        let candidate = candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.status(), CandidateStatus::WaitingForPeer);
        assert!(candidate.next_attempt_at_ms().unwrap() > 1_000);
    }

    #[tokio::test]
    async fn upgraded_candidate_automatically_completes_on_its_persisted_retry() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let trusted = Arc::new(InMemoryTrustedPeerRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let clock = Arc::new(ManualClock::new(1_000));
        let attestation = Arc::new(ScriptedAttestation(Mutex::new(VecDeque::from([
            Err(MembershipAttestationError::VersionIncompatible),
            Ok(verified_peer()),
        ]))));
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: clock.clone(),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation,
            verified_peer_promotion: in_memory_promotion(
                candidates.clone(),
                members.clone(),
                trusted.clone(),
                addresses.clone(),
            ),
            member_repo: members,
            trusted_peer_repo: trusted,
            peer_address_repo: addresses,
            hash: Arc::new(FixedHasher),
        });
        let mut upgrade_seed = seed(100);
        upgrade_seed.expires_at_ms = 100_000;
        gossip.accept_sponsor_seed(upgrade_seed).await.unwrap();

        let first = gossip.reconcile_once().await.unwrap();

        assert_eq!(first.confirmed_candidates, 0);
        let waiting = candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting.status(), CandidateStatus::WaitingForPeer);
        assert_eq!(
            waiting.last_failure(),
            Some(CandidateFailure::VersionIncompatible)
        );
        assert_eq!(
            gossip
                .convergence_status(&SpaceId::from("space-a"))
                .await
                .unwrap()
                .state,
            MembershipConvergenceState::WaitingForUpgrade
        );

        clock.set(waiting.next_attempt_at_ms().unwrap());
        let second = gossip.reconcile_once().await.unwrap();

        assert_eq!(second.confirmed_candidates, 1);
        assert_eq!(
            candidates
                .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
                .await
                .unwrap()
                .unwrap()
                .status(),
            CandidateStatus::Ready
        );
        assert_eq!(
            gossip
                .convergence_status(&SpaceId::from("space-a"))
                .await
                .unwrap()
                .state,
            MembershipConvergenceState::Complete
        );
    }

    #[tokio::test]
    async fn mismatched_verified_identity_is_rejected_before_formal_membership() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let mut wrong_peer = verified_peer();
        wrong_peer.space_id = SpaceId::from("wrong-space");
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(wrong_peer))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members.clone(),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });
        gossip.accept_sponsor_seed(seed(100)).await.unwrap();

        let error = gossip
            .confirm_candidate(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            super::SpaceMembershipGossipError::VerificationRejected
        ));
        assert!(members.list().await.unwrap().is_empty());
        let candidate = candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.status(), CandidateStatus::Rejected);
    }

    #[tokio::test]
    async fn verified_inbound_peer_is_promoted_even_without_a_prior_candidate() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let trusted = Arc::new(InMemoryTrustedPeerRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: in_memory_promotion(
                candidates.clone(),
                members.clone(),
                trusted.clone(),
                addresses.clone(),
            ),
            member_repo: members.clone(),
            trusted_peer_repo: trusted.clone(),
            peer_address_repo: addresses.clone(),
            hash: Arc::new(FixedHasher),
        });

        gossip.accept_verified_peer(verified_peer()).await.unwrap();

        assert!(members
            .get(&DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_some());
        assert!(trusted
            .get(&DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_some());
        assert!(addresses
            .get(&DeviceId::new("device-c"))
            .await
            .unwrap()
            .is_some());
        let candidate = candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(candidate.status(), CandidateStatus::Ready);
    }

    #[tokio::test]
    async fn simultaneous_attestations_converge_to_one_formal_relationship() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let trusted = Arc::new(InMemoryTrustedPeerRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: in_memory_promotion(
                candidates.clone(),
                members.clone(),
                trusted.clone(),
                addresses.clone(),
            ),
            member_repo: members.clone(),
            trusted_peer_repo: trusted.clone(),
            peer_address_repo: addresses.clone(),
            hash: Arc::new(FixedHasher),
        });

        let (first, second) = tokio::join!(
            gossip.accept_verified_peer(verified_peer()),
            gossip.accept_verified_peer(verified_peer())
        );

        first.unwrap();
        second.unwrap();
        assert_eq!(members.list().await.unwrap().len(), 1);
        assert_eq!(trusted.list().await.unwrap().len(), 1);
        assert_eq!(addresses.list().await.unwrap().len(), 1);
        let stored = candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.status(), CandidateStatus::Ready);
    }

    #[tokio::test]
    async fn digest_requests_newer_announcement_and_missing_request_returns_stored_event() {
        let announcements = Arc::new(InMemoryAnnouncementRepository::default());
        let local_announcement = DeviceAnnouncement {
            space_id: SpaceId::from("space-a"),
            device_id: DeviceId::new("device-a"),
            device_name: "Device A".to_owned(),
            identity_fingerprint: fingerprint("ANNOUNCEMENTFP01"),
            transport_public_key: b"key-a".to_vec(),
            transport_address_blob: b"address-a".to_vec(),
            sequence: 4,
            group_epoch: 4,
            expires_at_ms: 10_000,
            content_digest: [9; 32],
            signature: b"valid-signature".to_vec(),
        };
        announcements.save(&local_announcement).await.unwrap();
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: announcements,
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: membership_transport(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: Arc::new(InMemoryMemberRepository::default()),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });

        let response = MembershipGossipEndpointPort::handle_message(
            &gossip,
            &DeviceId::new("device-b"),
            MembershipGossipMessage::Digest(MembershipDigest {
                space_id: SpaceId::from("space-a"),
                group_epoch: 5,
                group_update_head_digest: Some([5; 32]),
                announcements: vec![MembershipAnnouncementVersion {
                    device_id: DeviceId::new("device-b"),
                    sequence: 2,
                    content_digest: [2; 32],
                }],
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            response,
            MembershipGossipMessage::RequestMissing(MembershipRequestMissing {
                space_id: SpaceId::from("space-a"),
                announcement_devices: vec![DeviceId::new("device-b")],
                security_updates_after_epoch: Some(4),
            })
        );

        let response = MembershipGossipEndpointPort::handle_message(
            &gossip,
            &DeviceId::new("device-b"),
            MembershipGossipMessage::RequestMissing(MembershipRequestMissing {
                space_id: SpaceId::from("space-a"),
                announcement_devices: vec![DeviceId::new("device-a")],
                security_updates_after_epoch: None,
            }),
        )
        .await
        .unwrap();
        let MembershipGossipMessage::EventBatch(batch) = response else {
            panic!("missing request did not return an event batch");
        };
        assert_eq!(
            batch.events,
            vec![MembershipEvent::Announcement(local_announcement)]
        );
    }

    #[tokio::test]
    async fn announcement_rejects_digest_signature_and_fingerprint_tampering() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let announcements = Arc::new(InMemoryAnnouncementRepository::default());
        let accepting = announcement_gossip(
            candidates.clone(),
            announcements.clone(),
            Arc::new(AcceptingMemberSignatures),
        );

        let mut wrong_digest = signed_announcement(1, "Device C");
        wrong_digest.content_digest[0] ^= 1;
        assert!(matches!(
            accepting.accept_verified_announcement(wrong_digest).await,
            Err(SpaceMembershipGossipError::VerificationRejected)
        ));

        let mut wrong_fingerprint = signed_announcement(1, "Device C");
        wrong_fingerprint.identity_fingerprint = fingerprint("DIFFERENTFP00001");
        wrong_fingerprint.content_digest =
            *blake3::hash(&wrong_fingerprint.content_bytes()).as_bytes();
        assert!(matches!(
            accepting
                .accept_verified_announcement(wrong_fingerprint)
                .await,
            Err(SpaceMembershipGossipError::VerificationRejected)
        ));

        let rejecting = announcement_gossip(
            candidates,
            announcements.clone(),
            Arc::new(RejectingMemberSignatures),
        );
        assert!(matches!(
            rejecting
                .accept_verified_announcement(signed_announcement(1, "Device C"))
                .await,
            Err(SpaceMembershipGossipError::VerificationRejected)
        ));
        assert!(announcements
            .list(&SpaceId::from("space-a"))
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn announcement_sequence_never_regresses_and_equal_sequence_conflicts_are_rejected() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let announcements = Arc::new(InMemoryAnnouncementRepository::default());
        let gossip = announcement_gossip(
            candidates,
            announcements.clone(),
            Arc::new(AcceptingMemberSignatures),
        );
        let current = signed_announcement(8, "Device C current");

        assert_eq!(
            gossip
                .accept_verified_announcement(current.clone())
                .await
                .unwrap(),
            CandidateMergeOutcome::Updated
        );
        assert_eq!(
            gossip
                .accept_verified_announcement(current.clone())
                .await
                .unwrap(),
            CandidateMergeOutcome::Unchanged
        );
        assert_eq!(
            gossip
                .accept_verified_announcement(signed_announcement(7, "Device C stale"))
                .await
                .unwrap(),
            CandidateMergeOutcome::Stale
        );
        assert!(matches!(
            gossip
                .accept_verified_announcement(signed_announcement(8, "Device C conflict"))
                .await,
            Err(SpaceMembershipGossipError::VerificationRejected)
        ));
        assert_eq!(
            announcements
                .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
                .await
                .unwrap(),
            Some(current)
        );
    }

    #[tokio::test]
    async fn local_announcement_is_signed_once_and_reused_after_restart() {
        let announcements = Arc::new(InMemoryAnnouncementRepository::default());
        let build = || {
            SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
                candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
                announcement_repo: announcements.clone(),
                outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
                security_updates: membership_security(4),
                transport: membership_transport(),
                clock: Arc::new(FixedClock(1_000)),
                device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
                announcement_material: Arc::new(FixedAnnouncementMaterial),
                member_signatures: Arc::new(AcceptingMemberSignatures),
                fingerprint_factory: Arc::new(FixedFingerprintFactory),
                attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
                verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
                member_repo: Arc::new(InMemoryMemberRepository::default()),
                trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
                peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
                hash: Arc::new(FixedHasher),
            })
        };

        let first = build().refresh_local_announcement().await.unwrap();
        let restored = build().refresh_local_announcement().await.unwrap();

        assert_eq!(first.sequence, 1);
        assert_eq!(restored, first);
        assert_eq!(first.content_digest, [9; 32]);
        assert_eq!(first.signature, b"valid-signature");
        assert_eq!(announcements.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn synchronize_member_completes_digest_request_batch_and_matching_ack() {
        let transport = Arc::new(ScriptedMembershipTransport {
            responses: Mutex::new(VecDeque::from([
                Ok(MembershipGossipMessage::RequestMissing(
                    MembershipRequestMissing {
                        space_id: SpaceId::from("space-a"),
                        announcement_devices: vec![DeviceId::new("device-a")],
                        security_updates_after_epoch: None,
                    },
                )),
                Ok(MembershipGossipMessage::Ack(
                    uc_core::membership::MembershipAck {
                        space_id: SpaceId::from("space-a"),
                        batch_id: [9; 32],
                    },
                )),
            ])),
            sent: Mutex::new(Vec::new()),
        });
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: transport.clone(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: Arc::new(InMemoryMemberRepository::default()),
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        });

        gossip
            .synchronize_member(&DeviceId::new("device-b"))
            .await
            .unwrap();

        let sent = transport.sent.lock().unwrap();
        assert!(matches!(
            sent.first(),
            Some(MembershipGossipMessage::Digest(_))
        ));
        assert!(matches!(
            sent.get(1),
            Some(MembershipGossipMessage::EventBatch(batch))
                if batch.events.len() == 1 && batch.batch_id == [9; 32]
        ));
    }

    #[tokio::test]
    async fn reconcile_once_delivers_due_outbox_and_promotes_due_candidate() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let outbox = Arc::new(InMemoryMembershipOutbox::default());
        let members = Arc::new(InMemoryMemberRepository::default());
        let trusted = Arc::new(InMemoryTrustedPeerRepository::default());
        let addresses = Arc::new(InMemoryPeerAddressRepository::default());
        let pending = PendingMembershipBatch::new(
            DeviceId::new("device-b"),
            uc_core::membership::MembershipEventBatch {
                space_id: SpaceId::from("space-a"),
                batch_id: [3; 32],
                events: Vec::new(),
            },
            1_000,
        )
        .unwrap();
        outbox.save(&pending).await.unwrap();
        let gossip = SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: candidates.clone(),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: outbox.clone(),
            security_updates: membership_security(4),
            transport: Arc::new(FixedMembershipTransport(Mutex::new(Ok(
                MembershipGossipMessage::Ack(uc_core::membership::MembershipAck {
                    space_id: SpaceId::from("space-a"),
                    batch_id: [3; 32],
                }),
            )))),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: in_memory_promotion(
                candidates.clone(),
                members.clone(),
                trusted.clone(),
                addresses.clone(),
            ),
            member_repo: members,
            trusted_peer_repo: trusted,
            peer_address_repo: addresses,
            hash: Arc::new(FixedHasher),
        });
        gossip.accept_sponsor_seed(seed(100)).await.unwrap();

        let outcome = gossip.reconcile_once().await.unwrap();

        assert_eq!(outcome.delivered_batches, 1);
        assert_eq!(outcome.confirmed_candidates, 1);
        assert!(outbox
            .list_pending(&SpaceId::from("space-a"))
            .await
            .unwrap()
            .is_empty());
        assert_eq!(
            candidates
                .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
                .await
                .unwrap()
                .unwrap()
                .status(),
            CandidateStatus::Ready
        );
    }

    #[tokio::test]
    async fn runtime_runs_on_start_pauses_resumes_and_shuts_down() {
        let transport = Arc::new(ScriptedMembershipTransport {
            responses: Mutex::new(VecDeque::from([
                Ok(MembershipGossipMessage::RequestMissing(
                    MembershipRequestMissing {
                        space_id: SpaceId::from("space-a"),
                        announcement_devices: Vec::new(),
                        security_updates_after_epoch: None,
                    },
                )),
                Ok(MembershipGossipMessage::Ack(
                    uc_core::membership::MembershipAck {
                        space_id: SpaceId::from("space-a"),
                        batch_id: [9; 32],
                    },
                )),
                Ok(MembershipGossipMessage::RequestMissing(
                    MembershipRequestMissing {
                        space_id: SpaceId::from("space-a"),
                        announcement_devices: Vec::new(),
                        security_updates_after_epoch: None,
                    },
                )),
                Ok(MembershipGossipMessage::Ack(
                    uc_core::membership::MembershipAck {
                        space_id: SpaceId::from("space-a"),
                        batch_id: [9; 32],
                    },
                )),
            ])),
            sent: Mutex::new(Vec::new()),
        });
        let members = Arc::new(InMemoryMemberRepository::default());
        members
            .save(&SpaceMember {
                device_id: DeviceId::new("device-b"),
                device_name: "Device B".to_owned(),
                identity_fingerprint: fingerprint("BBBBBBBBBBBBBBBB"),
                joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let gossip = Arc::new(SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: transport.clone(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        }));
        let (presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
        let runtime = gossip.start(presence_rx);
        let activity = runtime.activity();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if transport.sent.lock().unwrap().len() >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        activity.pause().await.unwrap();
        presence_tx
            .send(uc_core::ports::PresenceEvent {
                device_id: DeviceId::new("device-b"),
                state: uc_core::ports::ReachabilityState::Online,
                at: chrono::DateTime::from_timestamp_millis(1_100).unwrap(),
            })
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(50), async {
                loop {
                    if transport.sent.lock().unwrap().len() > 2 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_err()
        );

        activity.resume().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if transport.sent.lock().unwrap().len() >= 4 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn local_address_change_immediately_refreshes_and_propagates_announcement() {
        let transport = Arc::new(ScriptedMembershipTransport {
            responses: Mutex::new(VecDeque::from([
                Ok(MembershipGossipMessage::RequestMissing(
                    MembershipRequestMissing {
                        space_id: SpaceId::from("space-a"),
                        announcement_devices: Vec::new(),
                        security_updates_after_epoch: None,
                    },
                )),
                Ok(MembershipGossipMessage::Ack(
                    uc_core::membership::MembershipAck {
                        space_id: SpaceId::from("space-a"),
                        batch_id: [9; 32],
                    },
                )),
                Ok(MembershipGossipMessage::RequestMissing(
                    MembershipRequestMissing {
                        space_id: SpaceId::from("space-a"),
                        announcement_devices: Vec::new(),
                        security_updates_after_epoch: None,
                    },
                )),
                Ok(MembershipGossipMessage::Ack(
                    uc_core::membership::MembershipAck {
                        space_id: SpaceId::from("space-a"),
                        batch_id: [9; 32],
                    },
                )),
            ])),
            sent: Mutex::new(Vec::new()),
        });
        let members = Arc::new(InMemoryMemberRepository::default());
        members
            .save(&SpaceMember {
                device_id: DeviceId::new("device-b"),
                device_name: "Device B".to_owned(),
                identity_fingerprint: fingerprint("BBBBBBBBBBBBBBBB"),
                joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let announcements = Arc::new(InMemoryAnnouncementRepository::default());
        let announcement_material = Arc::new(NotifyingAnnouncementMaterial::new());
        let gossip = Arc::new(SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: announcements.clone(),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: transport.clone(),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: announcement_material.clone(),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        }));
        let (_presence_tx, presence_rx) = tokio::sync::broadcast::channel(4);
        let runtime = gossip.start(presence_rx);

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if transport.sent.lock().unwrap().len() >= 2 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            announcements
                .get(&SpaceId::from("space-a"), &DeviceId::new("device-a"))
                .await
                .unwrap()
                .unwrap()
                .sequence,
            1
        );

        announcement_material.change_address();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if transport.sent.lock().unwrap().len() >= 4 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let updated = announcements
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-a"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.sequence, 2);
        assert_eq!(updated.transport_address_blob, b"address-a-updated");

        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn runtime_pause_interrupts_an_inflight_network_pass() {
        let started = Arc::new(tokio::sync::Notify::new());
        let active = Arc::new(AtomicBool::new(false));
        let members = Arc::new(InMemoryMemberRepository::default());
        members
            .save(&SpaceMember {
                device_id: DeviceId::new("device-b"),
                device_name: "Device B".to_owned(),
                identity_fingerprint: fingerprint("BBBBBBBBBBBBBBBB"),
                joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
        let gossip = Arc::new(SpaceMembershipGossip::new(SpaceMembershipGossipDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            transport: Arc::new(BlockingMembershipTransport {
                started: Arc::clone(&started),
                active: Arc::clone(&active),
            }),
            clock: Arc::new(FixedClock(1_000)),
            device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-a"))),
            announcement_material: Arc::new(FixedAnnouncementMaterial),
            member_signatures: Arc::new(AcceptingMemberSignatures),
            fingerprint_factory: Arc::new(FixedFingerprintFactory),
            attestation: Arc::new(FixedAttestation(Ok(verified_peer()))),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo: members,
            trusted_peer_repo: Arc::new(InMemoryTrustedPeerRepository::default()),
            peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
            hash: Arc::new(FixedHasher),
        }));
        let (_presence_tx, presence_rx) = tokio::sync::broadcast::channel(1);
        let runtime = gossip.start(presence_rx);
        let activity = runtime.activity();
        tokio::time::timeout(Duration::from_secs(1), started.notified())
            .await
            .unwrap();

        tokio::time::timeout(Duration::from_secs(1), activity.pause())
            .await
            .unwrap()
            .unwrap();
        assert!(!active.load(Ordering::Acquire));
        runtime.shutdown().await;
    }

    #[test]
    fn candidate_retry_backs_off_and_has_stable_jitter() {
        let mut candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(100), 1_000).unwrap();
        let first = next_candidate_retry_at(&candidate, 1_000);
        candidate.mark_waiting_for_peer(CandidateFailure::PeerOffline, first, 1_000);
        let second = next_candidate_retry_at(&candidate, 1_000);
        candidate.mark_waiting_for_peer(CandidateFailure::PeerOffline, second, 1_000);
        let third = next_candidate_retry_at(&candidate, 1_000);

        assert!(first >= 31_000);
        assert!(second > first);
        assert!(third > second);
        assert_eq!(third, next_candidate_retry_at(&candidate, 1_000));
    }

    #[tokio::test]
    async fn scheduled_reconcile_uses_the_persisted_candidate_retry_deadline() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let mut candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(100), 1_000).unwrap();
        candidate.mark_waiting_for_peer(CandidateFailure::PeerOffline, 31_500, 1_000);
        candidates.save(&candidate).await.unwrap();
        let gossip = gossip(candidates, 1_000);

        assert_eq!(
            gossip.next_reconcile_delay().await,
            Duration::from_millis(30_500)
        );
    }

    #[tokio::test]
    async fn convergence_status_is_derived_from_persisted_work_after_restart() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let first = gossip(candidates.clone(), 1_000);
        first.accept_sponsor_seed(seed(100)).await.unwrap();

        let status = gossip(candidates.clone(), 1_000)
            .convergence_status(&SpaceId::from("space-a"))
            .await
            .unwrap();
        assert_eq!(status.state, MembershipConvergenceState::Converging);
        assert_eq!(status.pending_count, 1);
        assert_eq!(status.waiting_for_peer_count, 0);

        let mut candidate = candidates
            .get(&SpaceId::from("space-a"), &DeviceId::new("device-c"))
            .await
            .unwrap()
            .unwrap();
        candidate.mark_blocked(CandidateFailure::VersionIncompatible, 1_100);
        candidates.save(&candidate).await.unwrap();
        let status = gossip(candidates, 1_100)
            .convergence_status(&SpaceId::from("space-a"))
            .await
            .unwrap();
        assert_eq!(status.state, MembershipConvergenceState::WaitingForUpgrade);
        assert_eq!(status.version_incompatible_count, 1);
    }

    #[tokio::test]
    async fn current_convergence_status_uses_the_active_membership_space() {
        let candidates = Arc::new(InMemoryCandidateRepository::default());
        let gossip = gossip(candidates, 1_000);
        gossip.accept_sponsor_seed(seed(100)).await.unwrap();

        let status = gossip.current_convergence_status().await.unwrap();

        assert_eq!(status.state, MembershipConvergenceState::Converging);
        assert_eq!(status.pending_count, 1);
    }
}
