use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use tempfile::TempDir;
use uc_application::deps::{
    AdmissionRecoveryTrigger, JoinerStartMutation, JoinerStartStateError, JoinerStartStatePort,
    LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedgerError,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    ActiveSpaceGenerationManifestV2, AdmissionChannelPeerId, AdmissionContinuationCredential,
    AdmissionEncryptedPasswordEquivalent, AdmissionIdentitySignature, AdmissionJoinRequestV1,
    AdmissionJoinerPrivateState, AdmissionJoinerStartContext, AdmissionKeyPackage,
    AdmissionPeerBinding, AdmissionRecordPersistence, AdmissionRecoveryPublicKey,
    AdmissionRetryState, AdmissionRole, AdmissionShortInvitationCode, AdmissionSourceSnapshot,
    InvitationId, JoinId, JoinerAdmission, JoinerAdmissionTransition, JoinerInvitationResolution,
    MembershipCredential, PendingAdmissionExchange, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionId, SpaceAdmissionMessageKind, SpaceAdmissionRoute, SponsorAdmission,
    SponsorAdmissionTransition, UnreadableHistoryPolicy,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_infra::db::executor::DieselSqliteExecutor;
use uc_infra::db::pool::init_db_pool;
use uc_infra::security::{ActiveSpaceGenerationManifestStore, AdmissionKeyManager};
use uc_infra::space::SqliteSpaceAdmissionState;

#[path = "space_admission_state/activation.rs"]
mod activation;
#[path = "space_admission_state/sponsor.rs"]
mod sponsor;

#[derive(Default)]
struct MemorySecureStorage {
    values: Mutex<HashMap<String, Vec<u8>>>,
}

impl SecureStoragePort for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.values
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

struct UnusedMembershipLedger;

#[async_trait::async_trait]
impl LoadMembershipLedgerPort for UnusedMembershipLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Err(MembershipLedgerError::Unavailable)
    }
}

struct Fixture {
    _temp: TempDir,
    db_path: PathBuf,
    secure_storage: Arc<MemorySecureStorage>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    store: SqliteSpaceAdmissionState<Arc<DieselSqliteExecutor>>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("state.sqlite");
        let pool = init_db_pool(db_path.to_str().unwrap()).unwrap();
        let executor = Arc::new(DieselSqliteExecutor::new(pool));
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let keys = Arc::new(AdmissionKeyManager::new(secure_storage.clone(), [0x31; 16]));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            temp.path().join("vault"),
            Arc::clone(&keys),
        ));
        let store = SqliteSpaceAdmissionState::new(
            executor,
            keys,
            Arc::clone(&manifests),
            Arc::new(UnusedMembershipLedger),
        );
        Self {
            _temp: temp,
            db_path,
            secure_storage,
            manifests,
            store,
        }
    }

    fn reopen(&self) -> SqliteSpaceAdmissionState<Arc<DieselSqliteExecutor>> {
        let executor = Arc::new(DieselSqliteExecutor::new(
            init_db_pool(self.db_path.to_str().unwrap()).unwrap(),
        ));
        let keys = Arc::new(AdmissionKeyManager::new(
            self.secure_storage.clone(),
            [0x31; 16],
        ));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            self._temp.path().join("vault"),
            Arc::clone(&keys),
        ));
        SqliteSpaceAdmissionState::new(executor, keys, manifests, Arc::new(UnusedMembershipLedger))
    }

    fn execute(&self, sql: &str) {
        let executor =
            DieselSqliteExecutor::new(init_db_pool(self.db_path.to_str().unwrap()).unwrap());
        uc_infra::db::ports::DbExecutor::run(&executor, |conn| {
            sql_query(sql).execute(conn)?;
            Ok(())
        })
        .unwrap();
    }

    fn encrypted_payload(&self) -> Vec<u8> {
        #[derive(QueryableByName)]
        struct Row {
            #[diesel(sql_type = Binary)]
            encrypted_payload: Vec<u8>,
        }
        let executor =
            DieselSqliteExecutor::new(init_db_pool(self.db_path.to_str().unwrap()).unwrap());
        uc_infra::db::ports::DbExecutor::run(&executor, |conn| {
            Ok(sql_query(
                "SELECT encrypted_payload FROM admission_repository_state WHERE singleton_id = 1",
            )
            .get_result::<Row>(conn)?
            .encrypted_payload)
        })
        .unwrap()
    }
}

#[tokio::test]
async fn fresh_repository_supplies_one_joiner_start_view() {
    let loaded = JoinerStartStatePort::load(&Fixture::new().store)
        .await
        .unwrap();
    let (ordinal, snapshot, current, requires_transition, token) = loaded.into_parts();

    assert_eq!(ordinal, 0);
    assert!(!snapshot.as_bytes().is_empty());
    assert!(current.is_none());
    assert!(!requires_transition);
    assert_ne!(token.as_bytes(), &[0; 32]);
}

#[tokio::test]
async fn committed_joiner_is_reloaded_for_recovery_after_reopen() {
    let fixture = Fixture::new();
    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (ordinal, snapshot, _, _, token) = loaded.into_parts();
    let transition = start_join_transition(0x41, 0x42, ordinal, snapshot);
    let expected = transition.replacement().encode_persisted().unwrap();

    JoinerStartStatePort::commit(
        &fixture.store,
        token,
        JoinerStartMutation::new(transition, None),
    )
    .await
    .unwrap();

    let reopened = fixture.reopen();
    let pending =
        PendingAdmissionRecoveryStatePort::load(&reopened, AdmissionRecoveryTrigger::Startup)
            .await
            .unwrap();
    assert_eq!(pending.len(), 1);
    let (aggregate, _) = pending.into_iter().next().unwrap().into_parts();
    assert_eq!(aggregate.encode_persisted().unwrap(), expected);
}

#[tokio::test]
async fn stale_joiner_start_token_cannot_overwrite_new_state() {
    let fixture = Fixture::new();
    let first = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let second = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (first_ordinal, first_snapshot, _, _, first_token) = first.into_parts();
    let (second_ordinal, second_snapshot, _, _, second_token) = second.into_parts();

    JoinerStartStatePort::commit(
        &fixture.store,
        first_token,
        JoinerStartMutation::new(
            start_join_transition(0x51, 0x52, first_ordinal, first_snapshot),
            None,
        ),
    )
    .await
    .unwrap();
    let result = JoinerStartStatePort::commit(
        &fixture.store,
        second_token,
        JoinerStartMutation::new(
            start_join_transition(0x61, 0x62, second_ordinal, second_snapshot),
            None,
        ),
    )
    .await;

    assert_eq!(result, Err(JoinerStartStateError::StateChanged));
}

#[tokio::test]
async fn superseding_join_and_replacement_commit_atomically() {
    let fixture = Fixture::new();
    commit_fresh_join(&fixture, 0x71, 0x72).await;
    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (ordinal, snapshot, current, _, token) = loaded.into_parts();
    let current = current.unwrap();
    let superseded = current.supersede().unwrap();
    let replacement = start_join_transition(0x81, 0x82, ordinal, snapshot);

    fixture.execute(
        "CREATE TRIGGER fail_admission_update BEFORE UPDATE ON admission_repository_state \
         BEGIN SELECT RAISE(ABORT, 'injected'); END",
    );
    let result = JoinerStartStatePort::commit(
        &fixture.store,
        token,
        JoinerStartMutation::new(replacement, Some(superseded)),
    )
    .await;
    assert_eq!(result, Err(JoinerStartStateError::Unavailable));
    fixture.execute("DROP TRIGGER fail_admission_update");

    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (next_ordinal, _, current, _, _) = loaded.into_parts();
    assert_eq!(next_ordinal, 1);
    assert_eq!(current.unwrap().admission_id().as_bytes(), &[0x71; 32]);
}

#[tokio::test]
async fn successful_supersede_keeps_only_replacement_recoverable() {
    let fixture = Fixture::new();
    commit_fresh_join(&fixture, 0x73, 0x74).await;
    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (ordinal, snapshot, current, _, token) = loaded.into_parts();
    let superseded = current.unwrap().supersede().unwrap();
    let replacement = start_join_transition(0x83, 0x84, ordinal, snapshot);

    JoinerStartStatePort::commit(
        &fixture.store,
        token,
        JoinerStartMutation::new(replacement, Some(superseded)),
    )
    .await
    .unwrap();

    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (next_ordinal, _, current, _, _) = loaded.into_parts();
    assert_eq!(next_ordinal, 2);
    assert_eq!(current.unwrap().admission_id().as_bytes(), &[0x83; 32]);
    let pending = PendingAdmissionRecoveryStatePort::load(
        &fixture.store,
        AdmissionRecoveryTrigger::StateChanged,
    )
    .await
    .unwrap();
    assert_eq!(pending.len(), 1);
}

#[tokio::test]
async fn recovery_commit_advances_record_and_rejects_old_token() {
    let fixture = Fixture::new();
    commit_fresh_join(&fixture, 0x91, 0x92).await;
    let mut first =
        PendingAdmissionRecoveryStatePort::load(&fixture.store, AdmissionRecoveryTrigger::Startup)
            .await
            .unwrap();
    let mut stale =
        PendingAdmissionRecoveryStatePort::load(&fixture.store, AdmissionRecoveryTrigger::Startup)
            .await
            .unwrap();
    let (aggregate, token) = first.pop().unwrap().into_parts();
    let (stale_aggregate, stale_token) = stale.pop().unwrap().into_parts();
    let advanced = aggregate
        .with_authenticated_channel(peer_binding(), continuation())
        .unwrap();
    let loaded = PendingAdmissionRecoveryStatePort::commit(&fixture.store, token, advanced)
        .await
        .unwrap();
    let (advanced, _) = loaded.into_parts();
    assert_eq!(advanced.record_version(), 1);

    let stale_transition = stale_aggregate
        .with_authenticated_channel(peer_binding(), continuation())
        .unwrap();
    assert!(matches!(
        PendingAdmissionRecoveryStatePort::commit(&fixture.store, stale_token, stale_transition,)
            .await,
        Err(PendingAdmissionRecoveryStateError::StateChanged)
    ));
}

#[tokio::test]
async fn short_code_is_removed_before_the_single_resolution_request() {
    let fixture = Fixture::new();
    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (ordinal, snapshot, _, _, token) = loaded.into_parts();
    let transition = JoinerAdmission::start_resolving_invitation(
        SpaceAdmissionId::from_bytes([0x95; 32]).unwrap(),
        JoinId::from_bytes([0x96; 16]).unwrap(),
        ordinal,
        snapshot,
        AdmissionJoinerStartContext::from_bytes(b"secret-passphrase".to_vec()).unwrap(),
        AdmissionShortInvitationCode::from_bytes(b"ONCE-CODE".to_vec()).unwrap(),
    )
    .unwrap();
    JoinerStartStatePort::commit(
        &fixture.store,
        token,
        JoinerStartMutation::new(transition, None),
    )
    .await
    .unwrap();

    let mut pending =
        PendingAdmissionRecoveryStatePort::load(&fixture.store, AdmissionRecoveryTrigger::Startup)
            .await
            .unwrap();
    let (ready, token) = pending.pop().unwrap().into_parts();
    assert!(matches!(
        ready.invitation_resolution(),
        Some(JoinerInvitationResolution::Ready { short_code, .. })
            if short_code.as_bytes() == b"ONCE-CODE"
    ));
    let started = ready.mark_invitation_resolution_started().unwrap();
    let (transition, short_code) = started.into_parts();
    assert_eq!(short_code.as_bytes(), b"ONCE-CODE");
    let committed = PendingAdmissionRecoveryStatePort::commit(&fixture.store, token, transition)
        .await
        .unwrap();
    let (started, _) = committed.into_parts();
    assert!(matches!(
        started.invitation_resolution(),
        Some(JoinerInvitationResolution::Started { .. })
    ));
    assert!(!fixture
        .encrypted_payload()
        .windows(b"ONCE-CODE".len())
        .any(|window| window == b"ONCE-CODE"));
    assert!(!fixture
        .encrypted_payload()
        .windows(b"secret-passphrase".len())
        .any(|window| window == b"secret-passphrase"));
}

#[tokio::test]
async fn sqlite_payload_does_not_contain_admission_plaintext() {
    let fixture = Fixture::new();
    commit_fresh_join(&fixture, 0xa1, 0xa2).await;
    let encrypted = fixture.encrypted_payload();

    assert!(!encrypted.windows(32).any(|window| window == [0xa8; 32]));
    assert!(!encrypted.windows(64).any(|window| window == [0xa9; 64]));
    assert!(!encrypted.windows(64).any(|window| window == [0xaa; 64]));
}

#[tokio::test]
async fn corrupt_repository_payload_requires_recovery() {
    let fixture = Fixture::new();
    commit_fresh_join(&fixture, 0xb1, 0xb2).await;
    fixture.execute(
        "UPDATE admission_repository_state SET encrypted_payload = X'FF' WHERE singleton_id = 1",
    );

    assert!(matches!(
        JoinerStartStatePort::load(&fixture.store).await,
        Err(JoinerStartStateError::RecoveryRequired)
    ));
}

#[tokio::test]
async fn active_space_snapshot_requires_session_transition() {
    let fixture = Fixture::new();
    fixture
        .manifests
        .promote(
            &ActiveSpaceGenerationManifestV2::new(
                "space-a".to_owned(),
                [0xc1; 16],
                [0xc2; 16],
                [0xc3; 16],
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (_, _, _, requires_transition, _) = loaded.into_parts();
    assert!(requires_transition);
}

#[tokio::test]
async fn source_space_change_invalidates_joiner_start_token() {
    let fixture = Fixture::new();
    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (ordinal, snapshot, _, _, token) = loaded.into_parts();
    fixture
        .manifests
        .promote(
            &ActiveSpaceGenerationManifestV2::new(
                "space-b".to_owned(),
                [0xc4; 16],
                [0xc5; 16],
                [0xc6; 16],
            )
            .unwrap(),
        )
        .await
        .unwrap();

    let result = JoinerStartStatePort::commit(
        &fixture.store,
        token,
        JoinerStartMutation::new(start_join_transition(0xc7, 0xc8, ordinal, snapshot), None),
    )
    .await;

    assert_eq!(result, Err(JoinerStartStateError::StateChanged));
}

async fn commit_fresh_join(fixture: &Fixture, admission_byte: u8, join_byte: u8) {
    let loaded = JoinerStartStatePort::load(&fixture.store).await.unwrap();
    let (ordinal, snapshot, _, _, token) = loaded.into_parts();
    JoinerStartStatePort::commit(
        &fixture.store,
        token,
        JoinerStartMutation::new(
            start_join_transition(admission_byte, join_byte, ordinal, snapshot),
            None,
        ),
    )
    .await
    .unwrap();
}

fn peer_binding() -> AdmissionPeerBinding {
    AdmissionPeerBinding::new(
        AdmissionChannelPeerId::from_bytes([0xd1; 32]).unwrap(),
        AdmissionChannelPeerId::from_bytes([0xd2; 32]).unwrap(),
    )
    .unwrap()
}

fn continuation() -> AdmissionContinuationCredential {
    AdmissionContinuationCredential::from_bytes(vec![0xd3; 64]).unwrap()
}

fn start_join_transition(
    admission_byte: u8,
    join_byte: u8,
    ordinal: u64,
    source_snapshot: AdmissionSourceSnapshot,
) -> JoinerAdmissionTransition {
    let admission_id = SpaceAdmissionId::from_bytes([admission_byte; 32]).unwrap();
    let request = SpaceAdmissionEnvelopeV1::new(
        admission_id,
        AdmissionRole::Joiner,
        0,
        uc_core::membership::AdmissionMessageId::from_bytes([admission_byte + 1; 32]).unwrap(),
        None,
        SpaceAdmissionBodyV1::JoinRequest(
            AdmissionJoinRequestV1::new(
                InvitationId::from_bytes([admission_byte + 2; 32]).unwrap(),
                DeviceId::new("joining-device"),
                MembershipCredential::new(1, vec![admission_byte + 3; 32]),
                AdmissionKeyPackage::from_bytes(vec![admission_byte + 4; 48]).unwrap(),
                AdmissionRecoveryPublicKey::from_bytes([admission_byte + 5; 32]).unwrap(),
                AdmissionIdentitySignature::from_bytes(vec![admission_byte + 6; 64]).unwrap(),
                UnreadableHistoryPolicy::Discard,
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let pending = PendingAdmissionExchange::new(
        SpaceAdmissionRoute::from_bytes(vec![admission_byte + 7; 32]).unwrap(),
        request,
        SpaceAdmissionMessageKind::Candidate,
        AdmissionRetryState::new(0, 0).unwrap(),
    )
    .unwrap();
    JoinerAdmission::start_join(
        admission_id,
        JoinId::from_bytes([join_byte; 16]).unwrap(),
        ordinal,
        source_snapshot,
        AdmissionJoinerPrivateState::from_bytes(vec![admission_byte.wrapping_add(9); 64]).unwrap(),
        AdmissionEncryptedPasswordEquivalent::from_bytes(vec![admission_byte + 8; 64]).unwrap(),
        pending,
    )
    .unwrap()
}
