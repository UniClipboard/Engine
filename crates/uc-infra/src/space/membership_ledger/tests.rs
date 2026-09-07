use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::*;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::init_db_pool;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

#[derive(Default)]
struct MemoryStorage(Mutex<BTreeMap<String, Vec<u8>>>);

impl SecureStoragePort for MemoryStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, SecureStorageError> {
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

#[tokio::test]
async fn encrypted_legacy_rows_upgrade_to_v4_on_commit() {
    for (version, empty_fields) in [(1, 9), (2, 14), (3, 15)] {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("ledger.sqlite");
        let storage = Arc::new(MemoryStorage::default());
        let keys = Arc::new(AdmissionKeyManager::new(storage.clone(), [0x71; 16]));
        let executor = Arc::new(DieselSqliteExecutor::new(
            init_db_pool(db.to_str().unwrap()).unwrap(),
        ));
        // 固定旧布局字节，不借用新 LoadedMembershipLedger 的序列化生成旧样本。
        let mut old = vec![version];
        old.extend_from_slice(&[0x71; 16]);
        old.push(7);
        old.extend(std::iter::repeat_n(0, empty_fields));
        let encrypted = keys
            .seal_profile_payload(MEMBERSHIP_LEDGER_PURPOSE, &old)
            .unwrap();
        executor.run(|conn| { sql_query("INSERT INTO membership_ledger_state (singleton_id, encrypted_payload) VALUES (1, ?)")
            .bind::<Binary, _>(&encrypted).execute(conn)?; Ok(()) }).unwrap();
        let ledger = SqliteMembershipLedger::new(executor.clone(), keys.clone());
        let mut current = ledger.load().await.unwrap();
        assert_eq!(current.revision, 7);
        assert!(current.membership_conflict_presentations.is_empty());
        current.revision = 8;
        ledger
            .compare_and_commit(MembershipLedgerMutation {
                expected_revision: 7,
                expected_history_digest: None,
                replacement: current.clone(),
            })
            .await
            .unwrap();
        let encrypted = executor
            .run(|conn| {
                Ok(sql_query(
                    "SELECT encrypted_payload FROM membership_ledger_state WHERE singleton_id = 1",
                )
                .get_result::<EncryptedLedgerRow>(conn)?
                .encrypted_payload)
            })
            .unwrap();
        let plain = keys
            .open_profile_payload(MEMBERSHIP_LEDGER_PURPOSE, &encrypted)
            .unwrap();
        assert_eq!(postcard::take_from_bytes::<u16>(&plain).unwrap().0, 4);
        let reopened = SqliteMembershipLedger::new(
            Arc::new(DieselSqliteExecutor::new(
                init_db_pool(db.to_str().unwrap()).unwrap(),
            )),
            Arc::new(AdmissionKeyManager::new(storage, [0x71; 16])),
        );
        assert_eq!(reopened.load().await.unwrap(), current);
    }
}
