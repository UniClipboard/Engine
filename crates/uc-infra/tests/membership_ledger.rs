use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use uc_application::deps::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipEffectKind, MembershipEffectPhase, MembershipLedgerError, MembershipLedgerMutation,
    PeerHistorySyncState, PeerReconciliationRecord, PendingMembershipEffect,
};
use uc_core::ids::DeviceId;
use uc_core::membership::MembershipHistoryRelationship;
use uc_core::ports::{SecureStorageError, SecureStoragePort};
use uc_infra::db::executor::DieselSqliteExecutor;
use uc_infra::db::pool::init_db_pool;
use uc_infra::db::ports::DbExecutor;
use uc_infra::security::AdmissionKeyManager;
use uc_infra::space::SqliteMembershipLedger;

const SENSITIVE_HISTORY: &[u8] = b"sensitive-membership-history-marker";

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

struct Fixture {
    _temp: TempDir,
    db_path: PathBuf,
    secure_storage: Arc<MemorySecureStorage>,
    ledger: SqliteMembershipLedger<Arc<DieselSqliteExecutor>>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("membership.sqlite");
        let secure_storage = Arc::new(MemorySecureStorage::default());
        let ledger = Self::open(&db_path, secure_storage.clone());
        Self {
            _temp: temp,
            db_path,
            secure_storage,
            ledger,
        }
    }

    fn reopen(&self) -> SqliteMembershipLedger<Arc<DieselSqliteExecutor>> {
        Self::open(&self.db_path, self.secure_storage.clone())
    }

    fn open(
        db_path: &PathBuf,
        secure_storage: Arc<MemorySecureStorage>,
    ) -> SqliteMembershipLedger<Arc<DieselSqliteExecutor>> {
        let executor = Arc::new(DieselSqliteExecutor::new(
            init_db_pool(db_path.to_str().unwrap()).unwrap(),
        ));
        let keys = Arc::new(AdmissionKeyManager::new(secure_storage, [0x71; 16]));
        SqliteMembershipLedger::new(executor, keys)
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
async fn encrypted_ledger_survives_reopen_and_rejects_stale_commit() {
    let fixture = Fixture::new();
    let initial = fixture.ledger.load().await.unwrap();
    assert_eq!(initial, LoadedMembershipLedger::no_current_space());

    let mut replacement = initial.clone();
    replacement.revision = 1;
    replacement.membership_history = Some(SENSITIVE_HISTORY.to_vec());
    let committed = fixture
        .ledger
        .compare_and_commit(MembershipLedgerMutation {
            expected_revision: 0,
            expected_history_digest: None,
            replacement: replacement.clone(),
        })
        .await
        .unwrap();
    assert_eq!(committed, replacement);

    let encrypted = fixture.encrypted_payload();
    assert!(!encrypted
        .windows(SENSITIVE_HISTORY.len())
        .any(|window| window == SENSITIVE_HISTORY));
    assert_eq!(fixture.reopen().load().await.unwrap(), replacement);

    let stale = fixture
        .ledger
        .compare_and_commit(MembershipLedgerMutation {
            expected_revision: 0,
            expected_history_digest: None,
            replacement: LoadedMembershipLedger::no_current_space(),
        })
        .await;
    assert_eq!(stale, Err(MembershipLedgerError::Conflict));
}

#[tokio::test]
async fn sqlite_failure_keeps_history_fanout_and_effects_in_one_atomic_state() {
    let fixture = Fixture::new();
    let mut initial = fixture.ledger.load().await.unwrap();
    initial.revision = 1;
    initial.membership_history = Some(b"old-encrypted-history-state".to_vec());
    fixture
        .ledger
        .compare_and_commit(MembershipLedgerMutation {
            expected_revision: 0,
            expected_history_digest: None,
            replacement: initial.clone(),
        })
        .await
        .unwrap();

    let peer = DeviceId::new("fault-injection-peer");
    let mut replacement = initial.clone();
    replacement.revision = 2;
    replacement.membership_history = Some(b"new-encrypted-history-state".to_vec());
    replacement.peer_reconciliation.insert(
        peer.clone(),
        PeerReconciliationRecord {
            peer_device_id: peer.clone(),
            relationship: MembershipHistoryRelationship::Consistent,
            confirmed_position: None,
            sync_state: PeerHistorySyncState {
                pending_since_revision: Some(2),
                ..Default::default()
            },
            restricted_delivery: Vec::new(),
            updated_at_ms: 0,
        },
    );
    replacement.history_sync_cursor = Some(peer.clone());
    replacement.pending_effects.insert(
        [0x31; 32],
        PendingMembershipEffect {
            event_id: [0x31; 32],
            kind: MembershipEffectKind::AddDevice,
            phase: MembershipEffectPhase::Prepared,
            affected_device_ids: vec![peer],
            payload: b"encrypted-effect-payload".to_vec(),
        },
    );
    let mutation = MembershipLedgerMutation {
        expected_revision: 1,
        expected_history_digest: Some(<[u8; 32]>::from(Sha256::digest(
            initial.membership_history.as_deref().unwrap(),
        ))),
        replacement: replacement.clone(),
    };
    fixture.execute_sql(
        "CREATE TRIGGER fail_membership_ledger_update \
         BEFORE UPDATE ON membership_ledger_state \
         BEGIN SELECT RAISE(ABORT, 'injected membership ledger failure'); END",
    );

    let failed = fixture.ledger.compare_and_commit(mutation.clone()).await;

    assert_eq!(failed, Err(MembershipLedgerError::Unavailable));
    assert_eq!(fixture.reopen().load().await.unwrap(), initial);

    fixture.execute_sql("DROP TRIGGER fail_membership_ledger_update");
    assert_eq!(
        fixture.ledger.compare_and_commit(mutation).await.unwrap(),
        replacement
    );
    assert_eq!(fixture.reopen().load().await.unwrap(), replacement);
}
