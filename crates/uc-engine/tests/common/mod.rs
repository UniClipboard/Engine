//! 集成测试共享 helper：switch-space 4 个新 port 的最小 noop 实现。
//!
//! 现有 slice1/2 e2e 流程不驱动 switch-space，但 `SpaceFacadeDeps` 现在
//! 强制要求这 4 个字段；本模块给出 trivial 替身让旧测试继续编译，无需
//! 给每个 e2e 拆出一组拷贝。
//!
//! 这些替身**仅适用于不走 switch-space 路径的测试**。一旦某个测试需要
//! 验证迁移行为，应该换成真实 adapter（`FileMigrationStateRepository` /
//! `DefaultKeyMigrationAdapter` / `DieselBlobMigrationRepository` /
//! `BlobCipherAdapter`）或 mockall 替身。

use std::sync::Arc;

use async_trait::async_trait;

use uc_application::facade::clipboard_write::MobileConsumableBackfill;
use uc_application::facade::AutomaticLegacyUpgradeDeps;
use uc_core::crypto::domain::{Aad, ActiveSpace, Ciphertext, Plaintext};
use uc_core::ids::{DeviceId, EventId, RepresentationId, SpaceId};
use uc_core::membership::{
    DeviceAnnouncement, GroupEpoch, GroupRevocationPort, GroupRevocationResult,
    GroupUpdateDispatchError, GroupUpdateDispatchPort, KeyEpochError, LegacyProtectionCommand,
    LegacyProtectionPort, LegacyProtectionResult, LegacyProtectionSnapshot,
    LegacyRequestInspection, LegacyUpgradeDispatchError, LegacyUpgradeDispatchPort,
    LegacyUpgradeError, LegacyUpgradeRequest, LegacyUpgradeResponse, MemberRepositoryPort,
    MembershipAnnouncementRepositoryError, MembershipAnnouncementRepositoryPort,
    MembershipAppliedSecurityUpdateRepositoryError, MembershipAppliedSecurityUpdateRepositoryPort,
    MembershipAttestationError, MembershipAttestationPort, MembershipCandidateRepositoryError,
    MembershipCandidateRepositoryPort, MembershipError, MembershipGossipMessage,
    MembershipGossipTransportError, MembershipGossipTransportPort, MembershipOutboxRepositoryError,
    MembershipOutboxRepositoryPort, PendingGroupUpdate, PendingMembershipBatch,
    RelationshipStateResetError, RelationshipStateResetPort, RelayedSecurityUpdate, RevocationId,
    SpaceMember, SpaceMembershipCandidate, SpaceSecurityStateResetError,
    SpaceSecurityStateResetPort, VerifiedMembershipPeer, VerifiedPeerPromotionError,
    VerifiedPeerPromotionPort,
};
use uc_core::ports::clipboard::{BlobMigrationRepoError, BlobMigrationRepoPort, MigrationRecord};
use uc_core::ports::security::{
    BlobCipherError, BlobCipherPort, IdentityFingerprintFactoryPort, KeyMigrationError,
    KeyMigrationPort,
};
use uc_core::ports::setup::{MigrationStateError, MigrationStatePort};
use uc_core::ports::{ContentHashPort, PeerAddressRecord};
use uc_core::security::IdentityFingerprint;
use uc_core::setup::{MigrationPhase, MigrationRunId};
use uc_core::trusted_peer::TrustedPeer;

pub struct NoopMobileConsumableBackfill;

#[async_trait]
impl MobileConsumableBackfill for NoopMobileConsumableBackfill {
    async fn backfill(
        &self,
    ) -> Result<bool, uc_core::ports::clipboard::ActiveClipboardRegisterError> {
        Ok(false)
    }
}

pub fn mobile_consumable_backfill_noop() -> Arc<dyn MobileConsumableBackfill> {
    Arc::new(NoopMobileConsumableBackfill)
}

pub struct NoopRelationshipStateReset;

#[async_trait]
impl RelationshipStateResetPort for NoopRelationshipStateReset {
    async fn clear_all_relationships(&self) -> Result<(), RelationshipStateResetError> {
        Ok(())
    }
}

pub fn relationship_state_reset_noop() -> Arc<dyn RelationshipStateResetPort> {
    Arc::new(NoopRelationshipStateReset)
}

pub struct NoopSpaceSecurityStateReset;

#[async_trait]
impl SpaceSecurityStateResetPort for NoopSpaceSecurityStateReset {
    async fn clear_space_security_state_except(
        &self,
        _active_space_id: &SpaceId,
    ) -> Result<(), SpaceSecurityStateResetError> {
        Ok(())
    }
}

pub fn space_security_state_reset_noop() -> Arc<dyn SpaceSecurityStateResetPort> {
    Arc::new(NoopSpaceSecurityStateReset)
}

pub struct NoopMigrationState;

#[async_trait]
impl MigrationStatePort for NoopMigrationState {
    async fn get_current(&self) -> Result<Option<MigrationPhase>, MigrationStateError> {
        Ok(None)
    }
    async fn set_current(&self, _phase: Option<MigrationPhase>) -> Result<(), MigrationStateError> {
        Ok(())
    }
}

pub struct NoopKeyMigration;

#[async_trait]
impl KeyMigrationPort for NoopKeyMigration {
    async fn prepare_migration_key(&self) -> Result<MigrationRunId, KeyMigrationError> {
        Ok(MigrationRunId::new("e2e-noop-run"))
    }
    async fn encrypt_with_migration_key(
        &self,
        _run_id: &MigrationRunId,
        plaintext: &Plaintext,
        _aad: &Aad,
    ) -> Result<Ciphertext, KeyMigrationError> {
        Ok(Ciphertext::new(plaintext.as_bytes().to_vec()))
    }
    async fn decrypt_with_migration_key(
        &self,
        _run_id: &MigrationRunId,
        ciphertext: &Ciphertext,
        _aad: &Aad,
    ) -> Result<Plaintext, KeyMigrationError> {
        Ok(Plaintext::new(ciphertext.as_bytes().to_vec()))
    }
    async fn discard_migration_key(
        &self,
        _run_id: &MigrationRunId,
    ) -> Result<(), KeyMigrationError> {
        Ok(())
    }
}

pub struct NoopBlobMigrationRepo;

#[async_trait]
impl BlobMigrationRepoPort for NoopBlobMigrationRepo {
    async fn list_main_inline_representations(
        &self,
    ) -> Result<Vec<(EventId, RepresentationId)>, BlobMigrationRepoError> {
        Ok(Vec::new())
    }
    async fn read_main_inline_data(
        &self,
        _event_id: &EventId,
        _representation_id: &RepresentationId,
    ) -> Result<Option<Vec<u8>>, BlobMigrationRepoError> {
        Ok(None)
    }
    async fn upsert_record(&self, _record: &MigrationRecord) -> Result<(), BlobMigrationRepoError> {
        Ok(())
    }
    async fn count_records(&self) -> Result<u64, BlobMigrationRepoError> {
        Ok(0)
    }
    async fn list_records(&self) -> Result<Vec<MigrationRecord>, BlobMigrationRepoError> {
        Ok(Vec::new())
    }
    async fn update_main_inline_data(
        &self,
        _event_id: &EventId,
        _representation_id: &RepresentationId,
        _new_ciphertext: &[u8],
    ) -> Result<(), BlobMigrationRepoError> {
        Ok(())
    }
    async fn mark_unreadable_inline_data(
        &self,
        _event_id: &EventId,
        _representation_id: &RepresentationId,
    ) -> Result<(), BlobMigrationRepoError> {
        Ok(())
    }
    async fn discard_all_records(&self) -> Result<(), BlobMigrationRepoError> {
        Ok(())
    }
}

pub struct NoopBlobCipher;

#[async_trait]
impl BlobCipherPort for NoopBlobCipher {
    async fn encrypt(
        &self,
        _space: &ActiveSpace,
        plaintext: &Plaintext,
        _aad: &Aad,
    ) -> Result<Ciphertext, BlobCipherError> {
        Ok(Ciphertext::new(plaintext.as_bytes().to_vec()))
    }
    async fn decrypt(
        &self,
        _space: &ActiveSpace,
        ciphertext: &Ciphertext,
        _aad: &Aad,
    ) -> Result<Plaintext, BlobCipherError> {
        Ok(Plaintext::new(ciphertext.as_bytes().to_vec()))
    }
}

/// Convenience tuple for splatting all 4 new SpaceFacadeDeps fields at once.
///
/// 用法：
/// ```ignore
/// let mig = common::migration_noop_deps();
/// SpaceFacadeDeps {
///     // ... 旧字段
///     migration_state: mig.0,
///     key_migration: mig.1,
///     blob_migration_repo: mig.2,
///     blob_cipher: mig.3,
/// }
/// ```
pub fn migration_noop_deps() -> (
    Arc<dyn MigrationStatePort>,
    Arc<dyn KeyMigrationPort>,
    Arc<dyn BlobMigrationRepoPort>,
    Arc<dyn BlobCipherPort>,
) {
    (
        Arc::new(NoopMigrationState),
        Arc::new(NoopKeyMigration),
        Arc::new(NoopBlobMigrationRepo),
        Arc::new(NoopBlobCipher),
    )
}

// ───────────────────────────────────────────────────────────────────────
// Workspace convergence test owner (ADR-017): the pairing e2e tests drive
// the real admission seam, so the sponsor/joiner sides need a real
// `WorkspaceConvergence` whose member/trust/address persistence lands in
// the same repositories the e2e asserts on. Every other port is a minimal
// no-op; removal/notice/recovery flows are out of scope for these tests.
// ───────────────────────────────────────────────────────────────────────

use std::sync::Mutex;

use uc_application::facade::{
    MembershipConvergenceDeps, SpaceConvergenceAssembly, SpaceConvergenceDeps,
    WorkspaceConvergenceDeps,
};
use uc_core::membership::{
    CurrentMemberSignatureError, CurrentMemberSignaturePort, CurrentMembershipAnnouncementMaterial,
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentity, CurrentMembershipIdentityError,
    CurrentMembershipIdentityPort, MembershipSecurityUpdateError, MembershipSecurityUpdatePort,
    WorkspaceConvergenceRepositoryError, WorkspaceConvergenceRepositoryPort,
    WorkspaceConvergenceState,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort, PeerAddressRepositoryPort, SettingsPort};
use uc_core::trusted_peer::TrustedPeerRepositoryPort;

/// Build a workspace convergence owner whose member/trust/address writes go
/// through the provided repositories (mirroring production assembly), with
/// no-op ports everywhere else. The local member instance is derived
/// deterministically from the local device id.
pub fn test_workspace_convergence(
    member_repo: Arc<dyn MemberRepositoryPort>,
    trusted_peer_repo: Arc<dyn TrustedPeerRepositoryPort>,
    peer_addr_repo: Arc<dyn PeerAddressRepositoryPort>,
    device_identity: Arc<dyn DeviceIdentityPort>,
    local_identity: Arc<dyn uc_core::ports::LocalIdentityPort>,
    pairing_session: Arc<dyn uc_core::ports::pairing::PairingSessionPort>,
    settings: Arc<dyn SettingsPort>,
    clock: Arc<dyn ClockPort>,
) -> Arc<SpaceConvergenceAssembly> {
    let own_device = device_identity.current_device_id();
    let workspace = WorkspaceConvergenceDeps {
        repository: Arc::new(MemoryWorkspaceRepo::default()),
        member_signatures: Arc::new(FixedSigner),
        member_repo: Arc::clone(&member_repo),
        membership_identity: Arc::new(FixedMembershipIdentity::new(
            own_device,
            Arc::clone(&settings),
            Arc::clone(&local_identity),
        )),
        announcement_material: Arc::new(FixedAnnouncementMaterial::new(
            own_device,
            Arc::clone(&settings),
            Arc::clone(&local_identity),
            Arc::clone(&pairing_session),
        )),
        security_updates: Arc::new(NoopSecurityUpdates),
        clock: Arc::clone(&clock),
        device_identity: Arc::clone(&device_identity),
        membership_history_exchange: Arc::new(NoopExchange),
        legacy_peer_probe: Arc::new(NoopLegacyPeerProbe),
        trusted_peer_repo: Arc::clone(&trusted_peer_repo),
        peer_addr_repo: Arc::clone(&peer_addr_repo),
        own_device,
    };
    Arc::new(SpaceConvergenceAssembly::new(SpaceConvergenceDeps {
        workspace,
        membership: MembershipConvergenceDeps {
            candidate_repo: Arc::new(EmptyCandidateRepo),
            announcement_repo: Arc::new(EmptyAnnouncementRepo),
            outbox_repo: Arc::new(EmptyOutboxRepo),
            security_updates: Arc::new(NoopSecurityUpdates),
            applied_security_updates: Arc::new(EmptyAppliedSecurityUpdates),
            transport: Arc::new(NoopGossipTransport),
            clock,
            device_identity,
            announcement_material: Arc::new(FixedAnnouncementMaterial::new(
                own_device,
                Arc::clone(&settings),
                Arc::clone(&local_identity),
                Arc::clone(&pairing_session),
            )),
            member_signatures: Arc::new(FixedSigner),
            fingerprint_factory: Arc::new(NoopFingerprintFactory),
            attestation: Arc::new(NoopAttestation),
            verified_peer_promotion: Arc::new(NoopVerifiedPeerPromotion),
            member_repo,
            trusted_peer_repo,
            peer_address_repo: peer_addr_repo,
            hash: Arc::new(NoopHasher),
        },
        group_revocation: Arc::new(NoopGroupRevocation),
        group_update_dispatch: Arc::new(NoopGroupUpdateDispatch),
        legacy_upgrade: AutomaticLegacyUpgradeDeps {
            member_repo: Arc::new(EmptyMemberRepo),
            device_identity: Arc::new(NoopDeviceIdentity),
            protection: Arc::new(NoopLegacyProtection),
            dispatch: Arc::new(NoopLegacyUpgradeDispatch),
        },
    }))
}

#[derive(Default)]
struct EmptyCandidateRepo;
#[async_trait]
impl MembershipCandidateRepositoryPort for EmptyCandidateRepo {
    async fn get(
        &self,
        _space_id: &SpaceId,
        _device_id: &DeviceId,
    ) -> Result<Option<SpaceMembershipCandidate>, MembershipCandidateRepositoryError> {
        Ok(None)
    }
    async fn list(
        &self,
        _space_id: &SpaceId,
    ) -> Result<Vec<SpaceMembershipCandidate>, MembershipCandidateRepositoryError> {
        Ok(vec![])
    }
    async fn save(
        &self,
        _candidate: &SpaceMembershipCandidate,
    ) -> Result<(), MembershipCandidateRepositoryError> {
        Ok(())
    }
    async fn remove(
        &self,
        _space_id: &SpaceId,
        _device_id: &DeviceId,
    ) -> Result<bool, MembershipCandidateRepositoryError> {
        Ok(false)
    }
    async fn purge_expired(
        &self,
        _space_id: &SpaceId,
        _now_ms: i64,
    ) -> Result<usize, MembershipCandidateRepositoryError> {
        Ok(0)
    }
}

#[derive(Default)]
struct EmptyAnnouncementRepo;
#[async_trait]
impl MembershipAnnouncementRepositoryPort for EmptyAnnouncementRepo {
    async fn get(
        &self,
        _space_id: &SpaceId,
        _device_id: &DeviceId,
    ) -> Result<Option<DeviceAnnouncement>, MembershipAnnouncementRepositoryError> {
        Ok(None)
    }
    async fn list(
        &self,
        _space_id: &SpaceId,
    ) -> Result<Vec<DeviceAnnouncement>, MembershipAnnouncementRepositoryError> {
        Ok(vec![])
    }
    async fn save(
        &self,
        _announcement: &DeviceAnnouncement,
    ) -> Result<(), MembershipAnnouncementRepositoryError> {
        Ok(())
    }
    async fn remove(
        &self,
        _space_id: &SpaceId,
        _device_id: &DeviceId,
    ) -> Result<bool, MembershipAnnouncementRepositoryError> {
        Ok(false)
    }
}

#[derive(Default)]
struct EmptyOutboxRepo;
#[async_trait]
impl MembershipOutboxRepositoryPort for EmptyOutboxRepo {
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
        Ok(vec![])
    }
    async fn save(
        &self,
        _pending: &PendingMembershipBatch,
    ) -> Result<(), MembershipOutboxRepositoryError> {
        Ok(())
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

#[derive(Default)]
struct EmptyAppliedSecurityUpdates;
#[async_trait]
impl MembershipAppliedSecurityUpdateRepositoryPort for EmptyAppliedSecurityUpdates {
    async fn list(
        &self,
        _space_id: &SpaceId,
    ) -> Result<Vec<RelayedSecurityUpdate>, MembershipAppliedSecurityUpdateRepositoryError> {
        Ok(vec![])
    }
    async fn save(
        &self,
        _space_id: &SpaceId,
        _update: &RelayedSecurityUpdate,
    ) -> Result<(), MembershipAppliedSecurityUpdateRepositoryError> {
        Ok(())
    }
}

#[derive(Default)]
struct NoopGossipTransport;
#[async_trait]
impl MembershipGossipTransportPort for NoopGossipTransport {
    async fn exchange(
        &self,
        _recipient: &DeviceId,
        _message: MembershipGossipMessage,
    ) -> Result<MembershipGossipMessage, MembershipGossipTransportError> {
        Err(MembershipGossipTransportError::Offline)
    }
}

#[derive(Default)]
struct NoopFingerprintFactory;
#[async_trait]
impl IdentityFingerprintFactoryPort for NoopFingerprintFactory {
    fn from_public_key(&self, _public_key: &[u8]) -> anyhow::Result<IdentityFingerprint> {
        IdentityFingerprint::from_raw_string("AAAAAAAAAAAAAAAA")
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }
}

#[derive(Default)]
struct NoopAttestation;
#[async_trait]
impl MembershipAttestationPort for NoopAttestation {
    async fn attest_candidate(
        &self,
        _candidate: &SpaceMembershipCandidate,
    ) -> Result<VerifiedMembershipPeer, MembershipAttestationError> {
        Err(MembershipAttestationError::Offline)
    }
}

#[derive(Default)]
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

#[derive(Default)]
struct NoopHasher;
#[async_trait]
impl ContentHashPort for NoopHasher {
    fn hash_bytes(&self, _bytes: &[u8]) -> anyhow::Result<uc_core::clipboard::ContentHash> {
        Ok(uc_core::clipboard::ContentHash {
            alg: uc_core::clipboard::HashAlgorithm::Blake3V1,
            bytes: [0; 32],
        })
    }
}

#[derive(Default)]
struct NoopGroupRevocation;
#[async_trait]
impl GroupRevocationPort for NoopGroupRevocation {
    async fn revoke_group_member(
        &self,
        _target: &DeviceId,
        _retained_recipients: &[DeviceId],
        _now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        Err(KeyEpochError::Repository("unavailable".into()))
    }
    async fn acknowledge_group_update(
        &self,
        _revocation_id: &RevocationId,
        _recipient: &DeviceId,
        _now_ms: i64,
    ) -> Result<GroupRevocationResult, KeyEpochError> {
        Err(KeyEpochError::Repository("unavailable".into()))
    }
    async fn apply_group_epoch_update(&self, _payload: &[u8]) -> Result<GroupEpoch, KeyEpochError> {
        Err(KeyEpochError::Repository("unavailable".into()))
    }
    async fn pending_group_updates(
        &self,
        _revocation_id: &RevocationId,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        Ok(vec![])
    }
    async fn query_group_revocation(
        &self,
        _revocation_id: &RevocationId,
    ) -> Result<Option<GroupRevocationResult>, KeyEpochError> {
        Ok(None)
    }
    async fn resume_group_revocations(
        &self,
        _now_ms: i64,
    ) -> Result<Vec<GroupRevocationResult>, KeyEpochError> {
        Ok(vec![])
    }
    async fn pending_space_group_updates(&self) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        Ok(vec![])
    }
    async fn acknowledge_space_group_update(
        &self,
        _update_id: &str,
        _now_ms: i64,
    ) -> Result<bool, KeyEpochError> {
        Ok(false)
    }
}

#[derive(Default)]
struct NoopGroupUpdateDispatch;
#[async_trait]
impl GroupUpdateDispatchPort for NoopGroupUpdateDispatch {
    async fn dispatch_group_update(
        &self,
        _update: &PendingGroupUpdate,
    ) -> Result<(), GroupUpdateDispatchError> {
        Ok(())
    }
}

#[derive(Default)]
struct EmptyMemberRepo;
#[async_trait]
impl MemberRepositoryPort for EmptyMemberRepo {
    async fn get(&self, _device_id: &DeviceId) -> Result<Option<SpaceMember>, MembershipError> {
        Ok(None)
    }
    async fn list(&self) -> Result<Vec<SpaceMember>, MembershipError> {
        Ok(vec![])
    }
    async fn save(&self, _member: &SpaceMember) -> Result<(), MembershipError> {
        Ok(())
    }
    async fn remove(&self, _device_id: &DeviceId) -> Result<bool, MembershipError> {
        Ok(false)
    }
}

#[derive(Default)]
struct NoopDeviceIdentity;
impl DeviceIdentityPort for NoopDeviceIdentity {
    fn current_device_id(&self) -> DeviceId {
        DeviceId::new("noop-device")
    }
}

#[derive(Default)]
struct NoopLegacyProtection;
#[async_trait]
impl LegacyProtectionPort for NoopLegacyProtection {
    async fn snapshot(
        &self,
        _member_ids: &[DeviceId],
    ) -> Result<LegacyProtectionSnapshot, LegacyUpgradeError> {
        Err(LegacyUpgradeError::Unavailable)
    }
    async fn begin_attempt(
        &self,
        _source_device_id: &DeviceId,
        _target_device_id: &DeviceId,
    ) -> Result<LegacyUpgradeRequest, LegacyUpgradeError> {
        Err(LegacyUpgradeError::Unavailable)
    }
    async fn inspect_request(
        &self,
        _request: &LegacyUpgradeRequest,
    ) -> Result<LegacyRequestInspection, LegacyUpgradeError> {
        Err(LegacyUpgradeError::Unavailable)
    }
    async fn execute(
        &self,
        _command: LegacyProtectionCommand,
    ) -> Result<LegacyProtectionResult, LegacyUpgradeError> {
        Err(LegacyUpgradeError::Unavailable)
    }
}

#[derive(Default)]
struct NoopLegacyUpgradeDispatch;
#[async_trait]
impl LegacyUpgradeDispatchPort for NoopLegacyUpgradeDispatch {
    async fn exchange_legacy_upgrade(
        &self,
        _peer: &DeviceId,
        _request: &LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeResponse, LegacyUpgradeDispatchError> {
        Err(LegacyUpgradeDispatchError::Offline)
    }
}

#[derive(Clone, Default)]
struct MemoryWorkspaceRepo {
    state: Arc<Mutex<Option<WorkspaceConvergenceState>>>,
}
#[async_trait]
impl WorkspaceConvergenceRepositoryPort for MemoryWorkspaceRepo {
    async fn save_state(
        &self,
        state: &WorkspaceConvergenceState,
    ) -> Result<(), WorkspaceConvergenceRepositoryError> {
        *self.state.lock().unwrap() = Some(state.clone());
        Ok(())
    }
    async fn load_state(
        &self,
    ) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
        Ok(self.state.lock().unwrap().clone())
    }
}

#[derive(Clone, Default)]
struct FixedSigner;
#[async_trait]
impl CurrentMemberSignaturePort for FixedSigner {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        Ok(1)
    }
    async fn current_member_instance(
        &self,
        _device_id: &DeviceId,
    ) -> Result<uc_core::membership::MemberInstanceId, CurrentMemberSignatureError> {
        Ok(uc_core::membership::MemberInstanceId::from_bytes([1; 32]))
    }
    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        Ok(b"test-signature".to_vec())
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

#[derive(Clone)]
struct FixedMembershipIdentity {
    device_id: DeviceId,
    settings: Arc<dyn SettingsPort>,
    local_identity: Arc<dyn uc_core::ports::LocalIdentityPort>,
}
impl FixedMembershipIdentity {
    fn new(
        device_id: DeviceId,
        settings: Arc<dyn SettingsPort>,
        local_identity: Arc<dyn uc_core::ports::LocalIdentityPort>,
    ) -> Self {
        Self {
            device_id,
            settings,
            local_identity,
        }
    }
    async fn fingerprint(&self) -> IdentityFingerprint {
        self.local_identity
            .ensure()
            .await
            .unwrap_or_else(|_| IdentityFingerprint::from_raw_string("ABCDEFGHIJKLMNOP").unwrap())
    }
}
#[async_trait]
impl CurrentMembershipIdentityPort for FixedMembershipIdentity {
    async fn current_membership_identity(
        &self,
    ) -> Result<CurrentMembershipIdentity, CurrentMembershipIdentityError> {
        let name = self
            .settings
            .load()
            .await
            .ok()
            .and_then(|s| s.general.device_name)
            .unwrap_or_else(|| "test-device".to_owned());
        Ok(CurrentMembershipIdentity {
            space_id: uc_core::ids::SpaceId::from_str("e2e-space"),
            device_id: self.device_id,
            device_name: name,
            identity_fingerprint: self.fingerprint().await,
        })
    }
}

#[derive(Clone)]
struct FixedAnnouncementMaterial {
    device_id: DeviceId,
    settings: Arc<dyn SettingsPort>,
    local_identity: Arc<dyn uc_core::ports::LocalIdentityPort>,
    pairing_session: Arc<dyn uc_core::ports::pairing::PairingSessionPort>,
}
impl FixedAnnouncementMaterial {
    fn new(
        device_id: DeviceId,
        settings: Arc<dyn SettingsPort>,
        local_identity: Arc<dyn uc_core::ports::LocalIdentityPort>,
        pairing_session: Arc<dyn uc_core::ports::pairing::PairingSessionPort>,
    ) -> Self {
        Self {
            device_id,
            settings,
            local_identity,
            pairing_session,
        }
    }
}
#[async_trait]
impl CurrentMembershipAnnouncementPort for FixedAnnouncementMaterial {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        let name = self
            .settings
            .load()
            .await
            .ok()
            .and_then(|s| s.general.device_name)
            .unwrap_or_else(|| "test-device".to_owned());
        Ok(CurrentMembershipAnnouncementMaterial {
            space_id: uc_core::ids::SpaceId::from_str("e2e-space"),
            device_id: self.device_id,
            device_name: name,
            identity_fingerprint: self.local_identity.ensure().await.unwrap_or_else(|_| {
                IdentityFingerprint::from_raw_string("ABCDEFGHIJKLMNOP").unwrap()
            }),
            transport_public_key: vec![1; 32],
            transport_address_blob: self
                .pairing_session
                .local_transport_address_blob()
                .await
                .unwrap_or_default(),
        })
    }
    async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct NoopSecurityUpdates;
#[async_trait]
impl MembershipSecurityUpdatePort for NoopSecurityUpdates {
    async fn current_state(
        &self,
    ) -> Result<uc_core::membership::MembershipSecurityState, MembershipSecurityUpdateError> {
        Ok(uc_core::membership::MembershipSecurityState {
            space_id: uc_core::ids::SpaceId::from_str("e2e-space"),
            group_epoch: 0,
        })
    }
    async fn apply_group_epoch_update(
        &self,
        _payload: &[u8],
    ) -> Result<u64, MembershipSecurityUpdateError> {
        Ok(0)
    }
}

#[derive(Clone, Default)]
struct NoopExchange;
#[async_trait]
impl uc_core::membership::MembershipHistoryExchangePort for NoopExchange {
    async fn exchange_membership_history(
        &self,
        _recipient: &DeviceId,
        _message: uc_core::membership::MembershipHistoryMessage,
    ) -> Result<
        uc_core::membership::MembershipHistoryMessage,
        uc_core::membership::MembershipHistoryExchangeError,
    > {
        Ok(uc_core::membership::MembershipHistoryMessage::Ack(
            uc_core::membership::MembershipHistoryAck::Consistent,
        ))
    }
}

struct NoopLegacyPeerProbe;

#[async_trait]
impl uc_core::membership::LegacyPeerProbePort for NoopLegacyPeerProbe {
    async fn probe_legacy_peer(
        &self,
        _peer: &DeviceId,
    ) -> Result<(), uc_core::membership::LegacyPeerProbeError> {
        Err(uc_core::membership::LegacyPeerProbeError::Transport)
    }
}
