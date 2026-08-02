use serde::{Deserialize, Serialize};

use crate::ids::{DeviceId, SpaceId};
use crate::security::IdentityFingerprint;

const MAX_DEVICE_NAME_BYTES: usize = 256;
const MAX_TRANSPORT_ADDRESS_BYTES: usize = 16 * 1024;
const MAX_TRANSPORT_PUBLIC_KEY_BYTES: usize = 512;
const MAX_ANNOUNCEMENT_SIGNATURE_BYTES: usize = 4 * 1024;
const MAX_SECURITY_UPDATES: usize = 64;
const MAX_GOSSIP_MESSAGE_BYTES: usize = 256 * 1024;
const MAX_SECURITY_UPDATE_BYTES: usize = MAX_GOSSIP_MESSAGE_BYTES - 16 * 1024;
const MAX_SPONSOR_CANDIDATE_SEEDS: usize = 64;
const MAX_GOSSIP_DEVICES: usize = 64;
const MAX_GOSSIP_EVENTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateStatus {
    Pending,
    WaitingForPeer,
    WaitingForUpdate,
    Verifying,
    Ready,
    Blocked,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateFailure {
    PeerOffline,
    AddressUnavailable,
    MissingSecurityUpdate,
    VersionIncompatible,
    IdentityConflict,
    SecurityHistoryConflict,
    InvalidProof,
    Transport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateSource {
    SponsorSeed { sponsor_device_id: DeviceId },
    SelfAnnouncement,
    DirectAttestation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayedSecurityUpdate {
    pub previous_epoch: u64,
    pub next_epoch: u64,
    pub payload: Vec<u8>,
    pub digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorCandidateSeed {
    pub space_id: SpaceId,
    pub device_id: DeviceId,
    pub device_name_hint: String,
    pub identity_fingerprint_hint: IdentityFingerprint,
    pub transport_address_blob: Vec<u8>,
    pub address_observed_at_ms: i64,
    pub source_device_id: DeviceId,
    pub security_updates: Vec<RelayedSecurityUpdate>,
    pub expires_at_ms: i64,
}

impl SponsorCandidateSeed {
    pub fn validate_transfer_bounds(&self) -> Result<(), CandidateMergeError> {
        validate_device_name(&self.device_name_hint)?;
        validate_address(&self.transport_address_blob)?;
        validate_security_updates(&self.security_updates)
    }
}

pub fn validate_sponsor_candidate_seed_batch(
    seeds: &[SponsorCandidateSeed],
) -> Result<(), CandidateMergeError> {
    if seeds.len() > MAX_SPONSOR_CANDIDATE_SEEDS {
        return Err(CandidateMergeError::TooManySponsorSeeds);
    }
    let mut total = 64usize;
    for seed in seeds {
        seed.validate_transfer_bounds()?;
        total = total.saturating_add(estimated_seed_transfer_bytes(seed));
        if total > MAX_GOSSIP_MESSAGE_BYTES {
            return Err(CandidateMergeError::SponsorSeedBatchTooLarge);
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceAnnouncement {
    pub space_id: SpaceId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub identity_fingerprint: IdentityFingerprint,
    pub transport_public_key: Vec<u8>,
    pub transport_address_blob: Vec<u8>,
    pub sequence: u64,
    pub group_epoch: u64,
    pub expires_at_ms: i64,
    pub content_digest: [u8; 32],
    pub signature: Vec<u8>,
}

impl DeviceAnnouncement {
    pub fn content_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        append_announcement_field(&mut bytes, self.space_id.as_ref().as_bytes());
        append_announcement_field(&mut bytes, self.device_id.as_str().as_bytes());
        append_announcement_field(&mut bytes, self.device_name.as_bytes());
        append_announcement_field(
            &mut bytes,
            self.identity_fingerprint.as_display().as_bytes(),
        );
        append_announcement_field(&mut bytes, &self.transport_public_key);
        append_announcement_field(&mut bytes, &self.transport_address_blob);
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        bytes.extend_from_slice(&self.group_epoch.to_be_bytes());
        bytes.extend_from_slice(&self.expires_at_ms.to_be_bytes());
        bytes
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = b"uniclipboard/membership-announcement/1\0".to_vec();
        bytes.extend_from_slice(&self.content_digest);
        bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipAnnouncementVersion {
    pub device_id: DeviceId,
    pub sequence: u64,
    pub content_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipDigest {
    pub space_id: SpaceId,
    pub group_epoch: u64,
    pub group_update_head_digest: Option<[u8; 32]>,
    pub announcements: Vec<MembershipAnnouncementVersion>,
}

impl MembershipDigest {
    pub fn validate_transfer_bounds(&self) -> Result<(), MembershipGossipBoundsError> {
        if self.announcements.len() > MAX_GOSSIP_DEVICES {
            return Err(MembershipGossipBoundsError::TooManyDevices);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipRequestMissing {
    pub space_id: SpaceId,
    pub announcement_devices: Vec<DeviceId>,
    pub security_updates_after_epoch: Option<u64>,
}

impl MembershipRequestMissing {
    pub fn validate_transfer_bounds(&self) -> Result<(), MembershipGossipBoundsError> {
        if self.announcement_devices.len() > MAX_GOSSIP_DEVICES {
            return Err(MembershipGossipBoundsError::TooManyDevices);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipEvent {
    SponsorSeed(SponsorCandidateSeed),
    Announcement(DeviceAnnouncement),
    SecurityUpdate(RelayedSecurityUpdate),
}

impl MembershipEvent {
    fn validate_transfer_bounds(&self) -> Result<(), MembershipGossipBoundsError> {
        match self {
            Self::SponsorSeed(seed) => seed
                .validate_transfer_bounds()
                .map_err(MembershipGossipBoundsError::InvalidEvent),
            Self::Announcement(announcement) => validate_announcement_transfer_bounds(announcement)
                .map_err(MembershipGossipBoundsError::InvalidEvent),
            Self::SecurityUpdate(update) => validate_security_updates(std::slice::from_ref(update))
                .map_err(MembershipGossipBoundsError::InvalidEvent),
        }
    }

    fn estimated_transfer_bytes(&self) -> usize {
        match self {
            Self::SponsorSeed(seed) => estimated_seed_transfer_bytes(seed),
            Self::Announcement(announcement) => {
                announcement.space_id.as_ref().len()
                    + announcement.device_id.as_str().len()
                    + announcement.device_name.len()
                    + announcement.identity_fingerprint.to_string().len()
                    + announcement.transport_public_key.len()
                    + announcement.transport_address_blob.len()
                    + announcement.signature.len()
                    + 96
            }
            Self::SecurityUpdate(update) => update.payload.len() + 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipEventBatch {
    pub space_id: SpaceId,
    pub batch_id: [u8; 32],
    pub events: Vec<MembershipEvent>,
}

impl MembershipEventBatch {
    pub fn validate_transfer_bounds(&self) -> Result<(), MembershipGossipBoundsError> {
        if self.events.len() > MAX_GOSSIP_EVENTS {
            return Err(MembershipGossipBoundsError::TooManyEvents);
        }
        let mut total = self.space_id.as_ref().len().saturating_add(64);
        for event in &self.events {
            event.validate_transfer_bounds()?;
            total = total.saturating_add(event.estimated_transfer_bytes());
            if total > MAX_GOSSIP_MESSAGE_BYTES {
                return Err(MembershipGossipBoundsError::MessageTooLarge);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipAck {
    pub space_id: SpaceId,
    pub batch_id: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipGossipMessage {
    Digest(MembershipDigest),
    RequestMissing(MembershipRequestMissing),
    EventBatch(MembershipEventBatch),
    Ack(MembershipAck),
}

impl MembershipGossipMessage {
    pub fn validate_transfer_bounds(&self) -> Result<(), MembershipGossipBoundsError> {
        match self {
            Self::Digest(digest) => digest.validate_transfer_bounds(),
            Self::RequestMissing(request) => request.validate_transfer_bounds(),
            Self::EventBatch(batch) => batch.validate_transfer_bounds(),
            Self::Ack(_) => Ok(()),
        }
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum MembershipGossipBoundsError {
    #[error("membership gossip contains too many devices")]
    TooManyDevices,
    #[error("membership gossip contains too many events")]
    TooManyEvents,
    #[error("membership gossip message is too large")]
    MessageTooLarge,
    #[error("membership gossip event is invalid: {0}")]
    InvalidEvent(CandidateMergeError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingMembershipBatch {
    recipient_device_id: DeviceId,
    batch: MembershipEventBatch,
    attempt_count: u32,
    next_attempt_at_ms: i64,
    updated_at_ms: i64,
    #[serde(default)]
    last_failure: Option<CandidateFailure>,
}

impl PendingMembershipBatch {
    pub fn new(
        recipient_device_id: DeviceId,
        batch: MembershipEventBatch,
        now_ms: i64,
    ) -> Result<Self, MembershipGossipBoundsError> {
        batch.validate_transfer_bounds()?;
        Ok(Self {
            recipient_device_id,
            batch,
            attempt_count: 0,
            next_attempt_at_ms: now_ms,
            updated_at_ms: now_ms,
            last_failure: None,
        })
    }

    pub fn mark_retry(&mut self, next_attempt_at_ms: i64, now_ms: i64) {
        self.attempt_count = self.attempt_count.saturating_add(1);
        self.next_attempt_at_ms = next_attempt_at_ms;
        self.updated_at_ms = now_ms;
        self.last_failure = None;
    }

    pub fn mark_retry_after(
        &mut self,
        failure: CandidateFailure,
        next_attempt_at_ms: i64,
        now_ms: i64,
    ) {
        self.mark_retry(next_attempt_at_ms, now_ms);
        self.last_failure = Some(failure);
    }

    pub fn recipient_device_id(&self) -> &DeviceId {
        &self.recipient_device_id
    }

    pub fn batch(&self) -> &MembershipEventBatch {
        &self.batch
    }

    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    pub fn next_attempt_at_ms(&self) -> i64 {
        self.next_attempt_at_ms
    }

    pub fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }

    pub fn last_failure(&self) -> Option<CandidateFailure> {
        self.last_failure
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMembershipPeer {
    pub space_id: SpaceId,
    pub device_id: DeviceId,
    pub device_name: String,
    pub identity_fingerprint: IdentityFingerprint,
    pub transport_public_key: Vec<u8>,
    pub transport_address_blob: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateMergeOutcome {
    Updated,
    Unchanged,
    Stale,
    IdentityConflict,
    AnnouncementConflict,
    SecurityHistoryConflict,
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CandidateMergeError {
    #[error("candidate belongs to a different space")]
    SpaceMismatch,
    #[error("candidate belongs to a different device")]
    DeviceMismatch,
    #[error("candidate device name is invalid")]
    InvalidDeviceName,
    #[error("candidate transport address is invalid")]
    InvalidTransportAddress,
    #[error("candidate transport public key is invalid")]
    InvalidTransportPublicKey,
    #[error("candidate announcement signature is invalid")]
    InvalidSignature,
    #[error("candidate expiry is invalid")]
    InvalidExpiry,
    #[error("candidate contains too many security updates")]
    TooManySecurityUpdates,
    #[error("candidate batch contains too many sponsor seeds")]
    TooManySponsorSeeds,
    #[error("candidate sponsor seed batch is too large")]
    SponsorSeedBatchTooLarge,
    #[error("candidate security update is invalid")]
    InvalidSecurityUpdate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceMembershipCandidate {
    space_id: SpaceId,
    device_id: DeviceId,
    device_name_hint: String,
    identity_fingerprint_hint: IdentityFingerprint,
    transport_public_key: Option<Vec<u8>>,
    transport_address_blob: Vec<u8>,
    address_observed_at_ms: i64,
    source: CandidateSource,
    announcement_sequence: Option<u64>,
    announcement_digest: Option<[u8; 32]>,
    group_epoch: Option<u64>,
    security_updates: Vec<RelayedSecurityUpdate>,
    expires_at_ms: i64,
    status: CandidateStatus,
    last_failure: Option<CandidateFailure>,
    attempt_count: u32,
    next_attempt_at_ms: Option<i64>,
    updated_at_ms: i64,
}

impl SpaceMembershipCandidate {
    pub fn from_verified_announcement(
        announcement: DeviceAnnouncement,
        now_ms: i64,
    ) -> Result<Self, CandidateMergeError> {
        validate_announcement(&announcement, now_ms)?;
        Ok(Self {
            space_id: announcement.space_id,
            device_id: announcement.device_id,
            device_name_hint: announcement.device_name,
            identity_fingerprint_hint: announcement.identity_fingerprint,
            transport_public_key: Some(announcement.transport_public_key),
            transport_address_blob: announcement.transport_address_blob,
            address_observed_at_ms: now_ms,
            source: CandidateSource::SelfAnnouncement,
            announcement_sequence: Some(announcement.sequence),
            announcement_digest: Some(announcement.content_digest),
            group_epoch: Some(announcement.group_epoch),
            security_updates: Vec::new(),
            expires_at_ms: announcement.expires_at_ms,
            status: CandidateStatus::Pending,
            last_failure: None,
            attempt_count: 0,
            next_attempt_at_ms: Some(now_ms),
            updated_at_ms: now_ms,
        })
    }

    pub fn from_verified_peer(
        peer: &VerifiedMembershipPeer,
        expires_at_ms: i64,
        now_ms: i64,
    ) -> Result<Self, CandidateMergeError> {
        validate_device_name(&peer.device_name)?;
        validate_address(&peer.transport_address_blob)?;
        validate_expiry(expires_at_ms, now_ms)?;
        if peer.transport_public_key.is_empty()
            || peer.transport_public_key.len() > MAX_TRANSPORT_PUBLIC_KEY_BYTES
        {
            return Err(CandidateMergeError::InvalidTransportPublicKey);
        }
        Ok(Self {
            space_id: peer.space_id.clone(),
            device_id: peer.device_id,
            device_name_hint: peer.device_name.clone(),
            identity_fingerprint_hint: peer.identity_fingerprint.clone(),
            transport_public_key: Some(peer.transport_public_key.clone()),
            transport_address_blob: peer.transport_address_blob.clone(),
            address_observed_at_ms: now_ms,
            source: CandidateSource::DirectAttestation,
            announcement_sequence: None,
            announcement_digest: None,
            group_epoch: None,
            security_updates: Vec::new(),
            expires_at_ms,
            status: CandidateStatus::Verifying,
            last_failure: None,
            attempt_count: 0,
            next_attempt_at_ms: None,
            updated_at_ms: now_ms,
        })
    }

    pub fn from_sponsor_seed(
        seed: SponsorCandidateSeed,
        now_ms: i64,
    ) -> Result<Self, CandidateMergeError> {
        validate_seed(&seed, now_ms)?;
        Ok(Self {
            space_id: seed.space_id,
            device_id: seed.device_id,
            device_name_hint: seed.device_name_hint,
            identity_fingerprint_hint: seed.identity_fingerprint_hint,
            transport_public_key: None,
            transport_address_blob: seed.transport_address_blob,
            address_observed_at_ms: seed.address_observed_at_ms,
            source: CandidateSource::SponsorSeed {
                sponsor_device_id: seed.source_device_id,
            },
            announcement_sequence: None,
            announcement_digest: None,
            group_epoch: None,
            security_updates: seed.security_updates,
            expires_at_ms: seed.expires_at_ms,
            status: CandidateStatus::Pending,
            last_failure: None,
            attempt_count: 0,
            next_attempt_at_ms: Some(now_ms),
            updated_at_ms: now_ms,
        })
    }

    pub fn merge_sponsor_seed(
        &mut self,
        seed: SponsorCandidateSeed,
        now_ms: i64,
    ) -> Result<CandidateMergeOutcome, CandidateMergeError> {
        validate_seed(&seed, now_ms)?;
        self.ensure_same_candidate(&seed.space_id, &seed.device_id)?;
        if self.identity_fingerprint_hint != seed.identity_fingerprint_hint {
            self.block(CandidateFailure::IdentityConflict, now_ms);
            return Ok(CandidateMergeOutcome::IdentityConflict);
        }

        let mut changed = false;
        for update in seed.security_updates {
            if self
                .security_updates
                .iter()
                .any(|known| known.digest == update.digest)
            {
                continue;
            }
            if self.security_updates.iter().any(|known| {
                known.previous_epoch == update.previous_epoch
                    && known.next_epoch == update.next_epoch
            }) {
                self.block(CandidateFailure::SecurityHistoryConflict, now_ms);
                return Ok(CandidateMergeOutcome::SecurityHistoryConflict);
            }
            self.security_updates.push(update);
            changed = true;
        }
        self.security_updates
            .sort_by_key(|update| update.previous_epoch);

        let address_outcome = if seed.address_observed_at_ms > self.address_observed_at_ms {
            self.transport_address_blob = seed.transport_address_blob;
            self.address_observed_at_ms = seed.address_observed_at_ms;
            if !matches!(self.source, CandidateSource::SelfAnnouncement) {
                self.device_name_hint = seed.device_name_hint;
                self.source = CandidateSource::SponsorSeed {
                    sponsor_device_id: seed.source_device_id,
                };
            }
            changed = true;
            CandidateMergeOutcome::Updated
        } else if seed.address_observed_at_ms < self.address_observed_at_ms {
            CandidateMergeOutcome::Stale
        } else if seed.transport_address_blob == self.transport_address_blob {
            CandidateMergeOutcome::Unchanged
        } else {
            CandidateMergeOutcome::Stale
        };

        if seed.expires_at_ms > self.expires_at_ms {
            self.expires_at_ms = seed.expires_at_ms;
            changed = true;
        }
        if changed {
            self.updated_at_ms = now_ms;
            if self.status == CandidateStatus::WaitingForPeer {
                self.status = CandidateStatus::Pending;
                self.last_failure = None;
                self.next_attempt_at_ms = Some(now_ms);
            }
            Ok(CandidateMergeOutcome::Updated)
        } else {
            Ok(address_outcome)
        }
    }

    pub fn merge_verified_announcement(
        &mut self,
        announcement: DeviceAnnouncement,
        now_ms: i64,
    ) -> Result<CandidateMergeOutcome, CandidateMergeError> {
        validate_announcement(&announcement, now_ms)?;
        self.ensure_same_candidate(&announcement.space_id, &announcement.device_id)?;
        if self.identity_fingerprint_hint != announcement.identity_fingerprint {
            self.block(CandidateFailure::IdentityConflict, now_ms);
            return Ok(CandidateMergeOutcome::IdentityConflict);
        }

        if let Some(sequence) = self.announcement_sequence {
            if announcement.sequence < sequence {
                return Ok(CandidateMergeOutcome::Stale);
            }
            if announcement.sequence == sequence {
                if self.announcement_digest == Some(announcement.content_digest) {
                    return Ok(CandidateMergeOutcome::Unchanged);
                }
                self.block(CandidateFailure::IdentityConflict, now_ms);
                return Ok(CandidateMergeOutcome::AnnouncementConflict);
            }
        }

        self.device_name_hint = announcement.device_name;
        self.transport_public_key = Some(announcement.transport_public_key);
        self.transport_address_blob = announcement.transport_address_blob;
        self.address_observed_at_ms = now_ms;
        self.source = CandidateSource::SelfAnnouncement;
        self.announcement_sequence = Some(announcement.sequence);
        self.announcement_digest = Some(announcement.content_digest);
        self.group_epoch = Some(announcement.group_epoch);
        self.expires_at_ms = announcement.expires_at_ms;
        self.updated_at_ms = now_ms;
        if !matches!(
            self.status,
            CandidateStatus::Blocked | CandidateStatus::Rejected
        ) {
            self.status = CandidateStatus::Pending;
            self.last_failure = None;
            self.next_attempt_at_ms = Some(now_ms);
        }
        Ok(CandidateMergeOutcome::Updated)
    }

    fn ensure_same_candidate(
        &self,
        space_id: &SpaceId,
        device_id: &DeviceId,
    ) -> Result<(), CandidateMergeError> {
        if &self.space_id != space_id {
            return Err(CandidateMergeError::SpaceMismatch);
        }
        if &self.device_id != device_id {
            return Err(CandidateMergeError::DeviceMismatch);
        }
        Ok(())
    }

    fn block(&mut self, failure: CandidateFailure, now_ms: i64) {
        self.status = CandidateStatus::Blocked;
        self.last_failure = Some(failure);
        self.next_attempt_at_ms = None;
        self.updated_at_ms = now_ms;
    }

    pub fn mark_waiting_for_peer(
        &mut self,
        failure: CandidateFailure,
        next_attempt_at_ms: i64,
        now_ms: i64,
    ) {
        self.status = CandidateStatus::WaitingForPeer;
        self.last_failure = Some(failure);
        self.attempt_count = self.attempt_count.saturating_add(1);
        self.next_attempt_at_ms = Some(next_attempt_at_ms);
        self.updated_at_ms = now_ms;
    }

    pub fn mark_waiting_for_update(&mut self, now_ms: i64) {
        self.status = CandidateStatus::WaitingForUpdate;
        self.last_failure = Some(CandidateFailure::MissingSecurityUpdate);
        self.next_attempt_at_ms = None;
        self.updated_at_ms = now_ms;
    }

    pub fn mark_verifying(&mut self, now_ms: i64) {
        self.status = CandidateStatus::Verifying;
        self.last_failure = None;
        self.next_attempt_at_ms = None;
        self.updated_at_ms = now_ms;
    }

    pub fn mark_ready(&mut self, now_ms: i64) {
        self.status = CandidateStatus::Ready;
        self.last_failure = None;
        self.next_attempt_at_ms = None;
        self.updated_at_ms = now_ms;
    }

    pub fn mark_rejected(&mut self, failure: CandidateFailure, now_ms: i64) {
        self.status = CandidateStatus::Rejected;
        self.last_failure = Some(failure);
        self.next_attempt_at_ms = None;
        self.updated_at_ms = now_ms;
    }

    pub fn mark_blocked(&mut self, failure: CandidateFailure, now_ms: i64) {
        self.block(failure, now_ms);
    }

    pub fn apply_verified_peer(
        &mut self,
        peer: &VerifiedMembershipPeer,
        now_ms: i64,
    ) -> Result<CandidateMergeOutcome, CandidateMergeError> {
        self.ensure_same_candidate(&peer.space_id, &peer.device_id)?;
        validate_device_name(&peer.device_name)?;
        validate_address(&peer.transport_address_blob)?;
        if peer.transport_public_key.is_empty()
            || peer.transport_public_key.len() > MAX_TRANSPORT_PUBLIC_KEY_BYTES
        {
            return Err(CandidateMergeError::InvalidTransportPublicKey);
        }
        if self.identity_fingerprint_hint != peer.identity_fingerprint {
            self.block(CandidateFailure::IdentityConflict, now_ms);
            return Ok(CandidateMergeOutcome::IdentityConflict);
        }
        self.device_name_hint = peer.device_name.clone();
        self.transport_public_key = Some(peer.transport_public_key.clone());
        self.transport_address_blob = peer.transport_address_blob.clone();
        self.address_observed_at_ms = now_ms;
        self.source = CandidateSource::DirectAttestation;
        self.updated_at_ms = now_ms;
        Ok(CandidateMergeOutcome::Updated)
    }

    pub fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    pub fn device_name_hint(&self) -> &str {
        &self.device_name_hint
    }

    pub fn identity_fingerprint_hint(&self) -> &IdentityFingerprint {
        &self.identity_fingerprint_hint
    }

    pub fn transport_public_key(&self) -> Option<&[u8]> {
        self.transport_public_key.as_deref()
    }

    pub fn transport_address_blob(&self) -> &[u8] {
        &self.transport_address_blob
    }

    pub fn address_observed_at_ms(&self) -> i64 {
        self.address_observed_at_ms
    }

    pub fn source(&self) -> &CandidateSource {
        &self.source
    }

    pub fn announcement_sequence(&self) -> Option<u64> {
        self.announcement_sequence
    }

    pub fn announcement_digest(&self) -> Option<[u8; 32]> {
        self.announcement_digest
    }

    pub fn group_epoch(&self) -> Option<u64> {
        self.group_epoch
    }

    pub fn security_updates(&self) -> &[RelayedSecurityUpdate] {
        &self.security_updates
    }

    pub fn expires_at_ms(&self) -> i64 {
        self.expires_at_ms
    }

    pub fn status(&self) -> CandidateStatus {
        self.status
    }

    pub fn last_failure(&self) -> Option<CandidateFailure> {
        self.last_failure
    }

    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    pub fn next_attempt_at_ms(&self) -> Option<i64> {
        self.next_attempt_at_ms
    }

    pub fn updated_at_ms(&self) -> i64 {
        self.updated_at_ms
    }
}

fn validate_seed(seed: &SponsorCandidateSeed, now_ms: i64) -> Result<(), CandidateMergeError> {
    seed.validate_transfer_bounds()?;
    validate_expiry(seed.expires_at_ms, now_ms)?;
    Ok(())
}

fn estimated_seed_transfer_bytes(seed: &SponsorCandidateSeed) -> usize {
    seed.space_id.as_ref().len()
        + seed.device_id.as_str().len()
        + seed.device_name_hint.len()
        + seed.identity_fingerprint_hint.to_string().len()
        + seed.transport_address_blob.len()
        + seed.source_device_id.as_str().len()
        + seed
            .security_updates
            .iter()
            .map(|update| update.payload.len() + 64)
            .sum::<usize>()
        + 64
}

fn validate_announcement(
    announcement: &DeviceAnnouncement,
    now_ms: i64,
) -> Result<(), CandidateMergeError> {
    validate_announcement_transfer_bounds(announcement)?;
    validate_expiry(announcement.expires_at_ms, now_ms)
}

fn validate_announcement_transfer_bounds(
    announcement: &DeviceAnnouncement,
) -> Result<(), CandidateMergeError> {
    validate_device_name(&announcement.device_name)?;
    validate_address(&announcement.transport_address_blob)?;
    if announcement.transport_public_key.is_empty()
        || announcement.transport_public_key.len() > MAX_TRANSPORT_PUBLIC_KEY_BYTES
    {
        return Err(CandidateMergeError::InvalidTransportPublicKey);
    }
    if announcement.signature.is_empty()
        || announcement.signature.len() > MAX_ANNOUNCEMENT_SIGNATURE_BYTES
    {
        return Err(CandidateMergeError::InvalidSignature);
    }
    Ok(())
}

fn validate_device_name(name: &str) -> Result<(), CandidateMergeError> {
    if name.trim().is_empty() || name.len() > MAX_DEVICE_NAME_BYTES {
        return Err(CandidateMergeError::InvalidDeviceName);
    }
    Ok(())
}

fn validate_address(address: &[u8]) -> Result<(), CandidateMergeError> {
    if address.is_empty() || address.len() > MAX_TRANSPORT_ADDRESS_BYTES {
        return Err(CandidateMergeError::InvalidTransportAddress);
    }
    Ok(())
}

fn validate_expiry(expires_at_ms: i64, now_ms: i64) -> Result<(), CandidateMergeError> {
    if expires_at_ms <= now_ms {
        return Err(CandidateMergeError::InvalidExpiry);
    }
    Ok(())
}

fn validate_security_updates(updates: &[RelayedSecurityUpdate]) -> Result<(), CandidateMergeError> {
    if updates.len() > MAX_SECURITY_UPDATES {
        return Err(CandidateMergeError::TooManySecurityUpdates);
    }
    for update in updates {
        if update.payload.is_empty()
            || update.payload.len() > MAX_SECURITY_UPDATE_BYTES
            || update.next_epoch != update.previous_epoch.saturating_add(1)
        {
            return Err(CandidateMergeError::InvalidSecurityUpdate);
        }
    }
    Ok(())
}

fn append_announcement_field(bytes: &mut Vec<u8>, field: &[u8]) {
    bytes.extend_from_slice(&(field.len() as u64).to_be_bytes());
    bytes.extend_from_slice(field);
}

#[cfg(test)]
mod tests {
    use crate::ids::{DeviceId, SpaceId};
    use crate::security::IdentityFingerprint;

    use super::{
        CandidateFailure, CandidateMergeError, CandidateMergeOutcome, CandidateSource,
        CandidateStatus, DeviceAnnouncement, MembershipAnnouncementVersion, MembershipDigest,
        MembershipEvent, MembershipEventBatch, MembershipRequestMissing, PendingMembershipBatch,
        RelayedSecurityUpdate, SpaceMembershipCandidate, SponsorCandidateSeed,
        MAX_GOSSIP_MESSAGE_BYTES,
    };

    fn fingerprint(raw: &str) -> IdentityFingerprint {
        IdentityFingerprint::from_raw_string(raw).unwrap()
    }

    fn seed(observed_at_ms: i64) -> SponsorCandidateSeed {
        SponsorCandidateSeed {
            space_id: SpaceId::from("space-a"),
            device_id: DeviceId::new("device-a"),
            device_name_hint: "Alice laptop".to_string(),
            identity_fingerprint_hint: fingerprint("CANDIDATEFP00001"),
            transport_address_blob: b"address-v1".to_vec(),
            address_observed_at_ms: observed_at_ms,
            source_device_id: DeviceId::new("sponsor-b"),
            security_updates: vec![RelayedSecurityUpdate {
                previous_epoch: 4,
                next_epoch: 5,
                payload: b"epoch-4-to-5".to_vec(),
                digest: [4; 32],
            }],
            expires_at_ms: 50_000,
        }
    }

    #[test]
    fn sponsor_seed_creates_pending_candidate_without_granting_trust() {
        let candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(100), 1_000).unwrap();

        assert_eq!(candidate.status(), CandidateStatus::Pending);
        assert_eq!(
            candidate.source(),
            &CandidateSource::SponsorSeed {
                sponsor_device_id: DeviceId::new("sponsor-b")
            }
        );
        assert_eq!(candidate.transport_address_blob(), b"address-v1");
        assert_eq!(candidate.security_updates().len(), 1);
    }

    #[test]
    fn gossip_digest_and_request_reject_more_than_sixty_four_devices() {
        let versions = (0..65)
            .map(|index| MembershipAnnouncementVersion {
                device_id: DeviceId::new(format!("device-{index}")),
                sequence: 1,
                content_digest: [index as u8; 32],
            })
            .collect::<Vec<_>>();
        let digest = MembershipDigest {
            space_id: SpaceId::from("space-a"),
            group_epoch: 5,
            group_update_head_digest: Some([5; 32]),
            announcements: versions,
        };
        assert!(digest.validate_transfer_bounds().is_err());

        let request = MembershipRequestMissing {
            space_id: SpaceId::from("space-a"),
            announcement_devices: (0..65)
                .map(|index| DeviceId::new(format!("device-{index}")))
                .collect(),
            security_updates_after_epoch: Some(4),
        };
        assert!(request.validate_transfer_bounds().is_err());
    }

    #[test]
    fn gossip_event_batch_rejects_total_payload_over_message_limit() {
        let update = |byte| {
            MembershipEvent::SecurityUpdate(RelayedSecurityUpdate {
                previous_epoch: u64::from(byte),
                next_epoch: u64::from(byte) + 1,
                payload: vec![byte; 140 * 1024],
                digest: [byte; 32],
            })
        };
        let batch = MembershipEventBatch {
            space_id: SpaceId::from("space-a"),
            batch_id: [7; 32],
            events: vec![update(1), update(2)],
        };

        assert!(batch.validate_transfer_bounds().is_err());
    }

    #[test]
    fn single_security_update_leaves_room_for_the_message_envelope() {
        let mut seed = seed(100);
        seed.security_updates[0].payload = vec![1; MAX_GOSSIP_MESSAGE_BYTES];

        assert!(matches!(
            seed.validate_transfer_bounds(),
            Err(CandidateMergeError::InvalidSecurityUpdate)
        ));
    }

    #[test]
    fn sponsor_seed_batch_rejects_total_payload_over_message_limit() {
        let mut first = seed(100);
        first.security_updates[0].payload = vec![1; 140 * 1024];
        let mut second = seed(200);
        second.device_id = DeviceId::new("device-b");
        second.security_updates[0].payload = vec![2; 140 * 1024];

        assert!(super::validate_sponsor_candidate_seed_batch(&[first, second]).is_err());
    }

    #[test]
    fn pending_membership_batch_starts_immediately_and_records_retry() {
        let batch = MembershipEventBatch {
            space_id: SpaceId::from("space-a"),
            batch_id: [9; 32],
            events: vec![MembershipEvent::SponsorSeed(seed(100))],
        };
        let mut pending =
            PendingMembershipBatch::new(DeviceId::new("device-b"), batch, 1_000).unwrap();

        assert_eq!(pending.next_attempt_at_ms(), 1_000);
        assert_eq!(pending.attempt_count(), 0);

        pending.mark_retry(31_000, 1_100);

        assert_eq!(pending.next_attempt_at_ms(), 31_000);
        assert_eq!(pending.attempt_count(), 1);
        assert_eq!(pending.updated_at_ms(), 1_100);
    }

    #[test]
    fn pending_batch_remembers_that_the_recipient_requires_an_upgrade() {
        let mut pending = PendingMembershipBatch::new(
            DeviceId::new("legacy-member"),
            MembershipEventBatch {
                space_id: SpaceId::from("space-a"),
                batch_id: [7; 32],
                events: Vec::new(),
            },
            1_000,
        )
        .unwrap();

        pending.mark_retry_after(CandidateFailure::VersionIncompatible, 31_000, 1_100);

        assert_eq!(
            pending.last_failure(),
            Some(CandidateFailure::VersionIncompatible)
        );
        assert_eq!(pending.next_attempt_at_ms(), 31_000);
    }

    #[test]
    fn repeated_or_older_seed_does_not_replace_newer_address() {
        let mut candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(200), 1_000).unwrap();

        assert_eq!(
            candidate.merge_sponsor_seed(seed(200), 1_100).unwrap(),
            CandidateMergeOutcome::Unchanged
        );
        let mut older = seed(199);
        older.transport_address_blob = b"stale-address".to_vec();
        assert_eq!(
            candidate.merge_sponsor_seed(older, 1_200).unwrap(),
            CandidateMergeOutcome::Stale
        );
        assert_eq!(candidate.transport_address_blob(), b"address-v1");
    }

    #[test]
    fn newer_seed_replaces_address_and_merges_new_security_update() {
        let mut candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(100), 1_000).unwrap();
        let mut newer = seed(300);
        newer.transport_address_blob = b"address-v2".to_vec();
        newer.security_updates.push(RelayedSecurityUpdate {
            previous_epoch: 5,
            next_epoch: 6,
            payload: b"epoch-5-to-6".to_vec(),
            digest: [5; 32],
        });

        assert_eq!(
            candidate.merge_sponsor_seed(newer, 1_100).unwrap(),
            CandidateMergeOutcome::Updated
        );
        assert_eq!(candidate.transport_address_blob(), b"address-v2");
        assert_eq!(candidate.security_updates().len(), 2);
    }

    #[test]
    fn conflicting_identity_blocks_candidate() {
        let mut candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(100), 1_000).unwrap();
        let mut conflict = seed(200);
        conflict.identity_fingerprint_hint = fingerprint("CONFLICTFP000001");

        assert_eq!(
            candidate.merge_sponsor_seed(conflict, 1_100).unwrap(),
            CandidateMergeOutcome::IdentityConflict
        );
        assert_eq!(candidate.status(), CandidateStatus::Blocked);
    }

    fn announcement(sequence: u64, digest_seed: u8) -> DeviceAnnouncement {
        DeviceAnnouncement {
            space_id: SpaceId::from("space-a"),
            device_id: DeviceId::new("device-a"),
            device_name: "Alice laptop verified".to_string(),
            identity_fingerprint: fingerprint("CANDIDATEFP00001"),
            transport_public_key: b"iroh-public-key".to_vec(),
            transport_address_blob: b"self-address".to_vec(),
            sequence,
            group_epoch: 5,
            expires_at_ms: 60_000,
            content_digest: [digest_seed; 32],
            signature: b"member-signature".to_vec(),
        }
    }

    #[test]
    fn verified_announcement_replaces_seed_but_sequence_never_goes_backwards() {
        let mut candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(100), 1_000).unwrap();

        assert_eq!(
            candidate
                .merge_verified_announcement(announcement(8, 8), 1_100)
                .unwrap(),
            CandidateMergeOutcome::Updated
        );
        assert_eq!(candidate.announcement_sequence(), Some(8));
        assert_eq!(candidate.source(), &CandidateSource::SelfAnnouncement);

        assert_eq!(
            candidate
                .merge_verified_announcement(announcement(7, 7), 1_200)
                .unwrap(),
            CandidateMergeOutcome::Stale
        );
        assert_eq!(candidate.announcement_sequence(), Some(8));
    }

    #[test]
    fn same_announcement_sequence_with_different_content_blocks_candidate() {
        let mut candidate = SpaceMembershipCandidate::from_sponsor_seed(seed(100), 1_000).unwrap();
        candidate
            .merge_verified_announcement(announcement(8, 8), 1_100)
            .unwrap();

        assert_eq!(
            candidate
                .merge_verified_announcement(announcement(8, 9), 1_200)
                .unwrap(),
            CandidateMergeOutcome::AnnouncementConflict
        );
        assert_eq!(candidate.status(), CandidateStatus::Blocked);
    }
}
