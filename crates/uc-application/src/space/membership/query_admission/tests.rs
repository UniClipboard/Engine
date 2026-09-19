use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipActivationBaselineV2,
    MembershipAdmissionDecision, MembershipCredential, MembershipEventId,
    MembershipHistoryRelationship, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use super::*;
use crate::space::membership::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger, MembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation, PeerHistorySyncOutcome,
    PeerReconciliationRecord,
};

struct CountingRepository {
    loaded: LoadedMembershipLedger,
    loads: AtomicUsize,
}

#[async_trait]
impl LoadMembershipLedgerPort for CountingRepository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.loaded.clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for CountingRepository {
    async fn compare_and_commit(
        &self,
        _mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Err(MembershipLedgerError::Unavailable)
    }
}

struct AcceptingVerifier;

impl HistoricalMembershipSignatureVerifier for AcceptingVerifier {
    fn verify(
        &self,
        _signature_algorithm_version: u16,
        _public_key: &[u8],
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError> {
        Ok(true)
    }
}

fn member_facts(device: &str, credential_byte: u8) -> (AdmissionChangeFacts, MembershipCredential) {
    let device_id = DeviceId::new(device);
    let credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![credential_byte; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    (
        AdmissionChangeFacts {
            member_instance,
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

fn active_ledger() -> LoadedMembershipLedger {
    let (local_facts, local_credential) = member_facts("device-a", 0x41);
    let (peer_facts, peer_credential) = member_facts("device-b", 0x42);
    let local_member = local_facts.member_instance;
    let peer_device_id = peer_facts.device_id.clone();
    let history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: vec![
                (local_facts.clone(), local_credential),
                (peer_facts, peer_credential),
            ],
        },
    )
    .unwrap();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 7;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local_facts.device_id);
    loaded.local_member_instance = Some(local_member);
    loaded.local_join_active = true;
    loaded.peer_reconciliation.insert(
        peer_device_id.clone(),
        PeerReconciliationRecord {
            peer_device_id,
            relationship: MembershipHistoryRelationship::Consistent,
            confirmed_position: None,
            sync_state: Default::default(),
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    loaded
}

#[tokio::test]
async fn admission_snapshot_returns_generation_and_decision_from_one_load() {
    let repository = Arc::new(CountingRepository {
        loaded: active_ledger(),
        loads: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let query = QueryMembershipAdmissionUseCase::new(ledger);

    let snapshot = query.query_membership_admission(Some(7)).await.unwrap();

    assert_eq!(snapshot.current_generation, 7);
    assert_eq!(snapshot.decision, MembershipAdmissionDecision::Allowed);
    assert_eq!(repository.loads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn relationship_for_a_non_member_does_not_block_a_new_invitation() {
    let mut loaded = active_ledger();
    let former_peer = DeviceId::new("former-peer");
    loaded.peer_reconciliation.insert(
        former_peer.clone(),
        PeerReconciliationRecord {
            peer_device_id: former_peer,
            relationship: MembershipHistoryRelationship::Diverged,
            confirmed_position: None,
            sync_state: Default::default(),
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    let repository = Arc::new(CountingRepository {
        loaded,
        loads: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let query = QueryMembershipAdmissionUseCase::new(ledger);

    let snapshot = query.query_membership_admission(Some(7)).await.unwrap();

    assert_eq!(snapshot.decision, MembershipAdmissionDecision::Allowed);
}

#[tokio::test]
async fn offline_peer_awaiting_the_latest_history_does_not_block_a_new_invitation() {
    let mut loaded = active_ledger();
    let peer = loaded
        .peer_reconciliation
        .get_mut(&DeviceId::new("device-b"))
        .unwrap();
    peer.confirmed_position = None;
    peer.sync_state.pending_since_revision = Some(7);
    peer.sync_state.retry_attempt = 3;
    peer.sync_state.next_attempt_at_ms = 60_000;
    peer.sync_state.last_attempt_outcome = PeerHistorySyncOutcome::Deferred;
    let repository = Arc::new(CountingRepository {
        loaded,
        loads: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));
    let query = QueryMembershipAdmissionUseCase::new(ledger);

    let snapshot = query.query_membership_admission(Some(7)).await.unwrap();

    assert_eq!(snapshot.decision, MembershipAdmissionDecision::Allowed);
}

#[tokio::test]
async fn unsafe_active_peer_relationships_block_a_new_invitation() {
    for relationship in [
        MembershipHistoryRelationship::Unknown,
        MembershipHistoryRelationship::PendingRemovalDecision,
        MembershipHistoryRelationship::Diverged,
        MembershipHistoryRelationship::Invalid,
        MembershipHistoryRelationship::UpgradeRequired,
    ] {
        let mut loaded = active_ledger();
        loaded
            .peer_reconciliation
            .get_mut(&DeviceId::new("device-b"))
            .unwrap()
            .relationship = relationship;
        let repository = Arc::new(CountingRepository {
            loaded,
            loads: AtomicUsize::new(0),
        });
        let ledger = Arc::new(MembershipLedger::new(
            repository.clone(),
            repository,
            Arc::new(AcceptingVerifier),
        ));
        let query = QueryMembershipAdmissionUseCase::new(ledger);

        let snapshot = query.query_membership_admission(Some(7)).await.unwrap();

        assert_eq!(
            snapshot.decision,
            MembershipAdmissionDecision::AwaitingConvergence,
            "relationship {relationship:?} must keep admission closed"
        );
    }
}
