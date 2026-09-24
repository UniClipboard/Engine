use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use uc_application::deps::{
    MembershipLedgerError, MembershipProjectionPlan, MembershipRecord, MembershipRecordCommit,
    MembershipRecordStorePort, SpaceMembershipRecord,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier, LedgerMemberStatus,
    MembershipLedger, PeerLinkSnapshot, PeerRelation,
};
use uc_core::ports::{ClockPort, SecureStorageError, SecureStoragePort};

use super::codec::{self, Decoded};
use super::store::{EncryptedRecordRow, MEMBERSHIP_RECORD_PURPOSE};
use super::SqliteMembershipRecordStore;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::{init_db_pool, DbPool};
use crate::db::ports::DbExecutor;
use crate::db::repositories::test_relationship_store;
use crate::security::AdmissionKeyManager;

/// 计划 049 S0 固定向量写入时使用的 profile generation。
const FIXTURE_GENERATION: [u8; 16] = [0x49; 16];
const NOW: i64 = 1_800_000_000_000;

const TWO_MEMBER_ACTIVE: &[u8] = include_bytes!("fixtures/two_member_active.v4.bin");
const NOTICE_PENDING: &[u8] = include_bytes!("fixtures/sponsor_removal_notice_pending.v4.bin");
const NOTICE_DELIVERED: &[u8] = include_bytes!("fixtures/sponsor_removal_notice_delivered.v4.bin");
const PENDING_DECISION: &[u8] = include_bytes!("fixtures/local_removal_pending_decision.v4.bin");
const ACCEPTED: &[u8] = include_bytes!("fixtures/local_removal_accepted.v4.bin");
const REJECTED: &[u8] = include_bytes!("fixtures/local_removal_rejected.v4.bin");

const FIXTURES: [(&str, &[u8]); 6] = [
    ("two_member_active", TWO_MEMBER_ACTIVE),
    ("sponsor_removal_notice_pending", NOTICE_PENDING),
    ("sponsor_removal_notice_delivered", NOTICE_DELIVERED),
    ("local_removal_pending_decision", PENDING_DECISION),
    ("local_removal_accepted", ACCEPTED),
    ("local_removal_rejected", REJECTED),
];

/// 固定向量中的签名是测试替身。
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

struct RejectingVerifier;

impl HistoricalMembershipSignatureVerifier for RejectingVerifier {
    fn verify(
        &self,
        _: u16,
        _: &[u8],
        _: &[u8],
        _: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(false)
    }
}

struct FixedClock(i64);

impl ClockPort for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }
}

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

fn peer() -> DeviceId {
    DeviceId::new("device-b")
}

fn legacy_revision(bytes: &[u8]) -> u64 {
    // 布局：u16 版本、16 字节 generation、varint 修订号。
    let (_, rest) = postcard::take_from_bytes::<u16>(bytes).unwrap();
    postcard::take_from_bytes::<u64>(&rest[16..]).unwrap().0
}

fn migrate(name: &str, bytes: &[u8]) -> SpaceMembershipRecord {
    match codec::decode(bytes, FIXTURE_GENERATION, &AcceptingVerifier, NOW) {
        Ok(Decoded::Migrated(MembershipRecord::Space(space))) => *space,
        Ok(Decoded::Migrated(MembershipRecord::NoSpace { .. })) => {
            panic!("{name} must keep its space")
        }
        Ok(Decoded::Current(_)) => panic!("{name} is a V4 fixture and must migrate"),
        Err(error) => panic!("{name} must migrate: {error}"),
    }
}

fn restored(record: &SpaceMembershipRecord) -> MembershipLedger {
    MembershipLedger::restore(record.ledger.clone()).unwrap()
}

fn member_relation(record: &SpaceMembershipRecord) -> (PeerRelation, bool) {
    match record.ledger.peers.get(&peer()) {
        Some(PeerLinkSnapshot::Member(member)) => {
            (member.relation, member.outgoing_decision.is_some())
        }
        other => panic!(
            "device-b must be a member, got departing={}",
            other.is_some()
        ),
    }
}

#[test]
fn legacy_v4_fixtures_migrate_according_to_the_mapping_table() {
    for (name, bytes) in FIXTURES {
        let record = migrate(name, bytes);
        assert_eq!(
            record.ledger.revision,
            legacy_revision(bytes) + 1,
            "{name}: migration is one write"
        );
        // 迁移结果满足聚合不变量，且 V5 往返不变。
        restored(&record);
        let space = MembershipRecord::Space(Box::new(record));
        let encoded = codec::encode(&space, FIXTURE_GENERATION).unwrap();
        match codec::decode(&encoded, FIXTURE_GENERATION, &AcceptingVerifier, NOW + 1) {
            Ok(Decoded::Current(decoded)) => assert_eq!(decoded, space, "{name}: V5 round trip"),
            _ => panic!("{name}: V5 must decode as current"),
        }
    }

    // 当前有效成员：沿用一致关系。
    let active = migrate("two_member_active", TWO_MEMBER_ACTIVE);
    assert_eq!(member_relation(&active), (PeerRelation::Consistent, false));
    assert_eq!(restored(&active).local_status(), LedgerMemberStatus::Active);

    // 已移除、仍有待投递通知：正在离开，窗口从迁移时刻起算。
    let pending = migrate("sponsor_removal_notice_pending", NOTICE_PENDING);
    match pending.ledger.peers.get(&peer()) {
        Some(PeerLinkSnapshot::Departing(departing)) => assert_eq!(departing.since_ms, NOW),
        _ => panic!("undelivered removal notice must become a departing peer"),
    }

    // 已移除、无待投递通知：删除残留对端记录。
    let delivered = migrate("sponsor_removal_notice_delivered", NOTICE_DELIVERED);
    assert!(delivered.ledger.peers.get(&peer()).is_none());
    assert!(delivered.ledger.effects.is_empty());

    // 存在待本机决定的移除：发起方等待本机决定。
    let awaiting = migrate("local_removal_pending_decision", PENDING_DECISION);
    assert_eq!(
        member_relation(&awaiting),
        (PeerRelation::AwaitingLocalDecision, false)
    );

    // 本机已接受以自身为目标的移除：已移除终态，不保留无法送达的决定。
    let accepted = migrate("local_removal_accepted", ACCEPTED);
    assert_eq!(
        member_relation(&accepted),
        (PeerRelation::Consistent, false)
    );
    assert_eq!(
        restored(&accepted).local_status(),
        LedgerMemberStatus::Removed
    );

    // 本机拒绝移除：分叉，不保留无法送达的决定。
    let rejected = migrate("local_removal_rejected", REJECTED);
    assert_eq!(member_relation(&rejected), (PeerRelation::Diverged, false));
}

#[test]
fn damaged_or_unknown_records_fail_with_a_stable_error() {
    let decode = |bytes: &[u8]| {
        codec::decode(bytes, FIXTURE_GENERATION, &AcceptingVerifier, NOW)
            .err()
            .expect("record must be rejected")
    };
    let mut unknown = TWO_MEMBER_ACTIVE.to_vec();
    unknown[0] = 6;
    assert!(matches!(
        decode(&unknown),
        MembershipLedgerError::Corrupt { .. }
    ));
    assert!(matches!(
        decode(&TWO_MEMBER_ACTIVE[..TWO_MEMBER_ACTIVE.len() - 1]),
        MembershipLedgerError::Corrupt { .. }
    ));
    let mut trailing = TWO_MEMBER_ACTIVE.to_vec();
    trailing.push(0);
    assert!(matches!(
        decode(&trailing),
        MembershipLedgerError::Corrupt { .. }
    ));
    assert!(matches!(decode(&[]), MembershipLedgerError::Corrupt { .. }));
    assert!(matches!(
        codec::decode(TWO_MEMBER_ACTIVE, [0x50; 16], &AcceptingVerifier, NOW)
            .err()
            .expect("wrong generation must be rejected"),
        MembershipLedgerError::Corrupt { .. }
    ));
    assert!(matches!(
        codec::decode(
            TWO_MEMBER_ACTIVE,
            FIXTURE_GENERATION,
            &RejectingVerifier,
            NOW
        )
        .err()
        .expect("unverifiable history must be rejected"),
        MembershipLedgerError::Corrupt { .. }
    ));

    let current = codec::encode(
        &MembershipRecord::Space(Box::new(migrate("two_member_active", TWO_MEMBER_ACTIVE))),
        FIXTURE_GENERATION,
    )
    .unwrap();
    let mut trailing = current.clone();
    trailing.push(0);
    assert!(matches!(
        decode(&trailing),
        MembershipLedgerError::Corrupt { .. }
    ));
    assert!(matches!(
        decode(&current[..current.len() - 1]),
        MembershipLedgerError::Corrupt { .. }
    ));
}

struct Profile {
    _temp: tempfile::TempDir,
    pool: DbPool,
    executor: Arc<DieselSqliteExecutor>,
    keys: Arc<AdmissionKeyManager>,
}

impl Profile {
    fn new(generation: [u8; 16]) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("control.sqlite");
        let pool = init_db_pool(db.to_str().unwrap()).unwrap();
        let executor = Arc::new(DieselSqliteExecutor::new(pool.clone()));
        let keys = Arc::new(AdmissionKeyManager::new(
            Arc::new(MemoryStorage::default()),
            generation,
        ));
        Self {
            _temp: temp,
            pool,
            executor,
            keys,
        }
    }

    fn store(
        &self,
        verifier: impl HistoricalMembershipSignatureVerifier + 'static,
        now_ms: i64,
    ) -> SqliteMembershipRecordStore<Arc<DieselSqliteExecutor>> {
        SqliteMembershipRecordStore::new(
            self.executor.clone(),
            self.keys.clone(),
            Arc::new(verifier),
            Arc::new(FixedClock(now_ms)),
        )
    }

    fn write_plaintext(&self, plaintext: &[u8]) {
        let encrypted = self
            .keys
            .seal_profile_payload(MEMBERSHIP_RECORD_PURPOSE, plaintext)
            .unwrap();
        self.executor
            .run(|conn| {
                sql_query(
                    "INSERT INTO membership_ledger_state (singleton_id, encrypted_payload) \
                     VALUES (1, ?)",
                )
                .bind::<Binary, _>(&encrypted)
                .execute(conn)?;
                Ok(())
            })
            .unwrap();
    }

    fn encrypted_row(&self) -> Vec<u8> {
        self.executor
            .run(|conn| {
                Ok(sql_query(
                    "SELECT encrypted_payload FROM membership_ledger_state WHERE singleton_id = 1",
                )
                .get_result::<EncryptedRecordRow>(conn)?
                .encrypted_payload)
            })
            .unwrap()
    }

    fn stored_version(&self) -> u16 {
        let plain = self
            .keys
            .open_profile_payload(MEMBERSHIP_RECORD_PURPOSE, &self.encrypted_row())
            .unwrap();
        postcard::take_from_bytes::<u16>(&plain).unwrap().0
    }
}

#[test]
fn unlock_read_migrates_once_and_rereads_the_same_v5_record() {
    let profile = Profile::new(FIXTURE_GENERATION);
    profile.write_plaintext(NOTICE_PENDING);

    let migrated = profile.store(AcceptingVerifier, NOW).load().unwrap();
    assert_eq!(profile.stored_version(), 5);
    // 再次读取时不再迁移：离开窗口起点保持首次迁移时刻。
    let reread = profile
        .store(AcceptingVerifier, NOW + 60_000)
        .load()
        .unwrap();
    assert_eq!(reread, migrated);
    let MembershipRecord::Space(space) = &reread else {
        panic!("migrated record keeps its space");
    };
    assert!(matches!(
        space.ledger.peers.get(&peer()),
        Some(PeerLinkSnapshot::Departing(departing)) if departing.since_ms == NOW
    ));
}

#[test]
fn failed_migration_leaves_the_original_row_untouched() {
    let profile = Profile::new(FIXTURE_GENERATION);
    profile.write_plaintext(TWO_MEMBER_ACTIVE);
    let before = profile.encrypted_row();

    assert!(matches!(
        profile.store(RejectingVerifier, NOW).load().unwrap_err(),
        MembershipLedgerError::Corrupt { .. }
    ));
    assert_eq!(profile.encrypted_row(), before);
    assert_eq!(profile.stored_version(), 4);
}

#[test]
fn legacy_rows_without_a_space_migrate_to_an_empty_v5_record() {
    for (version, empty_fields) in [(1u8, 9), (2, 14), (3, 15), (4, 15)] {
        let profile = Profile::new([0x71; 16]);
        // 固定旧布局字节：版本、generation、修订号 7，其余字段全部为空。
        let mut old = vec![version];
        old.extend_from_slice(&[0x71; 16]);
        old.push(7);
        old.extend(std::iter::repeat_n(0, empty_fields));
        profile.write_plaintext(&old);

        let store = profile.store(AcceptingVerifier, NOW);
        assert_eq!(
            store.load().unwrap(),
            MembershipRecord::NoSpace { revision: 8 },
            "V{version}"
        );
        assert_eq!(profile.stored_version(), 5, "V{version}");
        assert_eq!(
            store.load().unwrap(),
            MembershipRecord::NoSpace { revision: 8 }
        );
    }
}

#[test]
fn commit_requires_the_expected_revision_and_a_larger_replacement() {
    let profile = Profile::new(FIXTURE_GENERATION);
    let store = profile.store(AcceptingVerifier, NOW);
    assert_eq!(
        store.load().unwrap(),
        MembershipRecord::NoSpace { revision: 0 }
    );

    assert!(matches!(
        store
            .commit_record(0, &MembershipRecord::NoSpace { revision: 0 }, None)
            .unwrap_err(),
        MembershipLedgerError::Conflict
    ));
    // 一次提交可以包含多项账本转换，修订号只要求增大。
    store
        .commit_record(0, &MembershipRecord::NoSpace { revision: 2 }, None)
        .unwrap();
    assert!(matches!(
        store
            .commit_record(0, &MembershipRecord::NoSpace { revision: 3 }, None)
            .unwrap_err(),
        MembershipLedgerError::Conflict
    ));

    let mut space = migrate("two_member_active", TWO_MEMBER_ACTIVE);
    space.ledger.revision = 3;
    let record = MembershipRecord::Space(Box::new(space));
    store.commit_record(2, &record, None).unwrap();
    assert_eq!(store.load().unwrap(), record);
}

/// 按账本当前成员形成的读模型计划：全部有效成员，本机以外的都可信。
fn projection_of(record: &SpaceMembershipRecord) -> MembershipProjectionPlan {
    let ledger = restored(record);
    let history = ledger.history();
    let members: Vec<_> = history
        .effective_members()
        .into_iter()
        .map(|member| history.admission_facts_for(member).unwrap().clone())
        .collect();
    MembershipProjectionPlan {
        local_device_id: *ledger.local_device_id(),
        trusted_device_ids: members
            .iter()
            .map(|facts| facts.device_id)
            .filter(|device| device != ledger.local_device_id())
            .collect(),
        members,
    }
}

#[tokio::test]
async fn the_read_model_is_written_in_the_record_transaction() {
    let profile = Profile::new(FIXTURE_GENERATION);
    let relationships = test_relationship_store(profile.pool.clone());
    let store = profile
        .store(AcceptingVerifier, NOW)
        .with_projection(relationships.clone());
    let mut space = migrate("two_member_active", TWO_MEMBER_ACTIVE);
    space.ledger.revision = 1;
    let projection = projection_of(&space);
    let replacement = MembershipRecord::Space(Box::new(space));

    let conflicting = MembershipRecordStorePort::commit(
        &store,
        MembershipRecordCommit {
            expected_revision: 7,
            replacement: replacement.clone(),
            projection: Some(projection.clone()),
        },
    )
    .await;
    assert!(matches!(conflicting, Err(MembershipLedgerError::Conflict)));
    assert!(relationships.list_members().await.unwrap().is_empty());

    MembershipRecordStorePort::commit(
        &store,
        MembershipRecordCommit {
            expected_revision: 0,
            replacement: replacement.clone(),
            projection: Some(projection.clone()),
        },
    )
    .await
    .unwrap();

    assert_eq!(store.load().unwrap(), replacement);
    let members: Vec<DeviceId> = relationships
        .list_members()
        .await
        .unwrap()
        .into_iter()
        .map(|member| member.device_id)
        .collect();
    assert_eq!(members.len(), projection.members.len());
    let trusted: Vec<DeviceId> = relationships
        .list_trusted_peers()
        .await
        .unwrap()
        .into_iter()
        .map(|peer| peer.peer_device_id)
        .collect();
    assert_eq!(
        trusted
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        projection.trusted_device_ids
    );
}

#[tokio::test]
async fn a_read_model_plan_without_a_read_model_store_writes_nothing() {
    let profile = Profile::new(FIXTURE_GENERATION);
    let store = profile.store(AcceptingVerifier, NOW);
    let mut space = migrate("two_member_active", TWO_MEMBER_ACTIVE);
    space.ledger.revision = 1;
    let projection = projection_of(&space);

    let result = MembershipRecordStorePort::commit(
        &store,
        MembershipRecordCommit {
            expected_revision: 0,
            replacement: MembershipRecord::Space(Box::new(space)),
            projection: Some(projection),
        },
    )
    .await;

    assert!(matches!(
        result,
        Err(MembershipLedgerError::Unavailable { .. })
    ));
    assert_eq!(
        store.load().unwrap(),
        MembershipRecord::NoSpace { revision: 0 }
    );
}
