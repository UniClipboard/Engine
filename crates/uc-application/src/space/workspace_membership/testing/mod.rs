//! WorkspaceMembership owner tests (ADR-016 flow semantics).

#[path = "../../admission/durable/tests.rs"]
mod admission_tests;
#[path = "../membership/tests.rs"]
mod membership_tests;
#[path = "../projection/tests.rs"]
mod projection_tests;

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
    CurrentWorkspacePeerScopePort, MemberInstanceId, MemberProtection, MemberProtectionStatus,
    MemberRepositoryPort, MembershipAdmissionDecision, MembershipAdmissionGatePort,
    MembershipHistoryMessage, MembershipHistoryV2Ack, MembershipOperation,
    MembershipReconciliation, MembershipSecurityUpdateError, MembershipSecurityUpdatePort,
    RemovalDecision, SpaceMembershipState, SpaceProtectionError, SpaceProtectionMode,
    SpaceProtectionSnapshot, SpaceProtectionStatusPort, WorkspaceConvergenceEvent,
};
use uc_core::ports::{ClockPort, DeviceIdentityPort};
use uc_core::ports::{PresenceError, PresenceEvent, PresencePort, ReachabilityState};

use super::*;
use crate::space::membership_state::{
    SpaceMembershipStateRepositoryError, SpaceMembershipStateRepositoryPort,
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

struct DeferredAdmissionDelivery;
struct UnusedAdmissionCompletionRecovery;

struct ConfirmingAdmissionDelivery;

struct BlockingLegacyMigrationRecovery {
    started: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl uc_core::ports::setup::LegacyMigrationRecoveryPort for BlockingLegacyMigrationRecovery {
    async fn recover(&self) -> Result<(), uc_core::ports::setup::LegacyMigrationRecoveryError> {
        self.started.notify_one();
        std::future::pending().await
    }
}

struct LoopbackHistoryExchange {
    receiver: Arc<WorkspaceMembership>,
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
        _route: Option<&uc_core::membership::AdmissionOutboxDeliveryRouteV1>,
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
        attempt_id: uc_core::membership::AdmissionAttemptId,
        message: &uc_core::membership::AdmissionOutboxMessageV1,
        _route: Option<&uc_core::membership::AdmissionOutboxDeliveryRouteV1>,
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
        if message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::CancelRequested {
            return Ok(
                uc_core::membership::AdmissionOutboxDeliveryResultV1::Rejected(
                    super::admission::durable_admission_message(
                        attempt_id,
                        uc_core::membership::AdmissionOutboxPurposeV1::Rejected,
                        &message.recipient,
                        Some(message.message_id),
                        &postcard::to_stdvec(&(
                            uc_core::membership::AdmissionRejectionReasonV1::Cancelled,
                            b"cancelled".to_vec(),
                        ))
                        .unwrap(),
                    ),
                ),
            );
        }
        Ok(
            uc_core::membership::AdmissionOutboxDeliveryResultV1::Persisted(
                super::admission::admission_acknowledgment(message),
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
    state: Arc<Mutex<Option<SpaceMembershipState>>>,
    failure: Arc<Mutex<Option<SpaceMembershipStateRepositoryError>>>,
}

#[async_trait]
impl SpaceMembershipStateRepositoryPort for MemoryWorkspaceRepository {
    async fn save_state(
        &self,
        state: &SpaceMembershipState,
    ) -> Result<(), SpaceMembershipStateRepositoryError> {
        if let Some(error) = self.failure.lock().unwrap().clone() {
            return Err(error);
        }
        *self.state.lock().unwrap() = Some(state.clone());
        Ok(())
    }

    async fn load_state(
        &self,
    ) -> Result<Option<SpaceMembershipState>, SpaceMembershipStateRepositoryError> {
        Ok(self.state.lock().unwrap().clone())
    }
}

struct LockedWorkspaceRepository;

#[async_trait]
impl SpaceMembershipStateRepositoryPort for LockedWorkspaceRepository {
    async fn save_state(
        &self,
        _state: &SpaceMembershipState,
    ) -> Result<(), SpaceMembershipStateRepositoryError> {
        Err(SpaceMembershipStateRepositoryError::Locked)
    }

    async fn load_state(
        &self,
    ) -> Result<Option<SpaceMembershipState>, SpaceMembershipStateRepositoryError> {
        Err(SpaceMembershipStateRepositoryError::Locked)
    }
}

struct LockedAdmissionRepository {
    allow_empty_history_reads: bool,
}

#[async_trait]
impl uc_core::membership::AdmissionAttemptRepositoryPort for LockedAdmissionRepository {
    async fn reset_admission_profile(
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
) -> crate::space::admission::durable::DurableAdmissionTransaction {
    durable_admission_owner_with_space_transition(repository, Arc::new(NoAdmissionSpaceTransition))
}

fn durable_admission_owner_with_space_transition(
    repository: Arc<dyn uc_core::membership::AdmissionAttemptRepositoryPort>,
    space_transition: Arc<dyn uc_core::membership::AdmissionSpaceTransitionPort>,
) -> crate::space::admission::durable::DurableAdmissionTransaction {
    crate::space::admission::durable::DurableAdmissionTransaction::new(
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
    owner: Arc<WorkspaceMembership>,
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
    let owner = WorkspaceMembership::new(deps);
    Harness {
        owner,
        repository,
        presence,
    }
}

#[tokio::test]
async fn runtime_pause_interrupts_an_in_flight_recovery() {
    let started = Arc::new(tokio::sync::Notify::new());
    let repository = MemoryWorkspaceRepository::default();
    let mut deps = test_deps(Arc::new(repository), "device-a", Vec::new());
    deps.legacy_migration_recovery = Arc::new(BlockingLegacyMigrationRecovery {
        started: Arc::clone(&started),
    });
    let owner = WorkspaceMembership::new(deps);
    let (presence_tx, presence_rx) = tokio::sync::broadcast::channel(1);
    let runtime = owner.start(presence_rx);
    started.notified().await;

    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        runtime.activity().pause(),
    )
    .await
    .expect("pause must not wait for network recovery")
    .unwrap();

    drop(presence_tx);
    runtime.shutdown().await;
}

/// Build the full dependency set with no-op defaults for every port except
/// the repository and the recovery view. Shared with other test modules in
/// this crate (`pub(crate)` under `cfg(test)`).
pub(crate) fn test_deps(
    repository: Arc<dyn SpaceMembershipStateRepositoryPort>,
    own_device: &str,
    _members: Vec<(DeviceId, MemberInstanceId)>,
) -> WorkspaceMembershipDeps {
    WorkspaceMembershipDeps {
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
        trusted_peer_repo: Arc::new(TestTrustedPeerRepo),
        peer_addr_repo: Arc::new(TestPeerAddrRepo),
        presence: Arc::new(FixedPresence::default()),
        space_protection: Arc::new(FixedSpaceProtection(SpaceProtectionMode::Ready)),
        group_bootstrap: Arc::new(UnusedGroupBootstrap),
        own_device: DeviceId::new(own_device),
    }
}

async fn install_current_history(
    deps: &mut WorkspaceMembershipDeps,
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
        })
    }
}

#[derive(Default)]
struct ProtectsQueriedMembers {
    queries: Mutex<Vec<Vec<DeviceId>>>,
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
        })
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

// 流程：C 收到 A 对 B 的移除，A 在线而 B 离线；一次查询直接返回来源、目标、两种后果和独立关系事实。
// 流程：同一待决定项先保留当前组，再重复相同和相反选择；只保存一次，结果稳定且可跨查询恢复。
// 流程：新入口完成决定后，旧入口重复相同决定；重复提交返回当前结果而不是普通失败。
// 流程：待决定移除精确包含本机；没有二次确认时不能写入决定，确认后才退出当前设备组。
// 流程：A 尝试移除不存在的设备或移除自己；操作失败，原成员历史和状态均不得保存变化。
// 流程：成员历史在邀请签发后继续前进；旧邀请绑定的历史位置失效，不能再用于加入。
// 流程：A 完成新成员加入后联系尚未建立历史关系的 B；首包携带受限的连续历史，
// 让 B 即使尚未保存 A 的最新成员资料也能验证并接纳本次引荐。

// 流程：A 不在线时 B 将 C 加入空间；A 恢复但未保存 C 的资料，C 首次联系即提交
// 从起点到 C 的连续成员记录，使 A 能从历史本身验证 C 的准入关系。

// 流程：普通内容面对一致设备可通过，面对待决定或已分叉设备被阻止；成员资格本身不被改写。
// 流程：A 已确认 B 低于 1.1 并重启；重启后升级提示和双向内容暂停仍然保留。
// 流程：A 已是 1.1，B 曾被标记为需要升级；B 升级到 1.1 后上线并完成当前成员历史回应。
// 证明：A 只运行当前流程、清除升级提示并恢复 B 的正常内容资格。

// 流程：A、B 都从低于 1.1 的同一旧 Space 升级，双方起初都没有 1.1 成员历史；
// A 建立唯一历史起点，B 通过当前问候提交自己的签名资料，双方保存同一历史后 A 清除升级提示。

// Flow: this device created the persisted legacy bootstrap before a restart, but its device ID is
// not the deterministic minimum; the bootstrap owner must still finish the missing history root.
// Flow: the deterministic initializer creates the signed history root while a retained legacy
// peer still awaits admission; ordinary peer scope must switch to current history immediately.
// Flow: a pre-ADR-020 installation has a retained legacy roster but no convergence-state row.
// Creating the current-history root must stop legacy records from granting ordinary membership.
// 流程：A 已是 1.1，B 低于 1.1；当前成员历史入口没有回应，但旧入口空连接成功。
// 证明：只有旧入口的正面证据会让 A 保存“B 需要升级”，并暂停内容同步。
// 流程：A 从已有 Space 启动为 1.1，B 仍低于 1.1 且已经在线；A 解锁后恢复成员活动，但没有新的上线通知。
// 证明：会话恢复会让负责人主动核对已保存的 B，并在旧入口确认后保存“B 需要升级”。
// 流程：A 与 B 均无法完成当前流程和旧入口空连接。
// 证明：网络或身份类失败不产生“需要升级”提示，也不改变原有关系。
// 流程：A 尝试与 B 进行本次 1.1 的成员历史核对，B 明确拒绝该请求；旧入口空连接即使可用也不能改写结果。
// 证明：明确拒绝属于当前流程或身份资料问题，不是旧版本的正面证据；A 不探测旧入口、不显示升级提示。
// 流程：B 的两次上线通知几乎同时到达 A；第一次核对尚未完成时，第二次必须等待，不能并行识别或拨号。
// 流程：B 收到 A 提交的有效移除历史；B 保存同一事件，但不改变成员集合，并发布一次待用户决定。

// 流程：B 收到 A 对 B 的移除时先保存事实但不改变本机安全状态；B 明确接受后，才应用该移除携带的安全更新。

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
) -> Arc<WorkspaceMembership> {
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
    WorkspaceMembership::new(deps)
}

// 流程：第二页先到时不改正式历史；第一页保存后重启，重复页保持幂等，最后一页才完成替换。
// 流程：同一来源混入另一轮第一页时拒绝整轮资料，并保留原来的正式历史。
// 流程：发送方通过唯一分页入口传完 257 条记录，接收方回执后双方保存相同历史。
// 流程：A 与 B 已经分叉；A 请求 B 的旧 Space 成员资料，B 必须拒绝，不再继续交换旧分支。

// 流程：A 已移除 B；B 接受后回传决定，A 仍依据移除前保存的成员关系验证并记录该回传。

// 流程：A 提交移除后，B 与 A、C 都曾交换到同一待决定历史；B 拒绝时把决定发给 A、C，等待双方按决定结果解除阻断或进入分叉。

// 流程：C 接受 A 对 B 的移除时，先按决定前的成员分支固定通知名单；即使应用后 B 已不再有效，也必须收到 C 的相反决定。

// 流程：B、C 对同一项移除都选择拒绝；B 收到 C 的签名决定后确认双方仍在同一旧分支，解除内容阻断。

// 流程：B 拒绝 A 提交的待决定移除后，保留原成员关系，并只隔离与 A 的旧分支。

// 流程：A 完成 B 的加入并提交历史；当前有效成员及其设备绑定写入签名历史，随后移除 B 也按该历史生效。
// 流程：赞助方当前分支仍有 A 的有效成员实例；A 再次使用邀请加入时必须拒绝重复成员。
// 流程：赞助方当前分支只保留 A 的旧移除记录；A 使用新成员实例重新加入时必须允许继续准入。
// 流程：新建空间的 A 首次邀请 B；即使此前没有成员历史，A 也先记录自己的成员实例并完成 B 的加入。
// 流程：持久成员历史仍指向 A 的旧实例，但当前安全状态已经使用新实例；
// A 必须先恢复这项身份冲突，不能继续邀请并对外报告加入成功。
// 流程：加入方收到的发起者历史摘要与本机事实不符；加入被拒绝，原历史位置保持不变。

// 流程：加入方保存准入资料前尚缺发起者的完整历史；先拉取并验证连续历史，匹配后才完成加入。

// 流程：A 的旧实例已被 C 移除，随后 C 把 A 的新实例加入同一条已验证历史；A
// 以新实例拉取整条历史时直接采用最终分支，不为已废弃的旧实例产生待确认项。

// Flow: a fresh join is durable before its first network send and reopening
// the owner returns the same attempt, join id, ordinal, and request outbox.
// Flow: the joiner verifies and saves the sponsor candidate before commit;
// both sides then persist the same activation result before completion.
// Flow: cancellation and formal commit have one persisted winner. A saved
// cancellation before commit rejects without a formal add; a saved commit
// makes cancellation too late and the same attempt continues forward.
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
    super::admission::DurableAdmissionCandidateV1,
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
    let candidate = super::admission::DurableAdmissionCandidateV1 {
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
