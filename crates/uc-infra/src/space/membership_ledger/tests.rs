use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::codec;
use super::*;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::init_db_pool;
use uc_application::deps::{MembershipEffectPhase, RestrictedMembershipDelivery};
use uc_core::ids::DeviceId;
use uc_core::membership::MembershipHistoryRelationship;
use uc_core::ports::{SecureStorageError, SecureStoragePort};

/// 计划 049 S0 固定向量写入时使用的 profile generation。
const LEGACY_FIXTURE_GENERATION: [u8; 16] = [0x49; 16];

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
                device_trust_changed: true,
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

// 计划 049 S0：固定 S2 迁移输入。样本由 Application 真实用例推进生成，这里证明它们是当前 V4
// 布局并锁定各自的残留形态；迁移实现不得改写这些文件。
#[test]
fn legacy_v4_fixtures_decode_with_recorded_residual_states() {
    let peer = DeviceId::new("device-b");
    let decode = |name: &str, bytes: &[u8]| {
        assert_eq!(
            postcard::take_from_bytes::<u16>(bytes).unwrap().0,
            4,
            "{name} must stay a V4 fixture"
        );
        codec::decode(bytes, LEGACY_FIXTURE_GENERATION)
            .unwrap_or_else(|error| panic!("{name} must decode as V4: {error}"))
    };

    let active = decode(
        "two_member_active",
        include_bytes!("fixtures/two_member_active.v4.bin"),
    );
    assert_eq!(
        active.peer_reconciliation[&peer].relationship,
        MembershipHistoryRelationship::Consistent
    );

    let notice_pending = decode(
        "sponsor_removal_notice_pending",
        include_bytes!("fixtures/sponsor_removal_notice_pending.v4.bin"),
    );
    assert!(matches!(
        notice_pending.peer_reconciliation[&peer]
            .restricted_delivery
            .as_slice(),
        [RestrictedMembershipDelivery::Event(_)]
    ));

    let notice_delivered = decode(
        "sponsor_removal_notice_delivered",
        include_bytes!("fixtures/sponsor_removal_notice_delivered.v4.bin"),
    );
    let residual = &notice_delivered.peer_reconciliation[&peer];
    assert_eq!(
        residual.relationship,
        MembershipHistoryRelationship::PendingRemovalDecision
    );
    assert!(residual.restricted_delivery.is_empty());
    assert!(notice_delivered
        .effect_journal
        .values()
        .all(|effect| effect.phase == MembershipEffectPhase::Activated));

    let pending_decision = decode(
        "local_removal_pending_decision",
        include_bytes!("fixtures/local_removal_pending_decision.v4.bin"),
    );
    assert_eq!(
        pending_decision.peer_reconciliation[&peer].relationship,
        MembershipHistoryRelationship::PendingRemovalDecision
    );

    for (name, bytes, relationship) in [
        (
            "local_removal_accepted",
            &include_bytes!("fixtures/local_removal_accepted.v4.bin")[..],
            MembershipHistoryRelationship::Consistent,
        ),
        (
            "local_removal_rejected",
            &include_bytes!("fixtures/local_removal_rejected.v4.bin")[..],
            MembershipHistoryRelationship::Diverged,
        ),
    ] {
        let decided = decode(name, bytes);
        let proposer = &decided.peer_reconciliation[&peer];
        assert_eq!(proposer.relationship, relationship, "{name}");
        assert!(
            matches!(
                proposer.restricted_delivery.as_slice(),
                [RestrictedMembershipDelivery::Decision(_)]
            ),
            "{name} must keep the undeliverable decision"
        );
    }
}
