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
    MembershipAdmissionDecision, MembershipAdmissionGatePort, MembershipHistoryMessage,
    MembershipHistoryV2Ack, MembershipOperation, MembershipReconciliation,
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
struct UnusedAdmissionCompletionRecovery;

struct ConfirmingAdmissionDelivery;

struct LoopbackHistoryExchange {
    receiver: Arc<WorkspaceConvergence>,
    source_device_id: DeviceId,
    sent_pages: AtomicUsize,
}

#[async_trait]
impl uc_core::membership::AdmissionCompletionRecoveryPort for UnusedAdmissionCompletionRecovery {
    async fn request_completion_recovery_challenge(
        &self,
        _helper: &DeviceId,
        _route: &[u8],
        _hello: uc_core::membership::AdmissionCompletionRecoveryHelloV1,
        _joiner_last_message_id: [u8; 32],
    ) -> Result<
        uc_core::membership::AdmissionCompletionRecoveryChallengeV1,
        uc_core::membership::AdmissionCompletionRecoveryTransportError,
    > {
        Err(uc_core::membership::AdmissionCompletionRecoveryTransportError::Offline)
    }

    async fn submit_completion_recovery_response(
        &self,
        _helper: &DeviceId,
        _route: &[u8],
        _hello: uc_core::membership::AdmissionCompletionRecoveryHelloV1,
        _response: uc_core::membership::AdmissionCompletionRecoveryResponseV1,
    ) -> Result<
        uc_core::pairing::DurableAdmissionFrame,
        uc_core::membership::AdmissionCompletionRecoveryTransportError,
    > {
        Err(uc_core::membership::AdmissionCompletionRecoveryTransportError::Offline)
    }
}

#[async_trait]
impl uc_core::membership::MembershipHistoryExchangePort for LoopbackHistoryExchange {
    async fn exchange_membership_history(
        &self,
        _recipient: &DeviceId,
        message: MembershipHistoryMessage,
    ) -> Result<MembershipHistoryMessage, uc_core::membership::MembershipHistoryExchangeError> {
        self.sent_pages.fetch_add(1, Ordering::SeqCst);
        self.receiver
            .handle_membership_history(&self.source_device_id, message)
            .await
            .map_err(|_| uc_core::membership::MembershipHistoryExchangeError::Rejected)
    }
}

#[derive(Default)]
struct RecordingLegacyMigrationRecovery {
    calls: AtomicUsize,
}

#[async_trait]
impl uc_core::ports::setup::LegacyMigrationRecoveryPort for RecordingLegacyMigrationRecovery {
    async fn recover(&self) -> Result<(), uc_core::ports::setup::LegacyMigrationRecoveryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

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
        Ok(MembershipHistoryMessage::AckV2(
            MembershipHistoryV2Ack::Consistent,
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
        Ok(MembershipHistoryMessage::AckV2(
            MembershipHistoryV2Ack::Consistent,
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

struct LockedWorkspaceRepository;

#[async_trait]
impl WorkspaceConvergenceRepositoryPort for LockedWorkspaceRepository {
    async fn save_state(
        &self,
        _state: &WorkspaceConvergenceState,
    ) -> Result<(), WorkspaceConvergenceRepositoryError> {
        Err(WorkspaceConvergenceRepositoryError::Locked)
    }

    async fn load_state(
        &self,
    ) -> Result<Option<WorkspaceConvergenceState>, WorkspaceConvergenceRepositoryError> {
        Err(WorkspaceConvergenceRepositoryError::Locked)
    }
}

struct LockedAdmissionRepository {
    allow_empty_history_reads: bool,
}

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

    async fn compare_and_replace_membership_history_v2(
        &self,
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
        if self.allow_empty_history_reads {
            Ok(None)
        } else {
            Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
        }
    }

    async fn scan_recoverable(
        &self,
    ) -> Result<
        Vec<uc_core::membership::AdmissionAttemptV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        if self.allow_empty_history_reads {
            Ok(Vec::new())
        } else {
            Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
        }
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
        if self.allow_empty_history_reads {
            Ok(uc_core::membership::AdmissionProfileMetadataV1::fresh(
                [0; 16],
            ))
        } else {
            Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
        }
    }

    async fn project_current_local_join(
        &self,
    ) -> Result<
        Option<uc_core::membership::CurrentLocalJoinProjectionV1>,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        if self.allow_empty_history_reads {
            Ok(None)
        } else {
            Err(uc_core::membership::AdmissionAttemptRepositoryError::Locked)
        }
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
    durable_admission_owner_with_space_transition(repository, Arc::new(NoAdmissionSpaceTransition))
}

fn durable_admission_owner_with_space_transition(
    repository: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    space_transition: Arc<dyn uc_core::membership::AdmissionSpaceTransitionPort>,
) -> super::admission_transaction::DurableAdmissionTransaction {
    super::admission_transaction::DurableAdmissionTransaction::new(
        repository,
        Arc::new(DeterministicHistoricalVerifier),
        Arc::new(EchoAdmissionSecurityTransition::default()),
        space_transition,
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

    async fn compare_and_replace_membership_history_v2(
        &self,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<
        uc_core::membership::AdmissionProfileMetadataV1,
        uc_core::membership::AdmissionAttemptRepositoryError,
    > {
        self.inner
            .compare_and_replace_membership_history_v2(
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

#[derive(Clone)]
struct CredentialBackedSigner {
    device_id: DeviceId,
    credential: uc_core::membership::MembershipCredential,
}

#[async_trait]
impl CurrentMemberSignaturePort for CredentialBackedSigner {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        Ok(1)
    }

    async fn current_membership_credential(
        &self,
        device_id: &DeviceId,
    ) -> Result<uc_core::membership::MembershipCredential, CurrentMemberSignatureError> {
        if device_id != &self.device_id {
            return Err(CurrentMemberSignatureError::Unavailable);
        }
        Ok(self.credential.clone())
    }

    async fn current_member_instance(
        &self,
        device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError> {
        if device_id != &self.device_id {
            return Err(CurrentMemberSignatureError::Unavailable);
        }
        Ok(self.credential.member_instance_id(device_id))
    }

    async fn sign_current_member_payload(
        &self,
        payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        Ok(DeterministicHistoricalVerifier.sign(&self.credential, payload))
    }

    async fn verify_current_member_payload(
        &self,
        member: &DeviceId,
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        Ok(member == &self.device_id
            && DeterministicHistoricalVerifier.sign(&self.credential, payload) == signature)
    }
}

#[derive(Clone, Default)]
struct UnusedSecurityUpdates;

struct UnusedGroupBootstrap;

#[async_trait]
impl uc_core::membership::GroupBootstrapPort for UnusedGroupBootstrap {
    async fn bootstrap_legacy_space(
        &self,
        _sponsor: &DeviceId,
        _retained_members: &[DeviceId],
        _now_ms: i64,
    ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>
    {
        Ok(uc_core::membership::GroupBootstrapResult::Complete {
            bootstrap_id: BootstrapId::generate(),
        })
    }

    async fn acknowledge_legacy_readmission(
        &self,
        _bootstrap_id: &BootstrapId,
        _member: &DeviceId,
        _now_ms: i64,
    ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>
    {
        Err(uc_core::membership::BootstrapError::Repository(
            "unused group bootstrap".to_owned(),
        ))
    }

    async fn withdraw_legacy_readmission(
        &self,
        _bootstrap_id: &BootstrapId,
        _member: &DeviceId,
        _now_ms: i64,
    ) -> Result<uc_core::membership::GroupBootstrapResult, uc_core::membership::BootstrapError>
    {
        Err(uc_core::membership::BootstrapError::Repository(
            "unused group bootstrap".to_owned(),
        ))
    }

    async fn query_legacy_bootstrap(
        &self,
        _bootstrap_id: &BootstrapId,
    ) -> Result<
        Option<uc_core::membership::GroupBootstrapResult>,
        uc_core::membership::BootstrapError,
    > {
        Ok(None)
    }

    async fn resume_legacy_bootstraps(
        &self,
        _now_ms: i64,
    ) -> Result<Vec<uc_core::membership::GroupBootstrapResult>, uc_core::membership::BootstrapError>
    {
        Ok(Vec::new())
    }
}

struct UnavailableSponsorAdmissionSecurity;

#[async_trait]
impl uc_core::membership::PrepareSponsorAdmissionSecurityPort
    for UnavailableSponsorAdmissionSecurity
{
    async fn prepare_sponsor_admission_security(
        &self,
        _request: uc_core::membership::SponsorAdmissionSecurityRequest,
    ) -> Result<
        uc_core::membership::SponsorPreparedAdmissionSecurity,
        uc_core::membership::AdmissionSecurityTransitionError,
    > {
        Err(uc_core::membership::AdmissionSecurityTransitionError::InvalidState)
    }
}

#[async_trait]
impl uc_core::membership::ActivateSponsorAdmissionSecurityPort
    for UnavailableSponsorAdmissionSecurity
{
    async fn activate_sponsor_admission_security(
        &self,
        _request: uc_core::membership::ActivateSponsorAdmissionSecurityRequest,
    ) -> Result<(), uc_core::membership::AdmissionSecurityTransitionError> {
        Err(uc_core::membership::AdmissionSecurityTransitionError::InvalidState)
    }
}

#[async_trait]
impl uc_core::membership::ActivateCompletionHelperAdmissionSecurityPort
    for UnavailableSponsorAdmissionSecurity
{
    async fn activate_completion_helper_admission_security(
        &self,
        _request: uc_core::membership::ActivateCompletionHelperAdmissionSecurityRequest,
    ) -> Result<(), uc_core::membership::AdmissionSecurityTransitionError> {
        Err(uc_core::membership::AdmissionSecurityTransitionError::InvalidState)
    }
}

#[async_trait]
impl uc_core::membership::ActivateSponsorAdmissionSecurityPort
    for RecordingSponsorAdmissionSecurity
{
    async fn activate_sponsor_admission_security(
        &self,
        request: uc_core::membership::ActivateSponsorAdmissionSecurityRequest,
    ) -> Result<(), uc_core::membership::AdmissionSecurityTransitionError> {
        self.activation_requests.lock().unwrap().push(request);
        Ok(())
    }
}

#[async_trait]
impl uc_core::membership::ActivateCompletionHelperAdmissionSecurityPort
    for RecordingSponsorAdmissionSecurity
{
    async fn activate_completion_helper_admission_security(
        &self,
        request: uc_core::membership::ActivateCompletionHelperAdmissionSecurityRequest,
    ) -> Result<(), uc_core::membership::AdmissionSecurityTransitionError> {
        self.helper_activation_requests
            .lock()
            .unwrap()
            .push(request);
        Ok(())
    }
}

#[derive(Default)]
struct RecordingSponsorAdmissionSecurity {
    requests: Mutex<Vec<uc_core::membership::SponsorAdmissionSecurityRequest>>,
    activation_requests: Mutex<Vec<uc_core::membership::ActivateSponsorAdmissionSecurityRequest>>,
    helper_activation_requests:
        Mutex<Vec<uc_core::membership::ActivateCompletionHelperAdmissionSecurityRequest>>,
}

#[async_trait]
impl uc_core::membership::PrepareSponsorAdmissionSecurityPort
    for RecordingSponsorAdmissionSecurity
{
    async fn prepare_sponsor_admission_security(
        &self,
        request: uc_core::membership::SponsorAdmissionSecurityRequest,
    ) -> Result<
        uc_core::membership::SponsorPreparedAdmissionSecurity,
        uc_core::membership::AdmissionSecurityTransitionError,
    > {
        use uc_core::membership::{
            AdmissionSecurityCommitmentV1, SponsorAdmissionSecurityDelivery,
            SponsorPreparedAdmissionSecurity, ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        };
        let catalog = admission_key_catalog();
        let commitment = AdmissionSecurityCommitmentV1::new(
            ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
            request.space_id.as_ref().to_owned(),
            request.space_id.as_ref().as_bytes().to_vec(),
            request.attempt_id,
            request.base_history_position.clone(),
            request.candidate_core_digest,
            1,
            3,
            4,
            [0x31; 32],
            [0x32; 32],
            [0x33; 32],
            catalog.digest(),
            [0x34; 32],
        )
        .unwrap();
        let deliveries = request
            .existing_recipients
            .iter()
            .map(|recipient| SponsorAdmissionSecurityDelivery {
                recipient: recipient.device_id.clone(),
                credential_id: recipient.credential_id,
                payload: b"existing-member-security-update".to_vec(),
            })
            .collect();
        self.requests.lock().unwrap().push(request);
        Ok(SponsorPreparedAdmissionSecurity {
            staged_state: b"sponsor-staged-security-state".to_vec(),
            commit: b"security-commit".to_vec(),
            welcome: b"security-welcome".to_vec(),
            public_commitment: commitment,
            target_protection_group_id: "target-protection-group".to_owned(),
            target_key_catalog: catalog,
            existing_member_deliveries: deliveries,
        })
    }
}

struct ConfiguredAnnouncementMaterial {
    device_id: DeviceId,
}

#[async_trait]
impl CurrentMembershipAnnouncementPort for ConfiguredAnnouncementMaterial {
    async fn current_announcement_material(
        &self,
    ) -> Result<CurrentMembershipAnnouncementMaterial, CurrentMembershipIdentityError> {
        Ok(CurrentMembershipAnnouncementMaterial {
            space_id: SpaceId::from_str(SPACE),
            device_id: self.device_id.clone(),
            device_name: self.device_id.as_str().to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "QRST-UVWX-YZ23-4567",
            )
            .unwrap(),
            transport_public_key: vec![0x35; 32],
            transport_address_blob: vec![0x36],
        })
    }

    async fn wait_for_announcement_change(&self) -> Result<(), CurrentMembershipIdentityError> {
        Ok(())
    }
}

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
    let presence = Arc::new(FixedPresence::default());
    let mut deps = test_deps(Arc::new(repository.clone()), own_device, members);
    deps.presence = presence.clone();
    let owner = WorkspaceConvergence::new(deps);
    Harness {
        owner,
        repository,
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
        admission_attempts: Arc::new(LockedAdmissionRepository {
            allow_empty_history_reads: true,
        }),
        historical_membership_signatures: Arc::new(DeterministicHistoricalVerifier),
        admission_security_transition: Arc::new(EchoAdmissionSecurityTransition::default()),
        prepare_sponsor_admission_security: Arc::new(UnavailableSponsorAdmissionSecurity),
        activate_sponsor_admission_security: Arc::new(UnavailableSponsorAdmissionSecurity),
        activate_completion_helper_admission_security: Arc::new(
            UnavailableSponsorAdmissionSecurity,
        ),
        admission_space_transition: Arc::new(NoAdmissionSpaceTransition),
        admission_outbox_delivery: Arc::new(DeferredAdmissionDelivery),
        admission_completion_recovery: Arc::new(UnusedAdmissionCompletionRecovery),
        legacy_migration_recovery: Arc::new(RecordingLegacyMigrationRecovery::default()),
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
        group_bootstrap: Arc::new(UnusedGroupBootstrap),
        own_device: DeviceId::new(own_device),
    }
}

async fn install_current_history(
    deps: &mut WorkspaceConvergenceDeps,
    directory: &tempfile::TempDir,
    attempt_byte: u8,
) -> MemberInstanceId {
    let admission_repository = durable_admission_repository(directory, [attempt_byte; 16]);
    let (mut history, event, commitment) =
        admission_verification_fixture_for_lineage([attempt_byte; 32], SPACE);
    let uc_core::membership::MembershipOperationV2::AddDevice { admission } = &event.operation
    else {
        unreachable!("fixture always creates AddDevice")
    };
    history
        .verify_and_receive_event(event.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    let mut receipt = uc_core::membership::AdmissionActivationReceipt::new(
        1,
        [attempt_byte; 32],
        event.event_id(),
        event.resulting_members_digest,
        commitment.security_commitment_id,
        admission.facts.member_instance,
        Vec::new(),
    );
    receipt.signature = DeterministicHistoricalVerifier
        .sign(&admission.membership_credential, &receipt.signing_payload());
    history
        .verify_and_record_activation_receipt(receipt, &DeterministicHistoricalVerifier)
        .unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(None, &history.encode_persisted_v2().unwrap())
        .await
        .unwrap();
    let own_instance = *history.active_members().iter().next().unwrap();
    let credential = history.credential_for(own_instance).unwrap().clone();
    deps.admission_attempts = admission_repository;
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential,
    });
    deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: DeviceId::new("sponsor"),
    });
    deps.membership_identity = Arc::new(FixedMembershipIdentity {
        space: SpaceId::from_str(SPACE),
        device_id: DeviceId::new("sponsor"),
    });
    deps.own_device = DeviceId::new("sponsor");
    own_instance
}

#[tokio::test]
async fn admission_recovery_starts_with_legacy_migration_import() {
    let directory = tempfile::tempdir().unwrap();
    let recovery = Arc::new(RecordingLegacyMigrationRecovery::default());
    let mut deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "device-1",
        Vec::new(),
    );
    deps.admission_attempts = durable_admission_repository(&directory, [0x71; 16]);
    deps.legacy_migration_recovery = recovery.clone();
    let owner = WorkspaceConvergence::new(deps);

    owner.recover_pending_admissions().await.unwrap();

    assert_eq!(recovery.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn workspace_query_uses_the_persisted_v2_history_as_its_current_truth() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x72; 16]);
    let (history, _, _) = admission_verification_fixture_for_lineage([0x73; 32], SPACE);
    let encoded_history = history.encode_persisted_v2().unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(None, &encoded_history)
        .await
        .unwrap();
    let repository = MemoryWorkspaceRepository::default();
    repository
        .save_state(&WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1))
        .await
        .unwrap();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);
    let expected_position = history.current_position().unwrap();

    let snapshot = owner.query().await.unwrap();

    assert_eq!(
        snapshot.history_event_count,
        usize::try_from(expected_position.depth.saturating_add(1)).unwrap()
    );
    assert_eq!(snapshot.effective_member_count, 1);
    assert_eq!(
        snapshot.convergence_digest,
        Some(uc_core::membership::WorkspaceDigest::from_bytes(
            expected_position.history_digest
        ))
    );
}

#[tokio::test]
async fn sponsor_recovery_finishes_the_same_activation_after_completion_save_fails() {
    use uc_core::membership::{
        AdmissionAttemptId, AdmissionOutboxPurposeV1, MembershipCredential,
        SponsorAdmissionStageV1, SponsorAdmissionStateV1, VersionedMembershipHistory,
        ED25519_SIGNATURE_ALGORITHM_V1,
    };

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x72; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0x73; 16]);
    let sponsor_transaction = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner_transaction = durable_admission_owner(joiner_repository);
    let attempt_id = AdmissionAttemptId::from_bytes([0x74; 32]);
    let (candidate, base_history, candidate_event, commitment, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let initiated = joiner_transaction
        .start_join(
            attempt_id,
            [0x75; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let candidate_message = sponsor_transaction
        .sponsor_accept_and_offer(
            attempt_id,
            [0x76; 32],
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
    sponsor_transaction
        .record_invitation_consume_result(
            attempt_id,
            super::admission_transaction::InvitationConsumeResultV1::Consumed,
        )
        .await
        .unwrap();
    let prepared = joiner_transaction
        .joiner_verify_and_prepare(
            attempt_id,
            &candidate_message,
            candidate,
            base_history,
            &candidate_event,
            &commitment,
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let commit = sponsor_transaction
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"verified-complete-history",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let receipt_payload = postcard::to_stdvec(&activation_receipt).unwrap();
    let applied = joiner_transaction
        .joiner_apply(
            attempt_id,
            &commit,
            &activation_receipt,
            b"sponsor",
            &receipt_payload,
        )
        .await
        .unwrap();
    let applied_frame = uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *attempt_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Applied,
        message_id: applied.message_id,
        predecessor_message_id: applied.predecessor_message_id,
        payload: receipt_payload.clone(),
    };
    let committed_history = sponsor_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let racing_repository = Arc::new(HistoryRaceAdmissionRepository {
        inner: Arc::clone(&sponsor_repository),
        inject_once: AtomicBool::new(true),
        replacement_history: committed_history,
    });
    let first_activation = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let member_repo = Arc::new(RecordingMemberRepo::default());
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x81; 32]);
    let mut first_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "sponsor",
        Vec::new(),
    );
    first_deps.admission_attempts = racing_repository;
    first_deps.activate_sponsor_admission_security = first_activation.clone();
    first_deps.member_repo = member_repo.clone();
    first_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential.clone(),
    });
    let first_owner = WorkspaceConvergence::new(first_deps);

    assert!(first_owner
        .complete_sponsor_applied(&applied_frame)
        .await
        .is_err());
    assert_eq!(
        first_activation.activation_requests.lock().unwrap().len(),
        1
    );
    let interrupted = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(interrupted.write_ahead_recovery.is_some());
    assert!(interrupted.completion.is_none());
    assert!(member_repo
        .get(&DeviceId::new("joiner"))
        .await
        .unwrap()
        .is_some());
    assert!(matches!(
        interrupted.role_state,
        uc_core::membership::AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Committed
        })
    ));

    let resumed_activation = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let mut resumed_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "sponsor",
        Vec::new(),
    );
    resumed_deps.admission_attempts = Arc::clone(&sponsor_repository);
    resumed_deps.activate_sponsor_admission_security = resumed_activation.clone();
    resumed_deps.member_repo = member_repo.clone();
    resumed_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential,
    });
    let resumed_owner = WorkspaceConvergence::new(resumed_deps);

    assert_eq!(resumed_owner.recover_pending_admissions().await.unwrap(), 1);
    let recovered = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    assert!(recovered.write_ahead_recovery.is_none());
    assert!(recovered.completion.is_some());
    assert_eq!(
        recovered
            .outboxes
            .iter()
            .filter(|message| {
                message.purpose == AdmissionOutboxPurposeV1::Complete && !message.superseded
            })
            .count(),
        1
    );
    let recovered_history = VersionedMembershipHistory::decode_persisted_v2(
        &sponsor_repository
            .load_membership_history_v2()
            .await
            .unwrap()
            .unwrap(),
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(recovered_history.active_members().len(), 2);
    assert_eq!(
        resumed_activation.activation_requests.lock().unwrap().len(),
        1
    );

    resumed_owner.recover_pending_admissions().await.unwrap();
    assert_eq!(
        resumed_activation.activation_requests.lock().unwrap().len(),
        1
    );
    assert_eq!(
        sponsor_repository
            .load(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .outboxes
            .iter()
            .filter(|message| {
                message.purpose == AdmissionOutboxPurposeV1::Complete && !message.superseded
            })
            .count(),
        1
    );
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

#[derive(Default)]
struct RecordingMemberRepo(Mutex<HashMap<DeviceId, uc_core::membership::SpaceMember>>);

#[async_trait]
impl MemberRepositoryPort for RecordingMemberRepo {
    async fn get(
        &self,
        device_id: &DeviceId,
    ) -> Result<Option<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError>
    {
        Ok(self.0.lock().unwrap().get(device_id).cloned())
    }

    async fn list(
        &self,
    ) -> Result<Vec<uc_core::membership::SpaceMember>, uc_core::membership::MembershipError> {
        Ok(self.0.lock().unwrap().values().cloned().collect())
    }

    async fn save(
        &self,
        member: &uc_core::membership::SpaceMember,
    ) -> Result<(), uc_core::membership::MembershipError> {
        self.0
            .lock()
            .unwrap()
            .insert(member.device_id.clone(), member.clone());
        Ok(())
    }

    async fn remove(
        &self,
        device_id: &DeviceId,
    ) -> Result<bool, uc_core::membership::MembershipError> {
        Ok(self.0.lock().unwrap().remove(device_id).is_some())
    }
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
async fn v2_current_peer_scope_requires_a_permanent_activation_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x72; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), false, false).await;
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert!(snapshot.peer_device_ids.is_empty());
}

#[tokio::test]
async fn v2_current_peer_scope_opens_for_an_observer_after_activation_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x73; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), true, false).await;
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("joiner")]);
    assert!(!owner.locally_removed(&DeviceId::new("joiner")).await);
}

#[tokio::test]
async fn v2_joiner_scope_stays_closed_until_the_local_join_is_active() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x74; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), true, true).await;
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Removed
    );
    assert!(snapshot.peer_device_ids.is_empty());
}

#[tokio::test]
async fn v2_joiner_scope_opens_after_the_local_join_is_active() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x77; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), true, false).await;
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("sponsor")]);
}

#[tokio::test]
async fn rejected_local_join_does_not_remove_an_existing_current_member() {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, AdmissionRejectionReasonV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, JoinerAdmissionStateV1,
    };

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x81; 16]);
    seed_v2_scope_history(Arc::clone(&admission_repository), true, true).await;
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x75; 32]);
    let mut rejected = admission_repository
        .load(attempt_id)
        .await
        .unwrap()
        .unwrap();
    let expected_version = rejected.record_version;
    rejected.record_version += 1;
    rejected.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
        stage: JoinerAdmissionStageV1::Rejected,
    });
    rejected.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
    rejected.rejection_reason = Some(AdmissionRejectionReasonV1::HistoryConflict);
    admission_repository
        .compare_and_advance(attempt_id, expected_version, &rejected)
        .await
        .unwrap();
    admission_repository
        .compact_terminal(attempt_id, rejected.record_version)
        .await
        .unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("sponsor"),
        legacy_member("joiner"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(
        snapshot.local_membership,
        uc_core::membership::CurrentWorkspaceLocalMembership::Active
    );
    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("sponsor")]);
}

#[tokio::test]
async fn current_peer_scope_fails_closed_when_v2_history_is_locked() {
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = Arc::new(LockedAdmissionRepository {
        allow_empty_history_reads: false,
    });
    let owner = WorkspaceConvergence::new(deps);

    let result = owner.snapshot().await;

    assert_eq!(
        result,
        Err(uc_core::membership::CurrentWorkspacePeerScopeError::Locked)
    );
}

#[tokio::test]
async fn current_peer_scope_fails_closed_when_v2_history_is_corrupt() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x78; 16]);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x79; 32]);
    let mut attempt = uc_core::membership::AdmissionAttemptV1::new_joiner(
        attempt_id,
        [0x7a; 16],
        uc_core::membership::JoinerAdmissionStageV1::Initiated,
    );
    attempt.join_id = None;
    attempt.local_join_ordinal = None;
    attempt.role_state = uc_core::membership::AdmissionAttemptRoleStateV1::Sponsor(
        uc_core::membership::SponsorAdmissionStateV1 {
            stage: uc_core::membership::SponsorAdmissionStageV1::Accepted,
        },
    );
    attempt.invitation_claim = Some(b"scope-invitation".to_vec());
    admission_repository
        .create(&attempt, None, Some(b"corrupt-history"))
        .await
        .unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);

    let result = owner.snapshot().await;

    assert_eq!(
        result,
        Err(uc_core::membership::CurrentWorkspacePeerScopeError::Corrupt)
    );
}

#[tokio::test]
async fn pending_cross_space_join_keeps_the_source_space_scope() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x7b; 16]);
    seed_v2_scope_history_for_lineage(
        Arc::clone(&admission_repository),
        "target-space",
        true,
        true,
        Some(SPACE),
    )
    .await;
    let own = instance(0x0a);
    let peer = instance(0x0b);
    let genesis = membership_event(None, 0, own, own, "joiner", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, own, peer, "source-peer", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), own);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own);
    state.membership_reconciliation = Some(history);
    let repository = MemoryWorkspaceRepository::default();
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("source-peer")]);
}

#[tokio::test]
async fn unrelated_pending_join_does_not_hide_a_v2_lineage_mismatch() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x7c; 16]);
    seed_v2_scope_history_for_lineage(
        Arc::clone(&admission_repository),
        "unrelated-space",
        true,
        true,
        None,
    )
    .await;
    let own = instance(0x0a);
    let genesis = membership_event(None, 0, own, own, "joiner", 1);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), own);
    history.receive_verified(genesis).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own);
    state.membership_reconciliation = Some(history);
    let repository = MemoryWorkspaceRepository::default();
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);

    let result = owner.snapshot().await;

    assert_eq!(
        result,
        Err(uc_core::membership::CurrentWorkspacePeerScopeError::Corrupt)
    );
}

#[tokio::test]
async fn rejected_cross_space_join_restores_the_source_space_scope() {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, AdmissionRejectionReasonV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, JoinerAdmissionStateV1,
    };

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x80; 16]);
    seed_v2_scope_history_for_lineage(
        Arc::clone(&admission_repository),
        "target-space",
        true,
        true,
        Some(SPACE),
    )
    .await;
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x75; 32]);
    let mut rejected = admission_repository
        .load(attempt_id)
        .await
        .unwrap()
        .unwrap();
    let expected_version = rejected.record_version;
    rejected.record_version += 1;
    rejected.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
        stage: JoinerAdmissionStageV1::Rejected,
    });
    rejected.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
    rejected.rejection_reason = Some(AdmissionRejectionReasonV1::RemovedBeforeActivation);
    rejected.identity_binding = Some(b"joiner-identity".to_vec());
    rejected.space_transition = None;
    rejected.target_access_state = None;
    rejected.staged_security_state = None;
    admission_repository
        .compare_and_advance(attempt_id, expected_version, &rejected)
        .await
        .unwrap();
    admission_repository
        .compact_terminal(attempt_id, rejected.record_version)
        .await
        .unwrap();

    let own = instance(0x0a);
    let peer = instance(0x0b);
    let genesis = membership_event(None, 0, own, own, "joiner", 1);
    let addition = membership_event(Some(genesis.event_id()), 1, own, peer, "source-peer", 2);
    let mut history = MembershipReconciliation::new(SPACE.to_owned(), own);
    history.receive_verified(genesis).unwrap();
    history.receive_verified(addition).unwrap();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own);
    state.membership_reconciliation = Some(history);
    let repository = MemoryWorkspaceRepository::default();
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    let owner = WorkspaceConvergence::new(deps);

    let snapshot = owner.snapshot().await.unwrap();

    assert_eq!(snapshot.peer_device_ids, vec![DeviceId::new("source-peer")]);
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

// 流程：A 不在线时 B 将 C 加入空间；A 恢复但未保存 C 的资料，C 首次联系即提交
// 从起点到 C 的连续成员记录，使 A 能从历史本身验证 C 的准入关系。

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
    let peer = DeviceId::new("joiner");
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

// 流程：A、B 都从低于 1.1 的同一旧 Space 升级，双方起初都没有 1.1 成员历史；
// A 建立唯一历史起点，B 通过当前问候提交自己的签名资料，双方保存同一历史后 A 清除升级提示。

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

// 流程：A 已是 1.1，B 低于 1.1；当前成员历史入口没有回应，但旧入口空连接成功。
// 证明：只有旧入口的正面证据会让 A 保存“B 需要升级”，并暂停内容同步。
#[tokio::test]
async fn confirmed_legacy_peer_is_marked_upgrade_required_after_current_flow_is_unavailable() {
    let directory = tempfile::tempdir().unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(ScriptedExchange::new(Vec::new()));
    let probe = Arc::new(ScriptedLegacyProbe::new(vec![Ok(())]));
    let peer = DeviceId::new("joiner");
    let mut deps = test_deps(Arc::new(repository.clone()), "sponsor", Vec::new());
    deps.membership_history_exchange = exchange.clone();
    deps.legacy_peer_probe = probe.clone();
    let own_instance = install_current_history(&mut deps, &directory, 0xb1).await;
    let owner = WorkspaceConvergence::new(deps);
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own_instance);
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
    let directory = tempfile::tempdir().unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(ScriptedExchange::new(Vec::new()));
    let probe = Arc::new(ScriptedLegacyProbe::new(vec![Ok(())]));
    let peer = DeviceId::new("joiner");
    let mut deps = test_deps(Arc::new(repository.clone()), "sponsor", Vec::new());
    deps.membership_history_exchange = exchange;
    deps.legacy_peer_probe = probe.clone();
    deps.peer_addr_repo = Arc::new(FixedPeerAddrRepo {
        records: vec![uc_core::ports::PeerAddressRecord {
            device_id: peer.clone(),
            addr_blob: vec![1],
            observed_at: chrono::Utc::now(),
        }],
    });
    let own_instance = install_current_history(&mut deps, &directory, 0xb2).await;
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own_instance);
    repository.save_state(&state).await.unwrap();
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
    let directory = tempfile::tempdir().unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(ScriptedExchange::new(Vec::new()));
    let probe = Arc::new(ScriptedLegacyProbe::new(vec![Err(
        uc_core::membership::LegacyPeerProbeError::Transport,
    )]));
    let peer = DeviceId::new("joiner");
    let mut deps = test_deps(Arc::new(repository.clone()), "sponsor", Vec::new());
    deps.membership_history_exchange = exchange;
    deps.legacy_peer_probe = probe.clone();
    let own_instance = install_current_history(&mut deps, &directory, 0xb3).await;
    let owner = WorkspaceConvergence::new(deps);
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own_instance);
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
    let directory = tempfile::tempdir().unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let probe = Arc::new(ScriptedLegacyProbe::new(vec![Ok(())]));
    let peer = DeviceId::new("joiner");
    let mut deps = test_deps(Arc::new(repository.clone()), "sponsor", Vec::new());
    deps.membership_history_exchange = Arc::new(RejectingExchange);
    deps.legacy_peer_probe = probe.clone();
    let own_instance = install_current_history(&mut deps, &directory, 0xb4).await;
    let owner = WorkspaceConvergence::new(deps);
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own_instance);
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
    let directory = tempfile::tempdir().unwrap();
    let repository = MemoryWorkspaceRepository::default();
    let exchange = Arc::new(BlockingTrackingExchange::new());
    let peer = DeviceId::new("device-b");
    let mut deps = test_deps(Arc::new(repository.clone()), "sponsor", Vec::new());
    deps.membership_history_exchange = exchange.clone();
    let own_instance = install_current_history(&mut deps, &directory, 0xb5).await;
    let owner = WorkspaceConvergence::new(deps);
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(own_instance);
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

// 流程：B 收到 A 对 B 的移除时先保存事实但不改变本机安全状态；B 明确接受后，才应用该移除携带的安全更新。

#[tokio::test]
async fn persisted_v2_removal_decision_is_retried_after_restart_for_a_diverged_author() {
    use uc_core::membership::{
        MembershipDecisionV2, MembershipOperationV2, MEMBERSHIP_DECISION_FORMAT_V2,
    };

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x93; 16]);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x94; 32]);
    let (_, mut local_history, candidate, _, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let MembershipOperationV2::AddDevice { admission } = &candidate.operation else {
        unreachable!("fixture always creates AddDevice")
    };
    let local_credential = admission.membership_credential.clone();
    let local_member = admission.facts.member_instance;
    local_history
        .verify_and_receive_event(candidate.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    local_history
        .verify_and_record_activation_receipt(activation_receipt, &DeterministicHistoricalVerifier)
        .unwrap();
    let removal = durable_candidate_removal_fixture(attempt_id);
    let mut author_history = local_history.clone();
    author_history
        .verify_and_receive_event(removal.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    local_history
        .merge_remote_history(
            &author_history,
            local_member,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    let mut rejection = MembershipDecisionV2::new(
        MEMBERSHIP_DECISION_FORMAT_V2,
        removal.lineage_id.clone(),
        removal.event_id(),
        local_member,
        local_credential.credential_id,
        local_credential.signature_algorithm_version,
        RemovalDecision::Reject,
        removal.parent_event_id,
        candidate.resulting_members_digest,
        [0x95; 16],
        Vec::new(),
    );
    rejection.signature =
        DeterministicHistoricalVerifier.sign(&local_credential, &rejection.signing_payload());
    local_history
        .verify_and_record_local_decision(rejection, local_member, &DeterministicHistoricalVerifier)
        .unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(
            None,
            &local_history.encode_persisted_v2().unwrap(),
        )
        .await
        .unwrap();

    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(removal.lineage_id.clone(), 1);
    state.own_instance = Some(local_member);
    state.peer_history_relationships.insert(
        DeviceId::new("sponsor"),
        uc_core::membership::MembershipHistoryRelationship::Diverged,
    );
    repository.save_state(&state).await.unwrap();
    let exchange = Arc::new(ScriptedExchange::new(vec![
        MembershipHistoryMessage::AckV2(
            uc_core::membership::MembershipHistoryV2Ack::UpdatesApplied,
        ),
    ]));
    let mut deps = test_deps(Arc::new(repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("joiner"),
        credential: local_credential,
    });
    deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: DeviceId::new("joiner"),
    });
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("sponsor")]));
    deps.membership_history_exchange = exchange.clone();
    let restarted = WorkspaceConvergence::new(deps);

    assert!(
        restarted.locally_removed(&DeviceId::new("sponsor")).await,
        "a diverged V2 peer must remain blocked for normal content after restart"
    );

    restarted
        .deliver_pending_membership_decisions()
        .await
        .unwrap();

    let sent = exchange.history_sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, DeviceId::new("sponsor"));
    assert!(matches!(
        sent[0].1,
        MembershipHistoryMessage::HistoryPageV2(_)
    ));
}

#[tokio::test]
async fn unknown_v2_member_may_introduce_a_complete_activated_extension() {
    use uc_core::membership::{MembershipHistoryMessage, MembershipHistoryV2Ack};

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0xa3; 16]);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xa4; 32]);
    let (_, base_history, candidate, _, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let mut incoming = base_history.clone();
    incoming
        .verify_and_receive_event(candidate.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    incoming
        .verify_and_record_activation_receipt(activation_receipt, &DeterministicHistoricalVerifier)
        .unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(
            None,
            &base_history.encode_persisted_v2().unwrap(),
        )
        .await
        .unwrap();

    let sponsor_credential = base_history
        .credential_for(candidate.author_member_instance_id)
        .unwrap()
        .clone();
    let uc_core::membership::MembershipOperationV2::AddDevice { admission } = &candidate.operation
    else {
        unreachable!("fixture always creates AddDevice")
    };
    let repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(candidate.lineage_id.clone(), 1);
    state.own_instance = Some(candidate.author_member_instance_id);
    repository.save_state(&state).await.unwrap();
    let mut deps = test_deps(Arc::new(repository), "sponsor", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential,
    });
    let owner = WorkspaceConvergence::new(deps);

    assert!(incoming.is_complete_extension_of(&base_history));
    assert!(incoming
        .active_members()
        .contains(&admission.facts.member_instance));
    let mut sender_facts = admission.facts.clone();
    sender_facts.identity_signature = DeterministicHistoricalVerifier.sign(
        &admission.membership_credential,
        &sender_facts.signing_payload(),
    );
    let pages = incoming
        .export_reconciliation_pages_v2(sender_facts)
        .unwrap();
    let imported = uc_core::membership::VersionedMembershipHistory::import_exchange_pages_v2(
        &pages,
        &DeterministicHistoricalVerifier,
    )
    .unwrap();
    assert_eq!(imported, incoming);

    let response = owner
        .handle_membership_history(
            &admission.facts.device_id,
            MembershipHistoryMessage::HistoryPageV2(pages[0].clone()),
        )
        .await
        .unwrap();

    assert_eq!(
        response,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::UpdatesApplied)
    );
    assert_eq!(
        owner.query().await.unwrap().history_event_count,
        usize::try_from(
            base_history
                .current_position()
                .unwrap()
                .depth
                .saturating_add(2)
        )
        .unwrap()
    );
}

fn paged_runtime_history_fixture(
    final_credential_number: u16,
) -> (
    uc_core::membership::VersionedMembershipHistory,
    uc_core::membership::VersionedMembershipHistory,
    Vec<uc_core::membership::MembershipHistoryPageV2>,
    uc_core::membership::MembershipCredential,
    uc_core::membership::MembershipCredential,
) {
    use uc_core::membership::{
        AdmissionActivationReceipt, MembershipAdmissionV2, MembershipCredential, MembershipEventId,
        MembershipEventV2, MembershipOperationV2, VersionedMembershipHistory,
        ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
    };

    fn admission(device: &str, credential_number: u16) -> MembershipAdmissionV2 {
        let credential = MembershipCredential::new(
            ED25519_SIGNATURE_ALGORITHM_V1,
            credential_number.to_be_bytes().repeat(16),
        );
        let device_id = DeviceId::new(device);
        let member_instance = credential.member_instance_id(&device_id);
        let marker = credential_number.to_be_bytes()[1];
        MembershipAdmissionV2 {
            facts: admission_facts_for(member_instance, &device_id),
            membership_credential: credential,
            resume_public_key_digest: [marker.wrapping_add(1); 32],
            security_commitment_id: [marker.wrapping_add(2); 32],
        }
    }

    fn event(
        history: &VersionedMembershipHistory,
        parent: Option<MembershipEventId>,
        author: &MembershipAdmissionV2,
        operation: MembershipOperationV2,
        operation_number: u16,
    ) -> MembershipEventV2 {
        let resulting_members_digest = history
            .expected_resulting_members_digest(parent, &operation)
            .unwrap();
        let mut operation_id = [0; 16];
        operation_id[..2].copy_from_slice(&operation_number.to_be_bytes());
        let marker = operation_number.to_be_bytes()[1];
        let mut event = MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            SPACE.to_owned(),
            parent,
            parent
                .map(|event_id| history.depth(event_id).unwrap().saturating_add(1))
                .unwrap_or(0),
            operation_id,
            author.facts.member_instance,
            author.membership_credential.credential_id,
            author.membership_credential.signature_algorithm_version,
            operation,
            resulting_members_digest,
            [marker; 32],
            vec![marker],
            Some([marker.wrapping_add(1); 32]),
            Vec::new(),
        );
        event.signature = DeterministicHistoricalVerifier
            .sign(&author.membership_credential, &event.signing_payload());
        event
    }

    fn activate(
        history: &mut VersionedMembershipHistory,
        event: &MembershipEventV2,
        admission: &MembershipAdmissionV2,
    ) {
        let mut receipt = AdmissionActivationReceipt::new(
            1,
            [event.operation_id[1]; 32],
            event.event_id(),
            event.resulting_members_digest,
            admission.security_commitment_id,
            admission.facts.member_instance,
            Vec::new(),
        );
        receipt.signature = DeterministicHistoricalVerifier
            .sign(&admission.membership_credential, &receipt.signing_payload());
        history
            .verify_and_record_activation_receipt(receipt, &DeterministicHistoricalVerifier)
            .unwrap();
    }

    let sponsor = admission("sponsor", 1);
    let joiner = admission("joiner", 2);
    let mut history = VersionedMembershipHistory::new(SPACE.to_owned());
    let genesis = event(
        &history,
        None,
        &sponsor,
        MembershipOperationV2::AddDevice {
            admission: sponsor.clone(),
        },
        1,
    );
    history
        .verify_and_receive_event(genesis.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    let add_joiner = event(
        &history,
        Some(genesis.event_id()),
        &sponsor,
        MembershipOperationV2::AddDevice {
            admission: joiner.clone(),
        },
        2,
    );
    history
        .verify_and_receive_event(add_joiner.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    activate(&mut history, &add_joiner, &joiner);
    let base_history = history.clone();
    let mut head = add_joiner.event_id();
    for index in 0..254u16 {
        let joining = admission(&format!("paged-device-{index}"), index + 20);
        let add = event(
            &history,
            Some(head),
            &sponsor,
            MembershipOperationV2::AddDevice {
                admission: joining.clone(),
            },
            3 + index,
        );
        history
            .verify_and_receive_event(add.clone(), &DeterministicHistoricalVerifier)
            .unwrap();
        activate(&mut history, &add, &joining);
        head = add.event_id();
    }
    let final_member = admission("paged-final", final_credential_number);
    let final_add = event(
        &history,
        Some(head),
        &sponsor,
        MembershipOperationV2::AddDevice {
            admission: final_member.clone(),
        },
        257,
    );
    history
        .verify_and_receive_event(final_add.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    activate(&mut history, &final_add, &final_member);
    let mut sender_facts = sponsor.facts.clone();
    sender_facts.identity_signature = DeterministicHistoricalVerifier.sign(
        &sponsor.membership_credential,
        &sender_facts.signing_payload(),
    );
    let pages = history
        .export_reconciliation_pages_v2(sender_facts)
        .unwrap();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].record_counts().events, 256);
    assert_eq!(pages[1].record_counts().events, 1);
    (
        base_history,
        history,
        pages,
        sponsor.membership_credential,
        joiner.membership_credential,
    )
}

fn paged_receiver(
    workspace_repository: MemoryWorkspaceRepository,
    admission_repository: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    joiner_credential: uc_core::membership::MembershipCredential,
) -> Arc<WorkspaceConvergence> {
    let mut deps = test_deps(Arc::new(workspace_repository), "joiner", Vec::new());
    deps.admission_attempts = admission_repository;
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("joiner"),
        credential: joiner_credential,
    });
    deps.membership_identity = Arc::new(FixedMembershipIdentity {
        space: SpaceId::from_str(SPACE),
        device_id: DeviceId::new("joiner"),
    });
    deps.own_device = DeviceId::new("joiner");
    WorkspaceConvergence::new(deps)
}

// 流程：第二页先到时不改正式历史；第一页保存后重启，重复页保持幂等，最后一页才完成替换。
#[tokio::test]
async fn paged_history_resumes_after_restart_and_applies_only_when_complete() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0xb1; 16]);
    let (base, incoming, pages, _sponsor_credential, joiner_credential) =
        paged_runtime_history_fixture(0x3c1);
    let base_bytes = base.encode_persisted_v2().unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(None, &base_bytes)
        .await
        .unwrap();
    let workspace_repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(joiner_credential.member_instance_id(&DeviceId::new("joiner")));
    workspace_repository.save_state(&state).await.unwrap();
    let receiver = paged_receiver(
        workspace_repository.clone(),
        admission_repository.clone(),
        joiner_credential.clone(),
    );

    let early = receiver
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[1].clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        early,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Continue {
            transfer_id: pages[0].transfer_id(),
            next_page_index: 0,
        })
    );
    assert_eq!(
        admission_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(base_bytes.clone())
    );

    let first = receiver
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[0].clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        first,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Continue {
            transfer_id: pages[0].transfer_id(),
            next_page_index: 1,
        })
    );
    drop(receiver);

    let restarted = paged_receiver(
        workspace_repository.clone(),
        admission_repository.clone(),
        joiner_credential,
    );
    let duplicate = restarted
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[0].clone()),
        )
        .await
        .unwrap();
    assert_eq!(duplicate, first);
    assert_eq!(
        admission_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(base_bytes)
    );

    let completed = restarted
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[1].clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        completed,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::UpdatesApplied)
    );
    assert_eq!(
        admission_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(incoming.encode_persisted_v2().unwrap())
    );
    assert!(workspace_repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .pending_membership_history_transfers
        .is_empty());
}

// 流程：同一来源混入另一轮第一页时拒绝整轮资料，并保留原来的正式历史。
#[tokio::test]
async fn paged_history_rejects_a_conflicting_transfer_and_clears_progress() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0xb2; 16]);
    let (base, _incoming, pages, _sponsor_credential, joiner_credential) =
        paged_runtime_history_fixture(0x3c2);
    let (_, _, conflicting_pages, _, _) = paged_runtime_history_fixture(0x3c3);
    let base_bytes = base.encode_persisted_v2().unwrap();
    admission_repository
        .compare_and_replace_membership_history_v2(None, &base_bytes)
        .await
        .unwrap();
    let workspace_repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(joiner_credential.member_instance_id(&DeviceId::new("joiner")));
    workspace_repository.save_state(&state).await.unwrap();
    let receiver = paged_receiver(
        workspace_repository.clone(),
        admission_repository.clone(),
        joiner_credential,
    );
    receiver
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(pages[0].clone()),
        )
        .await
        .unwrap();

    let response = receiver
        .handle_membership_history(
            &DeviceId::new("sponsor"),
            MembershipHistoryMessage::HistoryPageV2(conflicting_pages[0].clone()),
        )
        .await
        .unwrap();

    assert_eq!(
        response,
        MembershipHistoryMessage::AckV2(MembershipHistoryV2Ack::Invalid)
    );
    assert_eq!(
        admission_repository
            .load_membership_history_v2()
            .await
            .unwrap(),
        Some(base_bytes)
    );
    assert!(workspace_repository
        .load_state()
        .await
        .unwrap()
        .unwrap()
        .pending_membership_history_transfers
        .is_empty());
}

// 流程：发送方通过唯一分页入口传完 257 条记录，接收方回执后双方保存相同历史。
#[tokio::test]
async fn paged_history_transfers_257_events_end_to_end() {
    let receiver_directory = tempfile::tempdir().unwrap();
    let receiver_admission = durable_admission_repository(&receiver_directory, [0xb3; 16]);
    let (base, incoming, _pages, sponsor_credential, joiner_credential) =
        paged_runtime_history_fixture(0x3c4);
    receiver_admission
        .compare_and_replace_membership_history_v2(None, &base.encode_persisted_v2().unwrap())
        .await
        .unwrap();
    let receiver_workspace = MemoryWorkspaceRepository::default();
    let mut receiver_state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    receiver_state.own_instance =
        Some(joiner_credential.member_instance_id(&DeviceId::new("joiner")));
    receiver_workspace
        .save_state(&receiver_state)
        .await
        .unwrap();
    let receiver = paged_receiver(
        receiver_workspace,
        receiver_admission.clone(),
        joiner_credential,
    );
    let loopback = Arc::new(LoopbackHistoryExchange {
        receiver,
        source_device_id: DeviceId::new("sponsor"),
        sent_pages: AtomicUsize::new(0),
    });

    let sender_directory = tempfile::tempdir().unwrap();
    let sender_admission = durable_admission_repository(&sender_directory, [0xb4; 16]);
    sender_admission
        .compare_and_replace_membership_history_v2(None, &incoming.encode_persisted_v2().unwrap())
        .await
        .unwrap();
    let sender_workspace = MemoryWorkspaceRepository::default();
    let mut sender_state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    sender_state.own_instance =
        Some(sponsor_credential.member_instance_id(&DeviceId::new("sponsor")));
    sender_workspace.save_state(&sender_state).await.unwrap();
    let mut sender_deps = test_deps(Arc::new(sender_workspace), "sponsor", Vec::new());
    sender_deps.admission_attempts = sender_admission.clone();
    sender_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: DeviceId::new("sponsor"),
        credential: sponsor_credential,
    });
    sender_deps.membership_identity = Arc::new(FixedMembershipIdentity {
        space: SpaceId::from_str(SPACE),
        device_id: DeviceId::new("sponsor"),
    });
    sender_deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: DeviceId::new("sponsor"),
    });
    sender_deps.membership_history_exchange = loopback.clone();
    sender_deps.own_device = DeviceId::new("sponsor");
    let sender = WorkspaceConvergence::new(sender_deps);

    sender
        .reconcile_membership_history_with_peer(&DeviceId::new("joiner"))
        .await
        .unwrap();

    assert_eq!(loopback.sent_pages.load(Ordering::SeqCst), 2);
    let sender_history = sender_admission.load_membership_history_v2().await.unwrap();
    let receiver_history = receiver_admission
        .load_membership_history_v2()
        .await
        .unwrap();
    assert_eq!(receiver_history, sender_history);
}

// 流程：A 与 B 已经分叉；A 请求 B 的旧 Space 成员资料，B 必须拒绝，不再继续交换旧分支。

// 流程：A 已移除 B；B 接受后回传决定，A 仍依据移除前保存的成员关系验证并记录该回传。

// 流程：A 提交移除后，B 与 A、C 都曾交换到同一待决定历史；B 拒绝时把决定发给 A、C，等待双方按决定结果解除阻断或进入分叉。

// 流程：C 接受 A 对 B 的移除时，先按决定前的成员分支固定通知名单；即使应用后 B 已不再有效，也必须收到 C 的相反决定。

// 流程：B、C 对同一项移除都选择拒绝；B 收到 C 的签名决定后确认双方仍在同一旧分支，解除内容阻断。

// 流程：B 拒绝 A 提交的待决定移除后，保留原成员关系，并只隔离与 A 的旧分支。

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

#[tokio::test]
async fn single_member_legacy_history_forms_an_honest_v2_admission_base() {
    let repository = MemoryWorkspaceRepository::default();
    let device_id = DeviceId::new("device-a");
    let credential = uc_core::membership::MembershipCredential::new(1, vec![0x71; 32]);
    let mut deps = test_deps(Arc::new(repository), device_id.as_str(), Vec::new());
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: device_id.clone(),
        credential: credential.clone(),
    });
    let owner = WorkspaceConvergence::new(deps);
    owner.initialize_upgraded_legacy_space().await.unwrap();

    let history = owner.verified_admission_base_history().await.unwrap();

    let own_instance = credential.member_instance_id(&device_id);
    assert_eq!(history.effective_members(), [own_instance].into());
    assert_eq!(history.active_members(), [own_instance].into());
    assert_eq!(history.credential_for(own_instance), Some(&credential));
}

#[tokio::test]
async fn multi_member_legacy_history_cannot_claim_complete_v2_verification() {
    let repository = MemoryWorkspaceRepository::default();
    let device_id = DeviceId::new("device-a");
    let credential = uc_core::membership::MembershipCredential::new(1, vec![0x72; 32]);
    let mut deps = test_deps(Arc::new(repository), device_id.as_str(), Vec::new());
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: device_id.clone(),
        credential,
    });
    deps.member_repo = Arc::new(FixedMemberRepo(vec![legacy_member("device-b")]));
    let owner = WorkspaceConvergence::new(deps);
    owner.initialize_upgraded_legacy_space().await.unwrap();

    assert!(matches!(
        owner.verified_admission_base_history().await,
        Err(WorkspaceConvergenceError::RecoveryRequired)
    ));
}

#[tokio::test]
async fn sponsor_candidate_uses_only_members_active_in_verified_history() {
    use uc_core::membership::{
        AdmissionAttemptId, AdmissionAttemptRoleStateV1, AdmissionAttemptV1,
        AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, JoinerAdmissionStateV1, MembershipCredential, MembershipEventV2,
        MembershipOperationV2, ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
    };
    use uc_core::pairing::{InvitationCode, JoinerRequest, PairingSecurityCapability};

    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x41; 16]);
    let seed_attempt_id = AdmissionAttemptId::from_bytes([0x42; 32]);
    let (mut history, added_member, _) =
        admission_verification_fixture_for_lineage(*seed_attempt_id.as_bytes(), SPACE);
    history
        .verify_and_receive_event(added_member.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x81; 32]);
    let sponsor_device = DeviceId::new("sponsor");
    let sponsor_member = sponsor_credential.member_instance_id(&sponsor_device);
    let MembershipOperationV2::AddDevice {
        admission: removed_admission,
    } = &added_member.operation
    else {
        unreachable!()
    };
    let removal_operation = MembershipOperationV2::RemoveDevice {
        member: removed_admission.facts.member_instance,
    };
    let removal_members_digest = history
        .expected_resulting_members_digest(Some(added_member.event_id()), &removal_operation)
        .unwrap();
    let mut removal = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        SPACE.to_owned(),
        Some(added_member.event_id()),
        9,
        [0x43; 16],
        sponsor_member,
        sponsor_credential.credential_id,
        sponsor_credential.signature_algorithm_version,
        removal_operation,
        removal_members_digest,
        [0x44; 32],
        Vec::new(),
        None,
        Vec::new(),
    );
    removal.signature =
        DeterministicHistoricalVerifier.sign(&sponsor_credential, &removal.signing_payload());
    history
        .verify_and_receive_event(removal, &DeterministicHistoricalVerifier)
        .unwrap();
    assert_eq!(history.active_members(), [sponsor_member].into());

    let mut seed = AdmissionAttemptV1::new_joiner(
        seed_attempt_id,
        [0x45; 16],
        JoinerAdmissionStageV1::Rejected,
    );
    seed.local_join_ordinal = Some(0);
    seed.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
        stage: JoinerAdmissionStageV1::Rejected,
    });
    seed.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
    seed.rejection_reason = Some(AdmissionRejectionReasonV1::Cancelled);
    admission_repository
        .create(&seed, None, Some(&history.encode_persisted_v2().unwrap()))
        .await
        .unwrap();

    let workspace_repository = MemoryWorkspaceRepository::default();
    let mut state = WorkspaceConvergenceState::fresh(SPACE.to_owned(), 1);
    state.own_instance = Some(sponsor_member);
    workspace_repository.save_state(&state).await.unwrap();
    let security = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let mut deps = test_deps(
        Arc::new(workspace_repository),
        sponsor_device.as_str(),
        Vec::new(),
    );
    deps.admission_attempts = admission_repository;
    deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: sponsor_device.clone(),
        credential: sponsor_credential,
    });
    deps.membership_identity = Arc::new(FixedMembershipIdentity {
        space: SpaceId::from_str(SPACE),
        device_id: sponsor_device.clone(),
    });
    deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: sponsor_device,
    });
    deps.prepare_sponsor_admission_security = security.clone();
    deps.member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member("joiner"),
        legacy_member("stale-removed-member"),
    ]));
    let owner = WorkspaceConvergence::new(deps);

    let attempt_id = AdmissionAttemptId::from_bytes([0x46; 32]);
    let invitation = InvitationCode::new("candidate-history-filter");
    let joiner_device = DeviceId::new("new-device");
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x47; 32]);
    let joiner_member = joiner_credential.member_instance_id(&joiner_device);
    let mut facts = admission_facts_for(joiner_member, &joiner_device);
    facts.identity_signature =
        DeterministicHistoricalVerifier.sign(&joiner_credential, &facts.signing_payload());
    let binding = crate::space::admission::adapter::stable_join_request_binding(
        &joiner_device,
        &facts.identity_fingerprint,
    );
    let request_message = super::admission_transaction::durable_admission_message(
        attempt_id,
        AdmissionOutboxPurposeV1::JoinRequest,
        invitation.as_str().as_bytes(),
        None,
        &binding,
    );
    let request = JoinerRequest {
        attempt_id: *attempt_id.as_bytes(),
        join_id: [0x48; 16],
        request_message_id: request_message.message_id,
        invitation_code: invitation,
        device_id: joiner_device,
        device_name: facts.device_name.clone(),
        identity_fingerprint: facts.identity_fingerprint.clone(),
        nonce: Vec::new(),
        transport_address_blob: facts.transport_address_blob.clone(),
        security_capability: PairingSecurityCapability::ReliableGroupEpochV1,
        key_package: b"candidate-key-package".to_vec(),
        member_instance: joiner_member,
        membership_credential: joiner_credential,
        resume_public_key: vec![0x49; 32],
        admission: facts,
    };

    let frame = owner.prepare_sponsor_candidate(&request).await.unwrap();

    assert_eq!(
        frame.kind,
        uc_core::pairing::DurableAdmissionMessageKind::Candidate
    );
    let requests = security.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].existing_recipients.is_empty());
    let payload =
        super::admission_transaction::DurableAdmissionCandidatePayloadV1::decode(&frame.payload)
            .unwrap();
    assert_eq!(payload.candidate.target_relationships.len(), 2);
    assert!(payload
        .candidate
        .target_relationships
        .iter()
        .all(|facts| facts.device_id.as_str() != "joiner"
            && facts.device_id.as_str() != "stale-removed-member"));
    assert_eq!(payload.candidate.resume_public_key, vec![0x49; 32]);
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

// 流程：加入方保存准入资料前尚缺发起者的完整历史；先拉取并验证连续历史，匹配后才完成加入。

// 流程：A 的旧实例已被 C 移除，随后 C 把 A 的新实例加入同一条已验证历史；A
// 以新实例拉取整条历史时直接采用最终分支，不为已废弃的旧实例产生待确认项。

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
            b"joiner-target-access",
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

    let profile = super::ProfileWorkspaceConvergence::new(
        Arc::clone(&repository),
        DeviceId::new("joiner"),
        Arc::new(UnusedClock),
    );
    let fresh_snapshot = profile.query_device_trust().await.unwrap();
    assert!(fresh_snapshot.revision > 0);
    assert!(fresh_snapshot.devices.is_empty());
    assert!(matches!(
        fresh_snapshot.current_join,
        Some(super::CurrentJoinStatus::Pending {
            join_id: projected_join_id,
            ..
        }) if projected_join_id == join_id
    ));

    let reopened = durable_admission_owner(Arc::clone(&repository));
    assert!(matches!(
        reopened.current_local_join().await.unwrap(),
        Some(super::CurrentJoinStatus::Pending {
            join_id: projected_join_id,
            target_space_id: None,
            sponsor_device_id: None,
            sponsor_identity_fingerprint: None,
            cancel_requested: false,
        }) if projected_join_id == join_id
    ));
    let second = reopened
        .start_join(
            attempt_id,
            join_id,
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    assert_eq!(second, first);
    assert!(matches!(
        reopened.cancel_local_join(join_id).await.unwrap(),
        super::CurrentJoinStatus::Pending {
            join_id: projected_join_id,
            cancel_requested: true,
            ..
        } if projected_join_id == join_id
    ));
    assert!(matches!(
        reopened.cancel_local_join([0xff; 16]).await,
        Err(WorkspaceConvergenceError::JoinNotFound)
    ));
    assert!(matches!(
        durable_admission_owner(repository)
            .current_local_join()
            .await
            .unwrap(),
        Some(super::CurrentJoinStatus::Pending {
            join_id: projected_join_id,
            cancel_requested: true,
            ..
        }) if projected_join_id == join_id
    ));
}

#[tokio::test]
async fn durable_join_reopens_the_exact_member_and_resume_material() {
    use super::admission_transaction::DurableJoinRecoveryMaterialV1;
    use uc_core::membership::AdmissionAttemptRepositoryPort;

    let directory = tempfile::tempdir().unwrap();
    let repository: Arc<dyn AdmissionAttemptRepositoryPort> =
        durable_admission_repository(&directory, [0x73; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x74; 32]);
    let material = DurableJoinRecoveryMaterialV1 {
        pending_security_state: b"private-join-state".to_vec(),
        candidate_key_package: b"public-key-package".to_vec(),
        member_instance: MemberInstanceId::from_bytes([0x75; 32]),
        resume_public_key: vec![0x76; 32],
        resume_private_key: vec![0x77; 32],
    };

    owner
        .start_join_with_recovery_material(
            attempt_id,
            [0x78; 16],
            b"sponsor",
            b"join-request",
            &material,
        )
        .await
        .unwrap();

    let reopened = durable_admission_owner(repository);
    assert_eq!(
        reopened
            .load_join_recovery_material(attempt_id)
            .await
            .unwrap(),
        material
    );
}

#[tokio::test]
async fn durable_join_preparation_is_not_regenerated_after_restart() {
    use uc_core::ports::space::GroupAdmissionPort;
    use uc_core::space_access::{GroupAdmission, PreparedGroupJoin};

    struct CountingPreparation {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl GroupAdmissionPort for CountingPreparation {
        async fn prepare_group_join(
            &self,
            _device_id: &DeviceId,
        ) -> Result<PreparedGroupJoin, uc_core::ports::space::SpaceAccessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(PreparedGroupJoin::new(
                b"stable-key-package".to_vec(),
                b"stable-private-state".to_vec(),
            )
            .with_member_instance(MemberInstanceId::from_bytes([0x79; 32])))
        }

        async fn admit_group_member(
            &self,
            _space_id: &SpaceId,
            _sponsor_device_id: &DeviceId,
            _joiner_device_id: &DeviceId,
            _existing_member_ids: &[DeviceId],
            _key_package: &[u8],
        ) -> Result<GroupAdmission, uc_core::ports::space::SpaceAccessError> {
            Err(uc_core::ports::space::SpaceAccessError::Internal(
                "unused".to_owned(),
            ))
        }

        async fn install_group_join(
            &self,
            _space_id: &SpaceId,
            _passphrase: &uc_core::crypto::domain::Passphrase,
            _pending: PreparedGroupJoin,
            _welcome: &[u8],
            _encrypted_key_catalog: &[u8],
            _group_epoch: u64,
        ) -> Result<(), uc_core::ports::space::SpaceAccessError> {
            Err(uc_core::ports::space::SpaceAccessError::Internal(
                "unused".to_owned(),
            ))
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x7a; 16]);
    let preparation = CountingPreparation {
        calls: AtomicUsize::new(0),
    };

    let first = durable_admission_owner(Arc::clone(&repository))
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"sponsor",
            b"join-request",
            false,
        )
        .await
        .unwrap();
    let second = durable_admission_owner(Arc::clone(&repository))
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"sponsor",
            b"join-request",
            false,
        )
        .await
        .unwrap();

    assert_eq!(preparation.calls.load(Ordering::SeqCst), 1);
    assert_eq!(first.attempt, second.attempt);
    assert!(!first.attempt.preserve_unreadable_history);
    assert!(matches!(
        durable_admission_owner(Arc::clone(&repository))
            .prepare_join_before_network(
                &preparation,
                &DeviceId::new("joiner"),
                b"sponsor",
                b"join-request",
                true,
            )
            .await,
        Err(WorkspaceConvergenceError::AdmissionInProgress)
    ));
    assert_eq!(
        first.prepared_group_join.key_package,
        second.prepared_group_join.key_package
    );
    assert_eq!(
        first.prepared_group_join.private_state(),
        second.prepared_group_join.private_state()
    );
    assert_eq!(
        first.prepared_group_join.member_instance(),
        second.prepared_group_join.member_instance()
    );

    let confirmed_directory = tempfile::tempdir().unwrap();
    let confirmed_repository = durable_admission_repository(&confirmed_directory, [0x7b; 16]);
    let confirmed = durable_admission_owner(confirmed_repository)
        .prepare_join_before_network(
            &preparation,
            &DeviceId::new("joiner"),
            b"other-sponsor",
            b"other-join-request",
            true,
        )
        .await
        .unwrap();
    assert!(confirmed.attempt.preserve_unreadable_history);
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
            b"joiner-target-access",
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
            b"joiner-target-access",
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
            b"joiner-target-access",
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
async fn profile_reset_preparation_rejects_pending_join_without_hiding_it() {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x6a; 16]);
    let owner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x6b; 32]);
    let join_id = [0x6c; 16];
    owner
        .start_join(
            attempt_id,
            join_id,
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let profile = super::ProfileWorkspaceConvergence::new(
        repository,
        DeviceId::new("joiner"),
        Arc::new(UnusedClock),
    );
    let before = profile.query_device_trust().await.unwrap();

    assert!(matches!(
        profile.prepare_reset_space().await,
        Err(WorkspaceConvergenceError::Unavailable)
    ));
    assert_eq!(profile.query_device_trust().await.unwrap(), before);
}

#[tokio::test]
async fn profile_device_trust_is_explicitly_unavailable_while_active_space_is_locked() {
    let directory = tempfile::tempdir().unwrap();
    let admission_repository = durable_admission_repository(&directory, [0x6d; 16]);
    let mut deps = test_deps(Arc::new(LockedWorkspaceRepository), "device-a", Vec::new());
    deps.admission_attempts = Arc::clone(&admission_repository)
        as Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>;
    let active = WorkspaceConvergence::new(deps);
    let profile = super::ProfileWorkspaceConvergence::new(
        admission_repository,
        DeviceId::new("device-a"),
        Arc::new(UnusedClock),
    );
    profile.attach_active(Some(active)).await;

    let snapshot = profile.query_device_trust().await.unwrap();

    assert_eq!(snapshot.local_device_id, DeviceId::new("device-a"));
    assert_eq!(
        snapshot.local_membership,
        super::DeviceMembership::Unavailable
    );
    assert!(snapshot.devices.is_empty());
    assert_eq!(
        snapshot.blocked_reason,
        Some(super::ActionUnavailableReason::EngineUnavailable)
    );
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
            b"joiner-target-access",
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
            b"joiner-target-access",
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
            b"joiner-target-access",
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
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (history, event, commitment) = admission_verification_fixture([0x37; 32]);
    let target_relationships = admission_relationships(&event);
    let identity_binding = admission_identity_binding(&event, &target_relationships);
    let candidate = super::admission_transaction::DurableAdmissionCandidateV1 {
        lineage_id: event.lineage_id.clone(),
        base_history_position: postcard::to_stdvec(&commitment.base_history_position).unwrap(),
        candidate_event: postcard::to_stdvec(&event).unwrap(),
        candidate_event_id: *event.event_id().as_bytes(),
        candidate_key_package: b"joiner-key-package".to_vec(),
        resume_public_key: vec![0x8d; 32],
        target_members_digest: event.resulting_members_digest,
        security_commitment: postcard::to_stdvec(&commitment).unwrap(),
        security_commit: b"sealed-security-commit".to_vec(),
        security_welcome: postcard::to_stdvec(&commitment).unwrap(),
        target_protection_group_id: "target-protection-group".to_owned(),
        target_key_catalog: admission_key_catalog().encode().unwrap(),
        target_relationships,
        existing_member_deliveries: Vec::new(),
        staged_security_state: b"sponsor-staged-state".to_vec(),
        identity_binding,
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
            b"joiner-target-access",
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
            b"joiner-target-access",
            b"verified-complete-history",
            None,
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
async fn durable_join_is_saved_before_the_target_space_is_known() {
    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0x3f; 16]);
    let repository = durable_admission_repository(&joiner_dir, [0x40; 16]);
    let sponsor = durable_admission_owner(sponsor_repository);
    let joiner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x41; 32]);

    let initiated = joiner
        .start_join_before_network(
            attempt_id,
            [0x42; 16],
            b"invitation-code",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();

    assert!(initiated.target_access_state.is_none());
    assert_eq!(initiated.outboxes.len(), 1);
    assert_eq!(
        repository.load(attempt_id).await.unwrap(),
        Some(initiated.clone())
    );

    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0x43; 32],
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
            b"joiner-target-access",
            b"verified-complete-history",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();

    assert_eq!(
        repository
            .load(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .target_access_state
            .as_deref(),
        Some(b"joiner-target-access".as_slice())
    );
    assert_eq!(
        prepared.purpose,
        uc_core::membership::AdmissionOutboxPurposeV1::Prepared
    );
}

#[tokio::test]
async fn durable_join_start_reuses_the_saved_wire_identity_after_restart() {
    let joiner_dir = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&joiner_dir, [0x44; 16]);
    let first_owner = durable_admission_owner(Arc::clone(&repository));

    let first = first_owner
        .begin_join_before_network(
            b"invitation-route",
            b"join-request-body",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    assert_ne!(first.attempt_id.as_bytes(), &[0; 32]);
    assert_ne!(first.join_id.unwrap(), [0; 16]);
    assert_ne!(first.outboxes[0].message_id, [0; 32]);

    let reopened_owner = durable_admission_owner(Arc::clone(&repository));
    let replay = reopened_owner
        .begin_join_before_network(
            b"invitation-route",
            b"join-request-body",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await
        .unwrap();
    assert_eq!(replay, first);

    let conflict = reopened_owner
        .begin_join_before_network(
            b"another-invitation-route",
            b"another-join-request-body",
            b"joiner-pending-state",
            b"joiner-key-package",
        )
        .await;
    assert!(matches!(
        conflict,
        Err(WorkspaceConvergenceError::AdmissionInProgress)
    ));
}

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
            b"joiner-target-access",
        )
        .await
        .unwrap();
    assert_eq!(
        initiated.target_access_state.as_deref(),
        Some(b"joiner-target-access".as_slice())
    );
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
            b"joiner-target-access",
            b"verified-complete-history",
            None,
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
            b"joiner-target-access",
            b"verified-complete-history",
            None,
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
    let replayed_commit = durable_admission_owner(Arc::clone(&sponsor_repository))
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
    let replayed_applied = durable_admission_owner(Arc::clone(&joiner_repository))
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
    let replayed_complete = durable_admission_owner(Arc::clone(&sponsor_repository))
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
    assert_eq!(
        sponsor_before_ack.terminal_result,
        Some(AdmissionTerminalResultV1::Completed)
    );
    assert_eq!(
        joiner
            .joiner_activate(attempt_id, &complete_message, b"admission-completion")
            .await
            .unwrap(),
        super::admission_transaction::JoinerActivationOutcomeV1::SpaceTransitionRequired
    );
    let restarted_joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    assert!(restarted_joiner
        .requires_session_transition()
        .await
        .unwrap());
    assert_eq!(
        restarted_joiner
            .recover_space_transitions_after_session_drain()
            .await
            .unwrap(),
        1
    );
    assert!(!restarted_joiner
        .requires_session_transition()
        .await
        .unwrap());
    let complete_ack = match restarted_joiner
        .joiner_activate(attempt_id, &complete_message, b"admission-completion")
        .await
        .unwrap()
    {
        super::admission_transaction::JoinerActivationOutcomeV1::Active(acknowledgment) => {
            acknowledgment
        }
        super::admission_transaction::JoinerActivationOutcomeV1::SpaceTransitionRequired => {
            panic!("completed activation must rebuild its acknowledgment")
        }
    };
    let replayed_ack = durable_admission_owner(Arc::clone(&joiner_repository))
        .joiner_activate(attempt_id, &complete_message, b"admission-completion")
        .await
        .unwrap();
    assert_eq!(
        replayed_ack,
        super::admission_transaction::JoinerActivationOutcomeV1::Active(complete_ack.clone())
    );
    assert!(joiner_repository.load(attempt_id).await.unwrap().is_none());
    let joiner_saved = joiner_repository
        .load_terminal(attempt_id)
        .await
        .unwrap()
        .unwrap();
    let joiner_history = joiner_repository
        .load_membership_history_v2()
        .await
        .unwrap()
        .unwrap();
    let verified_history: uc_core::membership::VersionedMembershipHistory =
        uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &joiner_history,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    assert_eq!(verified_history.effective_members().len(), 2);
    assert_eq!(
        joiner_saved.terminal_result,
        AdmissionTerminalResultV1::Active
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
        Some(postcard::to_stdvec(&activation_receipt).unwrap())
    );
    assert_eq!(sponsor_saved.completion, Some(joiner_saved.replay_result));
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
    assert!(matches!(
        durable_admission_owner(Arc::clone(&joiner_repository))
            .current_local_join()
            .await
            .unwrap(),
        Some(super::CurrentJoinStatus::Active {
            join_id: projected_join_id,
            joined_space,
        }) if projected_join_id == join_id
            && joined_space.sponsor_device_id.as_str() == "sponsor"
            && joined_space.space_id == candidate_event.lineage_id
            && joined_space.self_device_id.as_str() == "joiner"
            && joined_space.migrated_records.is_none()
            && joined_space.preserved_unreadable_records.is_none()
    ));
    assert_eq!(
        sponsor_repository
            .load_terminal(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        AdmissionTerminalResultV1::Completed
    );
    durable_admission_owner(Arc::clone(&sponsor_repository))
        .sponsor_confirm_active(attempt_id, &complete_ack)
        .await
        .unwrap();
}

#[tokio::test]
async fn out_of_order_durable_messages_leave_the_saved_stage_unchanged() {
    use uc_core::membership::AdmissionOutboxPurposeV1;

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0xc1; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0xc2; 16]);
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner(Arc::clone(&joiner_repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xc3; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0xc4; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, history, event, commitment, receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let fake_commit = super::admission_transaction::durable_admission_message(
        attempt_id,
        AdmissionOutboxPurposeV1::Commit,
        b"joiner",
        Some([0xc5; 32]),
        b"early-commit",
    );
    let fake_complete = super::admission_transaction::durable_admission_message(
        attempt_id,
        AdmissionOutboxPurposeV1::Complete,
        b"joiner",
        Some([0xc6; 32]),
        b"early-complete",
    );
    let joiner_before = joiner_repository.load(attempt_id).await.unwrap().unwrap();

    assert!(joiner
        .joiner_apply(attempt_id, &fake_commit, &receipt, b"sponsor", b"applied")
        .await
        .is_err());
    assert!(joiner
        .joiner_activate(attempt_id, &fake_complete, b"completion")
        .await
        .is_err());
    assert_eq!(
        joiner_repository.load(attempt_id).await.unwrap(),
        Some(joiner_before)
    );

    sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0xc7; 32],
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
    let sponsor_before = sponsor_repository.load(attempt_id).await.unwrap().unwrap();
    let fake_applied = super::admission_transaction::durable_admission_message(
        attempt_id,
        AdmissionOutboxPurposeV1::Applied,
        b"sponsor",
        Some([0xc8; 32]),
        b"early-applied",
    );

    assert!(sponsor
        .sponsor_complete(
            attempt_id,
            &fake_applied,
            &receipt,
            b"completion",
            b"joiner",
            b"complete",
        )
        .await
        .is_err());
    assert_eq!(
        sponsor_repository.load(attempt_id).await.unwrap(),
        Some(sponsor_before)
    );
}

#[tokio::test]
async fn cross_space_activation_saves_complete_before_forward_only_recovery() {
    use uc_core::membership::{
        AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2, AdmissionTerminalResultV1,
        CrossSpaceTransitionPhaseV2,
    };

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0xc5; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0xc6; 16]);
    let transition = Arc::new(SimulatedAdmissionSpaceTransition::new_with_phase_failures());
    let sponsor = durable_admission_owner(Arc::clone(&sponsor_repository));
    let joiner = durable_admission_owner_with_space_transition(
        Arc::clone(&joiner_repository),
        transition.clone(),
    );
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xc7; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0xc8; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0xc9; 32],
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
            b"joiner-target-access",
            b"prepared-proof",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    let prepared_attempt = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    let AdmissionSpaceTransitionV2::CrossSpace(prepared_transition) =
        AdmissionSpaceTransitionV2::decode(prepared_attempt.space_transition.as_deref().unwrap())
            .unwrap()
    else {
        panic!("expected a cross-space transition");
    };
    assert_eq!(
        prepared_transition.phase,
        CrossSpaceTransitionPhaseV2::TargetStaged
    );

    let commit = sponsor
        .sponsor_commit(
            attempt_id,
            &prepared,
            b"prepared-proof",
            b"joiner",
            b"commit",
        )
        .await
        .unwrap();
    let applied = joiner
        .joiner_apply(
            attempt_id,
            &commit,
            &activation_receipt,
            b"sponsor",
            b"applied",
        )
        .await
        .unwrap();
    let complete = sponsor
        .sponsor_complete(
            attempt_id,
            &applied,
            &activation_receipt,
            b"completion",
            b"joiner",
            b"completion",
        )
        .await
        .unwrap();

    assert!(matches!(
        joiner
            .joiner_activate(attempt_id, &complete, b"completion")
            .await
            .unwrap(),
        super::admission_transaction::JoinerActivationOutcomeV1::SpaceTransitionRequired
    ));
    assert!(transition.advances.lock().unwrap().is_empty());
    assert!(joiner.requires_session_transition().await.unwrap());
    let interrupted = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        interrupted.completion.as_deref(),
        Some(b"completion".as_slice())
    );
    assert_eq!(interrupted.terminal_result, None);
    assert_eq!(
        match AdmissionSpaceTransitionV2::decode(interrupted.space_transition.as_deref().unwrap(),)
            .unwrap()
        {
            AdmissionSpaceTransitionV2::CrossSpace(transition) => transition.phase,
            _ => panic!("expected a cross-space transition"),
        },
        CrossSpaceTransitionPhaseV2::TargetStaged
    );

    for expected_phase in [
        CrossSpaceTransitionPhaseV2::TargetStaged,
        CrossSpaceTransitionPhaseV2::ActivationStarted,
        CrossSpaceTransitionPhaseV2::SourceFinalized,
        CrossSpaceTransitionPhaseV2::DataRewrapped,
        CrossSpaceTransitionPhaseV2::TargetPromoted,
        CrossSpaceTransitionPhaseV2::CleanupPending,
    ] {
        assert!(joiner
            .recover_space_transitions_after_session_drain()
            .await
            .is_err());
        let saved = joiner_repository.load(attempt_id).await.unwrap().unwrap();
        assert_eq!(saved.terminal_result, None);
        assert_eq!(
            match AdmissionSpaceTransitionV2::decode(saved.space_transition.as_deref().unwrap(),)
                .unwrap()
            {
                AdmissionSpaceTransitionV2::CrossSpace(transition) => transition.phase,
                _ => panic!("expected a cross-space transition"),
            },
            expected_phase
        );
    }

    let transitions_finished = joiner
        .recover_space_transitions_after_session_drain()
        .await
        .unwrap();
    assert_eq!(transitions_finished, 1);
    assert!(!joiner.requires_session_transition().await.unwrap());
    let recovery = joiner
        .recover_with(&DeferredAdmissionDelivery)
        .await
        .unwrap();
    assert_eq!(recovery.deliveries_confirmed, 0);
    assert_eq!(recovery.attempts_compacted, 0);
    assert!(joiner_repository.load(attempt_id).await.unwrap().is_none());
    let active = joiner_repository
        .load_terminal(attempt_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.terminal_result, AdmissionTerminalResultV1::Active);
    let AdmissionSpaceTransitionResultV2::CrossSpace(result) =
        AdmissionSpaceTransitionResultV2::decode(
            active.space_transition_result.as_deref().unwrap(),
        )
        .unwrap()
    else {
        panic!("expected a cross-space result");
    };
    let acknowledgment = match joiner
        .joiner_activate(attempt_id, &complete, b"completion")
        .await
        .unwrap()
    {
        super::admission_transaction::JoinerActivationOutcomeV1::Active(acknowledgment) => {
            acknowledgment
        }
        super::admission_transaction::JoinerActivationOutcomeV1::SpaceTransitionRequired => {
            panic!("compacted active admission must rebuild its acknowledgment")
        }
    };
    assert!(active.acknowledgment_rebuild.contains(&acknowledgment));
    let profile = super::ProfileWorkspaceConvergence::new(
        Arc::clone(&joiner_repository),
        DeviceId::new("joiner"),
        Arc::new(UnusedClock),
    );
    let pending_ack = profile
        .pending_joiner_complete_ack()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending_ack.sponsor_device_id, DeviceId::new("sponsor"));
    assert_eq!(
        pending_ack.frame.kind,
        uc_core::pairing::DurableAdmissionMessageKind::CompleteAck
    );
    assert_eq!(
        pending_ack.frame.predecessor_message_id,
        Some(acknowledgment.message_id)
    );
    assert_eq!(result.migrated_records, 3);
    assert_eq!(result.preserved_unreadable_records, 1);
}

#[tokio::test]
async fn cross_space_rejection_discards_target_only_before_activation() {
    use uc_core::membership::{AdmissionSpaceTransitionV2, AdmissionTerminalResultV1};

    let sponsor_dir = tempfile::tempdir().unwrap();
    let joiner_dir = tempfile::tempdir().unwrap();
    let sponsor_repository = durable_admission_repository(&sponsor_dir, [0xd1; 16]);
    let joiner_repository = durable_admission_repository(&joiner_dir, [0xd2; 16]);
    let transition = Arc::new(SimulatedAdmissionSpaceTransition::new_with_phase_failures());
    let sponsor = durable_admission_owner(sponsor_repository);
    let joiner = durable_admission_owner_with_space_transition(
        Arc::clone(&joiner_repository),
        transition.clone(),
    );
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xd3; 32]);
    let initiated = joiner
        .start_join(
            attempt_id,
            [0xd4; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();
    let (candidate, base_history, candidate_event, commitment, _) =
        durable_candidate_verification_fixture(attempt_id);
    let offered = sponsor
        .sponsor_accept_and_offer(
            attempt_id,
            [0xd5; 32],
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
            b"joiner-target-access",
            b"prepared-proof",
            None,
            b"sponsor",
            b"prepared",
        )
        .await
        .unwrap();
    assert!(matches!(
        AdmissionSpaceTransitionV2::decode(
            joiner_repository
                .load(attempt_id)
                .await
                .unwrap()
                .unwrap()
                .space_transition
                .as_deref()
                .unwrap()
        ),
        Some(AdmissionSpaceTransitionV2::CrossSpace(_))
    ));

    let cancel = joiner
        .request_cancel(attempt_id, b"sponsor", b"cancel")
        .await
        .unwrap();
    let rejected = sponsor
        .sponsor_decide_cancel(attempt_id, &cancel, b"joiner", b"cancelled")
        .await
        .unwrap();
    let acknowledgment = joiner
        .joiner_record_rejected(attempt_id, &rejected)
        .await
        .unwrap();
    let saved = joiner_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        saved.terminal_result,
        Some(AdmissionTerminalResultV1::Rejected)
    );
    assert!(saved.space_transition.is_none());
    assert!(saved.space_transition_result.is_none());
    assert!(saved.inbox_dedup.contains(&acknowledgment));
    assert_eq!(transition.discards.load(Ordering::SeqCst), 1);
    assert_eq!(
        prepared.purpose,
        uc_core::membership::AdmissionOutboxPurposeV1::Prepared
    );
}

#[tokio::test]
async fn third_member_completion_keeps_joiner_pending_until_helper_applies_its_update() {
    use ed25519_dalek::SigningKey;
    use sha2::{Digest, Sha256};
    use uc_core::membership::{
        AdmissionActivationReceipt, AdmissionAttemptV1, AdmissionIdentityBindingV1,
        AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1, AdmissionTerminalResultV1,
        JoinerAdmissionStageV1, MembershipActivationBaselineV2, MembershipAdmissionV2,
        MembershipCredential, MembershipEventId, MembershipEventV2, MembershipOperationV2,
        SponsorAdmissionSecurityDelivery, VersionedMembershipHistory,
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
        MEMBERSHIP_EVENT_FORMAT_V2,
    };

    let verifier = DeterministicHistoricalVerifier;
    let sponsor_device = DeviceId::new("sponsor");
    let helper_device = DeviceId::new("helper");
    let joiner_device = DeviceId::new("joiner");
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xc1; 32]);
    let helper_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xc2; 32]);
    let joiner_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0xc3; 32]);
    let sponsor_instance = sponsor_credential.member_instance_id(&sponsor_device);
    let helper_instance = helper_credential.member_instance_id(&helper_device);
    let joiner_instance = joiner_credential.member_instance_id(&joiner_device);
    let mut sponsor_facts = admission_facts_for(sponsor_instance, &sponsor_device);
    sponsor_facts.identity_signature =
        verifier.sign(&sponsor_credential, &sponsor_facts.signing_payload());
    let mut helper_facts = admission_facts_for(helper_instance, &helper_device);
    helper_facts.transport_public_key = vec![0x35; 32];
    helper_facts.transport_address_blob = b"helper-recovery-route".to_vec();
    helper_facts.identity_signature =
        verifier.sign(&helper_credential, &helper_facts.signing_payload());
    let base_head = MembershipEventId::from_hex(&"c4".repeat(32)).unwrap();
    let base_history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::FullyVerifiedMigration {
            lineage_id: SPACE.to_owned(),
            head_event_id: base_head,
            head_depth: 4,
            current_members: vec![
                (sponsor_facts.clone(), sponsor_credential.clone()),
                (helper_facts.clone(), helper_credential.clone()),
            ],
        },
    )
    .unwrap();
    let base_position = base_history.current_position().unwrap();
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xc5; 32]);
    let resume_private = [0xc6; 32];
    let resume_public = SigningKey::from_bytes(&resume_private)
        .verifying_key()
        .to_bytes()
        .to_vec();
    let key_catalog = admission_key_catalog();
    let commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        SPACE.to_owned(),
        SPACE.as_bytes().to_vec(),
        *attempt_id.as_bytes(),
        base_position.clone(),
        [0xc7; 32],
        1,
        3,
        4,
        [0xc8; 32],
        [0xc9; 32],
        [0xca; 32],
        key_catalog.digest(),
        [0xcb; 32],
    )
    .unwrap();
    let mut joiner_facts = admission_facts_for(joiner_instance, &joiner_device);
    joiner_facts.transport_public_key = vec![0x36; 32];
    joiner_facts.identity_signature =
        verifier.sign(&joiner_credential, &joiner_facts.signing_payload());
    let operation = MembershipOperationV2::AddDevice {
        admission: MembershipAdmissionV2 {
            facts: joiner_facts.clone(),
            membership_credential: joiner_credential.clone(),
            resume_public_key_digest: super::admission_resume_public_key_digest(&resume_public),
            security_commitment_id: commitment.security_commitment_id,
        },
    };
    let resulting_members_digest = base_history
        .expected_resulting_members_digest(Some(base_head), &operation)
        .unwrap();
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        SPACE.to_owned(),
        Some(base_head),
        5,
        [0xcc; 16],
        sponsor_instance,
        sponsor_credential.credential_id,
        sponsor_credential.signature_algorithm_version,
        operation,
        resulting_members_digest,
        [0xcd; 32],
        vec![0xce],
        Some(commitment.admission_bundle_digest),
        Vec::new(),
    );
    event.signature = verifier.sign(&sponsor_credential, &event.signing_payload());
    let mut completed_history = base_history.clone();
    completed_history
        .verify_and_receive_event(event.clone(), &verifier)
        .unwrap();
    let mut receipt = AdmissionActivationReceipt::new(
        1,
        *attempt_id.as_bytes(),
        event.event_id(),
        event.resulting_members_digest,
        commitment.security_commitment_id,
        joiner_instance,
        Vec::new(),
    );
    receipt.signature = verifier.sign(&joiner_credential, &receipt.signing_payload());
    completed_history
        .verify_and_record_activation_receipt(receipt.clone(), &verifier)
        .unwrap();
    let base_history_bytes = base_history.encode_persisted_v2().unwrap();
    let completed_history_bytes = completed_history.encode_persisted_v2().unwrap();
    let event_bytes = postcard::to_stdvec(&event).unwrap();
    let commitment_bytes = postcard::to_stdvec(&commitment).unwrap();
    let receipt_bytes = postcard::to_stdvec(&receipt).unwrap();
    let delivery = SponsorAdmissionSecurityDelivery {
        recipient: helper_device.clone(),
        credential_id: helper_credential.credential_id,
        payload: b"helper-security-update".to_vec(),
    };

    let joiner_directory = tempfile::tempdir().unwrap();
    let joiner_repository = durable_admission_repository(&joiner_directory, [0xcf; 16]);
    let mut joiner_attempt =
        AdmissionAttemptV1::new_joiner(attempt_id, [0xd0; 16], JoinerAdmissionStageV1::Applied);
    joiner_attempt.local_join_ordinal = Some(0);
    joiner_attempt.lineage_id = Some(SPACE.to_owned());
    joiner_attempt.base_history_position = Some(postcard::to_stdvec(&base_position).unwrap());
    joiner_attempt.candidate_event = Some(event_bytes.clone());
    joiner_attempt.candidate_event_id = Some(*event.event_id().as_bytes());
    joiner_attempt.candidate_key_package = Some(b"joiner-key-package".to_vec());
    joiner_attempt.target_members_digest = Some(resulting_members_digest);
    joiner_attempt.security_commitment = Some(commitment_bytes.clone());
    joiner_attempt.security_commit = Some(b"security-commit".to_vec());
    joiner_attempt.security_welcome = Some(b"security-welcome".to_vec());
    joiner_attempt.target_protection_group_id = Some("target-protection-group".to_owned());
    joiner_attempt.target_key_catalog = Some(key_catalog.encode().unwrap());
    joiner_attempt.target_relationships = Some(vec![
        sponsor_facts.clone(),
        helper_facts.clone(),
        joiner_facts.clone(),
    ]);
    joiner_attempt.existing_member_security_deliveries = Some(vec![delivery]);
    joiner_attempt.staged_security_state = Some(b"joiner-staged-security".to_vec());
    joiner_attempt.joiner_pending_security_state = Some(b"joiner-pending-security".to_vec());
    joiner_attempt.base_membership_history = Some(base_history_bytes);
    joiner_attempt.verified_membership_history = Some(completed_history_bytes.clone());
    joiner_attempt.prepared_proof = Some(b"prepared-proof".to_vec());
    joiner_attempt.activation_receipt = Some(receipt_bytes);
    joiner_attempt.resume_public_key = Some(resume_public.clone());
    joiner_attempt.resume_private_key = Some(resume_private.to_vec());
    joiner_attempt.target_access_state = Some(b"target-access".to_vec());
    joiner_attempt.identity_binding = Some(
        AdmissionIdentityBindingV1::new(
            SPACE.to_owned(),
            event.event_id(),
            &sponsor_facts,
            &joiner_facts,
        )
        .unwrap()
        .encode()
        .unwrap(),
    );
    joiner_attempt
        .outboxes
        .push(super::admission_transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Applied,
            b"sponsor",
            Some([0xd1; 32]),
            b"applied",
        ));
    joiner_repository
        .create(&joiner_attempt, None, Some(&completed_history_bytes))
        .await
        .unwrap();

    let helper_directory = tempfile::tempdir().unwrap();
    let helper_repository = durable_admission_repository(&helper_directory, [0xd2; 16]);
    helper_repository
        .compare_and_replace_membership_history_v2(None, &completed_history_bytes)
        .await
        .unwrap();

    let mut joiner_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "joiner",
        Vec::new(),
    );
    joiner_deps.admission_attempts = Arc::clone(&joiner_repository);
    joiner_deps.historical_membership_signatures = Arc::new(DeterministicHistoricalVerifier);
    let joiner = WorkspaceConvergence::new(joiner_deps);

    let mut blocked_helper_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "helper",
        Vec::new(),
    );
    blocked_helper_deps.admission_attempts = Arc::clone(&helper_repository);
    blocked_helper_deps.historical_membership_signatures =
        Arc::new(DeterministicHistoricalVerifier);
    blocked_helper_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: helper_device.clone(),
        credential: helper_credential.clone(),
    });
    blocked_helper_deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: helper_device.clone(),
    });
    let blocked_helper = WorkspaceConvergence::new(blocked_helper_deps);

    let hello = joiner
        .prepare_completion_recovery_hello(*attempt_id.as_bytes(), helper_instance)
        .await
        .unwrap();
    let transport_binding = uc_core::membership::AdmissionCompletionRecoveryTransportBindingV1 {
        joiner_transport_identity_digest: Sha256::digest(&joiner_facts.transport_public_key).into(),
        helper_transport_identity_digest: Sha256::digest(&helper_facts.transport_public_key).into(),
    };
    let joiner_applied_message_id = joiner_repository
        .load(attempt_id)
        .await
        .unwrap()
        .unwrap()
        .outboxes
        .iter()
        .find(|message| message.purpose == AdmissionOutboxPurposeV1::Applied)
        .unwrap()
        .message_id;
    let mut changed_transport_binding = transport_binding;
    changed_transport_binding.helper_transport_identity_digest = [0xff; 32];
    assert!(blocked_helper
        .challenge_completion_recovery(
            &hello,
            changed_transport_binding,
            joiner_applied_message_id,
            [0xd4; 32],
        )
        .await
        .is_err());
    let challenge = blocked_helper
        .challenge_completion_recovery(
            &hello,
            transport_binding,
            joiner_applied_message_id,
            [0xd4; 32],
        )
        .await
        .unwrap();
    let response = joiner
        .respond_to_completion_recovery(&hello, &challenge)
        .await
        .unwrap();

    assert!(blocked_helper
        .complete_recovered_admission(&hello, &response)
        .await
        .is_err());
    let blocked = helper_repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(blocked.stage_rank(), Some(5));
    assert_eq!(blocked.terminal_result, None);

    let helper_activation = Arc::new(RecordingSponsorAdmissionSecurity::default());
    let mut resumed_helper_deps = test_deps(
        Arc::new(MemoryWorkspaceRepository::default()),
        "helper",
        Vec::new(),
    );
    resumed_helper_deps.admission_attempts = Arc::clone(&helper_repository);
    resumed_helper_deps.historical_membership_signatures =
        Arc::new(DeterministicHistoricalVerifier);
    resumed_helper_deps.member_signatures = Arc::new(CredentialBackedSigner {
        device_id: helper_device.clone(),
        credential: helper_credential,
    });
    resumed_helper_deps.announcement_material = Arc::new(ConfiguredAnnouncementMaterial {
        device_id: helper_device,
    });
    resumed_helper_deps.activate_completion_helper_admission_security = helper_activation.clone();
    let resumed_helper = WorkspaceConvergence::new(resumed_helper_deps);
    let complete = resumed_helper
        .complete_recovered_admission(&hello, &response)
        .await
        .unwrap();
    let replayed_complete = resumed_helper
        .complete_recovered_admission(&hello, &response)
        .await
        .unwrap();
    assert_eq!(replayed_complete, complete);
    assert_eq!(
        helper_activation
            .helper_activation_requests
            .lock()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        helper_repository
            .load(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        Some(AdmissionTerminalResultV1::Completed)
    );

    assert!(matches!(
        joiner.activate_joiner_complete(&complete).await.unwrap(),
        crate::space::admission::adapter::DurableJoinerCompletion::Active(_)
    ));
    assert_eq!(
        joiner_repository
            .load_terminal(attempt_id)
            .await
            .unwrap()
            .unwrap()
            .terminal_result,
        AdmissionTerminalResultV1::Active
    );
}

#[tokio::test]
async fn explicit_sponsor_rejection_ends_a_join_before_candidate() {
    use uc_core::membership::{AdmissionRejectionReasonV1, AdmissionTerminalResultV1};

    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0x4d; 16]);
    let joiner = durable_admission_owner(Arc::clone(&repository));
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x4e; 32]);
    joiner
        .start_join(
            attempt_id,
            [0x4f; 16],
            b"sponsor",
            b"join-request",
            b"joiner-pending-state",
            b"joiner-key-package",
            b"joiner-target-access",
        )
        .await
        .unwrap();

    joiner
        .joiner_reject_before_candidate(attempt_id, AdmissionRejectionReasonV1::HistoryConflict)
        .await
        .unwrap();
    joiner
        .joiner_reject_before_candidate(attempt_id, AdmissionRejectionReasonV1::HistoryConflict)
        .await
        .unwrap();

    let rejected = repository.load(attempt_id).await.unwrap().unwrap();
    assert_eq!(
        rejected.terminal_result,
        Some(AdmissionTerminalResultV1::Rejected)
    );
    assert_eq!(
        rejected.rejection_reason,
        Some(AdmissionRejectionReasonV1::HistoryConflict)
    );
    assert!(rejected.joiner_pending_security_state.is_none());
    assert!(rejected.outboxes.iter().all(|message| message.superseded));
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
                b"joiner-target-access",
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
                b"joiner-target-access",
                b"verified-complete-history",
                None,
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
            b"joiner-target-access",
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
            b"joiner-target-access",
            b"verified-complete-history",
            None,
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
            b"joiner-target-access",
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
            b"joiner-target-access",
            b"verified-complete-history",
            None,
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
            b"joiner-target-access",
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
            b"joiner-target-access",
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
            b"joiner-target-access",
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
            b"joiner-target-access",
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
            b"joiner-target-access",
            b"verified-complete-history",
            None,
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
                b"joiner-target-access",
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
                b"joiner-target-access",
                b"verified-complete-history",
                None,
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
                b"joiner-target-access",
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
                b"joiner-target-access",
                b"verified-complete-history",
                None,
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
                stage: SponsorAdmissionStageV1::Applied | SponsorAdmissionStageV1::Completed,
            }) => {
                assert_eq!(history.effective_members().len(), 2);
                assert_eq!(history.active_members().len(), 2);
            }
            other => panic!("unexpected activation/removal race result: {other:?}"),
        }
    }
}

struct DeterministicHistoricalVerifier;

struct NoAdmissionSpaceTransition;

#[async_trait]
impl uc_core::membership::AdmissionSpaceTransitionPort for NoAdmissionSpaceTransition {
    async fn prepare_if_needed(
        &self,
        input: &uc_core::membership::AdmissionSpaceTransitionPreparationV2,
    ) -> Result<
        uc_core::membership::AdmissionSpaceTransitionV2,
        uc_core::membership::AdmissionSpaceTransitionError,
    > {
        Ok(uc_core::membership::AdmissionSpaceTransitionV2::Fresh(
            uc_core::membership::FreshSpaceTransitionV1 {
                transition_format_version: uc_core::membership::FRESH_SPACE_TRANSITION_FORMAT_V1,
                attempt_id: input.attempt_id,
                target_space_id: input.target_space_id.clone(),
                target_generation: [0xa1; 16],
                target_keyslot_ref: b"test-keyslot".to_vec(),
                target_workspace_ref: b"test-workspace".to_vec(),
                phase: uc_core::membership::FreshSpaceTransitionPhaseV1::TargetStaged,
            },
        ))
    }

    async fn advance(
        &self,
        transition: &uc_core::membership::AdmissionSpaceTransitionV2,
    ) -> Result<
        uc_core::membership::AdmissionSpaceTransitionStepV2,
        uc_core::membership::AdmissionSpaceTransitionError,
    > {
        use uc_core::membership::{
            AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionStepV2,
            AdmissionSpaceTransitionV2, FreshSpaceTransitionPhaseV1,
        };
        let AdmissionSpaceTransitionV2::Fresh(fresh) = transition else {
            return Err(uc_core::membership::AdmissionSpaceTransitionError::Inconsistent);
        };
        let next_phase = match fresh.phase {
            FreshSpaceTransitionPhaseV1::TargetStaged => {
                FreshSpaceTransitionPhaseV1::ActivationStarted
            }
            FreshSpaceTransitionPhaseV1::ActivationStarted => {
                FreshSpaceTransitionPhaseV1::TargetPromoted
            }
            FreshSpaceTransitionPhaseV1::TargetPromoted => {
                FreshSpaceTransitionPhaseV1::CleanupPending
            }
            FreshSpaceTransitionPhaseV1::CleanupPending => {
                return Ok(AdmissionSpaceTransitionStepV2::Finished(
                    AdmissionSpaceTransitionResultV2::Fresh {
                        target_space_id: fresh.target_space_id.clone(),
                    },
                ));
            }
        };
        let mut next = fresh.clone();
        next.phase = next_phase;
        Ok(AdmissionSpaceTransitionStepV2::Advanced(
            AdmissionSpaceTransitionV2::Fresh(next),
        ))
    }

    async fn discard_pre_activation(
        &self,
        _transition: &uc_core::membership::AdmissionSpaceTransitionV2,
    ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
        Ok(())
    }
}

struct SimulatedAdmissionSpaceTransition {
    fail_once_at: Mutex<VecDeque<uc_core::membership::CrossSpaceTransitionPhaseV2>>,
    advances: Mutex<Vec<uc_core::membership::CrossSpaceTransitionPhaseV2>>,
    discards: AtomicUsize,
}

impl SimulatedAdmissionSpaceTransition {
    fn new_with_phase_failures() -> Self {
        use uc_core::membership::CrossSpaceTransitionPhaseV2::*;
        Self {
            fail_once_at: Mutex::new(VecDeque::from([
                TargetStaged,
                ActivationStarted,
                SourceFinalized,
                DataRewrapped,
                TargetPromoted,
                CleanupPending,
            ])),
            advances: Mutex::new(Vec::new()),
            discards: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl uc_core::membership::AdmissionSpaceTransitionPort for SimulatedAdmissionSpaceTransition {
    async fn prepare_if_needed(
        &self,
        input: &uc_core::membership::AdmissionSpaceTransitionPreparationV2,
    ) -> Result<
        uc_core::membership::AdmissionSpaceTransitionV2,
        uc_core::membership::AdmissionSpaceTransitionError,
    > {
        assert_eq!(input.target_access_state, b"joiner-target-access");
        Ok(uc_core::membership::AdmissionSpaceTransitionV2::CrossSpace(
            uc_core::membership::CrossSpaceTransitionV2 {
                transition_format_version: uc_core::membership::CROSS_SPACE_TRANSITION_FORMAT_V2,
                attempt_id: input.attempt_id,
                source_space_id: "source-space".to_owned(),
                source_generation: [0xc1; 16],
                source_backup_ref: b"source-backup".to_vec(),
                source_backup_digest: [0xc2; 32],
                source_revision_at_backup: 7,
                target_space_id: input.target_space_id.clone(),
                target_generation: [0xc3; 16],
                target_keyslot_ref: b"target-keyslot".to_vec(),
                target_workspace_ref: b"target-workspace".to_vec(),
                phase: uc_core::membership::CrossSpaceTransitionPhaseV2::TargetStaged,
                final_source_revision: None,
                final_manifest_digest: None,
                migrated_records: 0,
                preserved_unreadable_records: 0,
                preserve_unreadable_history: input.preserve_unreadable_history,
            },
        ))
    }

    async fn advance(
        &self,
        transition: &uc_core::membership::AdmissionSpaceTransitionV2,
    ) -> Result<
        uc_core::membership::AdmissionSpaceTransitionStepV2,
        uc_core::membership::AdmissionSpaceTransitionError,
    > {
        use uc_core::membership::{
            AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionStepV2,
            AdmissionSpaceTransitionV2, CrossSpaceTransitionPhaseV2, CrossSpaceTransitionResultV2,
        };
        let AdmissionSpaceTransitionV2::CrossSpace(transition) = transition else {
            return Err(uc_core::membership::AdmissionSpaceTransitionError::Inconsistent);
        };
        self.advances.lock().unwrap().push(transition.phase);
        let should_fail =
            self.fail_once_at.lock().unwrap().front().copied() == Some(transition.phase);
        if should_fail {
            self.fail_once_at.lock().unwrap().pop_front();
            return Err(uc_core::membership::AdmissionSpaceTransitionError::Storage);
        }
        if transition.phase == CrossSpaceTransitionPhaseV2::CleanupPending {
            let result = CrossSpaceTransitionResultV2::from_cleanup_pending(transition)
                .ok_or(uc_core::membership::AdmissionSpaceTransitionError::Inconsistent)?;
            return Ok(AdmissionSpaceTransitionStepV2::Finished(
                AdmissionSpaceTransitionResultV2::CrossSpace(result),
            ));
        }
        let mut next = transition.clone();
        next.phase = transition
            .phase
            .successor()
            .ok_or(uc_core::membership::AdmissionSpaceTransitionError::Inconsistent)?;
        if next.phase == CrossSpaceTransitionPhaseV2::SourceFinalized {
            next.final_source_revision = Some(9);
            next.final_manifest_digest = Some([0xc4; 32]);
        }
        if next.phase == CrossSpaceTransitionPhaseV2::DataRewrapped {
            next.migrated_records = 3;
            next.preserved_unreadable_records = 1;
        }
        Ok(AdmissionSpaceTransitionStepV2::Advanced(
            AdmissionSpaceTransitionV2::CrossSpace(next),
        ))
    }

    async fn discard_pre_activation(
        &self,
        transition: &uc_core::membership::AdmissionSpaceTransitionV2,
    ) -> Result<(), uc_core::membership::AdmissionSpaceTransitionError> {
        let uc_core::membership::AdmissionSpaceTransitionV2::CrossSpace(transition) = transition
        else {
            return Err(uc_core::membership::AdmissionSpaceTransitionError::Inconsistent);
        };
        if transition.phase.rank()
            >= uc_core::membership::CrossSpaceTransitionPhaseV2::ActivationStarted.rank()
        {
            return Err(uc_core::membership::AdmissionSpaceTransitionError::Inconsistent);
        }
        self.discards.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

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
    admission_verification_fixture_for_lineage(attempt_id, "space-a")
}

fn admission_verification_fixture_for_lineage(
    attempt_id: [u8; 32],
    lineage_id: &str,
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
    let mut sponsor_facts = admission_facts_for(sponsor_member, &sponsor_device);
    sponsor_facts.identity_signature =
        verifier.sign(&sponsor_credential, &sponsor_facts.signing_payload());
    let base_head = MembershipEventId::from_hex(&"82".repeat(32)).unwrap();
    let history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::FullyVerifiedMigration {
            lineage_id: lineage_id.to_owned(),
            head_event_id: base_head,
            head_depth: 7,
            current_members: vec![(sponsor_facts, sponsor_credential.clone())],
        },
    )
    .unwrap();
    let base_position = BaseMembershipHistoryPositionV1 {
        event_id: Some(base_head),
        depth: 7,
        history_digest: [0x83; 32],
    };
    let key_catalog = admission_key_catalog();
    let commitment = AdmissionSecurityCommitmentV1::new(
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
        lineage_id.to_owned(),
        lineage_id.as_bytes().to_vec(),
        attempt_id,
        base_position.clone(),
        [0x85; 32],
        1,
        3,
        4,
        [0x86; 32],
        [0x87; 32],
        [0x88; 32],
        key_catalog.digest(),
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
        lineage_id.to_owned(),
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

async fn seed_v2_scope_history(
    repository: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    record_receipt: bool,
    pending_local_join: bool,
) {
    seed_v2_scope_history_for_lineage(repository, SPACE, record_receipt, pending_local_join, None)
        .await;
}

async fn seed_v2_scope_history_for_lineage(
    repository: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    lineage_id: &str,
    record_receipt: bool,
    pending_local_join: bool,
    cross_space_source: Option<&str>,
) {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, AdmissionAttemptV1, JoinerAdmissionStageV1,
        SponsorAdmissionStageV1, SponsorAdmissionStateV1,
    };

    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0x75; 32]);
    let (mut history, event, commitment) =
        admission_verification_fixture_for_lineage(*attempt_id.as_bytes(), lineage_id);
    let uc_core::membership::MembershipOperationV2::AddDevice { admission } = &event.operation
    else {
        unreachable!("scope fixture always creates AddDevice")
    };
    history
        .verify_and_receive_event(event.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    if record_receipt {
        let mut receipt = uc_core::membership::AdmissionActivationReceipt::new(
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
        history
            .verify_and_record_activation_receipt(receipt, &DeterministicHistoricalVerifier)
            .unwrap();
    }
    let encoded = history.encode_persisted_v2().unwrap();
    let mut attempt =
        AdmissionAttemptV1::new_joiner(attempt_id, [0x76; 16], JoinerAdmissionStageV1::Initiated);
    if pending_local_join {
        attempt.local_join_ordinal = Some(0);
        if let Some(source_space_id) = cross_space_source {
            attempt.role_state = uc_core::membership::AdmissionAttemptRoleStateV1::Joiner(
                uc_core::membership::JoinerAdmissionStateV1 {
                    stage: uc_core::membership::JoinerAdmissionStageV1::Prepared,
                },
            );
            attempt.lineage_id = Some(lineage_id.to_owned());
            attempt.base_history_position = Some(b"base-position".to_vec());
            attempt.candidate_event = Some(b"candidate-event".to_vec());
            attempt.candidate_event_id = Some(*event.event_id().as_bytes());
            attempt.candidate_key_package = Some(b"candidate-key-package".to_vec());
            attempt.target_members_digest = Some(event.resulting_members_digest);
            attempt.security_commitment = Some(b"security-commitment".to_vec());
            attempt.security_commit = Some(b"security-commit".to_vec());
            attempt.security_welcome = Some(b"security-welcome".to_vec());
            attempt.target_protection_group_id = Some("target-group".to_owned());
            attempt.target_key_catalog = Some(b"target-key-catalog".to_vec());
            attempt.target_relationships = Some(Vec::new());
            attempt.existing_member_security_deliveries = Some(Vec::new());
            attempt.staged_security_state = Some(b"staged-security".to_vec());
            attempt.base_membership_history = Some(encoded.clone());
            attempt.verified_membership_history = Some(encoded.clone());
            attempt.prepared_proof = Some(b"prepared-proof".to_vec());
            attempt.target_access_state = Some(b"target-access".to_vec());
            attempt.space_transition = uc_core::membership::AdmissionSpaceTransitionV2::CrossSpace(
                uc_core::membership::CrossSpaceTransitionV2 {
                    transition_format_version:
                        uc_core::membership::CROSS_SPACE_TRANSITION_FORMAT_V2,
                    attempt_id,
                    source_space_id: source_space_id.to_owned(),
                    source_generation: [0x7d; 16],
                    source_backup_ref: b"source-backup".to_vec(),
                    source_backup_digest: [0x7e; 32],
                    source_revision_at_backup: 1,
                    target_space_id: lineage_id.to_owned(),
                    target_generation: [0x7f; 16],
                    target_keyslot_ref: b"target-keyslot".to_vec(),
                    target_workspace_ref: b"target-workspace".to_vec(),
                    phase: uc_core::membership::CrossSpaceTransitionPhaseV2::TargetStaged,
                    final_source_revision: None,
                    final_manifest_digest: None,
                    migrated_records: 0,
                    preserved_unreadable_records: 0,
                    preserve_unreadable_history: false,
                },
            )
            .encode();
        }
    } else {
        attempt.join_id = None;
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Accepted,
        });
        attempt.invitation_claim = Some(b"scope-invitation".to_vec());
    }
    repository
        .create(&attempt, None, Some(&encoded))
        .await
        .unwrap();
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
    let target_relationships = admission_relationships(&event);
    let identity_binding = admission_identity_binding(&event, &target_relationships);
    let candidate = super::admission_transaction::DurableAdmissionCandidateV1 {
        lineage_id: event.lineage_id.clone(),
        base_history_position: postcard::to_stdvec(&commitment.base_history_position).unwrap(),
        candidate_event: postcard::to_stdvec(&event).unwrap(),
        candidate_event_id: *event.event_id().as_bytes(),
        candidate_key_package: b"joiner-key-package".to_vec(),
        resume_public_key: vec![0x8d; 32],
        target_members_digest: event.resulting_members_digest,
        security_commitment: postcard::to_stdvec(&commitment).unwrap(),
        security_commit: b"sealed-security-commit".to_vec(),
        security_welcome: postcard::to_stdvec(&commitment).unwrap(),
        target_protection_group_id: "target-protection-group".to_owned(),
        target_key_catalog: admission_key_catalog().encode().unwrap(),
        target_relationships,
        existing_member_deliveries: Vec::new(),
        staged_security_state: b"sponsor-staged-state".to_vec(),
        identity_binding,
    };
    (candidate, history, event, commitment, receipt)
}

fn admission_key_catalog() -> uc_core::membership::AdmissionContentKeyCatalogV1 {
    uc_core::membership::AdmissionContentKeyCatalogV1::new(
        "content-4",
        4,
        vec![
            uc_core::membership::AdmissionContentKeyEntryV1::new("legacy-v1", 0, vec![0x91; 32])
                .unwrap(),
            uc_core::membership::AdmissionContentKeyEntryV1::new("content-4", 4, vec![0x92; 32])
                .unwrap(),
        ],
    )
    .unwrap()
}

fn admission_relationships(
    event: &uc_core::membership::MembershipEventV2,
) -> Vec<uc_core::membership::AdmissionChangeFacts> {
    use uc_core::membership::{
        AdmissionChangeFacts, MembershipCredential, MembershipOperationV2,
        ED25519_SIGNATURE_ALGORITHM_V1,
    };
    let sponsor_device = DeviceId::new("sponsor");
    let sponsor_credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x81; 32]);
    let sponsor = AdmissionChangeFacts {
        member_instance: sponsor_credential.member_instance_id(&sponsor_device),
        device_id: sponsor_device,
        device_name: "sponsor".to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
            "QRST-UVWX-YZ23-4567",
        )
        .unwrap(),
        transport_public_key: vec![4],
        transport_address_blob: vec![5],
        identity_signature: vec![6],
    };
    let MembershipOperationV2::AddDevice { admission } = &event.operation else {
        unreachable!("fixture always creates AddDevice")
    };
    vec![sponsor, admission.facts.clone()]
}

fn admission_identity_binding(
    event: &uc_core::membership::MembershipEventV2,
    relationships: &[uc_core::membership::AdmissionChangeFacts],
) -> Vec<u8> {
    let sponsor = relationships
        .iter()
        .find(|facts| facts.member_instance == event.author_member_instance_id)
        .unwrap();
    let uc_core::membership::MembershipOperationV2::AddDevice { admission } = &event.operation
    else {
        unreachable!("fixture always creates AddDevice")
    };
    uc_core::membership::AdmissionIdentityBindingV1::new(
        event.lineage_id.clone(),
        event.event_id(),
        sponsor,
        &admission.facts,
    )
    .unwrap()
    .encode()
    .unwrap()
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
