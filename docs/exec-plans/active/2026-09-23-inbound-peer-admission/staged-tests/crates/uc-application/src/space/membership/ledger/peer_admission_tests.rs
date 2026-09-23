//! 网络准入与公开“可用”来自同一判定（切片 S3 暂存验收；落地路径即本文件在仓库中的相对路径）。
//!
//! 在 `ledger/mod.rs` 中以 `#[cfg(test)] mod peer_admission_tests;` 声明。
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipActivationBaselineV2, MembershipCredential,
    MembershipEventId, MembershipHistoryRelationship, PeerAdmissionError, PeerAdmissionPort,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use super::{
    CommitMembershipLedgerPort, CurrentSpaceMemberScopePort, LoadMembershipLedgerPort,
    LoadedMembershipLedger, MembershipLedger, MembershipLedgerError, MembershipLedgerMutation,
    PeerReconciliationRecord,
};
use crate::deps::build_membership_peer_admission;

const PEER: &str = "device-b";

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

/// `current_revision()` 保持默认 `None`，模拟生产中网络准入读取的原始 SQLite 账本：
/// 没有版本来源时，准入绝不能依赖缓存。
struct Repository {
    loaded: Mutex<Result<LoadedMembershipLedger, MembershipLedgerError>>,
}

impl Repository {
    fn new(loaded: LoadedMembershipLedger) -> Arc<Self> {
        Arc::new(Self {
            loaded: Mutex::new(Ok(loaded)),
        })
    }
    fn failing(error: MembershipLedgerError) -> Arc<Self> {
        Arc::new(Self {
            loaded: Mutex::new(Err(error)),
        })
    }
    fn update(&self, change: impl FnOnce(&mut LoadedMembershipLedger)) {
        let mut guard = self.loaded.lock().unwrap();
        change(guard.as_mut().unwrap());
    }
}

#[async_trait]
impl LoadMembershipLedgerPort for Repository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        self.loaded.lock().unwrap().clone()
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for Repository {
    async fn compare_and_commit(
        &self,
        _: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        panic!("admission is read-only")
    }
}

fn facts(device: &str, byte: u8) -> (AdmissionChangeFacts, MembershipCredential) {
    let device_id = DeviceId::new(device);
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![byte; 32]);
    (
        AdmissionChangeFacts {
            member_instance: credential.member_instance_id(&device_id),
            device_id,
            device_name: device.to_owned(),
            identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
                "ABCD-EFGH-IJKL-MNOP",
            )
            .unwrap(),
            transport_public_key: vec![1],
            transport_address_blob: vec![2],
            identity_signature: vec![3],
        },
        credential,
    )
}

fn consistent(device: &str) -> PeerReconciliationRecord {
    PeerReconciliationRecord {
        peer_device_id: DeviceId::new(device),
        relationship: MembershipHistoryRelationship::Consistent,
        confirmed_position: None,
        sync_state: Default::default(),
        restricted_delivery: Vec::new(),
        updated_at_ms: 1,
    }
}

/// `include_local=false` 构造“本机仍标记加入、但已不在当前成员中”的账本。
fn ledger(include_local: bool) -> LoadedMembershipLedger {
    let (local, local_credential) = facts("device-a", 0x41);
    let (peer, peer_credential) = facts(PEER, 0x42);
    let (third, third_credential) = facts("device-c", 0x43);
    let mut members = vec![(peer, peer_credential), (third, third_credential)];
    if include_local {
        members.insert(0, (local.clone(), local_credential));
    }
    let history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: members,
        },
    )
    .unwrap();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 8;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local.device_id);
    loaded.local_member_instance = Some(local.member_instance);
    loaded.local_join_active = true;
    loaded
        .peer_reconciliation
        .insert(DeviceId::new(PEER), consistent(PEER));
    loaded
}

fn membership(repository: &Arc<Repository>) -> MembershipLedger {
    MembershipLedger::new(
        Arc::clone(repository) as Arc<dyn LoadMembershipLedgerPort>,
        Arc::clone(repository) as Arc<dyn CommitMembershipLedgerPort>,
        Arc::new(AcceptingVerifier),
    )
}

async fn admitted(ledger: &MembershipLedger, device: &str) -> bool {
    ledger.is_admitted(&DeviceId::new(device)).await.unwrap()
}

#[tokio::test]
async fn a_consistent_active_peer_is_admitted_and_usable() {
    let repository = Repository::new(ledger(true));
    let ledger = membership(&repository);
    assert!(admitted(&ledger, PEER).await);
    let scope = ledger.snapshot().await.unwrap();
    assert_eq!(scope.usable_peer_device_ids, vec![DeviceId::new(PEER)]);
}

#[tokio::test]
async fn an_unconfirmed_relationship_is_not_admitted() {
    let repository = Repository::new(ledger(true));
    let ledger = membership(&repository);
    assert!(
        !admitted(&ledger, "device-c").await,
        "no reconciliation record"
    );
    repository.update(|loaded| {
        loaded
            .peer_reconciliation
            .get_mut(&DeviceId::new(PEER))
            .unwrap()
            .relationship = MembershipHistoryRelationship::Unknown;
    });
    assert!(!admitted(&ledger, PEER).await);
}

#[tokio::test]
async fn a_local_device_outside_the_current_members_admits_nobody() {
    // 旧的 Infra 规则只看 local_join_active 与 lineage，会在这里放行；公开状态却显示本机已失效。
    let repository = Repository::new(ledger(false));
    let ledger = membership(&repository);
    assert!(!ledger.snapshot().await.unwrap().local_member_active);
    assert!(!admitted(&ledger, PEER).await);
}

#[tokio::test]
async fn an_inactive_local_join_admits_nobody() {
    let mut loaded = ledger(true);
    loaded.local_join_active = false;
    let repository = Repository::new(loaded);
    assert!(!admitted(&membership(&repository), PEER).await);
}

#[tokio::test]
async fn a_record_for_a_device_outside_the_history_is_not_admitted() {
    let repository = Repository::new(ledger(true));
    repository.update(|loaded| {
        loaded
            .peer_reconciliation
            .insert(DeviceId::new("device-z"), consistent("device-z"));
    });
    assert!(!admitted(&membership(&repository), "device-z").await);
}

#[tokio::test]
async fn no_current_space_admits_nobody_without_error() {
    let repository = Repository::new(LoadedMembershipLedger::no_current_space());
    assert!(!admitted(&membership(&repository), PEER).await);
}

#[tokio::test]
async fn unreadable_or_corrupt_ledgers_fail_closed_with_distinct_errors() {
    let locked = membership(&Repository::failing(MembershipLedgerError::Locked));
    assert!(matches!(
        locked.is_admitted(&DeviceId::new(PEER)).await,
        Err(PeerAdmissionError::Unavailable)
    ));

    let mut corrupt = ledger(true);
    corrupt.membership_history = Some(vec![0xde, 0xad]);
    let corrupt = membership(&Repository::new(corrupt));
    assert!(matches!(
        corrupt.is_admitted(&DeviceId::new(PEER)).await,
        Err(PeerAdmissionError::InvalidState)
    ));
}

#[tokio::test]
async fn every_decision_reads_the_current_record_without_notification() {
    let repository = Repository::new(ledger(true));
    let ledger = membership(&repository);
    assert!(admitted(&ledger, PEER).await);
    repository.update(|loaded| {
        loaded.revision += 1;
        loaded
            .peer_reconciliation
            .get_mut(&DeviceId::new(PEER))
            .unwrap()
            .relationship = MembershipHistoryRelationship::Diverged;
    });
    assert!(
        !admitted(&ledger, PEER).await,
        "a revision change without a known revision source must not be hidden by a cache"
    );
}

#[tokio::test]
async fn usable_peers_are_always_admitted() {
    for loaded in [ledger(true), ledger(false)] {
        let repository = Repository::new(loaded);
        let ledger = membership(&repository);
        let scope = ledger.snapshot().await.unwrap();
        for device in &scope.usable_peer_device_ids {
            assert!(
                ledger.is_admitted(device).await.unwrap(),
                "{device:?} is usable but not admitted"
            );
        }
    }
}

#[tokio::test]
async fn the_engine_facing_builder_applies_the_same_rule() {
    let repository = Repository::new(ledger(false));
    let port = build_membership_peer_admission(
        Arc::clone(&repository) as Arc<dyn LoadMembershipLedgerPort>,
        Arc::new(AcceptingVerifier),
    );
    assert!(!port.is_admitted(&DeviceId::new(PEER)).await.unwrap());
    let repository = Repository::new(ledger(true));
    let port = build_membership_peer_admission(
        Arc::clone(&repository) as Arc<dyn LoadMembershipLedgerPort>,
        Arc::new(AcceptingVerifier),
    );
    assert!(port.is_admitted(&DeviceId::new(PEER)).await.unwrap());
}
