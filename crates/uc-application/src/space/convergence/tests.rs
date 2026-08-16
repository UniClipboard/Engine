//! WorkspaceConvergence owner tests (ADR-016 flow semantics).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    BootstrapId, CurrentMemberSignatureError, CurrentMemberSignaturePort,
    CurrentMembershipAnnouncementMaterial, CurrentMembershipAnnouncementPort,
    CurrentMembershipIdentity, CurrentMembershipIdentityError, CurrentMembershipIdentityPort,
    CurrentWorkspacePeerScopePort, LegacyBootstrapProgress, LegacyBootstrapStatus,
    MemberInstanceId, MemberProtection, MemberProtectionStatus, MemberRepositoryPort,
    MembershipAdmissionDecision, MembershipAdmissionGatePort, MembershipEventsResponse,
    MembershipHistoryAck, MembershipHistoryMessage, MembershipOperation, MembershipReconciliation,
    MembershipSecurityUpdateError, MembershipSecurityUpdatePort, RemovalDecision,
    SpaceProtectionError, SpaceProtectionMode, SpaceProtectionSnapshot, SpaceProtectionStatusPort,
    WorkspaceConvergenceEvent, WorkspaceConvergenceRepositoryError,
    WorkspaceConvergenceRepositoryPort, WorkspaceConvergenceState,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort};
use uc_core::ports::{PresenceError, PresenceEvent, PresencePort, ReachabilityState};

use crate::space::convergence::{
    WorkspaceConvergence, WorkspaceConvergenceDeps, WorkspaceConvergenceError,
};

const SPACE: &str = "test-workspace";

#[derive(Clone, Default)]
struct UnusedExchange {
    history_sent: Arc<Mutex<Vec<(DeviceId, MembershipHistoryMessage)>>>,
}

#[derive(Clone)]
struct ScriptedExchange {
    replies: Arc<Mutex<VecDeque<MembershipHistoryMessage>>>,
    history_sent: Arc<Mutex<Vec<(DeviceId, MembershipHistoryMessage)>>>,
}

struct RejectingExchange;

struct DeferredAdmissionDelivery;

struct ConfirmingAdmissionDelivery;

#[async_trait]
impl uc_core::membership::AdmissionOutboxDeliveryPort for DeferredAdmissionDelivery {
    async fn deliver(
        &self,
        _attempt_id: uc_core::membership::AdmissionAttemptId,
        _message: &uc_core::membership::AdmissionOutboxMessageV1,
    ) -> Result<
        uc_core::membership::AdmissionOutboxDeliveryResultV1,
        uc_core::membership::AdmissionOutboxDeliveryError,
    > {
        Ok(uc_core::membership::AdmissionOutboxDeliveryResultV1::Deferred)
    }
}

#[async_trait]
impl uc_core::membership::AdmissionOutboxDeliveryPort for ConfirmingAdmissionDelivery {
    async fn deliver(
        &self,
        _attempt_id: uc_core::membership::AdmissionAttemptId,
        message: &uc_core::membership::AdmissionOutboxMessageV1,
    ) -> Result<
        uc_core::membership::AdmissionOutboxDeliveryResultV1,
        uc_core::membership::AdmissionOutboxDeliveryError,
    > {
        if message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::InvitationConsume {
            return Ok(
                uc_core::membership::AdmissionOutboxDeliveryResultV1::InvitationConsume(
                    uc_core::membership::InvitationConsumeDeliveryResultV1::Consumed,
                ),
            );
        }
        Ok(
            uc_core::membership::AdmissionOutboxDeliveryResultV1::Persisted(
                super::admission_transaction::admission_acknowledgment(message),
            ),
        )
    }
}

struct BlockingTrackingExchange {
    active: AtomicUsize,
    calls: AtomicUsize,
    maximum_active: AtomicUsize,
    started: tokio::sync::Notify,
    releases: tokio::sync::Semaphore,
}

impl BlockingTrackingExchange {
    fn new() -> Self {
        Self {
            active: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            maximum_active: AtomicUsize::new(0),
            started: tokio::sync::Notify::new(),
            releases: tokio::sync::Semaphore::new(0),
        }
    }
}

#[async_trait]
impl uc_core::membership::MembershipHistoryExchangePort for BlockingTrackingExchange {
    async fn exchange_membership_history(
        &self,
        _recipient: &DeviceId,
        _message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, uc_core::membership::MembershipHistoryExchangeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum_active.fetch_max(active, Ordering::SeqCst);
        self.started.notify_waiters();
        let permit = self
            .releases
            .acquire()
            .await
            .expect("test exchange remains open");
        permit.forget();
        self.active.fetch_sub(1, Ordering::SeqCst);
        Ok(MembershipHistoryMessage::Ack(
            MembershipHistoryAck::Consistent,
        ))
    }
}

#[async_trait]
impl uc_core::membership::MembershipHistoryExchangePort for RejectingExchange {
    async fn exchange_membership_history(
        &self,
        _recipient: &DeviceId,
        _message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, uc_core::membership::MembershipHistoryExchangeError> {
        Err(uc_core::membership::MembershipHistoryExchangeError::Rejected)
    }
}

#[derive(Clone)]
struct ScriptedLegacyProbe {
    responses: Arc<Mutex<VecDeque<Result<(), uc_core::membership::LegacyPeerProbeError>>>>,
    calls: Arc<Mutex<Vec<DeviceId>>>,
}

impl ScriptedLegacyProbe {
    fn new(responses: Vec<Result<(), uc_core::membership::LegacyPeerProbeError>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl uc_core::membership::LegacyPeerProbePort for ScriptedLegacyProbe {
    async fn probe_legacy_peer(
        &self,
        peer: &DeviceId,
    ) -> Result<(), uc_core::membership::LegacyPeerProbeError> {
        self.calls.lock().unwrap().push(peer.clone());
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(uc_core::membership::LegacyPeerProbeError::Transport))
    }
}

impl ScriptedExchange {
    fn new(replies: Vec<MembershipHistoryMessage>) -> Self {
        Self {
            replies: Arc::new(Mutex::new(replies.into())),
            history_sent: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl uc_core::membership::MembershipHistoryExchangePort for ScriptedExchange {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, uc_core::membership::MembershipHistoryExchangeError> {
        self.history_sent
            .lock()
            .unwrap()
            .push((recipient.clone(), message));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or(uc_core::membership::MembershipHistoryExchangeError::Transport)
    }
}

#[async_trait]
impl uc_core::membership::MembershipHistoryExchangePort for UnusedExchange {
    async fn exchange_membership_history(
        &self,
        recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, uc_core::membership::MembershipHistoryExchangeError> {
        self.history_sent
            .lock()
            .unwrap()
            .push((recipient.clone(), message));
        Ok(MembershipHistoryMessage::Ack(
            MembershipHistoryAck::Consistent,
        ))
    }
}

#[derive(Clone, Default)]
pub(crate) struct MemoryWorkspaceRepository {
    state: Arc<Mutex<Option<WorkspaceConvergenceState>>>,
    failure: Arc<Mutex<Option<WorkspaceConvergenceRepositoryError>>>,
}

#[async_trait]
impl WorkspaceConvergenceRepositoryPort for MemoryWorkspaceRepository {
    async fn save_state(
        &self,
        state: &WorkspaceConvergenceState,
    ) -> Result<(), WorkspaceConvergenceRepositoryError> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        *self.state.lock().unwrap() = Some(state.clone());
        Ok(())
    }

    async fn load_state(
        &self,
    ) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
        Ok(self.state.lock().unwrap().clone())
    }
}

struct LockedAdmissionRepository;

#[async_trait]
impl uc_core::membership::AdmissionAttemptRepositoryPort for LockedAdmissionRepository {
    async fn create(
        &self,
        _attempt: &uc_core::membership::AdmissionAttemptV1,
        _consumed_invitation_digest: Option<[u8; 32]>,
        _initial_membership_history_v2: Option<&[u8]>,
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn load(
        &self,
        _attempt_id: uc_core::membership::AdmissionAttemptId,
    ) -> Result<
        Option<uc_core::membership::AdmissionAttemptV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn compare_and_advance(
        &self,
        _attempt_id: uc_core::membership::AdmissionAttemptId,
        _expected_record_version: u64,
        _next: &uc_core::membership::AdmissionAttemptV1,
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn compare_and_advance_with_membership_history_v2(
        &self,
        _attempt_id: uc_core::membership::AdmissionAttemptId,
        _expected_record_version: u64,
        _next: &uc_core::membership::AdmissionAttemptV1,
        _expected_membership_history_v2: Option<&[u8]>,
        _membership_history_v2: &[u8],
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn load_membership_history_v2(
        &self,
    ) -> Result<Option<Vec<u8>>, uc_core::membership::AdmissionAttemptRepositoryError> {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn scan_recoverable(
        &self,
    ) -> Result<
        Vec<uc_core::membership::AdmissionAttemptV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn compact_terminal(
        &self,
        _attempt_id: uc_core::membership::AdmissionAttemptId,
        _expected_record_version: u64,
    ) -> Result<
        uc_core::membership::TerminalAdmissionAttemptV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn load_terminal(
        &self,
        _attempt_id: uc_core::membership::AdmissionAttemptId,
    ) -> Result<
        Option<uc_core::membership::TerminalAdmissionAttemptV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn profile_metadata(
        &self,
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn project_current_local_join(
        &self,
    ) -> Result<
        Option<uc_core::membership::CurrentLocalJoinProjectionV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }

    async fn advance_projection_floor(
        &self,
        _expected_device_trust_revision: u64,
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
    }
}

#[derive(Default)]
struct TestAdmissionSecureStorage(Mutex<HashMap<String, Vec<u8>>>);

impl uc_core::ports::SecureStoragePort for TestAdmissionSecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, uc_core::ports::SecureStorageError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), uc_core::ports::SecureStorageError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), uc_core::ports::SecureStorageError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

fn durable_admission_repository(
    directory: &tempfile::TempDir,
    generation: [u8; 16],
) -> Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort> {
    let path = directory.path().join("admission.sqlite3");
    let pool = uc_infra::db::pool::init_db_pool(path.to_str().unwrap()).unwrap();
    Arc::new(
        uc_infra::db::repositories::DieselAdmissionAttemptStore::new(
            uc_infra::db::executor::DieselSqliteExecutor::new(pool),
            uc_infra::security::AdmissionKeyManager::new(
                Arc::new(TestAdmissionSecureStorage::default()),
                generation,
            ),
        ),
    )
}

fn durable_admission_owner(
    repository: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
) -> super::admission_transaction::DurableAdmissionTransaction {
    super::admission_transaction::DurableAdmissionTransaction::new(
        repository,
        Arc::new(DeterministicHistoricalVerifier),
        Arc::new(EchoAdmissionSecurityTransition::default()),
    )
}

struct HistoryRaceAdmissionRepository {
    inner: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    inject_once: AtomicBool,
    replacement_history: Vec<u8>,
}

#[async_trait]
impl uc_core::membership::AdmissionAttemptRepositoryPort for HistoryRaceAdmissionRepository {
    async fn create(
        &self,
        attempt: &uc_core::membership::AdmissionAttemptV1,
        consumed_invitation_digest: Option<[u8; 32]>,
        initial_membership_history_v2: Option<&[u8]>,
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner
            .create(
                attempt,
                consumed_invitation_digest,
                initial_membership_history_v2,
            )
            .await
    }

    async fn load(
        &self,
        attempt_id: uc_core::membership::AdmissionAttemptId,
    ) -> Result<
        Option<uc_core::membership::AdmissionAttemptV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner.load(attempt_id).await
    }

    async fn compare_and_advance(
        &self,
        attempt_id: uc_core::membership::AdmissionAttemptId,
        expected_record_version: u64,
        next: &uc_core::membership::AdmissionAttemptV1,
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner
            .compare_and_advance(attempt_id, expected_record_version, next)
            .await
    }

    async fn compare_and_advance_with_membership_history_v2(
        &self,
        attempt_id: uc_core::membership::AdmissionAttemptId,
        expected_record_version: u64,
        next: &uc_core::membership::AdmissionAttemptV1,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        if self.inject_once.swap(false, Ordering::SeqCst) {
            let mut concurrent = self
                .inner
                .load(attempt_id)
                .await?
                .ok_or(uc_core::membership::AdmissionAttemptRepositoryError::NotFound)?;
            let concurrent_version = concurrent.record_version;
            concurrent.record_version += 1;
            let current_history = self.inner.load_membership_history_v2().await?;
            self.inner
                .compare_and_advance_with_membership_history_v2(
                    attempt_id,
                    concurrent_version,
                    &concurrent,
                    current_history.as_deref(),
                    &self.replacement_history,
                )
                .await?;
        }
        self.inner
            .compare_and_advance_with_membership_history_v2(
                attempt_id,
                expected_record_version,
                next,
                expected_membership_history_v2,
                membership_history_v2,
            )
            .await
    }

    async fn load_membership_history_v2(
        &self,
    ) -> Result<Option<Vec<u8>>, uc_core::membership::AdmissionAttemptRepositoryError> {
        self.inner.load_membership_history_v2().await
    }

    async fn scan_recoverable(
        &self,
    ) -> Result<
        Vec<uc_core::membership::AdmissionAttemptV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner.scan_recoverable().await
    }

    async fn compact_terminal(
        &self,
        attempt_id: uc_core::membership::AdmissionAttemptId,
        expected_record_version: u64,
    ) -> Result<
        uc_core::membership::TerminalAdmissionAttemptV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner
            .compact_terminal(attempt_id, expected_record_version)
            .await
    }

    async fn load_terminal(
        &self,
        attempt_id: uc_core::membership::AdmissionAttemptId,
    ) -> Result<
        Option<uc_core::membership::TerminalAdmissionAttemptV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner.load_terminal(attempt_id).await
    }

    async fn profile_metadata(
        &self,
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner.profile_metadata().await
    }

    async fn project_current_local_join(
        &self,
    ) -> Result<
        Option<uc_core::membership::CurrentLocalJoinProjectionV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner.project_current_local_join().await
    }

    async fn advance_projection_floor(
        &self,
        expected_device_trust_revision: u64,
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner
            .advance_projection_floor(expected_device_trust_revision)
            .await
    }
}

#[derive(Clone)]
struct FixedMembershipIdentity {
    space: SpaceId,
    device_id: DeviceId,
}

#[async_trait]
impl CurrentMembershipIdentityPort for FixedMembershipIdentity {
    async fn current_membership_identity(
        &self,
    ) -> Result<CurrentMembershipIdentity, CurrentMembershipIdentityError> {
        Ok(CurrentMembershipIdentity {
            space_id: self.space.clone(),
            device_id: self.device_id,
            device_name: "a".to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
        })
    }
}

struct FixedAnnouncementMaterial;

#[async_trait]
impl CurrentMembershipAnnouncementPort for FixedAnnouncementMaterial {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        Ok(CurrentMembershipAnnouncementMaterial {
            space_id: SpaceId::from_str(SPACE),
            device_id: DeviceId::new("device-a"),
            device_name: "a".into(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1; 32],
            transport_address_blob: vec![2],
        })
    }
    async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
        Ok(())
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
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError> {
        Ok(instance(0x0a))
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        Ok(b"signature".to_vec())
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

#[derive(Clone, Default)]
struct EventSignatureOnlyVerifier;

#[async_trait]
impl CurrentMemberSignaturePort for EventSignatureOnlyVerifier {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        Ok(1)
    }

    async fn current_member_instance(
        &self,
        _device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError> {
        Ok(instance(0x1a))
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        Ok(b"signature".to_vec())
    }

    async fn verify_current_member_payload(
        &self,
        _member: &DeviceId,
        _payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        Ok(signature.len() != 64)
    }
}

#[derive(Clone, Default)]
struct UnusedSecurityUpdates;

#[async_trait]
impl MembershipSecurityUpdatePort for UnusedSecurityUpdates {
    async fn current_state(
        &self,
    ) -> Result<uc_core::membership::MembershipSecurityState, MembershipSecurityUpdateError> {
        Ok(uc_core::membership::MembershipSecurityState {
            space_id: SpaceId::from_str(SPACE),
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
struct RecordingSecurityUpdates {
    applied_payloads: Arc<Mutex<Vec<Vec<u8>>>>,
}

#[async_trait]
impl MembershipSecurityUpdatePort for RecordingSecurityUpdates {
    async fn current_state(
        &self,
    ) -> Result<uc_core::membership::MembershipSecurityState, MembershipSecurityUpdateError> {
        Ok(uc_core::membership::MembershipSecurityState {
            space_id: SpaceId::from_str(SPACE),
            group_epoch: 0,
        })
    }

    async fn apply_group_epoch_update(
        &self,
        payload: &[u8],
    ) -> Result<u64, MembershipSecurityUpdateError> {
        self.applied_payloads.lock().unwrap().push(payload.to_vec());
        Ok(1)
    }
}

#[derive(Clone, Default)]
struct UnusedClock;

#[async_trait]
impl ClockPort for UnusedClock {
    fn now_ms(&self) -> i64 {
        1_000
    }
}

#[derive(Clone, Default)]
struct UnusedDeviceIdentity;

#[async_trait]
impl DeviceIdentityPort for UnusedDeviceIdentity {
    fn current_device_id(&self) -> DeviceId {
        DeviceId::new("device-a")
    }
}

struct Harness {
    owner: Arc<WorkspaceConvergence>,
    repository: MemoryWorkspaceRepository,
    history_exchange: Arc<UnusedExchange>,
    presence: Arc<FixedPresence>,
}

#[derive(Clone, Default)]
struct FixedPresence {
    states: Arc<Mutex<std::collections::BTreeMap<DeviceId, ReachabilityState>>>,
}

#[async_trait]
impl PresencePort for FixedPresence {
    async fn ensure_reachable(
        &self,
        device: &DeviceId,
    ) -> Result<ReachabilityState, PresenceError> {
        Ok(self.current_state(device).await)
    }

    async fn current_state(&self, device: &DeviceId) -> ReachabilityState {
        self.states
            .lock()
            .unwrap()
            .get(device)
            .copied()
            .unwrap_or(ReachabilityState::Unknown)
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PresenceEvent> {
        let (sender, receiver) = tokio::sync::broadcast::channel(1);
        drop(sender);
        receiver
    }
}

fn instance(byte: u8) -> MemberInstanceId {
    MemberInstanceId::from_bytes([byte; 32])
}

fn harness(own_device: &str, members: Vec<(DeviceId, MemberInstanceId)>) -> Harness {
    let repository = MemoryWorkspaceRepository::default();
    let history_exchange = Arc::new(UnusedExchange::default());
    let presence = Arc::new(FixedPresence::default());
    let mut deps = test_deps(Arc::new(repository.clone()), own_device, members);
    deps.membership_history_exchange = history_exchange.clone();
    deps.presence = presence.clone();
    let owner = WorkspaceConvergence::new(deps);
    Harness {
        owner,
        repository,
        history_exchange,
        presence,
    }
}

/// Build the full dependency set with no-op defaults for every port except
/// the repository and the recovery view. Shared with other test modules in
/// this crate (`pub(crate)` under `cfg(test)`).
pub(crate) fn test_deps(
    repository: Arc<dyn WorkspaceConvergenceRepositoryPort>,
    own_device: &str,
    _members: Vec<(DeviceId, MemberInstanceId)>,
) -> WorkspaceConvergenceDeps {
    WorkspaceConvergenceDeps {
        initial_state_origin: super::WorkspaceConvergenceStateOrigin::CurrentInstallation,
        repository,
        admission_attempts: Arc::new(LockedAdmissionRepository),
        historical_membership_signatures: Arc::new(DeterministicHistoricalVerifier),
        admission_security_transition: Arc::new(EchoAdmissionSecurityTransition::default()),
        admission_outbox_delivery: Arc::new(DeferredAdmissionDelivery),
        member_signatures: Arc::new(FixedSigner),
        member_repo: Arc::new(uc_application_test_member_repo()),
        membership_identity: Arc::new(FixedMembershipIdentity {
            space: SpaceId::from_str(SPACE),
            device_id: DeviceId::new(own_device),
        }),
        announcement_material: Arc::new(FixedAnnouncementMaterial),
        security_updates: Arc::new(UnusedSecurityUpdates),
        clock: Arc::new(UnusedClock),
        device_identity: Arc::new(UnusedDeviceIdentity),
        membership_history_exchange: Arc::new(UnusedExchange::default()),
        legacy_peer_probe: Arc::new(UnusedLegacyProbe),
        trusted_peer_repo: Arc::new(TestTrustedPeerRepo),
        peer_addr_repo: Arc::new(TestPeerAddrRepo),
        presence: Arc::new(FixedPresence::default()),
        space_protection: Arc::new(FixedSpaceProtection(SpaceProtectionMode::Ready)),
        own_device: DeviceId::new(own_device),
    }
}

struct FixedSpaceProtection(SpaceProtectionMode);

#[async_trait]
impl SpaceProtectionStatusPort for FixedSpaceProtection {
    async fn query_space_protection(
        &self,
        _members: &[DeviceId],
    ) -> Result<SpaceProtectionSnapshot, SpaceProtectionError> {
        Ok(SpaceProtectionSnapshot {
            mode: self.0,
            members: Vec::new(),
            legacy_bootstrap: None,
        })
    }
}

struct PartiallyProtectedRoster;

#[async_trait]
impl SpaceProtectionStatusPort for PartiallyProtectedRoster {
    async fn query_space_protection(
        &self,
        members: &[DeviceId],
    ) -> Result<SpaceProtectionSnapshot, SpaceProtectionError> {
        Ok(SpaceProtectionSnapshot {
            mode: SpaceProtectionMode::Ready,
            members: members
                .iter()
                .map(|device_id| MemberProtection {
                    device_id: *device_id,
                    status: if device_id == &DeviceId::new("device-b") {
                        MemberProtectionStatus::AwaitingReadmission
                    } else {
                        MemberProtectionStatus::Protected
                    },
                })
                .collect(),
            legacy_bootstrap: Some(LegacyBootstrapProgress {
                bootstrap_id: BootstrapId::generate(),
                status: LegacyBootstrapStatus::AwaitingReadmission,
                pending_readmission: 1,
            }),
        })
    }
}

#[derive(Default)]
struct ProtectsQueriedMembers {
    queries: Mutex<Vec<Vec<DeviceId>>>,
    active_legacy_bootstrap: bool,
}

impl ProtectsQueriedMembers {
    fn with_active_legacy_bootstrap() -> Self {
        Self {
            active_legacy_bootstrap: true,
            ..Self::default()
        }
    }
}

#[async_trait]
impl SpaceProtectionStatusPort for ProtectsQueriedMembers {
    async fn query_space_protection(
        &self,
        members: &[DeviceId],
    ) -> Result<SpaceProtectionSnapshot, SpaceProtectionError> {
        self.queries.lock().unwrap().push(members.to_vec());
        Ok(SpaceProtectionSnapshot {
            mode: SpaceProtectionMode::Ready,
            members: members
                .iter()
                .map(|device_id| MemberProtection {
                    device_id: *device_id,
                    status: MemberProtectionStatus::Protected,
                })
                .collect(),
            legacy_bootstrap: self
                .active_legacy_bootstrap
                .then(|| LegacyBootstrapProgress {
                    bootstrap_id: BootstrapId::generate(),
                    status: LegacyBootstrapStatus::AwaitingReadmission,
                    pending_readmission: 1,
                }),
        })
    }
}

struct UnusedLegacyProbe;

#[async_trait]
impl uc_core::membership::LegacyPeerProbePort for UnusedLegacyProbe {
    async fn probe_legacy_peer(
        &self,
        _peer: &DeviceId,
    ) -> Result<(), uc_core::membership::LegacyPeerProbeError> {
        Err(uc_core::membership::LegacyPeerProbeError::Transport)
    }
}

struct TestTrustedPeerRepo;
#[async_trait]
impl uc_core::trusted_peer::TrustedPeerRepositoryPort for TestTrustedPeerRepo {
    async fn get(
        &self,
        _device_id: &DeviceId,
    ) -> Result<Option<uc_core::trusted_peer::TrustedPeer>, uc_core::trusted_peer::TrustedPeerError>
    {
        Ok(None)
    }
    async fn list(
        &self,
    ) -> Result<Vec<uc_core::trusted_peer::TrustedPeer>, uc_core::trusted_peer::TrustedPeerError>
    {
        Ok(Vec::new())
    }
    async fn save(
        &self,
        _peer: &uc_core::trusted_peer::TrustedPeer,
    ) -> Result<(), uc_core::trusted_peer::TrustedPeerError> {
        Ok(())
    }
    async fn remove(
        &self,
        _device_id: &DeviceId,
    ) -> Result<bool, uc_core::trusted_peer::TrustedPeerError> {
        Ok(true)
    }
}

struct TestPeerAddrRepo;
#[async_trait]
impl uc_core::ports::PeerAddressRepositoryPort for TestPeerAddrRepo {
    async fn get(
        &self,
        _device: &DeviceId,
    ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
        Ok(None)
    }
    async fn upsert(
        &self,
        _record: &uc_core::ports::PeerAddressRecord,
    ) -> Result<(), uc_core::ports::PeerAddressError> {
        Ok(())
    }
    async fn list(
        &self,
    ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
        Ok(Vec::new())
    }
    async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
        Ok(())
    }
}

struct FixedPeerAddrRepo {
    records: Vec<uc_core::ports::PeerAddressRecord>,
}

#[async_trait]
impl uc_core::ports::PeerAddressRepositoryPort for FixedPeerAddrRepo {
    async fn get(
        &self,
        device: &DeviceId,
    ) -> Result<Option<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
        Ok(self
            .records
            .iter()
            .find(|record| &record.device_id == device)
            .cloned())
    }

    async fn upsert(
        &self,
        _record: &uc_core::ports::PeerAddressRecord,
    ) -> Result<(), uc_core::ports::PeerAddressError> {
        Ok(())
    }

    async fn list(
        &self,
    ) -> Result<Vec<uc_core::ports::PeerAddressRecord>, uc_core::ports::PeerAddressError> {
        Ok(self.records.clone())
    }

    async fn remove(&self, _device: &DeviceId) -> Result<(), uc_core::ports::PeerAddressError> {
        Ok(())
    }
}

fn uc_application_test_member_repo() -> impl MemberRepositoryPort {
    struct Empty;
    #[async_trait]
    impl MemberRepositoryPort for Empty {
        async fn get(
            &self,
            _device_id: &DeviceId,
        ) -> Result<Option<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
        {
            Ok(None)
        }
        async fn list(
            &self,
        ) -> Result<Vec<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
        {
            Ok(Vec::new())
        }
        async fn save(
            &self,
            _member: &uc_core::membership::SpaceMember,
        ) -> Result<(), uc_core::membership::MembershipError> {
            Ok(())
        }
        async fn remove(
            &self,
            _device_id: &DeviceId,
        ) -> Result<bool, uc_core::membership::MembershipError> {
            Ok(true)
        }
    }
    Empty
}

struct FixedMemberRepo(Vec<uc_core::membership::SpaceMember>);

#[async_trait]
impl MemberRepositoryPort for FixedMemberRepo {
    async fn get(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
    {
        Ok(self
            .0
            .iter()
            .find(|member| &member.device_id == device_id)
            .cloned())
    }

    async fn list(
        &self,
    ) -> Result<Vec<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError> {
        Ok(self.0.clone())
    }

    async fn save(
        &self,
        _member: &uc_core::membership::SpaceMember,
    ) -> Result<(), uc_core::membership::MembershipError> {
        Ok(())
    }

    async fn remove(
        &self,
        _device_id: &DeviceId,
    ) -> Result<bool, uc_core::membership::MembershipError> {
        Ok(true)
    }
}

fn legacy_member(device_id: &str) -> uc_core::membership::SpaceMember {
    uc_core::membership::SpaceMember {
        device_id: DeviceId::new(device_id),
        device_name: device_id.to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
            "ABCD-EFGH-IJKL-MNOP",
        )
        .unwrap(),
        joined_at: chrono::Utc::now(),
        sync_preferences: uc_core::membership::MemberSyncPreferences::default(),
    }
}

fn admission_facts_for(
    instance: MemberInstanceId,
    device: &DeviceId,
) -> uc_core::membership::AdmissionChangeFacts {
    uc_core::membership::AdmissionChangeFacts {
        member_instance: instance,
        device_id: *device,
        device_name: "device".to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
            "ABCD-EFGH-IJKL-MNOP",
        )
        .unwrap(),
        transport_public_key: vec![1; 32],
        transport_address_blob: vec![2; 16],
        identity_signature: vec![3; 64],
    }
}

fn membership_event(
    parent: Option<uc_core::membership::MembershipEventId>,
    parent_depth: u64,
    author: MemberInstanceId,
    member: MemberInstanceId,
    device_id: &str,
    operation_byte: u8,
) -> uc_core::membership::MembershipEvent {
    uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        parent,
        parent_depth,
        [operation_byte; 16],
        author,
        MembershipOperation::AddDevice {
            admission: admission_facts_for(member, &DeviceId::new(device_id)),
        },
        [operation_byte; 32],
        [operation_byte.saturating_add(1); 32],
        Vec::new(),
        None,
        vec![operation_byte],
    )
}

#[tokio::test]
async fn current_peer_scope_excludes_an_accepted_removal() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let harness = harness("device-a", Vec::new());
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_addition = membership_event(Some(b_addition.event_id()), 2, a, c, "device-c", 3);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        3,
        [4; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [4; 32],
        [5; 32],
        Vec::new(),
        None,
        vec![4],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    for event in [genesis, b_addition, c_addition, removal] {
        history.receive_verified(event).unwrap();
    }
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-c")]);
}

#[tokio::test]
async fn migrated_workspace_with_applied_history_does_not_restore_a_removed_peer() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let repository = MemoryWorkspaceRepository::default();
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_addition = membership_event(Some(b_addition.event_id()), 2, a, c, "device-c", 3);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        3,
        [4; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [4; 32],
        [5; 32],
        Vec::new(),
        None,
        vec![4],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    for event in [genesis, b_addition, c_addition, removal] {
        history.receive_verified(event).unwrap();
    }
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
        legacy_member("device-c"),
    ]));
    deps.space_protection = Arc::new(ProtectsQueriedMembers::default());
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-c")]);
}

#[tokio::test]
async fn runtime_clears_a_stale_legacy_marker_after_current_history_exists() {
    let a = instance(0x0a);
    let repository = MemoryWorkspaceRepository::default();
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history
        .receive_verified(membership_event(None, 0, a, a, "device-a", 1))
        .unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let owner = WorkspaceConvergence::new(test_deps(
        Arc::new(repository.clone()),
        "device-a",
        Vec::new(),
    ));
    let (_presence_tx, presence_events) = tokio::sync::broadcast::channel(1);

    let runtime = Arc::clone(&owner).start(presence_events);
    for _ in 0..100 {
        if !repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .migrated_from_pre_adr_020
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    runtime.shutdown().await;

    let repaired = repository.load_state().await.unwrap().unwrap();
    assert!(!repaired.migrated_from_pre_adr_020);

    owner.recover_legacy_migration_marker().await.unwrap();
    assert_eq!(repository.load_state().await.unwrap().unwrap(), repaired);
}

#[tokio::test]
async fn session_resume_retries_a_legacy_marker_repair_deferred_while_locked() {
    let a = instance(0x0a);
    let repository = MemoryWorkspaceRepository::default();
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history
        .receive_verified(membership_event(None, 0, a, a, "device-a", 1))
        .unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    *repository.failure.lock().unwrap() = Some(WorkspaceConvergenceRepositoryError::Locked);
    let owner = WorkspaceConvergence::new(test_deps(
        Arc::new(repository.clone()),
        "device-a",
        Vec::new(),
    ));
    let (_presence_tx, presence_events) = tokio::sync::broadcast::channel(1);
    let runtime = Arc::clone(&owner).start(presence_events);
    tokio::task::yield_now().await;

    *repository.failure.lock().unwrap() = None;
    runtime.activity().resume().await.unwrap();
    for _ in 0..100 {
        if !repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .migrated_from_pre_adr_020
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    runtime.shutdown().await;

    assert!(
        !repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .migrated_from_pre_adr_020
    );
}

#[tokio::test]
async fn current_peer_scope_keeps_a_removal_pending_local_decision() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness("device-b", Vec::new());
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    for event in [genesis, addition, removal] {
        history.receive_verified(event).unwrap();
    }
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-a")]);
}

#[tokio::test]
async fn current_peer_scope_is_empty_after_local_removal() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness("device-b", Vec::new());
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.removed = true;
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.snapshot().await.unwrap();

    assert!(snapshot.peer_device_ids.is_empty());
    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Removed
    );
}

#[tokio::test]
async fn current_peer_scope_uses_legacy_members_only_in_explicit_legacy_mode() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Legacy));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::Legacy
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
}

#[tokio::test]
async fn current_peer_scope_accepts_a_legacy_roster_that_only_stores_remote_members() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-b")]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Legacy));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
}

#[tokio::test]
async fn device_trust_uses_the_legacy_scope_for_a_fresh_workspace() {
    use crate::space::convergence::DeviceMembership;

    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-a")]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Legacy));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.query_device_trust().await.unwrap();

    assert_eq!(snapshot.local_membership, DeviceMembership::Active);
    assert_eq!(snapshot.devices.len(), 1);
    assert_eq!(snapshot.devices[0].membership, DeviceMembership::Active);
}

#[tokio::test]
async fn device_trust_does_not_infer_membership_without_legacy_or_current_history() {
    use crate::space::convergence::DeviceMembership;

    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-a")]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Ready));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.query_device_trust().await.unwrap();

    assert_eq!(snapshot.local_membership, DeviceMembership::Unavailable);
    assert_eq!(
        snapshot.devices[0].membership,
        DeviceMembership::Unavailable
    );
}

#[tokio::test]
async fn current_peer_scope_keeps_a_migrated_pre_adr_020_workspace_in_legacy_upgrade() {
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(instance(0x0a));
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(ProtectsQueriedMembers::default());
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::Legacy
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
}

#[tokio::test]
async fn migrated_remote_only_roster_checks_local_protection_before_membership() {
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let protection = Arc::new(ProtectsQueriedMembers::default());
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-b")]));
    deps.space_protection = protection.clone();
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
    assert_eq!(
        protection.queries.lock().unwrap().as_slice(),
        &[vec![DeviceId::new("device-a"), DeviceId::new("device-b")]]
    );
}

#[tokio::test]
async fn active_legacy_bootstrap_keeps_remote_only_roster_in_upgrade_scope() {
    let repository = MemoryWorkspaceRepository::default();
    let protection = Arc::new(ProtectsQueriedMembers::with_active_legacy_bootstrap());
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-b")]));
    deps.space_protection = protection;
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::Legacy
    );
    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("device-b")]);
}

#[tokio::test]
async fn device_trust_query_returns_a_migrated_workspace_as_upgrade_required() {
    use crate::space::convergence::{DeviceCompatibility, SyncRelationship};

    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(instance(0x0a));
    state.migrated_from_pre_adr_020 = true;
    state.peer_history_relationships.insert(
        DeviceId::new("device-b"),
        uc_core::membership::MembershipHistoryRelationship::UpgradeRequired,
    );
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-b"))
        .unwrap();

    assert_eq!(snapshot.local_device_id, DeviceId::new("device-a"));
    assert_eq!(snapshot.devices.len(), 2);
    assert_eq!(peer.compatibility, DeviceCompatibility::UpgradeRequired);
    assert_eq!(
        peer.sync_relationship,
        SyncRelationship::PausedUpgradeRequired
    );
}

#[tokio::test]
async fn current_peer_scope_does_not_infer_legacy_mode_from_missing_history() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(FixedSpaceProtection(SpaceProtectionMode::Ready));
    let owner = WorkspaceConvergence::new(deps);

    let result = owner.snapshot().await;

    assert_eq!(
        result,
        Err(uc_core::membership::CurrentWorkspacePeerScopeError::Unavailable)
    );
}

#[tokio::test]
async fn current_peer_scope_hides_addition_until_pending_effects_finish() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness("device-a", Vec::new());
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.pending_applied_membership_effects.push(
        uc_core::membership::PendingAppliedMembershipEffect {
            event_id: addition.event_id(),
            member_facts_completed: false,
            security_update_completed: true,
        },
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.snapshot().await.unwrap();

    assert!(snapshot.peer_device_ids.is_empty());
}

#[tokio::test]
async fn restart_recovery_completes_and_clears_pending_membership_effects() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let repository = MemoryWorkspaceRepository::default();
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.pending_applied_membership_effects.push(
        uc_core::membership::PendingAppliedMembershipEffect {
            event_id: addition.event_id(),
            member_facts_completed: false,
            security_update_completed: true,
        },
    );
    repository.save_state(&state).await.unwrap();
    let owner = WorkspaceConvergence::new(test_deps(
        Arc::new(repository.clone()),
        "device-a",
        Vec::new(),
    ));

    owner.recover_pending_membership_effects().await.unwrap();

    let saved = repository.load_state().await.unwrap().unwrap();
    assert!(saved.pending_applied_membership_effects.is_empty());
    assert_eq!(
        owner.snapshot().await.unwrap().peer_device_ids,
        vec![DeviceId::new("device-b")]
    );
}

// 流程：C 收到 A 对 B 的移除，A 在线而 B 离线；一次查询直接返回来源、目标、两种后果和独立关系事实。
#[tokio::test]
async fn device_trust_query_returns_complete_pending_change_and_per_device_relationships() {
    use crate::space::convergence::{
        DeviceCompatibility, DeviceMembership, GroupRelationship, SyncRelationship,
    };

    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
            (DeviceId::new("device-c"), c),
        ],
    );
    harness.presence.states.lock().unwrap().extend([
        (DeviceId::new("device-a"), ReachabilityState::Online),
        (DeviceId::new("device-b"), ReachabilityState::Offline),
    ]);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_addition = membership_event(Some(b_addition.event_id()), 2, a, c, "device-c", 3);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        3,
        [4; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [4; 32],
        [5; 32],
        Vec::new(),
        None,
        vec![4],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [genesis, b_addition, c_addition] {
        history.receive_verified(event).unwrap();
    }
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision,
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let change = snapshot.current_change.expect("one current change");
    assert_eq!(change.change_id, removal.event_id());
    assert_eq!(change.proposed_by_device_id, DeviceId::new("device-a"));
    assert_eq!(change.target_device_ids, vec![DeviceId::new("device-b")]);
    assert!(!change.includes_local_device);
    assert!(change
        .apply_impact
        .requires_rejoin_device_ids
        .contains(&DeviceId::new("device-b")));
    assert!(change
        .keep_current_impact
        .paused_device_ids
        .contains(&DeviceId::new("device-a")));

    let a_view = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(a_view.reachability, ReachabilityState::Online);
    assert_eq!(a_view.membership, DeviceMembership::Active);
    assert_eq!(
        a_view.group_relationship,
        GroupRelationship::PendingLocalDecision
    );
    assert_eq!(a_view.compatibility, DeviceCompatibility::Compatible);
    assert_eq!(
        a_view.sync_relationship,
        SyncRelationship::WaitingForLocalDecision
    );

    let b_view = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-b"))
        .unwrap();
    assert_eq!(b_view.reachability, ReachabilityState::Offline);
    assert_eq!(b_view.membership, DeviceMembership::Active);
    assert_eq!(b_view.group_relationship, GroupRelationship::Unknown);
}

#[tokio::test]
async fn device_trust_query_reports_a_consistent_compatible_peer_as_usable() {
    use crate::space::convergence::SyncRelationship;

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Consistent,
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(peer.sync_relationship, SyncRelationship::Usable);
}

#[tokio::test]
async fn device_trust_query_keeps_reachability_independent_from_a_usable_relationship() {
    use crate::space::convergence::{GroupRelationship, SyncRelationship};

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    harness
        .presence
        .states
        .lock()
        .unwrap()
        .insert(DeviceId::new("device-a"), ReachabilityState::Offline);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Consistent,
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(peer.reachability, ReachabilityState::Offline);
    assert_eq!(peer.group_relationship, GroupRelationship::Consistent);
    assert_eq!(peer.sync_relationship, SyncRelationship::Usable);
    assert!(snapshot.current_change.is_none());
}

#[tokio::test]
async fn device_trust_query_reports_invalid_peer_facts_as_unverifiable_and_paused() {
    use crate::space::convergence::{ActionUnavailableReason, GroupRelationship, SyncRelationship};

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Invalid,
    );
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(peer.group_relationship, GroupRelationship::Unverifiable);
    assert_eq!(peer.sync_relationship, SyncRelationship::PausedUnverifiable);
    assert_eq!(
        peer.blocked_reason,
        Some(ActionUnavailableReason::DeviceFactsUnverifiable)
    );
}

#[tokio::test]
async fn device_trust_query_fails_closed_when_the_workspace_facts_are_unverifiable() {
    use crate::space::convergence::{ActionUnavailableReason, GroupRelationship, SyncRelationship};

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Consistent,
    );
    state.failure_category = Some(uc_core::membership::WorkspaceFailureCategory::DigestConflict);
    harness.repository.save_state(&state).await.unwrap();

    let snapshot = harness.owner.query_device_trust().await.unwrap();
    let peer = snapshot
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-a"))
        .unwrap();
    assert_eq!(peer.group_relationship, GroupRelationship::Unverifiable);
    assert_eq!(peer.sync_relationship, SyncRelationship::PausedUnverifiable);
    assert_eq!(
        snapshot.blocked_reason,
        Some(ActionUnavailableReason::DeviceFactsUnverifiable)
    );
    assert!(snapshot.allowed_actions.is_empty());
    assert!(snapshot.current_change.is_none());
}

// 流程：同一待决定项先保留当前组，再重复相同和相反选择；只保存一次，结果稳定且可跨查询恢复。
#[tokio::test]
async fn device_trust_decision_distinguishes_first_duplicate_and_conflicting_submissions() {
    use crate::space::convergence::DeviceTrustDecisionResult;

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [genesis, c_addition] {
        history.receive_verified(event).unwrap();
    }
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::KeptCurrentDeviceGroup { .. }
    ));
    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::AlreadyCompleted { .. }
    ));
    let restarted = WorkspaceConvergence::new(test_deps(
        Arc::new(harness.repository.clone()),
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    ));
    assert!(matches!(
        restarted
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::AlreadyCompleted { .. }
    ));
    assert!(matches!(
        restarted
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::ApplyChange,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::StateChanged { .. }
    ));
}

#[tokio::test]
async fn concurrent_matching_device_trust_decisions_save_only_one_completion() {
    use crate::space::convergence::DeviceTrustDecisionResult;

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(c_addition).unwrap();
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    let first_owner = Arc::clone(&harness.owner);
    let second_owner = Arc::clone(&harness.owner);
    let (first, second) = tokio::join!(
        first_owner.decide_device_trust_change(
            removal.event_id(),
            crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
            false,
        ),
        second_owner.decide_device_trust_change(
            removal.event_id(),
            crate::space::convergence::DeviceTrustChoice::KeepCurrentDeviceGroup,
            false,
        ),
    );
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                DeviceTrustDecisionResult::KeptCurrentDeviceGroup { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, DeviceTrustDecisionResult::AlreadyCompleted { .. }))
            .count(),
        1
    );
}

// 流程：新入口完成决定后，旧入口重复相同决定；重复提交返回当前结果而不是普通失败。
#[tokio::test]
async fn legacy_and_device_trust_decisions_share_idempotent_completion() {
    use crate::space::convergence::{DeviceTrustChoice, DeviceTrustDecisionResult};

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [genesis, c_addition] {
        history.receive_verified(event).unwrap();
    }
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                DeviceTrustChoice::KeepCurrentDeviceGroup,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::KeptCurrentDeviceGroup { .. }
    ));
    assert!(harness
        .owner
        .decide_membership_removal(removal.event_id(), RemovalDecision::Reject)
        .await
        .is_ok());
}

// 流程：待决定移除精确包含本机；没有二次确认时不能写入决定，确认后才退出当前设备组。
#[tokio::test]
async fn applying_a_change_that_removes_the_local_device_requires_explicit_confirmation() {
    use crate::space::convergence::DeviceTrustDecisionResult;

    let a = instance(0x0a);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let c_addition = membership_event(Some(genesis.event_id()), 1, a, c, "device-c", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: c },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        vec![3],
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [genesis, c_addition] {
        history.receive_verified(event).unwrap();
    }
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::ApplyChange,
                false,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::LocalDeviceConfirmationRequired { .. }
    ));
    assert_eq!(
        harness
            .repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .membership_reconciliation
            .unwrap()
            .pending_removal_decision(),
        Some(removal.event_id())
    );
    assert!(matches!(
        harness
            .owner
            .decide_device_trust_change(
                removal.event_id(),
                crate::space::convergence::DeviceTrustChoice::ApplyChange,
                true,
            )
            .await
            .unwrap(),
        DeviceTrustDecisionResult::Applied { .. }
    ));
}

// 流程：A 尝试移除不存在的设备或移除自己；操作失败，原成员历史和状态均不得保存变化。
#[tokio::test]
async fn removing_an_unknown_or_self_target_fails_without_saving() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis.clone()).unwrap();
    history.receive_verified(addition.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    let before = state.clone();
    harness.repository.save_state(&state).await.unwrap();

    assert!(matches!(
        harness
            .owner
            .submit_removal(&DeviceId::new("device-unknown"))
            .await,
        Err(WorkspaceConvergenceError::UnknownTarget)
    ));
    assert!(matches!(
        harness
            .owner
            .submit_removal(&DeviceId::new("device-a"))
            .await,
        Err(WorkspaceConvergenceError::SelfTarget)
    ));
    assert_eq!(
        harness.repository.load_state().await.unwrap(),
        Some(before),
        "failed removal must not change the saved state"
    );
}

// 流程：成员历史在邀请签发后继续前进；旧邀请绑定的历史位置失效，不能再用于加入。
#[tokio::test]
async fn membership_history_advancement_invalidates_an_older_invitation() {
    let a = instance(0x0a);
    let harness = harness("device-a", vec![(DeviceId::new("device-a"), a)]);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert_eq!(
        harness.owner.admission_decision(0).await,
        MembershipAdmissionDecision::SupersededInvitation
    );
}

// 流程：A 完成新成员加入后联系尚未建立历史关系的 B；首包携带受限的连续历史，
// 让 B 即使尚未保存 A 的最新成员资料也能验证并接纳本次引荐。
#[tokio::test]
async fn admission_sync_introduces_the_applied_signed_history_to_each_peer() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis.clone()).unwrap();
    history.receive_verified(addition.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    harness.owner.synchronize_chain().await.unwrap();

    let sent = harness.history_exchange.history_sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, DeviceId::new("device-b"));
    let MembershipHistoryMessage::EventsResponse(introduction) = &sent[0].1 else {
        panic!("unknown member must receive a signed history introduction");
    };
    assert_eq!(introduction.after_event_id, None);
    assert_eq!(introduction.events, vec![genesis, addition]);
}

// 流程：A 不在线时 B 将 C 加入空间；A 恢复但未保存 C 的资料，C 首次联系即提交
// 从起点到 C 的连续成员记录，使 A 能从历史本身验证 C 的准入关系。
#[tokio::test]
async fn unknown_admitted_member_introduces_its_signed_history_before_regular_exchange() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let repository = MemoryWorkspaceRepository::default();
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_join = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_join = membership_event(Some(b_join.event_id()), 2, b, c, "device-c", 3);
    let exchange = Arc::new(ScriptedExchange::new(vec![MembershipHistoryMessage::Ack(
        MembershipHistoryAck::UpdatesApplied,
    )]));
    let mut deps = test_deps(
        Arc::new(repository.clone()),
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
            (DeviceId::new("device-c"), c),
        ],
    );
    deps.membership_history_exchange = exchange.clone();
    deps.own_device = DeviceId::new("device-c");
    let owner = WorkspaceConvergence::new(deps);
    owner.record_local_readiness(c).await.unwrap();
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis.clone()).unwrap();
    history.receive_verified(b_join.clone()).unwrap();
    history.receive_verified(c_join.clone()).unwrap();
    let mut state = repository.load_state().await.unwrap().unwrap();
    state.membership_reconciliation = Some(history);
    repository.save_state(&state).await.unwrap();

    owner
        .reconcile_membership_history_with_peer(&DeviceId::new("device-a"))
        .await
        .unwrap();

    let sent = exchange.history_sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    let MembershipHistoryMessage::EventsResponse(introduction) = &sent[0].1 else {
        panic!("unknown member must send signed history on first contact");
    };
    assert_eq!(introduction.after_event_id, None);
    assert_eq!(introduction.events, vec![genesis, b_join, c_join]);
    let state = repository.load_state().await.unwrap().unwrap();
    assert_eq!(
        state
            .peer_history_relationships
            .get(&DeviceId::new("device-a")),
        Some(&uc_core::membership::MembershipHistoryRelationship::Consistent)
    );
}

// 流程：普通内容面对一致设备可通过，面对待决定或已分叉设备被阻止；成员资格本身不被改写。
#[tokio::test]
async fn content_gate_blocks_only_pending_or_diverged_history_peers() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let pending = DeviceId::new("device-pending");
    let unaffected = DeviceId::new("device-unaffected");
    let pending_instance = instance(0x0c);
    let unaffected_instance = instance(0x0d);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let pending_addition = membership_event(
        Some(genesis.event_id()),
        1,
        a,
        pending_instance,
        pending.as_str(),
        2,
    );
    let unaffected_addition = membership_event(
        Some(pending_addition.event_id()),
        2,
        a,
        unaffected_instance,
        unaffected.as_str(),
        3,
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    for event in [genesis, pending_addition, unaffected_addition] {
        history.receive_verified(event).unwrap();
    }
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state
        .apply(
            WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                peer: pending,
                relationship:
                    uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision,
            },
            2,
        )
        .unwrap();
    harness.repository.save_state(&state).await.unwrap();

    assert!(harness.owner.locally_removed(&pending).await);
    assert!(!harness.owner.locally_removed(&unaffected).await);
}

// 流程：A 已确认 B 低于 1.1 并重启；重启后升级提示和双向内容暂停仍然保留。
#[tokio::test]
async fn upgrade_required_peer_remains_blocked_after_owner_restart() {
    let a = instance(0x0a);
    let repository = MemoryWorkspaceRepository::default();
    let first = WorkspaceConvergence::new(test_deps(
        Arc::new(repository.clone()),
        "device-a",
        vec![(DeviceId::new("device-a"), a)],
    ));
    let peer = DeviceId::new("device-b");
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state
        .apply(
            WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                peer: peer.clone(),
                relationship: uc_core::membership::MembershipHistoryRelationship::UpgradeRequired,
            },
            2,
        )
        .unwrap();
    repository.save_state(&state).await.unwrap();
    assert!(first.locally_removed(&peer).await);

    let restarted = WorkspaceConvergence::new(test_deps(
        Arc::new(repository),
        "device-a",
        vec![(DeviceId::new("device-a"), a)],
    ));

    assert!(restarted.locally_removed(&peer).await);
    assert_eq!(
        restarted
            .query()
            .await
            .unwrap()
            .upgrade_required_peer_device_ids,
        vec![peer]
    );
}

// 流程：A 已是 1.1，B 曾被标记为需要升级；B 升级到 1.1 后上线并完成当前成员历史回应。
// 证明：A 只运行当前流程、清除升级提示并恢复 B 的正常内容资格。
#[tokio::test]
async fn current_peer_confirmation_clears_upgrade_required_without_legacy_probe() {
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(ScriptedExchange::new(vec![MembershipHistoryMessage::Ack(
        MembershipHistoryAck::Consistent,
    )]));
    let probe = Arc::new(ScriptedLegacyProbe::new(Vec::new()));
    let peer = DeviceId::new("device-b");
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.membership_history_exchange = exchange.clone();
    deps.legacy_peer_probe = probe.clone();
    let owner = WorkspaceConvergence::new(deps);
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    let a = instance(0x0a);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b = instance(0x0b);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state
        .apply(
            WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                peer: peer.clone(),
                relationship: uc_core::membership::MembershipHistoryRelationship::UpgradeRequired,
            },
            2,
        )
        .unwrap();
    repository.save_state(&state).await.unwrap();

    owner
        .reconcile_membership_history_with_peer(&peer)
        .await
        .unwrap();

    let snapshot = owner.query().await.unwrap();
    assert!(snapshot.upgrade_required_peer_device_ids.is_empty());
    assert!(!owner.locally_removed(&peer).await);
    assert_eq!(exchange.history_sent.lock().unwrap().len(), 1);
    assert!(probe.calls.lock().unwrap().is_empty());
}

// 流程：A、B 都从低于 1.1 的同一旧 Space 升级，双方起初都没有 1.1 成员历史；
// A 建立唯一历史起点，B 通过当前问候提交自己的签名资料，双方保存同一历史后 A 清除升级提示。
#[tokio::test]
async fn two_upgraded_legacy_members_establish_one_current_history_and_resume_exchange() {
    let a_repository = MemoryWorkspaceRepository::default();
    let mut a_state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    a_state.migrated_from_pre_adr_020 = true;
    a_state
        .apply(
            WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                peer: DeviceId::new("device-b"),
                relationship: uc_core::membership::MembershipHistoryRelationship::UpgradeRequired,
            },
            2,
        )
        .unwrap();
    a_repository.save_state(&a_state).await.unwrap();
    let mut a_deps = test_deps(Arc::new(a_repository.clone()), "device-a", Vec::new());
    a_deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    a_deps.space_protection = Arc::new(ProtectsQueriedMembers::with_active_legacy_bootstrap());
    let a_owner = WorkspaceConvergence::new(a_deps);

    let b_repository = MemoryWorkspaceRepository::default();
    let b = instance(0x0b);
    let mut b_state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    b_state.own_instance = Some(b);
    b_state.membership_reconciliation = Some(MembershipReconciliation::new(SPACE.to_owned(), b));
    b_state.migrated_from_pre_adr_020 = true;
    b_repository.save_state(&b_state).await.unwrap();
    let mut b_deps = test_deps(Arc::new(b_repository.clone()), "device-b", Vec::new());
    b_deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    b_deps.space_protection = Arc::new(ProtectsQueriedMembers::with_active_legacy_bootstrap());
    let b_owner = WorkspaceConvergence::new(b_deps);

    let initialized = a_owner.initialize_upgraded_legacy_space().await.unwrap();
    assert_eq!(initialized.history_event_count, 1);
    let response = a_owner
        .handle_membership_history(
            &DeviceId::new("device-b"),
            MembershipHistoryMessage::Hello(uc_core::membership::MembershipHistoryHello {
                lineage_id: SPACE.to_owned(),
                member_instance_id: b,
                admission: admission_facts_for(b, &DeviceId::new("device-b")),
                known_head: None,
                applied_head: None,
                applied_members_digest: None,
            }),
        )
        .await
        .unwrap();
    let MembershipHistoryMessage::EventsResponse(events) = response else {
        panic!("initializer must return the shared current history");
    };
    assert_eq!(events.events.len(), 2);

    let acknowledgement = b_owner
        .handle_membership_history(
            &DeviceId::new("device-a"),
            MembershipHistoryMessage::EventsResponse(events),
        )
        .await
        .unwrap();
    assert_eq!(
        acknowledgement,
        MembershipHistoryMessage::Ack(MembershipHistoryAck::UpdatesApplied)
    );
    let a_snapshot = a_owner.query().await.unwrap();
    let b_snapshot = b_owner.query().await.unwrap();
    assert_eq!(a_snapshot.history_event_count, 2);
    assert_eq!(b_snapshot.history_event_count, 2);
    assert_eq!(a_snapshot.effective_member_count, 2);
    assert_eq!(b_snapshot.effective_member_count, 2);
    assert!(a_snapshot.upgrade_required_peer_device_ids.is_empty());
    assert!(!a_owner.locally_removed(&DeviceId::new("device-b")).await);
    assert!(
        !a_repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .migrated_from_pre_adr_020
    );
    assert!(
        !b_repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .migrated_from_pre_adr_020
    );
    assert_eq!(
        a_owner.snapshot().await.unwrap().source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory
    );
    assert_eq!(
        b_owner.snapshot().await.unwrap().source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory
    );
}

// Flow: this device created the persisted legacy bootstrap before a restart, but its device ID is
// not the deterministic minimum; the bootstrap owner must still finish the missing history root.
#[tokio::test]
async fn active_legacy_bootstrap_owner_initializes_history_after_restart_even_when_not_smallest() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-z", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-a")]));
    deps.space_protection = Arc::new(ProtectsQueriedMembers::with_active_legacy_bootstrap());
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.initialize_upgraded_legacy_space().await.unwrap();

    assert_eq!(snapshot.history_event_count, 1);
}

// Flow: the deterministic initializer creates the signed history root while a retained legacy
// peer still awaits admission; ordinary peer scope must switch to current history immediately.
#[tokio::test]
async fn initialized_legacy_history_excludes_pending_legacy_peer_from_ordinary_scope() {
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(ProtectsQueriedMembers::with_active_legacy_bootstrap());
    let owner = WorkspaceConvergence::new(deps);

    let initialized = owner.initialize_upgraded_legacy_space().await.unwrap();
    let scope = owner.snapshot().await.unwrap();

    assert_eq!(initialized.history_event_count, 1);
    assert_eq!(
        scope.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory
    );
    assert!(scope.peer_device_ids.is_empty());
}

// Flow: a pre-ADR-020 installation has a retained legacy roster but no convergence-state row.
// Creating the current-history root must stop legacy records from granting ordinary membership.
#[tokio::test]
async fn fresh_legacy_upgrade_switches_to_current_history_after_initialization() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.initial_state_origin =
        super::WorkspaceConvergenceStateOrigin::UpgradeWithoutConvergenceState;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
        legacy_member("device-c"),
    ]));
    deps.space_protection = Arc::new(ProtectsQueriedMembers::with_active_legacy_bootstrap());
    let owner = WorkspaceConvergence::new(deps);

    let initialized = owner.initialize_upgraded_legacy_space().await.unwrap();
    let scope = owner.snapshot().await.unwrap();
    let saved = repository.load_state().await.unwrap().unwrap();

    assert_eq!(initialized.history_event_count, 1);
    assert!(!saved.migrated_from_pre_adr_020);
    assert_eq!(
        scope.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory
    );
    assert!(scope.peer_device_ids.is_empty());
}

#[test]
fn earlier_app_version_marks_an_upgrade_without_convergence_state() {
    assert_eq!(
        super::WorkspaceConvergenceStateOrigin::from_version_transition(
            Some("0.19.1"),
            "1.0.0-alpha.3"
        ),
        super::WorkspaceConvergenceStateOrigin::UpgradeWithoutConvergenceState
    );
    assert_eq!(
        super::WorkspaceConvergenceStateOrigin::from_version_transition(
            Some("1.0.0-alpha.3"),
            "1.0.0-alpha.3"
        ),
        super::WorkspaceConvergenceStateOrigin::CurrentInstallation
    );
    assert_eq!(
        super::WorkspaceConvergenceStateOrigin::from_version_transition(None, "1.0.0-alpha.3"),
        super::WorkspaceConvergenceStateOrigin::CurrentInstallation
    );
    assert_eq!(
        super::WorkspaceConvergenceStateOrigin::from_version_transition(
            Some("1.1.0"),
            "1.0.0-alpha.3"
        ),
        super::WorkspaceConvergenceStateOrigin::CurrentInstallation
    );
    assert_eq!(
        super::WorkspaceConvergenceStateOrigin::from_version_transition(
            Some("not-semver"),
            "1.0.0-alpha.3"
        ),
        super::WorkspaceConvergenceStateOrigin::CurrentInstallation
    );
    assert_eq!(
        super::WorkspaceConvergenceStateOrigin::from_version_transition(
            Some("0.19.1"),
            "not-semver"
        ),
        super::WorkspaceConvergenceStateOrigin::CurrentInstallation
    );
}

// Flow: the retained legacy peer submits its signed current identity after the initializer has
// created the history root; once the applied history covers the retained roster, the migration
// marker must be cleared and the current history becomes the only runtime scope.
#[tokio::test]
async fn admitted_legacy_roster_completes_migration_and_switches_to_current_history() {
    let repository = MemoryWorkspaceRepository::default();
    let a = instance(0x0a);
    let b = instance(0x0b);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history
        .receive_verified(membership_event(None, 0, a, a, "device-a", 1))
        .unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(ProtectsQueriedMembers::with_active_legacy_bootstrap());
    let owner = WorkspaceConvergence::new(deps);

    owner
        .handle_membership_history(
            &DeviceId::new("device-b"),
            MembershipHistoryMessage::Hello(uc_core::membership::MembershipHistoryHello {
                lineage_id: SPACE.to_owned(),
                member_instance_id: b,
                admission: admission_facts_for(b, &DeviceId::new("device-b")),
                known_head: None,
                applied_head: None,
                applied_members_digest: None,
            }),
        )
        .await
        .unwrap();

    let saved = repository.load_state().await.unwrap().unwrap();
    let scope = owner.snapshot().await.unwrap();
    assert!(!saved.migrated_from_pre_adr_020);
    assert_eq!(
        scope.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory
    );
    assert_eq!(scope.peer_device_ids, vec![DeviceId::new("device-b")]);
}

// Flow: signed membership history reaches the retained peer before that peer has joined the
// shared protection group. Ordinary scope still switches independently to current history.
#[tokio::test]
async fn membership_history_controls_ordinary_scope_before_protection_roster_is_ready() {
    let repository = MemoryWorkspaceRepository::default();
    let a = instance(0x0a);
    let b = instance(0x0b);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history
        .receive_verified(membership_event(None, 0, a, a, "device-a", 1))
        .unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state.migrated_from_pre_adr_020 = true;
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("device-a"),
        legacy_member("device-b"),
    ]));
    deps.space_protection = Arc::new(PartiallyProtectedRoster);
    let owner = WorkspaceConvergence::new(deps);

    owner
        .handle_membership_history(
            &DeviceId::new("device-b"),
            MembershipHistoryMessage::Hello(uc_core::membership::MembershipHistoryHello {
                lineage_id: SPACE.to_owned(),
                member_instance_id: b,
                admission: admission_facts_for(b, &DeviceId::new("device-b")),
                known_head: None,
                applied_head: None,
                applied_members_digest: None,
            }),
        )
        .await
        .unwrap();

    let saved = repository.load_state().await.unwrap().unwrap();
    let scope = owner.snapshot().await.unwrap();
    assert!(!saved.migrated_from_pre_adr_020);
    assert_eq!(
        scope.source,
        uc_core::membership::CurrentWorkspacePeerScopeSource::CurrentHistory
    );
    assert_eq!(scope.peer_device_ids, vec![DeviceId::new("device-b")]);
}

// Flow: the legacy joiner has joined the shared protection group but still has no applied
// membership history. Even when its device ID sorts before the sponsor, completing that join must
// fetch the sponsor history instead of creating a competing local root.
#[tokio::test]
async fn upgraded_legacy_joiner_fetches_sponsor_history_when_its_device_id_is_smallest() {
    let a = instance(0x09);
    let b = instance(0x0a);
    let genesis = membership_event(None, 0, a, a, "device-z", 1);
    let b_join = membership_event(Some(genesis.event_id()), 1, a, b, "device-a", 2);
    let exchange = Arc::new(ScriptedExchange::new(vec![
        MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
            lineage_id: SPACE.to_owned(),
            after_event_id: None,
            events: vec![genesis, b_join],
        }),
    ]));
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(
        Arc::new(repository),
        "device-a",
        vec![
            (DeviceId::new("device-z"), a),
            (DeviceId::new("device-a"), b),
        ],
    );
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-z")]));
    deps.membership_history_exchange = exchange.clone();
    deps.own_device = DeviceId::new("device-a");
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner
        .complete_upgraded_legacy_join(&DeviceId::new("device-z"))
        .await
        .unwrap();

    assert_eq!(snapshot.history_event_count, 2);
    assert_eq!(snapshot.effective_member_count, 2);
    let sent = exchange.history_sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, DeviceId::new("device-z"));
    assert!(matches!(sent[0].1, MembershipHistoryMessage::Hello(_)));
}

// 流程：A 已是 1.1，B 低于 1.1；当前成员历史入口没有回应，但旧入口空连接成功。
// 证明：只有旧入口的正面证据会让 A 保存“B 需要升级”，并暂停内容同步。
#[tokio::test]
async fn confirmed_legacy_peer_is_marked_upgrade_required_after_current_flow_is_unavailable() {
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(ScriptedExchange::new(Vec::new()));
    let probe = Arc::new(ScriptedLegacyProbe::new(vec![Ok(())]));
    let peer = DeviceId::new("device-b");
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.membership_history_exchange = exchange.clone();
    deps.legacy_peer_probe = probe.clone();
    let owner = WorkspaceConvergence::new(deps);
    let a = instance(0x0a);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    repository.save_state(&state).await.unwrap();

    owner
        .reconcile_membership_history_with_peer(&peer)
        .await
        .unwrap();

    assert!(owner.locally_removed(&peer).await);
    assert_eq!(
        owner
            .query()
            .await
            .unwrap()
            .upgrade_required_peer_device_ids,
        vec![peer.clone()]
    );
    assert_eq!(*probe.calls.lock().unwrap(), vec![peer]);
}

// 流程：A 从已有 Space 启动为 1.1，B 仍低于 1.1 且已经在线；A 解锁后恢复成员活动，但没有新的上线通知。
// 证明：会话恢复会让负责人主动核对已保存的 B，并在旧入口确认后保存“B 需要升级”。
#[tokio::test]
async fn session_resume_reconciles_an_existing_legacy_member_without_a_new_online_event() {
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(ScriptedExchange::new(Vec::new()));
    let probe = Arc::new(ScriptedLegacyProbe::new(vec![Ok(())]));
    let peer = DeviceId::new("device-b");
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.membership_history_exchange = exchange;
    deps.legacy_peer_probe = probe.clone();
    deps.peer_addr_repo = Arc::new(FixedPeerAddrRepo {
        records: vec![uc_core::ports::PeerAddressRecord {
            device_id: peer.clone(),
            addr_blob: vec![1],
            observed_at: chrono::Utc::now(),
        }],
    });
    let owner = WorkspaceConvergence::new(deps);
    let (_presence_tx, presence_events) = tokio::sync::broadcast::channel(1);

    let runtime = Arc::clone(&owner).start(presence_events);
    runtime
        .activity()
        .resume()
        .await
        .expect("resume workspace convergence after session unlock");
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if owner
                .query()
                .await
                .unwrap()
                .upgrade_required_peer_device_ids
                == vec![peer.clone()]
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("startup reconciliation marks the existing legacy member");
    runtime.shutdown().await;

    assert_eq!(*probe.calls.lock().unwrap(), vec![peer]);
}

// 流程：A 与 B 均无法完成当前流程和旧入口空连接。
// 证明：网络或身份类失败不产生“需要升级”提示，也不改变原有关系。
#[tokio::test]
async fn indeterminate_peer_does_not_be_reported_as_requiring_an_upgrade() {
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(ScriptedExchange::new(Vec::new()));
    let probe = Arc::new(ScriptedLegacyProbe::new(vec![Err(
        uc_core::membership::LegacyPeerProbeError::Transport,
    )]));
    let peer = DeviceId::new("device-b");
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.membership_history_exchange = exchange;
    deps.legacy_peer_probe = probe.clone();
    let owner = WorkspaceConvergence::new(deps);
    let a = instance(0x0a);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b = instance(0x0b);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    state
        .apply(
            WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                peer: peer.clone(),
                relationship: uc_core::membership::MembershipHistoryRelationship::Consistent,
            },
            2,
        )
        .unwrap();
    repository.save_state(&state).await.unwrap();

    assert!(owner
        .reconcile_membership_history_with_peer(&peer)
        .await
        .is_err());

    assert!(owner
        .query()
        .await
        .unwrap()
        .upgrade_required_peer_device_ids
        .is_empty());
    assert!(!owner.locally_removed(&peer).await);
    assert_eq!(*probe.calls.lock().unwrap(), vec![peer]);
}

// 流程：A 尝试与 B 进行本次 1.1 的成员历史核对，B 明确拒绝该请求；旧入口空连接即使可用也不能改写结果。
// 证明：明确拒绝属于当前流程或身份资料问题，不是旧版本的正面证据；A 不探测旧入口、不显示升级提示。
#[tokio::test]
async fn rejected_current_peer_is_not_probed_or_reported_as_requiring_an_upgrade() {
    let repository = MemoryWorkspaceRepository::default();
    let probe = Arc::new(ScriptedLegacyProbe::new(vec![Ok(())]));
    let peer = DeviceId::new("device-b");
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.membership_history_exchange = Arc::new(RejectingExchange);
    deps.legacy_peer_probe = probe.clone();
    let owner = WorkspaceConvergence::new(deps);
    let a = instance(0x0a);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b = instance(0x0b);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    repository.save_state(&state).await.unwrap();

    assert!(owner
        .reconcile_membership_history_with_peer(&peer)
        .await
        .is_err());

    assert!(owner
        .query()
        .await
        .unwrap()
        .upgrade_required_peer_device_ids
        .is_empty());
    assert!(!owner.locally_removed(&peer).await);
    assert!(probe.calls.lock().unwrap().is_empty());
}

// 流程：B 的两次上线通知几乎同时到达 A；第一次核对尚未完成时，第二次必须等待，不能并行识别或拨号。
#[tokio::test]
async fn concurrent_online_events_run_one_reconciliation_per_peer() {
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(BlockingTrackingExchange::new());
    let peer = DeviceId::new("device-b");
    let mut deps = test_deps(Arc::new(repository.clone()), "device-a", Vec::new());
    deps.membership_history_exchange = exchange.clone();
    let owner = WorkspaceConvergence::new(deps);
    let a = instance(0x0a);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    repository.save_state(&state).await.unwrap();

    let first_owner = Arc::clone(&owner);
    let first_peer = peer.clone();
    let first = tokio::spawn(async move {
        first_owner
            .reconcile_membership_history_with_peer(&first_peer)
            .await
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        exchange.started.notified(),
    )
    .await
    .expect("first reconciliation starts");

    let second_owner = Arc::clone(&owner);
    let second_peer = peer.clone();
    let second = tokio::spawn(async move {
        second_owner
            .reconcile_membership_history_with_peer(&second_peer)
            .await
    });
    tokio::task::yield_now().await;
    assert_eq!(exchange.calls.load(Ordering::SeqCst), 1);
    assert_eq!(exchange.maximum_active.load(Ordering::SeqCst), 1);

    exchange.releases.add_permits(2);
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(exchange.calls.load(Ordering::SeqCst), 2);
    assert_eq!(exchange.maximum_active.load(Ordering::SeqCst), 1);
}

// 流程：B 收到 A 提交的有效移除历史；B 保存同一事件，但不改变成员集合，并发布一次待用户决定。
#[tokio::test]
async fn received_remote_removal_history_is_saved_and_waits_for_a_local_decision() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-b",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    assert!(history.receive_verified(genesis).is_ok());
    assert!(history.receive_verified(addition.clone()).is_ok());
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    let reply = harness
        .owner
        .handle_membership_history(
            &DeviceId::new("device-a"),
            MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
                lineage_id: SPACE.to_owned(),
                after_event_id: Some(addition.event_id()),
                events: vec![removal.clone()],
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        reply,
        MembershipHistoryMessage::Ack(MembershipHistoryAck::RemovalDecisionRequired {
            removal_event_id: removal.event_id(),
        })
    );
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.effective_members(), [a, b].into());
    assert_eq!(
        state
            .peer_history_relationships
            .get(&DeviceId::new("device-a")),
        Some(&uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision)
    );
}

// 流程：B 收到 A 对 B 的移除时先保存事实但不改变本机安全状态；B 明确接受后，才应用该移除携带的安全更新。
#[tokio::test]
async fn pending_remote_removal_does_not_apply_its_security_update_before_acceptance() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let repository = MemoryWorkspaceRepository::default();
    let security_updates = Arc::new(RecordingSecurityUpdates::default());
    let mut deps = test_deps(Arc::new(repository.clone()), "device-b", Vec::new());
    deps.security_updates = security_updates.clone();
    deps.own_device = DeviceId::new("device-b");
    let owner = WorkspaceConvergence::new(deps);
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [3; 32],
        [4; 32],
        b"pending-removal-security-update".to_vec(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.membership_reconciliation = Some(history);
    repository.save_state(&state).await.unwrap();

    owner
        .handle_membership_history(
            &DeviceId::new("device-a"),
            MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
                lineage_id: SPACE.to_owned(),
                after_event_id: Some(addition.event_id()),
                events: vec![removal.clone()],
            }),
        )
        .await
        .unwrap();

    assert!(
        security_updates.applied_payloads.lock().unwrap().is_empty(),
        "a remote removal cannot alter local security state before the local user accepts it"
    );

    owner
        .decide_membership_removal(
            removal.event_id(),
            uc_core::membership::RemovalDecision::Accept,
        )
        .await
        .unwrap();

    assert_eq!(
        security_updates.applied_payloads.lock().unwrap().as_slice(),
        [b"pending-removal-security-update".to_vec()],
        "the accepted removal must apply its saved security update exactly once"
    );
}

#[tokio::test]
async fn pending_membership_decision_delivery_survives_failure_and_restart() {
    let repository = MemoryWorkspaceRepository::default();
    let recipient = DeviceId::new("device-a");
    let decision = uc_core::membership::MembershipDecision::new(
        SPACE.to_owned(),
        membership_event(None, 0, instance(0x0a), instance(0x0a), "device-a", 1).event_id(),
        instance(0x0b),
        RemovalDecision::Accept,
        None,
        [1; 32],
        [2; 16],
        vec![3],
    );
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.pending_membership_decision_deliveries.push(
        uc_core::membership::PendingMembershipDecisionDelivery {
            recipient: recipient.clone(),
            decision: decision.clone(),
        },
    );
    repository.save_state(&state).await.unwrap();
    let mut failed_deps = test_deps(Arc::new(repository.clone()), "device-b", Vec::new());
    failed_deps.membership_history_exchange = Arc::new(RejectingExchange);
    WorkspaceConvergence::new(failed_deps)
        .deliver_pending_membership_decisions()
        .await
        .unwrap();
    assert_eq!(
        repository
            .load_state()
            .await
            .unwrap()
            .unwrap()
            .pending_membership_decision_deliveries
            .len(),
        1
    );

    let exchange = Arc::new(UnusedExchange::default());
    let mut restarted_deps = test_deps(Arc::new(repository.clone()), "device-b", Vec::new());
    restarted_deps.membership_history_exchange = exchange.clone();
    WorkspaceConvergence::new(restarted_deps)
        .deliver_pending_membership_decisions()
        .await
        .unwrap();

    assert!(repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .pending_membership_decision_deliveries
        .is_empty());
    assert_eq!(
        exchange.history_sent.lock().unwrap().as_slice(),
        &[(recipient, MembershipHistoryMessage::Decision(decision))]
    );
}

// 流程：A 与 B 已经分叉；A 请求 B 的旧 Space 成员资料，B 必须拒绝，不再继续交换旧分支。
#[tokio::test]
async fn diverged_peer_cannot_request_old_space_membership_history() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-b",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    history.receive_verified(genesis).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::Diverged,
    );
    harness.repository.save_state(&state).await.unwrap();

    let reply = harness
        .owner
        .handle_membership_history(
            &DeviceId::new("device-a"),
            MembershipHistoryMessage::EventsRequest(uc_core::membership::MembershipEventsRequest {
                lineage_id: SPACE.to_owned(),
                after_event_id: None,
                max_events: 1,
            }),
        )
        .await
        .unwrap();

    assert_eq!(
        reply,
        MembershipHistoryMessage::Ack(MembershipHistoryAck::Diverged),
        "a diverged peer must not receive old-space membership history"
    );
}

// 流程：A 已移除 B；B 接受后回传决定，A 仍依据移除前保存的成员关系验证并记录该回传。
#[tokio::test]
async fn removal_author_records_the_removed_peer_acceptance() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), a);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition.clone()).unwrap();
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(a);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();
    let decision = uc_core::membership::MembershipDecision::new(
        SPACE.to_owned(),
        removal.event_id(),
        b,
        uc_core::membership::RemovalDecision::Accept,
        Some(addition.event_id()),
        removal.resulting_members_digest,
        [4; 16],
        b"signature".to_vec(),
    );

    let reply = harness
        .owner
        .handle_membership_history(
            &DeviceId::new("device-b"),
            MembershipHistoryMessage::Decision(decision.clone()),
        )
        .await
        .unwrap();

    assert_eq!(
        reply,
        MembershipHistoryMessage::Ack(MembershipHistoryAck::RemovalAccepted {
            removal_event_id: removal.event_id(),
        })
    );
    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(
        state
            .membership_reconciliation
            .unwrap()
            .decision_for(removal.event_id(), b),
        Some(&decision)
    );
}

// 流程：A 提交移除后，B 与 A、C 都曾交换到同一待决定历史；B 拒绝时把决定发给 A、C，等待双方按决定结果解除阻断或进入分叉。
#[tokio::test]
async fn rejecting_a_pending_removal_notifies_author_and_pending_peers() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let harness = harness(
        "device-b",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_join = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_join = membership_event(Some(b_join.event_id()), 2, a, c, "device-c", 3);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_join.event_id()),
        3,
        [4; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [4; 32],
        [5; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(b_join).unwrap();
    history.receive_verified(c_join).unwrap();
    assert!(matches!(
        history.receive_verified(removal.clone()),
        Ok(uc_core::membership::MembershipReconciliationOutcome::RemovalDecisionRequired { .. })
    ));
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-a"),
        uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision,
    );
    state.peer_history_relationships.insert(
        DeviceId::new("device-c"),
        uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision,
    );
    harness.repository.save_state(&state).await.unwrap();

    harness
        .owner
        .decide_membership_removal(
            removal.event_id(),
            uc_core::membership::RemovalDecision::Reject,
        )
        .await
        .unwrap();

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(
        state
            .peer_history_relationships
            .get(&DeviceId::new("device-a")),
        Some(&uc_core::membership::MembershipHistoryRelationship::Diverged)
    );
    assert_eq!(
        state
            .peer_history_relationships
            .get(&DeviceId::new("device-c")),
        Some(&uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision)
    );
    let sent = harness.history_exchange.history_sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].0, DeviceId::new("device-a"));
    assert_eq!(sent[1].0, DeviceId::new("device-c"));
    assert!(sent
        .iter()
        .all(|(_, message)| matches!(message, MembershipHistoryMessage::Decision(_))));
}

// 流程：C 接受 A 对 B 的移除时，先按决定前的成员分支固定通知名单；即使应用后 B 已不再有效，也必须收到 C 的相反决定。
#[tokio::test]
async fn accepting_a_removal_still_notifies_the_removed_target() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_join = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_join = membership_event(Some(b_join.event_id()), 2, a, c, "device-c", 3);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_join.event_id()),
        3,
        [4; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [4; 32],
        [5; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(b_join).unwrap();
    history.receive_verified(c_join).unwrap();
    history.receive_verified(removal.clone()).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    harness
        .owner
        .decide_membership_removal(
            removal.event_id(),
            uc_core::membership::RemovalDecision::Accept,
        )
        .await
        .unwrap();

    let sent = harness.history_exchange.history_sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert!(sent.iter().any(|(device, _)| device.as_str() == "device-a"));
    assert!(sent.iter().any(|(device, _)| device.as_str() == "device-b"));
}

// 流程：B、C 对同一项移除都选择拒绝；B 收到 C 的签名决定后确认双方仍在同一旧分支，解除内容阻断。
#[tokio::test]
async fn matching_rejections_restore_a_consistent_peer_relationship() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let harness = harness(
        "device-b",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
            (DeviceId::new("device-c"), c),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_join = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_join = membership_event(Some(b_join.event_id()), 2, a, c, "device-c", 3);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(c_join.event_id()),
        3,
        [4; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [4; 32],
        [5; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(b_join).unwrap();
    history.receive_verified(c_join.clone()).unwrap();
    history.receive_verified(removal.clone()).unwrap();
    let local_decision = uc_core::membership::MembershipDecision::new(
        SPACE.to_owned(),
        removal.event_id(),
        b,
        uc_core::membership::RemovalDecision::Reject,
        Some(c_join.event_id()),
        c_join.resulting_members_digest,
        [5; 16],
        b"signature".to_vec(),
    );
    history.record_decision(local_decision).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.membership_reconciliation = Some(history);
    state.peer_history_relationships.insert(
        DeviceId::new("device-c"),
        uc_core::membership::MembershipHistoryRelationship::PendingRemovalDecision,
    );
    harness.repository.save_state(&state).await.unwrap();
    let peer_decision = uc_core::membership::MembershipDecision::new(
        SPACE.to_owned(),
        removal.event_id(),
        c,
        uc_core::membership::RemovalDecision::Reject,
        Some(c_join.event_id()),
        c_join.resulting_members_digest,
        [6; 16],
        b"signature".to_vec(),
    );

    harness
        .owner
        .handle_membership_history(
            &DeviceId::new("device-c"),
            MembershipHistoryMessage::Decision(peer_decision),
        )
        .await
        .unwrap();

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(
        state
            .peer_history_relationships
            .get(&DeviceId::new("device-c")),
        Some(&uc_core::membership::MembershipHistoryRelationship::Consistent)
    );
}

// 流程：B 拒绝 A 提交的待决定移除后，保留原成员关系，并只隔离与 A 的旧分支。
#[tokio::test]
async fn rejecting_a_pending_removal_keeps_membership_and_isolates_only_that_peer() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-b",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(addition.event_id()),
        2,
        [3; 16],
        a,
        MembershipOperation::RemoveDevice { member: b },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), b);
    assert!(history.receive_verified(genesis).is_ok());
    assert!(history.receive_verified(addition).is_ok());
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(b);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();
    harness
        .owner
        .handle_membership_history(
            &DeviceId::new("device-a"),
            MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
                lineage_id: SPACE.to_owned(),
                after_event_id: removal.parent_event_id,
                events: vec![removal.clone()],
            }),
        )
        .await
        .unwrap();

    harness
        .owner
        .decide_membership_removal(
            removal.event_id(),
            uc_core::membership::RemovalDecision::Reject,
        )
        .await
        .unwrap();

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.effective_members(), [a, b].into());
    assert_eq!(
        state
            .peer_history_relationships
            .get(&DeviceId::new("device-a")),
        Some(&uc_core::membership::MembershipHistoryRelationship::Diverged)
    );
}

// 流程：A 完成 B 的加入并提交历史；当前有效成员及其设备绑定写入签名历史，随后移除 B 也按该历史生效。
#[tokio::test]
async fn committed_admission_records_the_effective_members_in_signed_history() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let session = uc_core::ports::pairing::PairingSessionId::new("history-admission");
    harness.owner.record_local_readiness(a).await.unwrap();
    harness
        .owner
        .begin_admission(&session, &DeviceId::new("device-b"), 0)
        .await
        .unwrap();
    let joiner = uc_core::membership::AdmissionChangeFacts {
        member_instance: b,
        device_id: DeviceId::new("device-b"),
        device_name: "b".to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
            "ABCD-EFGH-IJKL-MNOP",
        )
        .unwrap(),
        transport_public_key: vec![2; 32],
        transport_address_blob: vec![3],
        identity_signature: vec![4],
    };

    harness
        .owner
        .commit_joiner_admission(&session, joiner, vec![5])
        .await
        .unwrap();

    let state = harness.repository.load_state().await.unwrap().unwrap();
    let history = state.membership_reconciliation.as_ref().unwrap();
    assert_eq!(history.effective_members(), [a, b].into());
    assert_eq!(
        history.device_for_member(&a),
        Some(DeviceId::new("device-a"))
    );
    assert_eq!(
        history.device_for_member(&b),
        Some(DeviceId::new("device-b"))
    );
    assert_eq!(state.effective_members(), [a, b].into());

    harness
        .owner
        .submit_removal(&DeviceId::new("device-b"))
        .await
        .unwrap();
    let state = harness.repository.load_state().await.unwrap().unwrap();
    let history = state.membership_reconciliation.as_ref().unwrap();
    assert_eq!(history.effective_members(), [a].into());
    assert_eq!(state.effective_members(), [a].into());
}

// 流程：赞助方当前分支仍有 A 的有效成员实例；A 再次使用邀请加入时必须拒绝重复成员。
#[tokio::test]
async fn sponsor_rejects_a_joiner_with_an_active_member_instance() {
    let c = instance(0x0c);
    let a = instance(0x0a);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-c"), c),
            (DeviceId::new("device-a"), a),
        ],
    );
    let genesis = membership_event(None, 0, c, c, "device-c", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, c, a, "device-a", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert_eq!(
        harness
            .owner
            .admission_decision_for_joiner(2, &DeviceId::new("device-a"))
            .await,
        MembershipAdmissionDecision::Unavailable
    );
}

// 流程：赞助方当前分支只保留 A 的旧移除记录；A 使用新成员实例重新加入时必须允许继续准入。
#[tokio::test]
async fn sponsor_allows_a_removed_device_to_rejoin_with_a_new_instance() {
    let c = instance(0x0c);
    let a = instance(0x0a);
    let harness = harness(
        "device-c",
        vec![
            (DeviceId::new("device-c"), c),
            (DeviceId::new("device-a"), a),
        ],
    );
    let genesis = membership_event(None, 0, c, c, "device-c", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, c, a, "device-a", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(addition.event_id()),
        2,
        [3; 16],
        c,
        MembershipOperation::RemoveDevice { member: a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), c);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    history.receive_verified(removal).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(c);
    state.membership_reconciliation = Some(history);
    harness.repository.save_state(&state).await.unwrap();

    assert_eq!(
        harness
            .owner
            .admission_decision_for_joiner(3, &DeviceId::new("device-a"))
            .await,
        MembershipAdmissionDecision::Allowed
    );
}

// 流程：新建空间的 A 首次邀请 B；即使此前没有成员历史，A 也先记录自己的成员实例并完成 B 的加入。
#[tokio::test]
async fn first_sponsor_admission_records_the_initial_member_instance() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );
    let session = uc_core::ports::pairing::PairingSessionId::new("first-admission");
    harness
        .owner
        .begin_admission(&session, &DeviceId::new("device-b"), 0)
        .await
        .unwrap();

    harness
        .owner
        .commit_joiner_admission(
            &session,
            admission_facts_for(b, &DeviceId::new("device-b")),
            vec![5],
        )
        .await
        .expect("a newly created space can sponsor its first admission");

    let state = harness.repository.load_state().await.unwrap().unwrap();
    assert_eq!(state.own_instance, Some(a));
    assert_eq!(state.effective_members(), [a, b].into());
}

// 流程：持久成员历史仍指向 A 的旧实例，但当前安全状态已经使用新实例；
// A 必须先恢复这项身份冲突，不能继续邀请并对外报告加入成功。
#[tokio::test]
async fn sponsor_rejects_admission_when_persisted_and_current_local_instances_differ() {
    let old_a = instance(0x0b);
    let current_a = instance(0x0a);
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    let genesis = membership_event(None, 0, old_a, old_a, "device-a", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), old_a);
    history.receive_verified(genesis).unwrap();
    state.own_instance = Some(old_a);
    state.membership_reconciliation = Some(history);
    repository.save_state(&state).await.unwrap();

    let deps = test_deps(
        Arc::new(repository.clone()),
        "device-a",
        vec![(DeviceId::new("device-a"), current_a)],
    );
    let owner = WorkspaceConvergence::new(deps);
    let session = uc_core::ports::pairing::PairingSessionId::new("stale-local-instance");

    let result = owner
        .begin_admission(&session, &DeviceId::new("device-c"), 1)
        .await;

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(message))
            if message == "current member identity does not match persisted membership history"
    ));
    assert!(repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .pending_admissions
        .is_empty());
}

// 流程：加入方收到的发起者历史摘要与本机事实不符；加入被拒绝，原历史位置保持不变。
#[tokio::test]
async fn saved_admission_rejects_a_mismatched_sponsor_history() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let harness = harness(
        "device-a",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
        ],
    );

    harness.owner.record_local_readiness(a).await.unwrap();
    let before = harness.repository.load_state().await.unwrap().unwrap();
    let before_history = before.membership_reconciliation.unwrap();

    let result = harness
        .owner
        .record_admission_saved(uc_core::membership::AdmissionSavedFacts {
            history_digest: [0x11; 32],
            history_event_count: before_history.known_event_count() as u64,
            sponsor_facts: admission_facts_for(b, &DeviceId::new("device-b")),
        })
        .await;

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(message))
            if message == "sponsor admission history is incomplete or mismatched"
    ));

    let after = harness.repository.load_state().await.unwrap().unwrap();
    let after_history = after.membership_reconciliation.unwrap();
    assert_eq!(after_history.known_head(), before_history.known_head());
    assert_eq!(after_history.applied_head(), before_history.applied_head());
    assert_eq!(
        after_history.known_event_count(),
        before_history.known_event_count()
    );
}

// 流程：加入方保存准入资料前尚缺发起者的完整历史；先拉取并验证连续历史，匹配后才完成加入。
#[tokio::test]
async fn saved_admission_fetches_the_sponsor_history_before_join_completion() {
    let a = instance(0x0a);
    let b = instance(0x0b);
    let c = instance(0x0c);
    let repository = MemoryWorkspaceRepository::default();
    let genesis = membership_event(None, 0, a, a, "device-a", 1);
    let b_join = membership_event(Some(genesis.event_id()), 1, a, b, "device-b", 2);
    let c_join = membership_event(Some(b_join.event_id()), 2, b, c, "device-c", 3);
    let mut sponsor_history = MembershipReconciliation::new(SPACE.to_owned(), b);
    sponsor_history.receive_verified(genesis.clone()).unwrap();
    sponsor_history.receive_verified(b_join.clone()).unwrap();
    sponsor_history.receive_verified(c_join.clone()).unwrap();
    let exchange = Arc::new(ScriptedExchange::new(vec![
        MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
            lineage_id: SPACE.to_owned(),
            after_event_id: None,
            events: vec![genesis, b_join, c_join],
        }),
    ]));
    let mut deps = test_deps(
        Arc::new(repository.clone()),
        "device-c",
        vec![
            (DeviceId::new("device-a"), a),
            (DeviceId::new("device-b"), b),
            (DeviceId::new("device-c"), c),
        ],
    );
    deps.membership_history_exchange = exchange.clone();
    deps.own_device = DeviceId::new("device-c");
    let owner = WorkspaceConvergence::new(deps);
    owner.record_local_readiness(c).await.unwrap();

    owner
        .record_admission_saved(uc_core::membership::AdmissionSavedFacts {
            history_digest: sponsor_history.applied_members_digest().unwrap(),
            history_event_count: sponsor_history.known_event_count() as u64,
            sponsor_facts: admission_facts_for(b, &DeviceId::new("device-b")),
        })
        .await
        .unwrap();

    let state = repository.load_state().await.unwrap().unwrap();
    let history = state.membership_reconciliation.unwrap();
    assert_eq!(history.known_event_count(), 3);
    assert_eq!(
        history.applied_members_digest(),
        sponsor_history.applied_members_digest()
    );
    let sent = exchange.history_sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, DeviceId::new("device-b"));
    assert!(matches!(sent[0].1, MembershipHistoryMessage::Hello(_)));
}

// 流程：A 的旧实例已被 C 移除，随后 C 把 A 的新实例加入同一条已验证历史；A
// 以新实例拉取整条历史时直接采用最终分支，不为已废弃的旧实例产生待确认项。
#[tokio::test]
async fn saved_readmission_adopts_history_that_replaces_the_local_old_instance() {
    let c = instance(0x0c);
    let old_a = instance(0x0a);
    let new_a = instance(0x1a);
    let repository = MemoryWorkspaceRepository::default();
    let genesis = membership_event(None, 0, c, c, "device-c", 1);
    let old_admission = membership_event(Some(genesis.event_id()), 1, c, old_a, "device-a", 2);
    let removal = uc_core::membership::MembershipEvent::new(
        SPACE.to_owned(),
        Some(old_admission.event_id()),
        2,
        [3; 16],
        c,
        MembershipOperation::RemoveDevice { member: old_a },
        [3; 32],
        [4; 32],
        Vec::new(),
        None,
        b"signature".to_vec(),
    );
    let readmission = membership_event(Some(removal.event_id()), 3, c, new_a, "device-a", 4);
    let mut sponsor_history = MembershipReconciliation::new(SPACE.to_owned(), c);
    for event in [
        genesis.clone(),
        old_admission.clone(),
        removal.clone(),
        readmission.clone(),
    ] {
        sponsor_history.receive_verified(event).unwrap();
    }
    let exchange = Arc::new(ScriptedExchange::new(vec![
        MembershipHistoryMessage::EventsResponse(MembershipEventsResponse {
            lineage_id: SPACE.to_owned(),
            after_event_id: None,
            events: vec![genesis, old_admission, removal, readmission],
        }),
    ]));
    let mut deps = test_deps(
        Arc::new(repository.clone()),
        "device-a",
        vec![
            (DeviceId::new("device-c"), c),
            (DeviceId::new("device-a"), new_a),
        ],
    );
    deps.membership_history_exchange = exchange;
    deps.member_signatures = Arc::new(EventSignatureOnlyVerifier);
    deps.own_device = DeviceId::new("device-a");
    let owner = WorkspaceConvergence::new(deps);
    owner.record_local_readiness(new_a).await.unwrap();

    let snapshot = owner
        .record_admission_saved(uc_core::membership::AdmissionSavedFacts {
            history_digest: sponsor_history.applied_members_digest().unwrap(),
            history_event_count: sponsor_history.known_event_count() as u64,
            sponsor_facts: admission_facts_for(c, &DeviceId::new("device-c")),
        })
        .await
        .unwrap();

    assert_eq!(snapshot.effective_member_count, 2);
    assert!(snapshot.pending_removal_decision_event_id.is_none());
    assert!(!snapshot.removed);
    assert_eq!(
        snapshot.convergence_digest,
        sponsor_history
            .applied_members_digest()
            .map(uc_core::membership::WorkspaceDigest::from_bytes)
    );
}

// Flow: a fresh join is durable before its first network send and reopening
// the owner returns the same attempt, join id, ordinal, and request outbox.
#[tokio::test]
async fn durable_join_starts_once_and_survives_owner_restart() {
    use uc_core::membership::{
        AdmissionAttemptRepositoryPort, AdmissionOutboxPurposeV1, JoinerAdmissionStageV1,
    };

    let directory = tempfile::tempdir().unwrap();
    let repository: Arc<dyn AdmissionAttemptRepositoryPort> =
        durable_admission_repository(&directory, [0x23; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x31; 32]);
    let join_id = [0x32; 16];

    let first = owner
        .start_join(
            attempt_id,
            join_id,
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    assert_eq!(first.local_join_ordinal, Some(0));
    assert!(matches!(
        first.role_state,
        uc_core::membership::AdmissionAttemptRoleStateV1::Joiner(
            uc_core::membership::JoinerAdmissionStateV1 {
                stage: JoinerAdmissionStageV1::Initiated
            }
        )
    ));
    assert_eq!(first.outboxes.len(), 1);
    assert_eq!(
        first.outboxes[0].purpose,
        AdmissionOutboxPurposeV1::JoinRequest
    );

    let reopened = durable_admission_owner(repository);
    let second = reopened
        .start_join(
            attempt_id,
            join_id,
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    assert_eq!(second, first);
}

#[tokio::test]
async fn admission_unavailable_keeps_the_exact_pending_join() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x24; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x25; 32]);
    let started = owner
        .start_join(
            attempt_id,
            [0x26; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let metadata_before = repository.profile_metadata().await.unwrap();

    let retry = owner
        .record_admission_unavailable(attempt_id, &started.outboxes[0])
        .await
        .unwrap();

    assert_eq!(retry, started.outboxes[0]);
    assert_eq!(repository.load(attempt_id).await.unwrap(), Some(started));
    assert_eq!(
        repository.profile_metadata().await.unwrap(),
        metadata_before
    );
}

#[tokio::test]
async fn delivery_ack_clears_only_the_exact_supported_outbox() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x27; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x28; 32]);
    let started = owner
        .start_join(
            attempt_id,
            [0x29; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let mut wrong = super::admission_transaction::admission_acknowledgment(&started.outboxes[0]);
    wrong.payload_digest[0] ^= 0xff;

    assert!(owner
        .acknowledge_delivery(attempt_id, &wrong)
        .await
        .is_err());
    assert!(!repository.load(attempt_id).await.unwrap().unwrap().outboxes[0].superseded);

    let exact = super::admission_transaction::admission_acknowledgment(&started.outboxes[0]);
    owner
        .acknowledge_delivery(attempt_id, &exact)
        .await
        .unwrap();
    let saved = repository.load(attempt_id).await.unwrap().unwrap();
    assert!(saved.outboxes[0].superseded);
    assert!(saved.inbox_dedup.contains(&exact));
}

#[tokio::test]
async fn reset_projection_is_atomic_and_requires_a_quiet_admission_repository() {
    let busy_directory = tempfile::tempdir().unwrap();
    let busy_repository = durable_admission_repository(&busy_directory, [0x2a; 16]);
    let busy_owner = durable_admission_owner(Arc::clone(&busy_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x2b; 32]);
    let started = busy_owner
        .start_join(
            attempt_id,
            [0x2c; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let metadata_before = busy_repository.profile_metadata().await.unwrap();

    assert!(matches!(
        busy_owner.reset_join_projection_if_quiet().await,
        Err(WorkspaceConvergenceError::Unavailable)
    ));
    assert_eq!(
        busy_repository.load(attempt_id).await.unwrap(),
        Some(started)
    );
    assert_eq!(
        busy_repository.profile_metadata().await.unwrap(),
        metadata_before
    );

    let quiet_directory = tempfile::tempdir().unwrap();
    let quiet_repository = durable_admission_repository(&quiet_directory, [0x2d; 16]);
    let quiet_owner = durable_admission_owner(Arc::clone(&quiet_repository));
    let reset = quiet_owner.reset_join_projection_if_quiet().await.unwrap();
    assert_eq!(reset.join_projection_floor_ordinal, 0);
    assert_eq!(reset.device_trust_revision, 1);
}

#[tokio::test]
async fn invitation_consume_retry_is_no_write_and_terminal_compaction_waits_for_resolution() {
    use super::admission_transaction::InvitationConsumeResultV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x2e; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x2f; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x30; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x31; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, history, event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x32; 32],
            &initiated.outboxes[0],
            candidate,
            history,
            &event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();

    let before_retry = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let metadata_before_retry = sponsor_repository.profile_metadata().await.unwrap();
    sponsor
        .record_invitation_consume_result(attempt_id, InvitationConsumeResultV1::Retryable)
        .await
        .unwrap();
    assert_eq!(
        sponsor_repository.load(attempt_id).await.unwrap(),
        Some(before_retry)
    );
    assert_eq!(
        sponsor_repository.profile_metadata().await.unwrap(),
        metadata_before_retry
    );

    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"rejected")
        .await
        .unwrap();
    let rejected_ack = joiner
        .joiner_record_rejected(attempt_id, &rejected)
        .await
        .unwrap();
    sponsor
        .sponsor_confirm_rejected(attempt_id, &rejected_ack)
        .await
        .unwrap();

    assert!(sponsor.compact_if_settled(attempt_id).await.is_err());
    sponsor
        .record_invitation_consume_result(attempt_id, InvitationConsumeResultV1::Conflict)
        .await
        .unwrap();
    let after_conflict = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let metadata_after_conflict = sponsor_repository.profile_metadata().await.unwrap();
    sponsor
        .record_invitation_consume_result(attempt_id, InvitationConsumeResultV1::Conflict)
        .await
        .unwrap();
    assert_eq!(
        sponsor_repository.load(attempt_id).await.unwrap(),
        Some(after_conflict)
    );
    assert_eq!(
        sponsor_repository.profile_metadata().await.unwrap(),
        metadata_after_conflict
    );
    sponsor.compact_if_settled(attempt_id).await.unwrap();
}

#[tokio::test]
async fn restart_recovery_delivers_durable_outboxes_and_compacts_settled_terminal_attempts() {
    use uc_core::membership::{AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1};

    let joiner_dir = tempfile::tempdir().unwrap();
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x79; 16]);
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let joiner_attempt = uc_core::membership::AdmissionAttemptId::from_bytes([0x7a; 32]);
    joiner
        .start_join(
            joiner_attempt,
            [0x7b; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();

    let report = joiner
        .recover_with(&ConfirmingAdmissionDelivery)
        .await
        .unwrap();

    assert_eq!(report.deliveries_attempted, 1);
    assert_eq!(report.deliveries_confirmed, 1);
    assert_eq!(report.attempts_compacted, 0);
    let recovered_join = joiner_repository
        .load(joiner_attempt)
        .await
        .unwrap()
        .unwrap();
    assert!(recovered_join.outboxes[0].superseded);
    assert_eq!(recovered_join.inbox_dedup.len(), 1);

    let sponsor_dir = tempfile::tempdir().unwrap();
    let remote_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x7c; 16]);
    let remote_repository = durable_admission_repository(&remote_dir, [0x7d; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let remote = durable_admission_owner(remote_repository);
    let sponsor_attempt = uc_core::membership::AdmissionAttemptId::from_bytes([0x7e; 32]);
    let initiated = remote
        .start_join(
            sponsor_attempt,
            [0x7f; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, history, event, commitment, _) =
        durable_candidate_verification_fixture(sponsor_attempt);
    sponsor
        .sponsor_accept_and_offer(
            sponsor_attempt,
            [0x80; 32],
            &initiated.outboxes[0],
            candidate,
            history,
            &event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_reject_before_commit(
            sponsor_attempt,
            AdmissionRejectionReasonV1::IdentityConflict,
            b"joiner",
        )
        .await
        .unwrap();
    assert_eq!(rejected.purpose, AdmissionOutboxPurposeV1::Rejected);

    let report = durable_admission_owner(Arc::clone(&sponsor_repository))
        .recover_with(&ConfirmingAdmissionDelivery)
        .await
        .unwrap();

    assert_eq!(report.deliveries_attempted, 2);
    assert_eq!(report.deliveries_confirmed, 2);
    assert_eq!(report.attempts_compacted, 1);
    assert!(sponsor_repository
        .load(sponsor_attempt)
        .await
        .unwrap()
        .is_none());
    assert!(sponsor_repository
        .load_terminal(sponsor_attempt)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn candidate_bound_to_another_attempt_leaves_sponsor_state_unchanged() {
    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x33; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x34; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x35; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x36; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (history, event, commitment) = admission_verification_fixture([0x37; 32]);
    let candidate = super::admission_transaction::DurableAdmissionCandidateV1 {
        lineage_id: event.lineage_id.clone(),
        base_history_position: postcard::to_stdvec(&commitment.base_history_position).unwrap(),
        candidate_event: postcard::to_stdvec(&event).unwrap(),
        candidate_event_id: *event.event_id().as_bytes(),
        candidate_key_package: b"joiner-key-package".to_vec(),
        target_members_digest: event.resulting_members_digest,
        security_commitment: postcard::to_stdvec(&commitment).unwrap(),
        security_commit: b"sealed-security-commit".to_vec(),
        security_welcome: postcard::to_stdvec(&commitment).unwrap(),
        staged_security_state: b"sponsor-staged-state".to_vec(),
        identity_binding: b"sponsor-joiner-binding".to_vec(),
    };

    let result = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x38; 32],
            &initiated.outboxes[0],
            candidate,
            history,
            &event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await;

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(_))
    ));
    assert!(sponsor_repository.load(attempt_id).await.unwrap().is_none());
    assert!(sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn activation_receipt_bound_to_another_attempt_leaves_joiner_state_unchanged() {
    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x39; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x3a; 16]);
    let sponsor = durable_admission_owner(sponsor_repository);
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x3b; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x3c; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x3d; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &offered,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"verified-complete-history",
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let commit = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let before_attempt = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    let before_history = joiner_repository
        .load_membership_history_v2()
        .await
        .unwrap();
    let other_attempt = uc_core::membership::AdmissionAttemptId::from_bytes([0x3e; 32]);
    let wrong_receipt = durable_candidate_verification_fixture(other_attempt).4;

    let result = joiner
        .joiner_apply(attempt_id, &commit, &wrong_receipt, b"sponsor", b"applied")
        .await;

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(_))
    ));
    assert_eq!(
        joiner_repository.load(attempt_id).await.unwrap(),
        Some(before_attempt)
    );
    assert_eq!(
        joiner_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        before_history
    );
}

// Flow: the joiner verifies and saves the sponsor candidate before commit;
// both sides then persist the same activation result before completion.
#[tokio::test]
async fn durable_admission_becomes_complete_only_after_both_sides_save() {
    use uc_core::membership::AdmissionTerminalResultV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x41; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x42; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x43; 32]);
    let join_id = [0x44; 16];
    let (candidate, base_history, candidate_event, commitment, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);

    let initiated = joiner
        .start_join(
            attempt_id,
            join_id,
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let candidate_message = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x47; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let replayed_candidate = durable_admission_owner(Arc::clone(&sponsor_repository))
        .sponsor_accept_and_offer(
            attempt_id,
            [0x47; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    assert_eq!(replayed_candidate, candidate_message);
    let sponsor_candidate_state = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(sponsor_candidate_state.outboxes.iter().any(|message| {
        message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::InvitationConsume
            && !message.superseded
    }));
    sponsor
        .record_invitation_consume_result(
            attempt_id,
            super::admission_transaction::InvitationConsumeResultV1::Retryable,
        )
        .await
        .unwrap();
    assert!(sponsor_repository
        .load(attempt_id)
        .await
        .unwrap()
        .unwrap()
        .outboxes
        .iter()
        .any(|message| {
            message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::InvitationConsume
                && !message.superseded
        }));
    sponsor
        .record_invitation_consume_result(
            attempt_id,
            super::admission_transaction::InvitationConsumeResultV1::Consumed,
        )
        .await
        .unwrap();
    let prepared_message = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"verified-complete-history",
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let replayed_prepared = durable_admission_owner(Arc::clone(&joiner_repository))
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"verified-complete-history",
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    assert_eq!(replayed_prepared, prepared_message);
    let commit_message = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared_message,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let replayed_commit = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared_message,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    assert_eq!(replayed_commit, commit_message);
    let sponsor_committed_history = sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let sponsor_committed_history =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &sponsor_committed_history,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(sponsor_committed_history.effective_members().len(), 2);
    assert_eq!(sponsor_committed_history.active_members().len(), 1);
    let applied_message = joiner
        .joiner_apply(
            attempt_id,
            &commit_message,
            &activation_receipt,
            b"sponsor",
            b"applied",
        )
        .await
        .unwrap();
    let replayed_applied = joiner
        .joiner_apply(
            attempt_id,
            &commit_message,
            &activation_receipt,
            b"sponsor",
            b"applied",
        )
        .await
        .unwrap();
    assert_eq!(replayed_applied, applied_message);
    let joiner_applied_history = joiner_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let joiner_applied_history =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &joiner_applied_history,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(joiner_applied_history.active_members().len(), 2);
    let complete_message = sponsor
        .sponsor_complete(
            attempt_id,
            &applied_message,
            &activation_receipt,
            b"admission-completion",
            b"joiner",
            b"complete",
        )
        .await
        .unwrap();
    let security_update = sponsor
        .enqueue_post_commit_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate,
            b"existing-member",
            b"event-epoch-and-security-commitment",
        )
        .await
        .unwrap();
    let other_security_update = sponsor
        .enqueue_post_commit_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate,
            b"other-existing-member",
            b"event-epoch-and-security-commitment",
        )
        .await
        .unwrap();
    assert_ne!(security_update.message_id, other_security_update.message_id);
    assert_ne!(security_update.recipient, other_security_update.recipient);
    let history_batch = sponsor
        .enqueue_post_commit_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::HistoryOrReceiptBatch,
            b"existing-member",
            b"history-page-and-receipt-ids",
        )
        .await
        .unwrap();
    let security_ack = super::admission_transaction::admission_acknowledgment(&security_update);
    assert!(sponsor
        .acknowledge_delivery(attempt_id, &security_ack)
        .await
        .is_err());
    sponsor
        .acknowledge_persisted_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate,
            &security_ack,
        )
        .await
        .unwrap();
    let after_exact_security_ack = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(after_exact_security_ack
        .outboxes
        .iter()
        .any(|message| { message.message_id == security_update.message_id && message.superseded }));
    assert!(after_exact_security_ack.outboxes.iter().any(|message| {
        message.message_id == other_security_update.message_id && !message.superseded
    }));
    assert!(after_exact_security_ack.outboxes.iter().any(|message| {
        message.message_id == complete_message.message_id && !message.superseded
    }));
    sponsor
        .acknowledge_persisted_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::HistoryOrReceiptBatch,
            &super::admission_transaction::admission_acknowledgment(&history_batch),
        )
        .await
        .unwrap();
    let replayed_complete = sponsor
        .sponsor_complete(
            attempt_id,
            &applied_message,
            &activation_receipt,
            b"admission-completion",
            b"joiner",
            b"complete",
        )
        .await
        .unwrap();
    assert_eq!(replayed_complete, complete_message);
    let sponsor_applied_history = sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let sponsor_applied_history =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &sponsor_applied_history,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(sponsor_applied_history.active_members().len(), 2);

    let sponsor_after_complete = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let ordinary_removal = sponsor
        .sponsor_remove_pending_member(
            attempt_id,
            &durable_candidate_removal_fixture(attempt_id),
            b"joiner",
            b"removed-before-activation",
        )
        .await
        .unwrap();
    assert!(matches!(
        ordinary_removal,
        super::admission_transaction::PendingMemberRemovalOutcomeV1::OrdinaryMemberRemovalRequired
    ));
    assert_eq!(
        sponsor_repository.load(attempt_id).await.unwrap(),
        Some(sponsor_after_complete)
    );

    let sponsor_before_ack = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(sponsor_before_ack.terminal_result, None);
    let complete_ack = joiner
        .joiner_activate(attempt_id, &complete_message, b"admission-completion")
        .await
        .unwrap();
    let replayed_ack = joiner
        .joiner_activate(attempt_id, &complete_message, b"admission-completion")
        .await
        .unwrap();
    assert_eq!(replayed_ack, complete_ack);
    let joiner_saved = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    let verified_history: uc_core::membership::VersionedMembershipHistory =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            joiner_saved.verified_membership_history.as_deref().unwrap(),
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(verified_history.effective_members().len(), 2);
    assert_eq!(
        joiner_saved.terminal_result,
        Some(AdmissionTerminalResultV1::Active)
    );

    sponsor
        .sponsor_confirm_active(attempt_id, &complete_ack)
        .await
        .unwrap();
    sponsor
        .sponsor_confirm_active(attempt_id, &complete_ack)
        .await
        .unwrap();
    let sponsor_saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        sponsor_saved.terminal_result,
        Some(AdmissionTerminalResultV1::Completed)
    );
    assert_eq!(
        sponsor_saved.activation_receipt,
        joiner_saved.activation_receipt
    );
    assert_eq!(sponsor_saved.completion, joiner_saved.completion);
    assert!(sponsor_saved.outboxes.iter().any(|message| {
        message.message_id == other_security_update.message_id && !message.superseded
    }));
    assert!(sponsor.compact_if_settled(attempt_id).await.is_err());
    sponsor
        .acknowledge_persisted_delivery(
            attempt_id,
            uc_core::membership::AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate,
            &super::admission_transaction::admission_acknowledgment(&other_security_update),
        )
        .await
        .unwrap();

    let sponsor_terminal = sponsor.compact_if_settled(attempt_id).await.unwrap();
    let joiner_terminal = joiner.compact_if_settled(attempt_id).await.unwrap();
    assert_eq!(
        sponsor_terminal.terminal_result,
        AdmissionTerminalResultV1::Completed
    );
    assert_eq!(
        joiner_terminal.terminal_result,
        AdmissionTerminalResultV1::Active
    );
    assert!(sponsor_repository.load(attempt_id).await.unwrap().is_none());
    assert!(joiner_repository.load(attempt_id).await.unwrap().is_none());
    assert_eq!(
        sponsor_repository
            .load_terminal(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        AdmissionTerminalResultV1::Completed
    );
}

// Flow: cancellation and formal commit have one persisted winner. A saved
// cancellation before commit rejects without a formal add; a saved commit
// makes cancellation too late and the same attempt continues forward.
#[tokio::test]
async fn durable_admission_cancel_and_commit_have_exactly_one_winner() {
    use uc_core::membership::AdmissionOutboxPurposeV1;
    async fn prepared_pair(
        sponsor: &super::admission_transaction::DurableAdmissionTransaction,
        joiner: &super::admission_transaction::DurableAdmissionTransaction,
        attempt_id: uc_core::membership::AdmissionAttemptId,
        join_id: [u8; 16],
    ) -> uc_core::membership::AdmissionOutboxMessageV1 {
        let initiated = joiner
            .start_join(
                attempt_id,
                join_id,
                b"sponsor",
                b"join-request",
                b"joiner-pending-state",
                b"joiner-key-package",
            )
            .await
            .unwrap();
        let (candidate, base_history, candidate_event, commitment, _activation_receipt) =
            durable_candidate_verification_fixture(attempt_id);
        let offered = sponsor
            .sponsor_accept_and_offer(
                attempt_id,
                [0x53; 32],
                &initiated.outboxes[0],
                candidate.clone(),
                base_history.clone(),
                &candidate_event,
                &commitment,
                b"joiner",
                b"candidate",
            )
            .await
            .unwrap();
        sponsor
            .record_invitation_consume_result(
                attempt_id,
                super::admission_transaction::InvitationConsumeResultV1::NotFound,
            )
            .await
            .unwrap();
        joiner
            .joiner_verify_and_prepare(
                attempt_id,
                &offered,
                candidate,
                base_history,
                &candidate_event,
                &commitment,
                b"verified-complete-history",
                b"sponsor",
                b"prepared",
            )
            .await
            .unwrap()
    }

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x54; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x55; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x56; 32]);
    let prepared = prepared_pair(&sponsor, &joiner, attempt_id, [0x57; 16]).await;
    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    assert_eq!(rejected.purpose, AdmissionOutboxPurposeV1::Rejected);
    let sponsor_rejected = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(sponsor_rejected.candidate_event.is_some());
    assert!(sponsor_rejected.activation_receipt.is_none());
    assert!(sponsor_rejected.terminal_result.is_some());
    assert!(sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit"
        )
        .await
        .is_err());
    let rejected_ack = joiner
        .joiner_record_rejected(attempt_id, &rejected)
        .await
        .unwrap();
    let replayed_rejected_ack = joiner
        .joiner_record_rejected(attempt_id, &rejected)
        .await
        .unwrap();
    assert_eq!(replayed_rejected_ack, rejected_ack);
    sponsor
        .sponsor_confirm_rejected(attempt_id, &rejected_ack)
        .await
        .unwrap();
    let joiner_rejected = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        joiner_rejected.terminal_result,
        Some(uc_core::membership::AdmissionTerminalResultV1::Rejected)
    );
    assert_eq!(
        joiner_rejected.rejection_reason,
        Some(uc_core::membership::AdmissionRejectionReasonV1::Cancelled)
    );
    assert!(joiner_rejected
        .outboxes
        .iter()
        .all(|message| message.superseded));
    let sponsor_rejected = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(sponsor_rejected
        .outboxes
        .iter()
        .all(|message| message.superseded));
    joiner.compact_if_settled(attempt_id).await.unwrap();
    sponsor.compact_if_settled(attempt_id).await.unwrap();

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x58; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x59; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x5a; 32]);
    let prepared = prepared_pair(&sponsor, &joiner, attempt_id, [0x5b; 16]).await;
    let committed = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let still_committed = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    assert_eq!(still_committed, committed);
    let activation_receipt = durable_candidate_verification_fixture(attempt_id).4;
    let applied = joiner
        .joiner_apply(
            attempt_id,
            &committed,
            &activation_receipt,
            b"sponsor",
            b"applied",
        )
        .await
        .unwrap();
    let joiner_state = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        joiner_state.cancel_outcome,
        Some(b"too_late_committed".to_vec())
    );
    assert_eq!(applied.purpose, AdmissionOutboxPurposeV1::Applied);
}

#[tokio::test]
async fn base_history_change_after_candidate_is_durably_rejected_without_add() {
    use uc_core::membership::{AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x60; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x61; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x62; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x63; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let candidate_message = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x64; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared_message = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"verified-complete-history",
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();

    let mut concurrent = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let expected_version = concurrent.record_version;
    concurrent.record_version += 1;
    let current_history = sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    sponsor_repository
        .compare_and_advance_with_membership_history_v2(
            attempt_id,
            expected_version,
            &concurrent,
            Some(&current_history),
            b"newer-formal-history",
        )
        .await
        .unwrap();

    let rejected = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared_message,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();

    assert_eq!(rejected.purpose, AdmissionOutboxPurposeV1::Rejected);
    let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        saved.rejection_reason,
        Some(AdmissionRejectionReasonV1::BaseHistoryChanged)
    );
    assert_eq!(
        sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(b"newer-formal-history".to_vec())
    );
}

#[tokio::test]
async fn base_history_change_during_commit_is_durably_rejected() {
    use uc_core::membership::{AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x5c; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x5d; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x5e; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x5f; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x60; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &offered,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"verified-complete-history",
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let racing_repository: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort> =
        Arc::new(HistoryRaceAdmissionRepository {
            inner: Arc::clone(&sponsor_repository),
            inject_once: AtomicBool::new(true),
            replacement_history: b"concurrent-formal-history".to_vec(),
        });
    let racing_sponsor = durable_admission_owner(racing_repository);

    let result = racing_sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();

    assert_eq!(result.purpose, AdmissionOutboxPurposeV1::Rejected);
    let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        saved.rejection_reason,
        Some(AdmissionRejectionReasonV1::BaseHistoryChanged)
    );
    assert_eq!(
        sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(b"concurrent-formal-history".to_vec())
    );
}

#[tokio::test]
async fn pending_member_removal_before_commit_rejects_without_add() {
    use super::admission_transaction::PendingMemberRemovalOutcomeV1;
    use uc_core::membership::{AdmissionRejectionReasonV1, VersionedMembershipHistory};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x65; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x66; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x67; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x68; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x69; 32],
            &initiated.outboxes[0],
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let removal = durable_candidate_removal_fixture(attempt_id);

    let outcome = sponsor
        .sponsor_remove_pending_member(
            attempt_id,
            &removal,
            b"joiner",
            b"removed-before-activation",
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        PendingMemberRemovalOutcomeV1::AdmissionRejected(_)
    ));
    let attempt = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        attempt.rejection_reason,
        Some(AdmissionRejectionReasonV1::RemovedBeforeActivation)
    );
    let history = VersionedMembershipHistory::decode_persisted_v2(
        &sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap()
            .unwrap(),
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(history.effective_members().len(), 1);
}

#[tokio::test]
async fn pending_inbound_projection_shows_only_the_active_lineage_non_terminal_candidate() {
    use uc_core::membership::AdmissionRejectionReasonV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x74; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x75; 16]);
    let sponsor = durable_admission_owner(sponsor_repository);
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x76; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x77; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x78; 32],
            &initiated.outboxes[0],
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();

    assert_eq!(
        sponsor.pending_inbound_member("space-b").await.unwrap(),
        None
    );
    let projected = sponsor
        .pending_inbound_member("space-a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(projected.device_id, DeviceId::new("joiner"));
    assert_eq!(projected.display_name, "joiner");

    sponsor
        .sponsor_reject_before_commit(
            attempt_id,
            AdmissionRejectionReasonV1::IdentityConflict,
            b"joiner",
        )
        .await
        .unwrap();
    assert_eq!(
        sponsor.pending_inbound_member("space-a").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn sponsor_business_rejection_before_commit_is_durable_and_replayable() {
    use uc_core::membership::{
        AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1, VersionedMembershipHistory,
    };

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x6f; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x70; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x71; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x72; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x73; 32],
            &initiated.outboxes[0],
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();

    let rejected = sponsor
        .sponsor_reject_before_commit(
            attempt_id,
            AdmissionRejectionReasonV1::IdentityConflict,
            b"joiner",
        )
        .await
        .unwrap();
    let replayed = sponsor
        .sponsor_reject_before_commit(
            attempt_id,
            AdmissionRejectionReasonV1::IdentityConflict,
            b"joiner",
        )
        .await
        .unwrap();

    assert_eq!(rejected, replayed);
    assert_eq!(rejected.purpose, AdmissionOutboxPurposeV1::Rejected);
    let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        saved.rejection_reason,
        Some(AdmissionRejectionReasonV1::IdentityConflict)
    );
    let history = VersionedMembershipHistory::decode_persisted_v2(
        &sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap()
            .unwrap(),
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(history.effective_members().len(), 1);
}

#[tokio::test]
async fn pending_member_removal_after_commit_permanently_keeps_add_then_remove() {
    use super::admission_transaction::PendingMemberRemovalOutcomeV1;
    use uc_core::membership::{AdmissionRejectionReasonV1, VersionedMembershipHistory};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x6a; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x6b; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(joiner_repository);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x6c; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0x6d; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let candidate_message = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x6e; 32],
            &initiated.outboxes[0],
            candidate.clone(),
            base_history.clone(),
            &candidate_event,
            &commitment,
            b"joiner",
            b"candidate",
        )
        .await
        .unwrap();
    let prepared = joiner
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"verified-complete-history",
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let removal = durable_candidate_removal_fixture(attempt_id);

    let outcome = sponsor
        .sponsor_remove_pending_member(
            attempt_id,
            &removal,
            b"joiner",
            b"removed-before-activation",
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        PendingMemberRemovalOutcomeV1::AdmissionRejected(_)
    ));
    let attempt = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        attempt.rejection_reason,
        Some(AdmissionRejectionReasonV1::RemovedBeforeActivation)
    );
    let history = VersionedMembershipHistory::decode_persisted_v2(
        &sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap()
            .unwrap(),
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(history.effective_members().len(), 1);
    assert_eq!(history.active_members().len(), 1);
    assert_eq!(history.depth(removal.event_id()), Some(9));
}

#[tokio::test]
async fn pending_member_removal_races_commit_and_activation_without_partial_state() {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, SponsorAdmissionStageV1, SponsorAdmissionStateV1,
        VersionedMembershipHistory,
    };

    for iteration in 0..8u8 {
        let sponsor_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        let sponsor_repository = durable_admission_repository(&sponsor_dir, [0xa0 | iteration; 16]);
        let joiner_repository = durable_admission_repository(&joiner_dir, [0xb0 | iteration; 16]);
        let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
        let joiner = durable_admission_owner(joiner_repository);
        let attempt_id =
            uc_core::membership::AdmissionAttemptId::from_bytes([0xc0 | iteration; 32]);
        let initiated = joiner
            .start_join(
                attempt_id,
                [0xd0 | iteration; 16],
                b"sponsor",
                b"join-request",
                b"joiner-pending-state",
                b"joiner-key-package",
            )
            .await
            .unwrap();
        let (candidate, base_history, candidate_event, commitment, _) =
            durable_candidate_verification_fixture(attempt_id);
        let offered = sponsor
            .sponsor_accept_and_offer(
                attempt_id,
                [0xe0 | iteration; 32],
                &initiated.outboxes[0],
                candidate.clone(),
                base_history.clone(),
                &candidate_event,
                &commitment,
                b"joiner",
                b"candidate",
            )
            .await
            .unwrap();
        let prepared = joiner
            .joiner_verify_and_prepare(
                attempt_id,
                &offered,
                candidate,
                base_history,
                &candidate_event,
                &commitment,
                b"verified-complete-history",
                b"sponsor",
                b"prepared",
            )
            .await
            .unwrap();
        let removal = durable_candidate_removal_fixture(attempt_id);

        let _ = tokio::join!(
            sponsor.sponsor_commit(
                attempt_id,
                &prepared,
                b"verified-complete-history",
                b"joiner",
                b"commit",
            ),
            sponsor.sponsor_remove_pending_member(
                attempt_id,
                &removal,
                b"joiner",
                b"removed-before-activation",
            )
        );

        let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &sponsor_repository
                .load_membership_history_v2()
                .await
                .unwrap()
                .unwrap(),
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
        match saved.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected,
            }) => {
                assert_eq!(history.effective_members().len(), 1);
                assert_eq!(history.active_members().len(), 1);
            }
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Committed,
            }) => {
                assert_eq!(history.effective_members().len(), 2);
                assert_eq!(history.active_members().len(), 1);
            }
            other => panic!("unexpected commit/removal race result: {other:?}"),
        }
    }

    for iteration in 0..8u8 {
        let sponsor_dir = tempfile::tempdir().unwrap();
        let joiner_dir = tempfile::tempdir().unwrap();
        let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x10 | iteration; 16]);
        let joiner_repository = durable_admission_repository(&joiner_dir, [0x20 | iteration; 16]);
        let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
        let joiner = durable_admission_owner(joiner_repository);
        let attempt_id =
            uc_core::membership::AdmissionAttemptId::from_bytes([0x30 | iteration; 32]);
        let initiated = joiner
            .start_join(
                attempt_id,
                [0x40 | iteration; 16],
                b"sponsor",
                b"join-request",
                b"joiner-pending-state",
                b"joiner-key-package",
            )
            .await
            .unwrap();
        let (candidate, base_history, candidate_event, commitment, receipt) =
            durable_candidate_verification_fixture(attempt_id);
        let offered = sponsor
            .sponsor_accept_and_offer(
                attempt_id,
                [0x50 | iteration; 32],
                &initiated.outboxes[0],
                candidate.clone(),
                base_history.clone(),
                &candidate_event,
                &commitment,
                b"joiner",
                b"candidate",
            )
            .await
            .unwrap();
        let prepared = joiner
            .joiner_verify_and_prepare(
                attempt_id,
                &offered,
                candidate,
                base_history,
                &candidate_event,
                &commitment,
                b"verified-complete-history",
                b"sponsor",
                b"prepared",
            )
            .await
            .unwrap();
        let commit = sponsor
            .sponsor_commit(
                attempt_id,
                &prepared,
                b"verified-complete-history",
                b"joiner",
                b"commit",
            )
            .await
            .unwrap();
        let applied = joiner
            .joiner_apply(attempt_id, &commit, &receipt, b"sponsor", b"applied")
            .await
            .unwrap();
        let removal = durable_candidate_removal_fixture(attempt_id);

        let _ = tokio::join!(
            sponsor.sponsor_complete(
                attempt_id,
                &applied,
                &receipt,
                b"admission-completion",
                b"joiner",
                b"complete",
            ),
            sponsor.sponsor_remove_pending_member(
                attempt_id,
                &removal,
                b"joiner",
                b"removed-before-activation",
            )
        );

        let saved = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &sponsor_repository
                .load_membership_history_v2()
                .await
                .unwrap()
                .unwrap(),
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
        match saved.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected,
            }) => {
                assert_eq!(history.effective_members().len(), 1);
                assert_eq!(history.active_members().len(), 1);
            }
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Applied,
            }) => {
                assert_eq!(history.effective_members().len(), 2);
                assert_eq!(history.active_members().len(), 2);
            }
            other => panic!("unexpected activation/removal race result: {other:?}"),
        }
    }
}

struct DeterministicHistoricalVerifier;

#[derive(Default)]
struct EchoAdmissionSecurityTransition {
    stage_joiner_calls: AtomicUsize,
}

impl uc_core::membership::AdmissionSecurityTransitionPort for EchoAdmissionSecurityTransition {
    fn prepare_sponsor(
        &self,
        _sponsor_state: &[u8],
        _candidate_identity: &[u8],
        _key_package: &[u8],
        _input: &uc_core::membership::AdmissionSecurityTransitionInput,
    ) -> Result<
        uc_core::membership::SponsorPreparedSecurityTransition,
        uc_core::membership::AdmissionSecurityTransitionError,
    > {
        unreachable!("sponsor preparation is not used by this test")
    }

    fn stage_joiner(
        &self,
        _pending_state: &[u8],
        _key_package: &[u8],
        _expected_space_id: &[u8],
        welcome: &[u8],
        _commit: &[u8],
        _input: &uc_core::membership::AdmissionSecurityTransitionInput,
    ) -> Result<
        uc_core::membership::JoinerStagedSecurityTransition,
        uc_core::membership::AdmissionSecurityTransitionError,
    > {
        self.stage_joiner_calls.fetch_add(1, Ordering::SeqCst);
        Ok(uc_core::membership::JoinerStagedSecurityTransition {
            staged_state: b"joiner-staged-state".to_vec(),
            public_commitment: postcard::from_bytes(welcome).unwrap(),
        })
    }

    fn derive_public_commitment(
        &self,
        _staged_state: &[u8],
        _commit: &[u8],
        _input: &uc_core::membership::AdmissionSecurityTransitionInput,
    ) -> Result<
        uc_core::membership::AdmissionSecurityCommitmentV1,
        uc_core::membership::AdmissionSecurityTransitionError,
    > {
        unreachable!("direct derivation is not used by this test")
    }

    fn activate(
        &self,
        staged_state: Vec<u8>,
        _commit: &[u8],
        _expected: &uc_core::membership::AdmissionSecurityCommitmentV1,
        _input: &uc_core::membership::AdmissionSecurityTransitionInput,
    ) -> Result<Vec<u8>, uc_core::membership::AdmissionSecurityTransitionError> {
        Ok(staged_state)
    }

    fn discard(&self, _staged_state: Vec<u8>) {}
}

impl DeterministicHistoricalVerifier {
    fn sign(
        &self,
        credential: &uc_core::membership::MembershipCredential,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut hasher = sha2::Sha256::new();
        use sha2::Digest;
        hasher.update(b"admission-transaction-history-test\0");
        hasher.update(&credential.public_key);
        hasher.update(payload);
        hasher.finalize().to_vec()
    }
}

impl uc_core::membership::HistoricalMembershipSignatureVerifier
    for DeterministicHistoricalVerifier
{
    fn verify(
        &self,
        signature_algorithm_version: u16,
        public_key: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, uc_core::membership::HistoricalMembershipSignatureError> {
        use uc_core::membership::{MembershipCredential, ED25519_SIGNATURE_ALGORITHM_V1};
        if signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(
                uc_core::membership::HistoricalMembershipSignatureError::UnsupportedAlgorithm,
            );
        }
        let credential =
            MembershipCredential::new(signature_algorithm_version, public_key.to_vec());
        Ok(self.sign(&credential, payload) == signature)
    }
}

fn admission_verification_fixture(
    attempt_id: [u8; 32],
) -> (
    uc_core::membership::VersionedMembershipHistory,
    uc_core::membership::MembershipEventV2,
    uc_core::membership::AdmissionSecurityCommitmentV1,
) {
    use sha2::Digest;
    use uc_core::membership::{
        AdmissionChangeFacts, AdmissionSecurityCommitmentV1, BaseMembershipHistoryPositionV1,
        MembershipActivationBaselineV2, MembershipAdmissionV2, MembershipCredential,
        MembershipEventId, MembershipEventV2, MembershipOperationV2, VersionedMembershipHistory,
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
        MEMBERSHIP_EVENT_FORMAT_V2,
    };

    let verifier = DeterministicHistoricalVerifier;
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x81; 32]);
    let sponsor_device = DeviceId::new("sponsor");
    let sponsor_member = sponsor_credential.member_instance_id(&sponsor_device);
    let base_head = MembershipEventId::from_hex(&"82".repeat(32)).unwrap();
    let history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::FullyVerifiedMigration {
            lineage_id: "space-a".to_owned(),
            head_event_id: base_head,
            head_depth: 7,
            current_member_credentials: vec![(sponsor_member, sponsor_credential.clone())],
        },
    )
    .unwrap();
    let base_position = BaseMembershipHistoryPositionV1 {
        event_id: Some(base_head),
        depth: 7,
        history_digest: [0x83; 32],
    };
    let commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        "space-a".to_owned(),
        b"space-a".to_vec(),
        attempt_id,
        base_position.clone(),
        [0x85; 32],
        1,
        3,
        4,
        [0x86; 32],
        [0x87; 32],
        [0x88; 32],
        [0x89; 32],
        [0x8a; 32],
    )
    .unwrap();
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x8b; 32]);
    let joiner_device = DeviceId::new("joiner");
    let joiner_member = joiner_credential.member_instance_id(&joiner_device);
    let operation = MembershipOperationV2::AddDevice {
        admission: MembershipAdmissionV2 {
            facts: AdmissionChangeFacts {
                member_instance: joiner_member,
                device_id: joiner_device,
                device_name: "joiner".to_owned(),
                identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .unwrap(),
                transport_public_key: vec![1],
                transport_address_blob: vec![2],
                identity_signature: vec![3],
            },
            membership_credential: joiner_credential.clone(),
            resume_public_key_digest: [0x8c; 32],
            security_commitment_id: commitment.security_commitment_id,
        },
    };
    let resulting_members_digest = history
        .expected_resulting_members_digest(Some(base_head), &operation)
        .unwrap();
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        "space-a".to_owned(),
        Some(base_head),
        8,
        [0x8d; 16],
        sponsor_member,
        sponsor_credential.credential_id,
        sponsor_credential.signature_algorithm_version,
        operation,
        resulting_members_digest,
        [0x8e; 32],
        vec![0x8f],
        Some([0x8a; 32]),
        Vec::new(),
    );
    event.signature = verifier.sign(&sponsor_credential, &event.signing_payload());
    let _: [u8; 32] = sha2::Sha256::digest(&event.signature).into();
    (history, event, commitment)
}

fn durable_candidate_verification_fixture(
    attempt_id: uc_core::membership::AdmissionAttemptId,
) -> (
    super::admission_transaction::DurableAdmissionCandidateV1,
    uc_core::membership::VersionedMembershipHistory,
    uc_core::membership::MembershipEventV2,
    uc_core::membership::AdmissionSecurityCommitmentV1,
    uc_core::membership::AdmissionActivationReceipt,
) {
    use uc_core::membership::{AdmissionActivationReceipt, MembershipOperationV2};
    let (history, event, commitment) = admission_verification_fixture(*attempt_id.as_bytes());
    let MembershipOperationV2::AddDevice { admission } = &event.operation else {
        unreachable!("fixture always creates AddDevice")
    };
    let mut receipt = AdmissionActivationReceipt::new(
        1,
        *attempt_id.as_bytes(),
        event.event_id(),
        event.resulting_members_digest,
        commitment.security_commitment_id,
        admission.facts.member_instance,
        Vec::new(),
    );
    receipt.signature = DeterministicHistoricalVerifier
        .sign(&admission.membership_credential, &receipt.signing_payload());
    let candidate = super::admission_transaction::DurableAdmissionCandidateV1 {
        lineage_id: event.lineage_id.clone(),
        base_history_position: postcard::to_stdvec(&commitment.base_history_position).unwrap(),
        candidate_event: postcard::to_stdvec(&event).unwrap(),
        candidate_event_id: *event.event_id().as_bytes(),
        candidate_key_package: b"joiner-key-package".to_vec(),
        target_members_digest: event.resulting_members_digest,
        security_commitment: postcard::to_stdvec(&commitment).unwrap(),
        security_commit: b"sealed-security-commit".to_vec(),
        security_welcome: postcard::to_stdvec(&commitment).unwrap(),
        staged_security_state: b"sponsor-staged-state".to_vec(),
        identity_binding: b"sponsor-joiner-binding".to_vec(),
    };
    (candidate, history, event, commitment, receipt)
}

fn durable_candidate_removal_fixture(
    attempt_id: uc_core::membership::AdmissionAttemptId,
) -> uc_core::membership::MembershipEventV2 {
    use uc_core::membership::{
        MembershipCredential, MembershipEventV2, MembershipOperationV2,
        ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
    };

    let (_, mut history, candidate, _, _) = durable_candidate_verification_fixture(attempt_id);
    history
        .verify_and_receive_event(candidate.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x81; 32]);
    let sponsor_member = sponsor_credential.member_instance_id(&DeviceId::new("sponsor"));
    let uc_core::membership::MembershipOperationV2::AddDevice { admission } = &candidate.operation
    else {
        unreachable!("fixture always creates AddDevice")
    };
    let operation = MembershipOperationV2::RemoveDevice {
        member: admission.facts.member_instance,
    };
    let resulting_members_digest = history
        .expected_resulting_members_digest(Some(candidate.event_id()), &operation)
        .unwrap();
    let mut removal = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        candidate.lineage_id.clone(),
        Some(candidate.event_id()),
        9,
        [0x90; 16],
        sponsor_member,
        sponsor_credential.credential_id,
        sponsor_credential.signature_algorithm_version,
        operation,
        resulting_members_digest,
        [0x91; 32],
        vec![0x92],
        None,
        Vec::new(),
    );
    removal.signature =
        DeterministicHistoricalVerifier.sign(&sponsor_credential, &removal.signing_payload());
    removal
}

#[test]
fn durable_admission_preparation_rejects_unverified_history() {
    let (history, mut candidate_event, commitment) = admission_verification_fixture([0x84; 32]);
    candidate_event.signature[0] ^= 0xff;

    let result = super::admission_transaction::verify_candidate_preparation(
        history,
        &candidate_event,
        &commitment,
        &commitment,
        &DeterministicHistoricalVerifier,
    );

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(_))
    ));
}

#[test]
fn durable_admission_preparation_rejects_security_result_mismatch() {
    let (history, candidate_event, commitment) = admission_verification_fixture([0x84; 32]);
    let mut different = commitment.clone();
    different.security_commitment_id[0] ^= 0xff;

    let result = super::admission_transaction::verify_candidate_preparation(
        history,
        &candidate_event,
        &commitment,
        &different,
        &DeterministicHistoricalVerifier,
    );

    assert!(matches!(
        result,
        Err(WorkspaceConvergenceError::Inconsistent(_))
    ));
}
