use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use uc_application::deps::{
    JoinerActivationStatePort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerError,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};

use super::persisted::{PersistedSpaceAdmissionRepositoryV2, StoredSpaceAdmissionV1};
use super::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::init_db_pool;
use crate::security::{ActiveSpaceGenerationManifestStore, AdmissionKeyManager};

#[derive(Default)]
struct MemoryStorage(Mutex<HashMap<String, Vec<u8>>>, AtomicBool);

impl SecureStoragePort for MemoryStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        if self.1.load(Ordering::SeqCst) {
            return Err(SecureStorageError::PermissionDenied("locked".to_owned()));
        }
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecureStorageError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecureStorageError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

struct ConcurrentWriterStorage {
    values: Mutex<HashMap<String, Vec<u8>>>,
    replacement_payload: Mutex<Option<Vec<u8>>>,
    reads: AtomicUsize,
    interference_attempted: AtomicBool,
    database: PathBuf,
}

impl ConcurrentWriterStorage {
    fn new(database: PathBuf) -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
            replacement_payload: Mutex::new(None),
            reads: AtomicUsize::new(0),
            interference_attempted: AtomicBool::new(false),
            database,
        }
    }

    fn replace_payload_on_third_read(&self, payload: Vec<u8>) {
        *self.replacement_payload.lock().unwrap() = Some(payload);
    }

    fn reset_reads(&self) {
        self.reads.store(0, Ordering::SeqCst);
    }
}

impl SecureStoragePort for ConcurrentWriterStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
        let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        if read == 3 {
            self.interference_attempted.store(true, Ordering::SeqCst);
            let mut connection =
                SqliteConnection::establish(self.database.to_str().ok_or_else(|| {
                    SecureStorageError::Other("test database path is invalid".to_owned())
                })?)
                .map_err(|error| SecureStorageError::Other(error.to_string()))?;
            if let Some(payload) = self.replacement_payload.lock().unwrap().clone() {
                let _ = sql_query(
                    "UPDATE admission_repository_state SET encrypted_payload = ? WHERE singleton_id = 1",
                )
                .bind::<Binary, _>(payload)
                .execute(&mut connection);
            }
        }
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

struct UnusedMembership;

#[async_trait]
impl LoadMembershipLedgerPort for UnusedMembership {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Err(MembershipLedgerError::Unavailable)
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    connection: SqliteConnection,
    keys: Arc<AdmissionKeyManager>,
    storage: Arc<MemoryStorage>,
    repository: SqliteSpaceAdmissionState<DieselSqliteExecutor>,
}

#[derive(QueryableByName)]
struct CountRow {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    count: i64,
}

#[derive(QueryableByName)]
struct PayloadRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("profile.sqlite");
        let pool = init_db_pool(path.to_str().unwrap()).unwrap();
        let connection = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
        let storage = Arc::new(MemoryStorage::default());
        let keys = Arc::new(AdmissionKeyManager::new(storage.clone(), [0x31; 16]));
        let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
            directory.path().join("vault"),
            keys.clone(),
        ));
        let repository = SqliteSpaceAdmissionState::new(
            DieselSqliteExecutor::new(pool),
            keys.clone(),
            manifests,
            Arc::new(UnusedMembership),
        );
        Self {
            _directory: directory,
            connection,
            keys,
            storage,
            repository,
        }
    }

    fn write_state(&mut self, state: &PersistedSpaceAdmissionRepositoryV2) {
        let bytes = postcard::to_stdvec(state).unwrap();
        let encrypted = self
            .keys
            .seal_profile_payload(b"space-admission-repository-v1", &bytes)
            .unwrap();
        sql_query("INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?) ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload")
            .bind::<Binary, _>(encrypted)
            .execute(&mut self.connection)
            .unwrap();
    }
}

#[tokio::test]
async fn activation_query_migrates_legacy_state_and_ignores_unrelated_record_payloads() {
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    state.records.insert(
        [0x41; 32],
        StoredSpaceAdmissionV1 {
            wrapped_data_key: fixture.keys.create_wrapped_attempt_key([0x41; 32]).unwrap(),
            encrypted_payload: vec![0x51; 1024 * 1024].into(),
        },
    );
    fixture.write_state(&state);

    let loaded = JoinerActivationStatePort::load(&fixture.repository)
        .await
        .unwrap();
    assert!(loaded.is_none());
    let rows = sql_query("SELECT COUNT(*) AS count FROM admission_repository_record")
        .get_result::<CountRow>(&mut fixture.connection)
        .unwrap();
    assert_eq!(rows.count, 1);

    sql_query("UPDATE admission_repository_record SET encrypted_payload = x'010203'")
        .execute(&mut fixture.connection)
        .unwrap();
    let loaded = JoinerActivationStatePort::load(&fixture.repository)
        .await
        .unwrap();
    assert!(loaded.is_none());
}

#[tokio::test]
async fn concurrent_database_write_does_not_abort_legacy_activation_query() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("profile.sqlite");
    let pool = init_db_pool(database.to_str().unwrap()).unwrap();
    let mut connection = SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    let storage = Arc::new(ConcurrentWriterStorage::new(database));
    let keys = Arc::new(AdmissionKeyManager::new(storage.clone(), [0x31; 16]));
    let manifests = Arc::new(ActiveSpaceGenerationManifestStore::new(
        directory.path().join("vault"),
        keys.clone(),
    ));
    let repository = SqliteSpaceAdmissionState::new(
        DieselSqliteExecutor::new(pool),
        keys.clone(),
        manifests,
        Arc::new(UnusedMembership),
    );
    let state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    let bytes = postcard::to_stdvec(&state).unwrap();
    let encrypted = keys
        .seal_profile_payload(b"space-admission-repository-v1", &bytes)
        .unwrap();
    let mut replacement_state = state.clone();
    replacement_state.next_local_join_ordinal = 1;
    let replacement_bytes = postcard::to_stdvec(&replacement_state).unwrap();
    let replacement = keys
        .seal_profile_payload(b"space-admission-repository-v1", &replacement_bytes)
        .unwrap();
    sql_query(
        "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?)",
    )
    .bind::<Binary, _>(encrypted)
    .execute(&mut connection)
    .unwrap();
    storage.replace_payload_on_third_read(replacement);
    storage.reset_reads();

    let loaded = JoinerActivationStatePort::load(&repository).await;

    assert!(storage.interference_attempted.load(Ordering::SeqCst));
    assert!(loaded.is_ok());
}

#[test]
fn metadata_only_save_keeps_unchanged_record_ciphertext() {
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    state.records.insert(
        [0x41; 32],
        StoredSpaceAdmissionV1 {
            wrapped_data_key: fixture.keys.create_wrapped_attempt_key([0x41; 32]).unwrap(),
            encrypted_payload: vec![0x51; 1024 * 1024].into(),
        },
    );
    fixture
        .repository
        .save_state_on(&mut fixture.connection, &state)
        .unwrap();
    let before = sql_query("SELECT encrypted_payload FROM admission_repository_record")
        .get_result::<PayloadRow>(&mut fixture.connection)
        .unwrap()
        .encrypted_payload;

    state.next_local_join_ordinal = 1;
    fixture
        .repository
        .save_state_on(&mut fixture.connection, &state)
        .unwrap();
    let after = sql_query("SELECT encrypted_payload FROM admission_repository_record")
        .get_result::<PayloadRow>(&mut fixture.connection)
        .unwrap()
        .encrypted_payload;

    assert_eq!(after, before);
}

#[test]
#[ignore = "大状态性能验收，显式运行以避免普通测试的资源峰值"]
fn unchanged_large_repository_reads_avoid_repeated_decode() {
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    // 查询当前加入为空时，不应反复处理其他加入的约 25 MB 密文记录。
    state.records.insert(
        [0x41; 32],
        StoredSpaceAdmissionV1 {
            wrapped_data_key: fixture.keys.create_wrapped_attempt_key([0x41; 32]).unwrap(),
            encrypted_payload: vec![0x51; 25 * 1024 * 1024].into(),
        },
    );
    fixture.write_state(&state);
    drop(state);
    let cold_started = Instant::now();
    let cold = fixture
        .repository
        .load_state_on(&mut fixture.connection)
        .unwrap();
    let cold_duration = cold_started.elapsed();
    assert_eq!(cold.records.len(), 1);
    drop(cold);
    let repeated_started = Instant::now();
    for _ in 0..4 {
        let loaded = fixture
            .repository
            .load_state_on(&mut fixture.connection)
            .unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.current_local_join_id, None);
    }
    let repeated_duration = repeated_started.elapsed();
    eprintln!("cold={cold_duration:?}, four_unchanged_reads={repeated_duration:?}");
    assert!(repeated_duration < cold_duration * 2,
        "four unchanged reads must cost less than two cold reads: cold={cold_duration:?}, repeated={repeated_duration:?}");
}

#[test]
fn repository_reads_observe_external_writes_and_deleted_rows() {
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    fixture.write_state(&state);
    let mut reader = SqliteConnection::establish(
        fixture
            ._directory
            .path()
            .join("profile.sqlite")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        fixture.repository.load_state_on(&mut reader).unwrap(),
        state
    );
    state.next_local_join_ordinal = 7;
    fixture.write_state(&state);
    assert_eq!(
        fixture.repository.load_state_on(&mut reader).unwrap(),
        state
    );
    sql_query("DELETE FROM admission_repository_state")
        .execute(&mut fixture.connection)
        .unwrap();
    assert_eq!(
        fixture
            .repository
            .load_state_on(&mut reader)
            .unwrap()
            .next_local_join_ordinal,
        0
    );
    assert!(fixture.repository.read_cache.lock().unwrap().is_none());
}

#[test]
fn repository_reads_do_not_publish_rolled_back_state() {
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    fixture.write_state(&state);
    fixture
        .repository
        .load_state_on(&mut fixture.connection)
        .unwrap();
    state.next_local_join_ordinal = 7;
    let rolled_back = fixture
        .connection
        .immediate_transaction::<(), diesel::result::Error, _>(|connection| {
            fixture
                .repository
                .save_state_on(connection, &state)
                .unwrap();
            assert_eq!(fixture.repository.load_state_on(connection).unwrap(), state);
            Err(diesel::result::Error::RollbackTransaction)
        });
    assert!(rolled_back.is_err());
    assert_eq!(
        fixture
            .repository
            .load_state_on(&mut fixture.connection)
            .unwrap()
            .next_local_join_ordinal,
        0
    );
}

#[test]
fn repository_reads_reject_changed_key_and_corrupted_ciphertext() {
    let mut fixture = Fixture::new();
    let state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    fixture.write_state(&state);
    fixture
        .repository
        .load_state_on(&mut fixture.connection)
        .unwrap();
    let key_name = "profile_admission_master_key:v1";
    let original_key = fixture.storage.get(key_name).unwrap().unwrap();
    fixture.storage.set(key_name, &[0x99; 32]).unwrap();
    assert!(matches!(
        fixture.repository.load_state_on(&mut fixture.connection),
        Err(SpaceAdmissionStateStoreError::Corrupt)
    ));
    assert!(fixture.repository.read_cache.lock().unwrap().is_none());
    fixture.storage.set(key_name, &original_key).unwrap();
    assert_eq!(
        fixture
            .repository
            .load_state_on(&mut fixture.connection)
            .unwrap(),
        state
    );
    sql_query("UPDATE admission_repository_state SET encrypted_payload = x'010203'")
        .execute(&mut fixture.connection)
        .unwrap();
    assert!(matches!(
        fixture.repository.load_state_on(&mut fixture.connection),
        Err(SpaceAdmissionStateStoreError::Corrupt)
    ));
    assert!(fixture.repository.read_cache.lock().unwrap().is_none());
}

#[test]
fn cached_state_mutation_does_not_change_the_persisted_snapshot() {
    let mut fixture = Fixture::new();
    let state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    fixture.write_state(&state);
    let mut loaded = fixture
        .repository
        .load_state_on(&mut fixture.connection)
        .unwrap();
    loaded.next_local_join_ordinal = 8;
    assert_eq!(
        fixture
            .repository
            .load_state_on(&mut fixture.connection)
            .unwrap(),
        state
    );
}

#[test]
fn cached_repository_requires_secure_storage_access_on_every_read() {
    let mut fixture = Fixture::new();
    let state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    fixture.write_state(&state);
    fixture
        .repository
        .load_state_on(&mut fixture.connection)
        .unwrap();
    fixture.storage.1.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.repository.load_state_on(&mut fixture.connection),
        Err(SpaceAdmissionStateStoreError::Locked)
    ));
    assert!(fixture.repository.read_cache.lock().unwrap().is_none());
    fixture.storage.1.store(false, Ordering::SeqCst);
    assert_eq!(
        fixture
            .repository
            .load_state_on(&mut fixture.connection)
            .unwrap(),
        state
    );
}

#[test]
#[ignore = "显式指定只读现场数据库与资料根目录，仅输出计数和耗时"]
fn local_profile_repository_read_probe() {
    #[derive(QueryableByName)]
    struct Row {
        #[diesel(sql_type = Binary)]
        encrypted_payload: Vec<u8>,
    }
    let root = PathBuf::from(
        std::env::var_os("UC_ADMISSION_PROFILE_PROBE_ROOT").expect("profile probe root"),
    );
    let database = PathBuf::from(
        std::env::var_os("UC_ADMISSION_DATABASE_PROBE").expect("database probe path"),
    );
    assert!(database.is_file());
    let mut source = SqliteConnection::establish(database.to_str().unwrap())
        .expect("open existing probe database");
    sql_query("PRAGMA query_only = ON")
        .execute(&mut source)
        .unwrap();
    let row = sql_query(
        "SELECT encrypted_payload FROM admission_repository_state WHERE singleton_id = 1",
    )
    .get_result::<Row>(&mut source)
    .unwrap();
    drop(source);
    let keyring = root.join("keyring");
    let marker = fs::read(keyring.join(format!(
        "{}.bin",
        hex::encode("profile_lifecycle_marker:v1")
    )))
    .expect("read lifecycle marker");
    let (format, generation, _phase): (u16, [u8; 16], u8) =
        postcard::from_bytes(&marker).expect("decode lifecycle marker");
    assert_eq!(format, 1);
    let key_name = "profile_admission_master_key:v1";
    let key = fs::read(keyring.join(format!("{}.bin", hex::encode(key_name))))
        .expect("read admission key");
    let mut fixture = Fixture::new();
    fixture.storage.set(key_name, &key).unwrap();
    fixture.keys = Arc::new(AdmissionKeyManager::new(
        fixture.storage.clone(),
        generation,
    ));
    fixture.repository.keys = fixture.keys.clone();
    sql_query(
        "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?)",
    )
    .bind::<Binary, _>(row.encrypted_payload)
    .execute(&mut fixture.connection)
    .unwrap();
    let cold_started = Instant::now();
    let state = fixture
        .repository
        .load_state_on(&mut fixture.connection)
        .unwrap();
    let cold_duration = cold_started.elapsed();
    let warm_started = Instant::now();
    for _ in 0..4 {
        let loaded = fixture
            .repository
            .load_state_on(&mut fixture.connection)
            .unwrap();
        assert_eq!(loaded.records.len(), state.records.len());
        assert!(loaded.current_local_join_id == state.current_local_join_id);
    }
    let warm_duration = warm_started.elapsed();
    let record_started = Instant::now();
    for (id, record) in &state.records {
        fixture
            .repository
            .open_record(*id, record)
            .expect("record remains readable");
    }
    let record_duration = record_started.elapsed();
    eprintln!("records={}, cold={cold_duration:?}, four_unchanged_reads={warm_duration:?}, all_records_decode={record_duration:?}", state.records.len());
    assert!(warm_duration < cold_duration * 2);
}

fn pending_join_for_recovery(id: u8) -> uc_core::membership::JoinerAdmission {
    use uc_core::membership::{
        AdmissionJoinerStartContext, AdmissionShortInvitationCode, AdmissionSourceSnapshot, JoinId,
        JoinerAdmission, SpaceAdmissionId,
    };
    JoinerAdmission::start_resolving_invitation(
        SpaceAdmissionId::from_bytes([id; 32]).unwrap(),
        JoinId::from_bytes([id; 16]).unwrap(),
        1,
        AdmissionSourceSnapshot::from_bytes(vec![1; 32]).unwrap(),
        AdmissionJoinerStartContext::from_bytes(vec![2; 32]).unwrap(),
        AdmissionShortInvitationCode::from_bytes(b"test-code".to_vec()).unwrap(),
        1_000,
    )
    .unwrap()
    .into_replacement()
}

#[tokio::test]
async fn recovery_does_not_reopen_unchanged_unrelated_records() {
    use uc_application::deps::{AdmissionRecoveryTrigger, PendingAdmissionRecoveryStatePort};
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    let terminal = pending_join_for_recovery(0x41)
        .supersede()
        .unwrap()
        .into_replacement();
    state.records.insert(
        [0x41; 32],
        fixture.repository.seal_new_record(&terminal).unwrap(),
    );
    fixture.write_state(&state);
    assert!(PendingAdmissionRecoveryStatePort::load(
        &fixture.repository,
        AdmissionRecoveryTrigger::Startup,
        0
    )
    .await
    .unwrap()
    .is_empty());
    fixture.repository.record_reads.store(0, Ordering::SeqCst);
    for _ in 0..3 {
        assert!(PendingAdmissionRecoveryStatePort::load(
            &fixture.repository,
            AdmissionRecoveryTrigger::Periodic,
            0
        )
        .await
        .unwrap()
        .is_empty());
    }
    assert_eq!(
        fixture.repository.record_reads.load(Ordering::SeqCst),
        0,
        "没有待办的恢复扫描不能读取无关记录正文"
    );
}

#[tokio::test]
async fn recovery_index_pages_through_large_unrelated_history_without_reopening_bodies() {
    use uc_application::deps::{AdmissionRecoveryTrigger, PendingAdmissionRecoveryStatePort};
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    for id in 1..=130 {
        let terminal = pending_join_for_recovery(id)
            .supersede()
            .unwrap()
            .into_replacement();
        state.records.insert(
            [id; 32],
            fixture.repository.seal_new_record(&terminal).unwrap(),
        );
    }
    fixture.write_state(&state);
    assert!(PendingAdmissionRecoveryStatePort::load(
        &fixture.repository,
        AdmissionRecoveryTrigger::Startup,
        0,
    )
    .await
    .unwrap()
    .is_empty());

    fixture.repository.record_reads.store(0, Ordering::SeqCst);
    assert!(PendingAdmissionRecoveryStatePort::load(
        &fixture.repository,
        AdmissionRecoveryTrigger::Periodic,
        0,
    )
    .await
    .unwrap()
    .is_empty());
    assert_eq!(fixture.repository.record_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovery_summary_tracks_changes_and_rollbacks_without_reading_other_records() {
    use uc_application::deps::{AdmissionRecoveryTrigger, PendingAdmissionRecoveryStatePort};
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    let terminal = pending_join_for_recovery(0x41)
        .supersede()
        .unwrap()
        .into_replacement();
    state.records.insert(
        [0x41; 32],
        fixture.repository.seal_new_record(&terminal).unwrap(),
    );
    fixture.write_state(&state);
    assert!(PendingAdmissionRecoveryStatePort::load(
        &fixture.repository,
        AdmissionRecoveryTrigger::Startup,
        0
    )
    .await
    .unwrap()
    .is_empty());
    let pending = pending_join_for_recovery(0x42);
    state.records.insert(
        [0x42; 32],
        fixture.repository.seal_new_record(&pending).unwrap(),
    );
    // 当前加入指针缺省也不能漏掉恢复任务。
    fixture
        .repository
        .save_state_on(&mut fixture.connection, &state)
        .unwrap();
    fixture.repository.record_reads.store(0, Ordering::SeqCst);
    let loaded = PendingAdmissionRecoveryStatePort::load(
        &fixture.repository,
        AdmissionRecoveryTrigger::Periodic,
        0,
    )
    .await
    .unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(fixture.repository.record_reads.load(Ordering::SeqCst), 1);
    let (mut pending, confirmations, abandonments, next_deadline_ms, sponsor_confirmation_pending) =
        loaded.into_parts();
    assert!(confirmations.is_empty());
    assert!(abandonments.is_empty());
    assert!(!sponsor_confirmation_pending);
    assert_eq!(next_deadline_ms, Some(301_000));
    let (aggregate, _) = pending.pop().unwrap().into_parts();
    let cancelled = aggregate.supersede().unwrap().into_replacement();
    state.records.insert(
        [0x42; 32],
        fixture.repository.seal_new_record(&cancelled).unwrap(),
    );
    let rollback: Result<(), diesel::result::Error> = fixture.connection.transaction(|conn| {
        fixture.repository.save_state_on(conn, &state).unwrap();
        Err(diesel::result::Error::RollbackTransaction)
    });
    assert!(rollback.is_err());
    assert_eq!(
        PendingAdmissionRecoveryStatePort::load(
            &fixture.repository,
            AdmissionRecoveryTrigger::Resume,
            0
        )
        .await
        .unwrap()
        .len(),
        1
    );
    fixture
        .repository
        .save_state_on(&mut fixture.connection, &state)
        .unwrap();
    assert!(PendingAdmissionRecoveryStatePort::load(
        &fixture.repository,
        AdmissionRecoveryTrigger::Periodic,
        0
    )
    .await
    .unwrap()
    .is_empty());
    fixture.repository.record_reads.store(0, Ordering::SeqCst);
    assert!(PendingAdmissionRecoveryStatePort::load(
        &fixture.repository,
        AdmissionRecoveryTrigger::Startup,
        0
    )
    .await
    .unwrap()
    .is_empty());
    assert_eq!(fixture.repository.record_reads.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn recovery_summary_rejects_corruption_and_locked_keys() {
    use uc_application::deps::{
        AdmissionRecoveryTrigger, PendingAdmissionRecoveryStateError,
        PendingAdmissionRecoveryStatePort,
    };
    let mut fixture = Fixture::new();
    let mut state = PersistedSpaceAdmissionRepositoryV2::fresh([0x31; 16]);
    let pending = pending_join_for_recovery(0x41);
    state.records.insert(
        [0x41; 32],
        fixture.repository.seal_new_record(&pending).unwrap(),
    );
    fixture.write_state(&state);
    assert_eq!(
        PendingAdmissionRecoveryStatePort::load(
            &fixture.repository,
            AdmissionRecoveryTrigger::Startup,
            0
        )
        .await
        .unwrap()
        .len(),
        1
    );
    fixture.storage.1.store(true, Ordering::SeqCst);
    assert!(matches!(
        PendingAdmissionRecoveryStatePort::load(
            &fixture.repository,
            AdmissionRecoveryTrigger::Periodic,
            0
        )
        .await,
        Err(PendingAdmissionRecoveryStateError::Locked)
    ));
    fixture.storage.1.store(false, Ordering::SeqCst);
    sql_query("UPDATE admission_recovery_summary SET encrypted_payload = x'010203'")
        .execute(&mut fixture.connection)
        .unwrap();
    assert!(matches!(
        PendingAdmissionRecoveryStatePort::load(
            &fixture.repository,
            AdmissionRecoveryTrigger::Periodic,
            0
        )
        .await,
        Err(PendingAdmissionRecoveryStateError::RecoveryRequired)
    ));
}
