//! 成员记录仓储的持久化性质：整条记录密文保存、重开后一致、过期提交被拒、写入失败不留半截状态。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use tempfile::TempDir;
use uc_application::deps::{
    MembershipLedgerError, MembershipRecord, MembershipRecordCommit, MembershipRecordStorePort,
    SpaceMembershipRecord,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipCredential, MembershipLedger,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_infra::db::executor::DieselSqliteExecutor;
use uc_infra::db::pool::init_db_pool;
use uc_infra::db::ports::DbExecutor;
use uc_infra::security::AdmissionKeyManager;
use uc_infra::space::SqliteMembershipRecordStore;
use uc_infra::time::SystemClock;

const SENSITIVE_DEVICE_NAME: &str = "sensitive-membership-device-name-marker";

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

/// 测试历史使用占位凭据，签名校验不在本测试范围内。
struct AcceptingVerifier;

impl HistoricalMembershipSignatureVerifier for AcceptingVerifier {
    fn verify(
        &self,
        _: u16,
        _: &[u8],
        _: &[u8],
        _: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

type Store = SqliteMembershipRecordStore<Arc<DieselSqliteExecutor>>;

/// 单成员 Space 的记录，设备名携带敏感标记。
fn single_member_space(revision: u64) -> SpaceMembershipRecord {
    let device_id = DeviceId::new("local-private-device");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x51; 32]);
    let member = credential.member_instance_id(&device_id);
    let facts = AdmissionChangeFacts {
        member_instance: member,
        device_id,
        device_name: SENSITIVE_DEVICE_NAME.to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
            "ABCD-EFGH-IJKL-MNOP",
        )
        .unwrap(),
        transport_public_key: vec![0x52],
        transport_address_blob: vec![0x53],
        identity_signature: vec![0x54],
    };
    let history = VersionedMembershipHistory::new_single_member_root(
        "private-space".to_owned(),
        facts,
        credential,
    )
    .unwrap();
    let ledger = MembershipLedger::start(history, device_id, member, revision).unwrap();
    SpaceMembershipRecord {
        ledger: ledger.snapshot(),
        history_exchange: Default::default(),
        branch_recovery: Default::default(),
    }
}

async fn commit(
    store: &Store,
    expected_revision: u64,
    replacement: MembershipRecord,
) -> Result<(), MembershipLedgerError> {
    store
        .commit(MembershipRecordCommit {
            expected_revision,
            replacement,
            projection: None,
        })
        .await
}

fn contains(haystack: &[u8], marker: &[u8]) -> bool {
    haystack
        .windows(marker.len())
        .any(|window| window == marker)
}

struct Fixture {
    _temp: TempDir,
    db_path: PathBuf,
    secure_storage: Arc<MemorySecureStorage>,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("membership.sqlite");
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let store = Self::open(&db_path, secure_storage.clone());
        Self {
            _temp: temp,
            db_path,
            secure_storage,
            store,
        }
    }

    fn reopen(&self) -> Store {
        Self::open(&self.db_path, self.secure_storage.clone())
    }

    fn open(db_path: &PathBuf, secure_storage: Arc<MemorySecureStorage>) -> Store {
        let executor = Arc::new(DieselSqliteExecutor::new(
            init_db_pool(db_path.to_str().unwrap()).unwrap(),
        ));
        let keys = Arc::new(AdmissionKeyManager::new(secure_storage, [0x71; 16]));
        SqliteMembershipRecordStore::new(
            executor,
            keys,
            Arc::new(AcceptingVerifier),
            Arc::new(SystemClock),
        )
    }

    fn encrypted_payload(&self) -> Vec<u8> {
        #[derive(QueryableByName)]
        struct Row {
            #[diesel(sql_type = Binary)]
            encrypted_payload: Vec<u8>,
        }

        let executor =
            DieselSqliteExecutor::new(init_db_pool(self.db_path.to_str().unwrap()).unwrap());
        executor
            .run(|conn| {
                Ok(sql_query(
                    "SELECT encrypted_payload FROM membership_ledger_state WHERE singleton_id = 1",
                )
                .get_result::<Row>(conn)?
                .encrypted_payload)
            })
            .unwrap()
    }

    fn execute_sql(&self, statement: &str) {
        let executor =
            DieselSqliteExecutor::new(init_db_pool(self.db_path.to_str().unwrap()).unwrap());
        executor
            .run(|conn| {
                sql_query(statement).execute(conn)?;
                Ok(())
            })
            .unwrap();
    }
}

#[tokio::test]
async fn the_whole_record_is_encrypted_and_survives_reopen() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.store.load().unwrap(),
        MembershipRecord::NoSpace { revision: 0 }
    );
    let record = MembershipRecord::Space(Box::new(single_member_space(1)));

    commit(&fixture.store, 0, record.clone()).await.unwrap();

    assert_eq!(fixture.reopen().load().unwrap(), record);
    let marker = SENSITIVE_DEVICE_NAME.as_bytes();
    assert!(!contains(&fixture.encrypted_payload(), marker));
    for path in [
        fixture.db_path.clone(),
        PathBuf::from(format!("{}-wal", fixture.db_path.display())),
        PathBuf::from(format!("{}-shm", fixture.db_path.display())),
    ] {
        if path.exists() {
            assert!(!contains(&std::fs::read(path).unwrap(), marker));
        }
    }
    // 记录的调试输出同样不包含设备资料。
    assert!(!format!("{record:?}").contains(SENSITIVE_DEVICE_NAME));
}

#[tokio::test]
async fn a_stale_commit_is_rejected() {
    let fixture = Fixture::new();
    let record = MembershipRecord::Space(Box::new(single_member_space(1)));
    commit(&fixture.store, 0, record.clone()).await.unwrap();

    let stale = commit(&fixture.store, 0, MembershipRecord::NoSpace { revision: 2 }).await;

    assert!(matches!(stale, Err(MembershipLedgerError::Conflict)));
    assert_eq!(fixture.reopen().load().unwrap(), record);
}

#[tokio::test]
async fn a_failed_write_keeps_the_previous_record_whole() {
    let fixture = Fixture::new();
    let initial = MembershipRecord::Space(Box::new(single_member_space(1)));
    commit(&fixture.store, 0, initial.clone()).await.unwrap();
    let replacement = MembershipRecord::NoSpace { revision: 2 };
    fixture.execute_sql(
        "CREATE TRIGGER fail_membership_record_update \
         BEFORE UPDATE ON membership_ledger_state \
         BEGIN SELECT RAISE(ABORT, 'injected membership record failure'); END",
    );

    let failed = commit(&fixture.store, 1, replacement.clone()).await;

    assert!(matches!(
        failed,
        Err(MembershipLedgerError::Unavailable { .. })
    ));
    assert_eq!(fixture.reopen().load().unwrap(), initial);

    fixture.execute_sql("DROP TRIGGER fail_membership_record_update");
    commit(&fixture.store, 1, replacement.clone())
        .await
        .unwrap();
    assert_eq!(fixture.reopen().load().unwrap(), replacement);
}
