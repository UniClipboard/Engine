use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    CandidateEvent, CandidateFailure, CandidateMergeOutcome, CandidateStatus,
    CurrentMemberSignatureError, CurrentMemberSignaturePort, CurrentMembershipAnnouncementMaterial,
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentityError, DeviceAnnouncement,
    MemberRepositoryPort, MembershipAnnouncementRepositoryError,
    MembershipAnnouncementRepositoryPort, MembershipAnnouncementVersion,
    MembershipAppliedSecurityUpdateRepositoryError, MembershipAppliedSecurityUpdateRepositoryPort,
    MembershipAttestationError, MembershipAttestationPort, MembershipCandidateRepositoryError,
    MembershipCandidateRepositoryPort, MembershipDigest, MembershipError, MembershipEvent,
    MembershipEventBatch, MembershipGossipEndpointPort, MembershipGossipMessage,
    MembershipGossipTransportError, MembershipGossipTransportPort, MembershipOutboxRepositoryError,
    MembershipOutboxRepositoryPort, MembershipRequestMissing, MembershipSecurityState,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort, MembershipSharedDevicePage,
    MembershipSharedDevicePageRequest, PendingMembershipBatch, RelayedSecurityUpdate, SpaceMember,
    SpaceMembershipCandidate, SponsorCandidateSeed, VerifiedMembershipPeer,
    VerifiedPeerPromotionError, VerifiedPeerPromotionPort,
};
use uc_core::ports::security::IdentityFingerprintFactoryPort;
use uc_core::ports::{
    ClockPort, ContentHashPort, DeviceIdentityPort, PeerAddressError, PeerAddressRecord,
    PeerAddressRepositoryPort,
};
use uc_core::security::IdentityFingerprint;
use uc_core::trusted_peer::{TrustedPeer, TrustedPeerError, TrustedPeerRepositoryPort};

use super::candidates::next_candidate_retry_at;
use super::{MembershipConvergence, MembershipConvergenceDeps, MembershipConvergenceError};

#[derive(Default)]
struct InMemoryCandidateRepository {
    candidates: Mutex<HashMap<(String, DeviceId), SpaceMembershipCandidate>>,
    save_count: Mutex<usize>,
}

#[derive(Default)]
struct InMemoryMembershipOutbox(Mutex<Vec<PendingMembershipBatch>>);

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
        announcements.retain(|known| &known.space_id != space_id || &known.device_id != device_id);
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
    ) -> Result<uc_core::membership::MembershipGossipMessage, MembershipGossipTransportError> {
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

/// Minimal `MembershipConvergenceDeps` for tests that only need an inert
/// gossip owner (e.g. facade assembly). Every port is an in-memory or
/// fixed-value double; nothing dials or persists.
pub(crate) fn test_deps() -> MembershipConvergenceDeps {
    MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
        transport: membership_transport(),
        clock: Arc::new(FixedClock(0)),
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
    }
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
struct InMemoryAppliedSecurityUpdateRepository(Mutex<Vec<RelayedSecurityUpdate>>);

#[async_trait]
impl MembershipAppliedSecurityUpdateRepositoryPort for InMemoryAppliedSecurityUpdateRepository {
    async fn list(
        &self,
        _space_id: &SpaceId,
    ) -> Result<Vec<RelayedSecurityUpdate>, MembershipAppliedSecurityUpdateRepositoryError> {
        Ok(self.0.lock().unwrap().clone())
    }

    async fn save(
        &self,
        _space_id: &SpaceId,
        update: &RelayedSecurityUpdate,
    ) -> Result<(), MembershipAppliedSecurityUpdateRepositoryError> {
        let mut updates = self.0.lock().unwrap();
        if !updates.iter().any(|known| known.digest == update.digest) {
            updates.push(update.clone());
        }
        Ok(())
    }
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
    async fn get(&self, device: &DeviceId) -> Result<Option<PeerAddressRecord>, PeerAddressError> {
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

fn gossip(repo: Arc<InMemoryCandidateRepository>, now_ms: i64) -> MembershipConvergence {
    MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: repo,
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
) -> MembershipConvergence {
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
) -> MembershipConvergence {
    MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates,
        announcement_repo: announcements,
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
        Err(MembershipConvergenceError::VerificationRejected)
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
    candidate.apply(CandidateEvent::Admitted, 950).unwrap();
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
        Err(MembershipConvergenceError::VerificationRejected)
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
async fn relayed_security_updates_are_applied_contiguously() {
    let security = Arc::new(InMemoryMembershipSecurity {
        state: Mutex::new(MembershipSecurityState {
            space_id: SpaceId::from("space-a"),
            group_epoch: 4,
        }),
        applied: Mutex::new(Vec::new()),
    });
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: security.clone(),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: outbox.clone(),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: outbox.clone(),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
}

#[tokio::test]
async fn inbound_event_batch_persists_candidate_and_returns_matching_ack() {
    let candidates = Arc::new(InMemoryCandidateRepository::default());
    let members = Arc::new(InMemoryMemberRepository::default());
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
        Err(MembershipConvergenceError::Relationship(_))
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
    candidate.apply(CandidateEvent::Confirming, 1_000).unwrap();
    candidates.save(&candidate).await.unwrap();
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
        super::MembershipConvergenceError::PeerUnavailable
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
}

#[tokio::test]
async fn mismatched_verified_identity_is_rejected_before_formal_membership() {
    let candidates = Arc::new(InMemoryCandidateRepository::default());
    let members = Arc::new(InMemoryMemberRepository::default());
    let mut wrong_peer = verified_peer();
    wrong_peer.space_id = SpaceId::from("wrong-space");
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
        super::MembershipConvergenceError::VerificationRejected
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: announcements,
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
async fn shared_device_page_returns_other_members_in_stable_cursor_order() {
    let members = Arc::new(InMemoryMemberRepository::default());
    let addresses = Arc::new(InMemoryPeerAddressRepository::default());
    for (device_id, device_name, fingerprint_raw) in [
        ("device-a", "Device A", "AAAAAAAAAAAAAAAA"),
        ("device-b", "Device B", "BBBBBBBBBBBBBBBB"),
        ("device-c", "Device C", "CCCCCCCCCCCCCCCC"),
        ("device-d", "Device D", "DDDDDDDDDDDDDDDD"),
    ] {
        members
            .save(&SpaceMember {
                device_id: DeviceId::new(device_id),
                device_name: device_name.to_owned(),
                identity_fingerprint: fingerprint(fingerprint_raw),
                joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
                sync_preferences: uc_core::MemberSyncPreferences::default(),
            })
            .await
            .unwrap();
    }
    for device_id in ["device-c", "device-d"] {
        addresses
            .upsert(&PeerAddressRecord {
                device_id: DeviceId::new(device_id),
                addr_blob: format!("address-{device_id}").into_bytes(),
                observed_at: chrono::DateTime::from_timestamp_millis(700).unwrap(),
            })
            .await
            .unwrap();
    }
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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

    let request = |after_device_id| {
        MembershipGossipMessage::RequestSharedDevicePage(MembershipSharedDevicePageRequest {
            space_id: SpaceId::from("space-a"),
            after_device_id,
        })
    };
    let response = MembershipGossipEndpointPort::handle_message(
        &gossip,
        &DeviceId::new("device-a"),
        request(None),
    )
    .await
    .unwrap();
    let MembershipGossipMessage::SharedDevicePage(MembershipSharedDevicePage {
        seeds,
        next_after_device_id,
        ..
    }) = response
    else {
        panic!("shared device request did not return a page");
    };
    assert_eq!(
        seeds
            .iter()
            .map(|seed| seed.device_id.as_str())
            .collect::<Vec<_>>(),
        vec!["device-c", "device-d"]
    );
    assert!(seeds
        .iter()
        .all(|seed| seed.source_device_id == DeviceId::new("device-b")));
    assert_eq!(next_after_device_id, None);

    let response = MembershipGossipEndpointPort::handle_message(
        &gossip,
        &DeviceId::new("device-a"),
        request(Some(DeviceId::new("device-c"))),
    )
    .await
    .unwrap();
    let MembershipGossipMessage::SharedDevicePage(page) = response else {
        panic!("shared device cursor request did not return a page");
    };
    assert_eq!(
        page.seeds
            .iter()
            .map(|seed| seed.device_id.as_str())
            .collect::<Vec<_>>(),
        vec!["device-d"]
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
        Err(MembershipConvergenceError::VerificationRejected)
    ));

    let mut wrong_fingerprint = signed_announcement(1, "Device C");
    wrong_fingerprint.identity_fingerprint = fingerprint("DIFFERENTFP00001");
    wrong_fingerprint.content_digest = *blake3::hash(&wrong_fingerprint.content_bytes()).as_bytes();
    assert!(matches!(
        accepting
            .accept_verified_announcement(wrong_fingerprint)
            .await,
        Err(MembershipConvergenceError::VerificationRejected)
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
        Err(MembershipConvergenceError::VerificationRejected)
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
        Err(MembershipConvergenceError::VerificationRejected)
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
        MembershipConvergence::new(MembershipConvergenceDeps {
            candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
            announcement_repo: announcements.clone(),
            outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
            security_updates: membership_security(4),
            applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: outbox.clone(),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = Arc::new(MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = Arc::new(MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: announcements.clone(),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    let gossip = Arc::new(MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
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
    candidate
        .apply(
            CandidateEvent::AttestationFailed {
                failure: CandidateFailure::PeerOffline,
                retry_at_ms: Some(first),
            },
            1_000,
        )
        .unwrap();
    let second = next_candidate_retry_at(&candidate, 1_000);
    candidate
        .apply(
            CandidateEvent::AttestationFailed {
                failure: CandidateFailure::PeerOffline,
                retry_at_ms: Some(second),
            },
            1_000,
        )
        .unwrap();
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
    candidate
        .apply(
            CandidateEvent::AttestationFailed {
                failure: CandidateFailure::PeerOffline,
                retry_at_ms: Some(31_500),
            },
            1_000,
        )
        .unwrap();
    candidates.save(&candidate).await.unwrap();
    let gossip = gossip(candidates, 1_000);

    assert_eq!(
        gossip.next_reconcile_delay().await,
        Duration::from_millis(30_500)
    );
}

fn verified_peer_b() -> VerifiedMembershipPeer {
    VerifiedMembershipPeer {
        space_id: SpaceId::from("space-a"),
        device_id: DeviceId::new("device-b"),
        device_name: "Device B".to_owned(),
        identity_fingerprint: fingerprint("BBBBBBBBBBBBBBBB"),
        transport_public_key: b"transport-key-b".to_vec(),
        transport_address_blob: b"address-b".to_vec(),
    }
}

fn seed_for_b(observed_at_ms: i64) -> SponsorCandidateSeed {
    SponsorCandidateSeed {
        space_id: SpaceId::from("space-a"),
        device_id: DeviceId::new("device-b"),
        device_name_hint: "Device B".to_owned(),
        identity_fingerprint_hint: fingerprint("BBBBBBBBBBBBBBBB"),
        transport_address_blob: b"address-b".to_vec(),
        address_observed_at_ms: observed_at_ms,
        source_device_id: DeviceId::new("device-a"),
        security_updates: Vec::new(),
        expires_at_ms: 100_000,
    }
}

async fn save_member(
    members: &InMemoryMemberRepository,
    device_id: &str,
    device_name: &str,
    fingerprint_raw: &str,
) {
    members
        .save(&SpaceMember {
            device_id: DeviceId::new(device_id),
            device_name: device_name.to_owned(),
            identity_fingerprint: fingerprint(fingerprint_raw),
            joined_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
            sync_preferences: uc_core::MemberSyncPreferences::default(),
        })
        .await
        .unwrap();
}

async fn save_trusted_peer(
    trusted: &InMemoryTrustedPeerRepository,
    local_device_id: &str,
    peer_device_id: &str,
    fingerprint_raw: &str,
) {
    trusted
        .save(&TrustedPeer {
            local_device_id: DeviceId::new(local_device_id),
            peer_device_id: DeviceId::new(peer_device_id),
            peer_fingerprint: fingerprint(fingerprint_raw),
            trusted_at: chrono::DateTime::from_timestamp_millis(500).unwrap(),
        })
        .await
        .unwrap();
}

struct FixedDeviceAnnouncementMaterial {
    device_id: DeviceId,
    device_name: String,
}

#[async_trait]
impl CurrentMembershipAnnouncementPort for FixedDeviceAnnouncementMaterial {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        Ok(CurrentMembershipAnnouncementMaterial {
            space_id: SpaceId::from("space-a"),
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
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

#[tokio::test]
async fn applied_security_updates_are_saved_and_served_to_missing_peers() {
    let applied = Arc::new(InMemoryAppliedSecurityUpdateRepository::default());
    let security = membership_security(4);
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: security.clone(),
        applied_security_updates: applied.clone(),
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
    let update = RelayedSecurityUpdate {
        previous_epoch: 4,
        next_epoch: 5,
        payload: b"update-4-to-5".to_vec(),
        digest: [9; 32],
    };

    let epoch = gossip
        .apply_relayed_security_updates(&SpaceId::from("space-a"), &[update.clone()])
        .await
        .unwrap();
    assert_eq!(epoch, 5);
    let stored = applied.list(&SpaceId::from("space-a")).await.unwrap();
    assert_eq!(stored, vec![update.clone()]);

    let response = MembershipGossipEndpointPort::handle_message(
        &gossip,
        &DeviceId::new("device-c"),
        MembershipGossipMessage::RequestMissing(MembershipRequestMissing {
            space_id: SpaceId::from("space-a"),
            announcement_devices: Vec::new(),
            security_updates_after_epoch: Some(4),
        }),
    )
    .await
    .unwrap();
    let MembershipGossipMessage::EventBatch(batch) = response else {
        panic!("missing request did not return an event batch");
    };
    assert!(batch
        .events
        .contains(&MembershipEvent::SecurityUpdate(update)));
}

#[tokio::test]
async fn waiting_for_update_candidate_pulls_updates_from_connected_members() {
    let members = Arc::new(InMemoryMemberRepository::default());
    let trusted = Arc::new(InMemoryTrustedPeerRepository::default());
    let candidates = Arc::new(InMemoryCandidateRepository::default());
    let addresses = Arc::new(InMemoryPeerAddressRepository::default());
    save_member(&members, "device-a", "Device A", "AAAAAAAAAAAAAAAA").await;
    save_trusted_peer(&trusted, "device-c", "device-a", "AAAAAAAAAAAAAAAA").await;
    let clock = Arc::new(ManualClock::new(1_000));
    let attestation = Arc::new(ScriptedAttestation(Mutex::new(VecDeque::from([
        Err(MembershipAttestationError::MissingSecurityUpdate),
        Ok(verified_peer_b()),
    ]))));
    let update = RelayedSecurityUpdate {
        previous_epoch: 4,
        next_epoch: 5,
        payload: b"update-4-to-5".to_vec(),
        digest: [9; 32],
    };
    let transport = Arc::new(ScriptedMembershipTransport {
        responses: Mutex::new(VecDeque::from([Ok(MembershipGossipMessage::EventBatch(
            MembershipEventBatch {
                space_id: SpaceId::from("space-a"),
                batch_id: [3; 32],
                events: vec![MembershipEvent::SecurityUpdate(update.clone())],
            },
        ))])),
        sent: Mutex::new(Vec::new()),
    });
    let gossip = Arc::new(MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: candidates.clone(),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: Arc::new(InMemoryAppliedSecurityUpdateRepository::default()),
        transport: transport.clone(),
        clock: clock.clone(),
        device_identity: Arc::new(FixedDeviceIdentity(DeviceId::new("device-c"))),
        announcement_material: Arc::new(FixedDeviceAnnouncementMaterial {
            device_id: DeviceId::new("device-c"),
            device_name: "Device C".to_owned(),
        }),
        member_signatures: Arc::new(AcceptingMemberSignatures),
        fingerprint_factory: Arc::new(FixedFingerprintFactory),
        attestation: attestation.clone(),
        verified_peer_promotion: in_memory_promotion(
            candidates.clone(),
            members.clone(),
            trusted.clone(),
            addresses,
        ),
        member_repo: members.clone(),
        trusted_peer_repo: trusted.clone(),
        peer_address_repo: Arc::new(InMemoryPeerAddressRepository::default()),
        hash: Arc::new(FixedHasher),
    }));

    gossip.accept_sponsor_seed(seed_for_b(100)).await.unwrap();
    gossip
        .confirm_candidate(&SpaceId::from("space-a"), &DeviceId::new("device-b"))
        .await
        .unwrap_err();
    let candidate = candidates
        .get(&SpaceId::from("space-a"), &DeviceId::new("device-b"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(candidate.status(), CandidateStatus::WaitingForUpdate);

    let outcome = gossip.reconcile_once().await.unwrap();
    assert_eq!(outcome.confirmed_candidates, 1);
    assert!(members
        .get(&DeviceId::new("device-b"))
        .await
        .unwrap()
        .is_some());
    assert!(transport
        .sent
        .lock()
        .unwrap()
        .iter()
        .any(|message| matches!(
            message,
            MembershipGossipMessage::RequestMissing(request)
                if request.security_updates_after_epoch == Some(4)
        )));
}

#[tokio::test]
async fn shared_device_page_carries_applied_security_updates() {
    let members = Arc::new(InMemoryMemberRepository::default());
    let addresses = Arc::new(InMemoryPeerAddressRepository::default());
    save_member(&members, "device-b", "Device B", "BBBBBBBBBBBBBBBB").await;
    save_member(&members, "device-c", "Device C", "CCCCCCCCCCCCCCCC").await;
    addresses
        .upsert(&PeerAddressRecord {
            device_id: DeviceId::new("device-b"),
            addr_blob: b"address-b".to_vec(),
            observed_at: chrono::DateTime::from_timestamp_millis(700).unwrap(),
        })
        .await
        .unwrap();
    let applied = Arc::new(InMemoryAppliedSecurityUpdateRepository::default());
    applied
        .save(
            &SpaceId::from("space-a"),
            &RelayedSecurityUpdate {
                previous_epoch: 4,
                next_epoch: 5,
                payload: b"update-4-to-5".to_vec(),
                digest: [9; 32],
            },
        )
        .await
        .unwrap();
    let gossip = MembershipConvergence::new(MembershipConvergenceDeps {
        candidate_repo: Arc::new(InMemoryCandidateRepository::default()),
        announcement_repo: Arc::new(InMemoryAnnouncementRepository::default()),
        outbox_repo: Arc::new(InMemoryMembershipOutbox::default()),
        security_updates: membership_security(4),
        applied_security_updates: applied,
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
        peer_address_repo: addresses,
        hash: Arc::new(FixedHasher),
    });

    let response = MembershipGossipEndpointPort::handle_message(
        &gossip,
        &DeviceId::new("device-c"),
        MembershipGossipMessage::RequestSharedDevicePage(MembershipSharedDevicePageRequest {
            space_id: SpaceId::from("space-a"),
            after_device_id: None,
        }),
    )
    .await
    .unwrap();
    let MembershipGossipMessage::SharedDevicePage(page) = response else {
        panic!("shared device request did not return a page");
    };
    assert_eq!(page.seeds.len(), 1);
    assert_eq!(page.seeds[0].security_updates.len(), 1);
    assert_eq!(page.seeds[0].security_updates[0].next_epoch, 5);
}
