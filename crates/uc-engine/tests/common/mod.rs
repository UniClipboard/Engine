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

use uc_application::clipboard_write::MobileConsumableBackfill;
use uc_core::crypto::domain::{Aad, ActiveSpace, Ciphertext, Plaintext};
use uc_core::ids::{EventId, RepresentationId, SpaceId};
use uc_core::membership::{
    RelationshipStateResetError, RelationshipStateResetPort, SpaceSecurityStateResetError,
    SpaceSecurityStateResetPort,
};
use uc_core::ports::clipboard::{BlobMigrationRepoError, BlobMigrationRepoPort, MigrationRecord};
use uc_core::ports::security::{
    BlobCipherError, BlobCipherPort, KeyMigrationError, KeyMigrationPort,
};
use uc_core::ports::setup::{MigrationStateError, MigrationStatePort};
use uc_core::setup::{MigrationPhase, MigrationRunId};

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

use uc_application::space::convergence::{WorkspaceConvergence, WorkspaceConvergenceDeps};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignatureError, CurrentMemberSignaturePort, CurrentMembershipAnnouncementMaterial,
    CurrentMembershipAnnouncementPort, CurrentMembershipIdentity, CurrentMembershipIdentityError,
    CurrentMembershipIdentityPort, MemberInstanceId, MemberRepositoryPort,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort, RemovalCausalProof,
    RemovalCausalProofMember, RemovalExchangeMessage, RemovalIntentVerificationError,
    RemovalIntentVerificationPort, RemovalLateAcceptance, RemovalLateRejectionReason,
    RemovalLateSubmission, RemovalLateSubmissionPort, RemovalNotice, RemovalNoticeAcceptance,
    RemovalNoticePort, RemovalNoticeVerificationError, RemovalNoticeVerificationPort,
    RemovalRecoveryError, RemovalRecoveryPort, RemovalViewMember, RemovalViewSnapshot,
    SignedRemovalIntent, WorkspaceConvergenceRepositoryError, WorkspaceConvergenceRepositoryPort,
    WorkspaceConvergenceState,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort, PeerAddressRepositoryPort, SettingsPort};
use uc_core::security::IdentityFingerprint;
use uc_core::setup::SetupStatus;
use uc_core::trusted_peer::{TrustedPeer, TrustedPeerError, TrustedPeerRepositoryPort};

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
) -> Arc<WorkspaceConvergence> {
    let own_device = device_identity.current_device_id();
    WorkspaceConvergence::new(WorkspaceConvergenceDeps {
        repository: Arc::new(MemoryWorkspaceRepo::default()),
        verification: Arc::new(NoopIntentVerifier),
        recovery: Arc::new(FixedRecovery::new(own_device)),
        member_signatures: Arc::new(FixedSigner),
        member_repo,
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
        clock,
        device_identity,
        exchange: Arc::new(NoopExchange),
        late_submission: Arc::new(NoopLateSubmission),
        notice: Arc::new(NoopNotice),
        notice_verification: Arc::new(NoopNoticeVerification),
        trusted_peer_repo,
        peer_addr_repo,
        own_device,
    })
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

struct NoopIntentVerifier;
#[async_trait]
impl RemovalIntentVerificationPort for NoopIntentVerifier {
    async fn verify_intent(
        &self,
        _intent: &SignedRemovalIntent,
    ) -> Result<(), RemovalIntentVerificationError> {
        Ok(())
    }
}

#[derive(Clone)]
struct FixedRecovery {
    own_device: DeviceId,
}
impl FixedRecovery {
    fn new(own_device: DeviceId) -> Self {
        Self { own_device }
    }
    fn own_instance(&self) -> MemberInstanceId {
        let bytes: [u8; 32] = {
            let mut b = [0u8; 32];
            let id = self.own_device.as_str();
            let n = id.len().min(32);
            b[..n].copy_from_slice(id.as_bytes()[..n].try_into().unwrap());
            b
        };
        MemberInstanceId::from_bytes(bytes)
    }
}
#[async_trait]
impl RemovalRecoveryPort for FixedRecovery {
    async fn current_view(&self) -> Result<RemovalViewSnapshot, RemovalRecoveryError> {
        let member = RemovalViewMember {
            device_id: self.own_device,
            instance: self.own_instance(),
            signing_public_key: self.own_instance().as_bytes().to_vec(),
        };
        let causal = RemovalCausalProofMember {
            device_id: member.device_id,
            instance: member.instance,
            signing_public_key: member.signing_public_key.clone(),
        };
        Ok(RemovalViewSnapshot {
            epoch: 1,
            members: vec![member],
            causal_proof: RemovalCausalProof::new(1, vec![causal]),
        })
    }
    async fn own_instance(&self) -> Result<Option<MemberInstanceId>, RemovalRecoveryError> {
        Ok(Some(self.own_instance()))
    }
}

#[derive(Clone, Default)]
struct FixedSigner;
#[async_trait]
impl CurrentMemberSignaturePort for FixedSigner {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        Ok(1)
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
impl uc_core::membership::RemovalExchangePort for NoopExchange {
    async fn exchange(
        &self,
        _recipient: &DeviceId,
        _message: RemovalExchangeMessage,
    ) -> Result<RemovalExchangeMessage, uc_core::membership::RemovalExchangeError> {
        Ok(RemovalExchangeMessage::IntentAck(
            uc_core::membership::RemovalIntentId::from_bytes([0; 32]),
        ))
    }
}

#[derive(Clone, Default)]
struct NoopLateSubmission;
#[async_trait]
impl RemovalLateSubmissionPort for NoopLateSubmission {
    async fn submit_late(
        &self,
        _recipient: &DeviceId,
        _submission: RemovalLateSubmission,
    ) -> Result<RemovalLateAcceptance, uc_core::membership::RemovalLateSubmissionTransportError>
    {
        Ok(RemovalLateAcceptance::Rejected {
            reason: RemovalLateRejectionReason::Invalid,
        })
    }
}

#[derive(Clone, Default)]
struct NoopNotice;
#[async_trait]
impl RemovalNoticePort for NoopNotice {
    async fn send_notice(
        &self,
        _recipient: &DeviceId,
        _notice: RemovalNotice,
    ) -> Result<RemovalNoticeAcceptance, uc_core::membership::RemovalNoticeTransportError> {
        Ok(RemovalNoticeAcceptance::Accepted {
            intent_id: uc_core::membership::RemovalIntentId::from_bytes([0; 32]),
        })
    }
}

#[derive(Clone, Default)]
struct NoopNoticeVerification;
#[async_trait]
impl RemovalNoticeVerificationPort for NoopNoticeVerification {
    async fn verify_notice_signature(
        &self,
        _notice: &RemovalNotice,
        _issuer_public_key: &[u8],
    ) -> Result<(), RemovalNoticeVerificationError> {
        Ok(())
    }
}
