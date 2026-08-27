use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionProfileMetadata, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, JoinerAdmissionStage, MembershipActivationBaselineV2,
    MembershipCredential, MembershipEventId, MembershipHistoryRelationship, SpaceJoinRecord,
    SpaceJoinRecordId, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use super::*;
use crate::space::admission::{
    AcceptAdmissionError, ConsumedInvitation, InboundAdmissionStatePort, LoadMemberAdmissionError,
    PreparedMemberAdmissionActivation,
};

#[derive(Clone)]
struct MemoryLedgerRepository {
    loaded: Arc<Mutex<LoadedMembershipLedger>>,
    commit_calls: Arc<AtomicUsize>,
}

impl MemoryLedgerRepository {
    fn new(loaded: LoadedMembershipLedger) -> Self {
        Self {
            loaded: Arc::new(Mutex::new(loaded)),
            commit_calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl LoadMembershipLedgerPort for MemoryLedgerRepository {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        self.loaded
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)
            .map(|loaded| loaded.clone())
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryLedgerRepository {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        self.commit_calls.fetch_add(1, Ordering::SeqCst);
        let mut loaded = self
            .loaded
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)?;
        if loaded.revision != mutation.expected_revision {
            return Err(MembershipLedgerError::Conflict);
        }
        let current_history_digest = loaded
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        if current_history_digest != mutation.expected_history_digest {
            return Err(MembershipLedgerError::Conflict);
        }
        *loaded = mutation.replacement;
        Ok(loaded.clone())
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

fn active_single_member_ledger() -> LoadedMembershipLedger {
    let device_id = DeviceId::new("device-a");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x41; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    let facts = AdmissionChangeFacts {
        member_instance,
        device_id: device_id.clone(),
        device_name: "Device A".to_owned(),
        identity_fingerprint: uc_core::security::IdentityFingerprint::from_display_string(
            "ABCD-EFGH-IJKL-MNOP",
        )
        .unwrap(),
        transport_public_key: vec![1],
        transport_address_blob: vec![2],
        identity_signature: vec![3],
    };
    let history =
        VersionedMembershipHistory::new_single_member_root("space-a".to_owned(), facts, credential)
            .unwrap();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 7;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(device_id);
    loaded.local_member_instance = Some(member_instance);
    loaded.local_join_active = true;
    loaded
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

fn active_two_member_ledger() -> LoadedMembershipLedger {
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
    loaded.revision = 8;
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
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    loaded
}

#[tokio::test]
async fn no_current_space_has_no_authorized_scope() {
    let repository = Arc::new(MemoryLedgerRepository::new(
        LoadedMembershipLedger::no_current_space(),
    ));
    let ledger = MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));

    let error = ledger.current_scope().await.unwrap_err();

    assert_eq!(error, CurrentSpaceMemberScopeError::NoCurrentSpace);
}

#[tokio::test]
async fn active_v2_root_authorizes_only_the_local_member() {
    let repository = Arc::new(MemoryLedgerRepository::new(active_single_member_ledger()));
    let ledger = MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));

    let scope = ledger.current_scope().await.unwrap();

    assert_eq!(scope.revision, 7);
    assert!(scope.local_member_active);
    assert!(scope.usable_peer_device_ids.is_empty());
    assert!(scope.paused_peer_devices.is_empty());
}

#[tokio::test]
async fn consistent_v2_member_is_an_authorized_peer() {
    let repository = Arc::new(MemoryLedgerRepository::new(active_two_member_ledger()));
    let ledger = MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));

    let scope = ledger.current_scope().await.unwrap();

    assert_eq!(
        scope.usable_peer_device_ids,
        vec![DeviceId::new("device-b")]
    );
    assert!(scope.paused_peer_devices.is_empty());
}

#[tokio::test]
async fn pending_local_decision_pauses_only_that_peer() {
    let mut loaded = active_two_member_ledger();
    loaded
        .peer_reconciliation
        .get_mut(&DeviceId::new("device-b"))
        .unwrap()
        .relationship = MembershipHistoryRelationship::PendingRemovalDecision;
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));

    let scope = ledger.current_scope().await.unwrap();

    assert!(scope.usable_peer_device_ids.is_empty());
    assert_eq!(
        scope.paused_peer_devices,
        vec![PausedSpaceMember {
            device_id: DeviceId::new("device-b"),
            reason: SpaceMemberPauseReason::PendingLocalDecision,
        }]
    );
}

#[tokio::test]
async fn prepared_effect_keeps_the_affected_peer_paused() {
    let mut loaded = active_two_member_ledger();
    loaded.pending_effects.insert(
        [0x71; 32],
        PendingMembershipEffect {
            event_id: [0x71; 32],
            kind: MembershipEffectKind::AddDevice,
            phase: MembershipEffectPhase::Prepared,
            affected_device_ids: vec![DeviceId::new("device-b")],
            payload: vec![0x72],
        },
    );
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));

    let scope = ledger.current_scope().await.unwrap();

    assert!(scope.usable_peer_device_ids.is_empty());
    assert_eq!(
        scope.paused_peer_devices,
        vec![PausedSpaceMember {
            device_id: DeviceId::new("device-b"),
            reason: SpaceMemberPauseReason::EffectPending,
        }]
    );
}

#[tokio::test]
async fn inactive_local_join_closes_the_entire_peer_scope() {
    let mut loaded = active_two_member_ledger();
    loaded.local_join_active = false;
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));

    let scope = ledger.current_scope().await.unwrap();

    assert!(!scope.local_member_active);
    assert!(scope.usable_peer_device_ids.is_empty());
    assert_eq!(
        scope.paused_peer_devices,
        vec![PausedSpaceMember {
            device_id: DeviceId::new("device-b"),
            reason: SpaceMemberPauseReason::LocalMemberInactive,
        }]
    );
}

#[tokio::test]
async fn restricted_relationships_keep_their_stable_pause_reason() {
    let cases = [
        (
            MembershipHistoryRelationship::Diverged,
            SpaceMemberPauseReason::Diverged,
        ),
        (
            MembershipHistoryRelationship::Invalid,
            SpaceMemberPauseReason::Invalid,
        ),
        (
            MembershipHistoryRelationship::UpgradeRequired,
            SpaceMemberPauseReason::UpgradeRequired,
        ),
    ];
    for (relationship, expected_reason) in cases {
        let mut loaded = active_two_member_ledger();
        loaded
            .peer_reconciliation
            .get_mut(&DeviceId::new("device-b"))
            .unwrap()
            .relationship = relationship;
        let repository = Arc::new(MemoryLedgerRepository::new(loaded));
        let ledger =
            MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));

        let scope = ledger.current_scope().await.unwrap();

        assert!(scope.usable_peer_device_ids.is_empty());
        assert_eq!(scope.paused_peer_devices[0].reason, expected_reason);
    }
}

#[tokio::test]
async fn revision_overflow_is_rejected_before_persistence() {
    let mut loaded = active_single_member_ledger();
    loaded.revision = u64::MAX;
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );

    let error = ledger.compare_and_commit(|_| Ok(())).await.unwrap_err();

    assert_eq!(error, MembershipLedgerError::Corrupt);
    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn consumers_read_scope_through_one_snapshot_port() {
    let repository = Arc::new(MemoryLedgerRepository::new(active_two_member_ledger()));
    let ledger: Arc<dyn CurrentSpaceMemberScopePort> = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository,
        Arc::new(AcceptingVerifier),
    ));

    let scope = ledger.snapshot().await.unwrap();

    assert_eq!(scope.revision, 8);
    assert_eq!(
        scope.usable_peer_device_ids,
        vec![DeviceId::new("device-b")]
    );
}

#[tokio::test]
async fn memory_adapter_rejects_a_stale_history_digest() {
    let loaded = active_single_member_ledger();
    let repository = MemoryLedgerRepository::new(loaded.clone());
    let mut replacement = loaded.clone();
    replacement.revision += 1;

    let error = repository
        .compare_and_commit(MembershipLedgerMutation {
            expected_revision: loaded.revision,
            expected_history_digest: Some([0; 32]),
            replacement,
        })
        .await
        .unwrap_err();

    assert_eq!(error, MembershipLedgerError::Conflict);
    assert_eq!(repository.load().await.unwrap(), loaded);
}

#[tokio::test]
async fn admission_record_and_initial_history_commit_atomically() {
    let mut loaded = active_single_member_ledger();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0x61; 16]));
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );
    let record_id = SpaceJoinRecordId::from_bytes([0x62; 32]);
    let record =
        SpaceJoinRecord::new_joiner(record_id, [0x63; 16], JoinerAdmissionStage::Initiated);
    let history = active_single_member_ledger().membership_history.unwrap();

    let metadata = ledger
        .start_join_record(record.clone(), None, Some(history.clone()))
        .await
        .unwrap();

    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(metadata.device_trust_revision, 8);
    let persisted = repository.load().await.unwrap();
    assert_eq!(
        persisted.admission_records.get(record_id.as_bytes()),
        Some(&record)
    );
    assert_eq!(persisted.membership_history, Some(history));
    assert_eq!(persisted.revision, 8);
}

#[tokio::test]
async fn inbound_admission_load_returns_one_consistent_context() {
    let loaded = active_single_member_ledger();
    let expected_history = loaded.membership_history.clone().unwrap();
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));
    let record_id = SpaceJoinRecordId::from_bytes([0x64; 32]);

    let state = ledger.load(record_id).await.unwrap();
    assert_eq!(state.required_invitation_generation(), 7);

    let context = state.preparation_context(Some(7));
    assert_eq!(context.invitation_generation, Some(7));
    assert_eq!(context.membership_history_v2, expected_history);
    assert!(context.current_record.is_none());
}

#[tokio::test]
async fn inbound_admission_load_requires_signed_membership_history() {
    let mut loaded = active_single_member_ledger();
    loaded.membership_history = None;
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(repository.clone(), repository, Arc::new(AcceptingVerifier));

    let error = ledger
        .load(SpaceJoinRecordId::from_bytes([0x65; 32]))
        .await
        .err()
        .unwrap();

    assert_eq!(error, LoadMemberAdmissionError::RecoveryRequired);
}

#[tokio::test]
async fn stale_inbound_admission_token_commits_nothing() {
    let mut loaded = active_single_member_ledger();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0x64; 16]));
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );
    let record_id = SpaceJoinRecordId::from_bytes([0x65; 32]);
    let state = ledger.load(record_id).await.unwrap();
    let context = state.preparation_context(Some(7));
    let prepared = PreparedMemberAdmissionActivation::new(
        SpaceJoinRecord::new_joiner(record_id, [0x66; 16], JoinerAdmissionStage::Prepared),
        context.membership_history_v2,
        PeerReconciliationRecord {
            peer_device_id: DeviceId::new("device-b"),
            relationship: MembershipHistoryRelationship::Unknown,
            confirmed_position: None,
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
        PendingMembershipEffect {
            event_id: [0x67; 32],
            kind: MembershipEffectKind::AddDevice,
            phase: MembershipEffectPhase::Prepared,
            affected_device_ids: vec![DeviceId::new("device-b")],
            payload: vec![0x68],
        },
    );
    {
        let mut changed = repository.loaded.lock().unwrap();
        changed.revision += 1;
        changed
            .admission_profile
            .as_mut()
            .unwrap()
            .device_trust_revision = changed.revision;
    }
    let before = repository.load().await.unwrap();

    let error = ledger
        .accept(
            state.into_commit_token(),
            prepared,
            Some(ConsumedInvitation::new([0x69; 32])),
        )
        .await
        .unwrap_err();

    assert_eq!(error, AcceptAdmissionError::StateChanged);
    assert_eq!(repository.load().await.unwrap(), before);
}

#[tokio::test]
async fn inbound_admission_record_version_is_advanced_by_the_ledger() {
    let mut loaded = active_single_member_ledger();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0x6d; 16]));
    let record_id = SpaceJoinRecordId::from_bytes([0x6e; 32]);
    let record =
        SpaceJoinRecord::new_joiner(record_id, [0x6f; 16], JoinerAdmissionStage::Candidate);
    loaded
        .admission_records
        .insert(*record_id.as_bytes(), record.clone());
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );
    let state = ledger.load(record_id).await.unwrap();
    let context = state.preparation_context(None);
    let mut next = record;
    next.set_joiner_stage(JoinerAdmissionStage::Prepared);
    let prepared = PreparedMemberAdmissionActivation::new(
        next,
        context.membership_history_v2,
        PeerReconciliationRecord {
            peer_device_id: DeviceId::new("device-b"),
            relationship: MembershipHistoryRelationship::Unknown,
            confirmed_position: None,
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
        PendingMembershipEffect {
            event_id: [0x70; 32],
            kind: MembershipEffectKind::AddDevice,
            phase: MembershipEffectPhase::Prepared,
            affected_device_ids: vec![DeviceId::new("device-b")],
            payload: vec![0x71],
        },
    );

    ledger
        .accept(state.into_commit_token(), prepared, None)
        .await
        .unwrap();

    let persisted = repository.load().await.unwrap();
    assert_eq!(
        persisted
            .admission_records
            .get(record_id.as_bytes())
            .unwrap()
            .record_version,
        1
    );
}

#[tokio::test]
async fn admission_record_version_is_advanced_by_the_ledger() {
    let mut loaded = active_single_member_ledger();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0x6a; 16]));
    let record_id = SpaceJoinRecordId::from_bytes([0x6b; 32]);
    let record =
        SpaceJoinRecord::new_joiner(record_id, [0x6c; 16], JoinerAdmissionStage::Initiated);
    loaded
        .admission_records
        .insert(*record_id.as_bytes(), record.clone());
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );
    let mut next = record;
    next.set_joiner_stage(JoinerAdmissionStage::Candidate);

    ledger.save_join_record_progress(next).await.unwrap();

    let persisted = repository.load().await.unwrap();
    let record = persisted
        .admission_records
        .get(record_id.as_bytes())
        .unwrap();
    assert_eq!(record.record_version, 1);
    assert_eq!(record.stage_rank(), Some(2));
}

#[tokio::test]
async fn admission_progress_and_history_advance_in_one_cas() {
    let mut loaded = active_single_member_ledger();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0x71; 16]));
    let record_id = SpaceJoinRecordId::from_bytes([0x72; 32]);
    let mut record =
        SpaceJoinRecord::new_joiner(record_id, [0x73; 16], JoinerAdmissionStage::Initiated);
    record.joiner_member_instance = loaded.local_member_instance;
    loaded
        .admission_records
        .insert(*record_id.as_bytes(), record.clone());
    let repository = Arc::new(MemoryLedgerRepository::new(loaded.clone()));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );
    let mut next = record;
    next.set_joiner_stage(JoinerAdmissionStage::Candidate);
    let expected_history = loaded.membership_history.clone().unwrap();

    let metadata = ledger
        .activate_joined_space(next.clone(), expected_history.clone(), expected_history)
        .await
        .unwrap();

    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 1);
    assert_eq!(metadata.device_trust_revision, 8);
    let persisted = repository.load().await.unwrap();
    let mut expected = next;
    expected.record_version = 1;
    assert_eq!(
        persisted.admission_records.get(record_id.as_bytes()),
        Some(&expected)
    );
    assert_eq!(persisted.revision, 8);
}

#[tokio::test]
async fn completed_cross_space_join_switches_lineage_and_local_member_atomically() {
    let mut loaded = active_single_member_ledger();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0x81; 16]));
    let record_id = SpaceJoinRecordId::from_bytes([0x82; 32]);
    let mut record =
        SpaceJoinRecord::new_joiner(record_id, [0x83; 16], JoinerAdmissionStage::Committed);
    let (target_facts, target_credential) = member_facts("device-a", 0x91);
    let target_history = VersionedMembershipHistory::new_single_member_root(
        "space-b".to_owned(),
        target_facts.clone(),
        target_credential,
    )
    .unwrap()
    .encode_persisted_v2()
    .unwrap();
    record.joiner_member_instance = Some(target_facts.member_instance);
    loaded
        .admission_records
        .insert(*record_id.as_bytes(), record.clone());
    let source_history = loaded.membership_history.clone().unwrap();
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );
    let next = record;

    ledger
        .activate_joined_space(next, source_history, target_history)
        .await
        .unwrap();

    let persisted = repository.load().await.unwrap();
    assert_eq!(persisted.lineage_id.as_deref(), Some("space-b"));
    assert_eq!(
        persisted.local_member_instance,
        Some(target_facts.member_instance)
    );
    assert_eq!(persisted.local_device_id, Some(target_facts.device_id));
    assert!(persisted.local_join_active);
    assert!(ledger.current_scope().await.unwrap().local_member_active);
}

#[derive(Default)]
struct RecordingEffectPorts {
    calls: Mutex<Vec<&'static str>>,
}

#[async_trait]
impl ApplyMembershipMemberFactsPort for RecordingEffectPorts {
    async fn apply_member_facts(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        self.calls.lock().unwrap().push("member_facts");
        Ok(())
    }
}

#[async_trait]
impl ApplyMembershipSecurityPort for RecordingEffectPorts {
    async fn apply_membership_security(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        self.calls.lock().unwrap().push("security");
        Ok(())
    }
}

#[async_trait]
impl ActivateMembershipEffectPort for RecordingEffectPorts {
    async fn activate_membership_effect(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        self.calls.lock().unwrap().push("activate");
        Ok(())
    }
}

#[tokio::test]
async fn prepared_effect_resumes_each_persisted_phase_in_order() {
    let mut loaded = active_two_member_ledger();
    loaded.pending_effects.insert(
        [0x81; 32],
        PendingMembershipEffect {
            event_id: [0x81; 32],
            kind: MembershipEffectKind::AddDevice,
            phase: MembershipEffectPhase::Prepared,
            affected_device_ids: vec![DeviceId::new("device-b")],
            payload: vec![0x82],
        },
    );
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let ports = Arc::new(RecordingEffectPorts::default());
    let recover =
        RecoverMembershipEffectsUseCase::new(ledger, ports.clone(), ports.clone(), ports.clone());

    let report = recover.execute().await;

    assert_eq!(report.completed_count, 1);
    assert_eq!(
        ports.calls.lock().unwrap().as_slice(),
        &["member_facts", "security", "activate"]
    );
    assert_eq!(
        repository
            .load()
            .await
            .unwrap()
            .pending_effects
            .get(&[0x81; 32])
            .unwrap()
            .phase,
        MembershipEffectPhase::Activated
    );
}

struct DeliverRestrictedOnce;

#[async_trait]
impl RestrictedMembershipDeliveryPort for DeliverRestrictedOnce {
    async fn deliver_restricted_membership(
        &self,
        _peer: &DeviceId,
        _delivery: &RestrictedMembershipDelivery,
    ) -> Result<(), RestrictedMembershipDeliveryError> {
        Ok(())
    }
}

#[tokio::test]
async fn confirmed_restricted_delivery_is_removed_from_the_ledger() {
    let mut loaded = active_two_member_ledger();
    let peer = DeviceId::new("device-b");
    let history = VersionedMembershipHistory::decode_persisted_v2(
        loaded.membership_history.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    let local_member = loaded.local_member_instance.unwrap();
    let peer_member = history.effective_member_for_device(&peer).unwrap();
    let credential = history.credential_for(local_member).unwrap();
    let event = history
        .create_unsigned_local_removal_event(
            local_member,
            credential,
            peer_member,
            [0x91; 16],
            [0x92; 32],
        )
        .unwrap();
    loaded
        .peer_reconciliation
        .get_mut(&peer)
        .unwrap()
        .restricted_delivery = vec![RestrictedMembershipDelivery::Event(event)];
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let deliver = DeliverRestrictedMembershipUseCase::new(ledger, Arc::new(DeliverRestrictedOnce));

    let report = deliver.execute().await;

    assert_eq!(report.completed_count, 1);
    assert!(repository
        .load()
        .await
        .unwrap()
        .peer_reconciliation
        .get(&peer)
        .unwrap()
        .restricted_delivery
        .is_empty());
}

#[tokio::test]
async fn new_space_root_and_local_activation_commit_once() {
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0xd1; 16]));
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );
    let (facts, credential) = member_facts("device-a", 0x41);

    ledger
        .initialize_current_space("space-a".to_owned(), facts.clone(), credential)
        .await
        .unwrap();

    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 1);
    let persisted = repository.load().await.unwrap();
    assert_eq!(persisted.lineage_id.as_deref(), Some("space-a"));
    assert_eq!(persisted.local_device_id, Some(facts.device_id));
    assert_eq!(persisted.local_member_instance, Some(facts.member_instance));
    assert!(persisted.local_join_active);
    assert!(persisted.membership_history.is_some());
}

#[tokio::test]
async fn space_rebuild_reset_clears_every_previous_membership_fact_atomically() {
    let mut loaded = active_two_member_ledger();
    loaded.admission_profile = Some(AdmissionProfileMetadata::fresh([0xe1; 16]));
    let record_id = SpaceJoinRecordId::from_bytes([0xe2; 32]);
    loaded.admission_records.insert(
        *record_id.as_bytes(),
        SpaceJoinRecord::new_joiner(record_id, [0xe3; 16], JoinerAdmissionStage::Initiated),
    );
    loaded.pending_effects.insert(
        [0xe4; 32],
        PendingMembershipEffect {
            event_id: [0xe4; 32],
            kind: MembershipEffectKind::AddDevice,
            phase: MembershipEffectPhase::Prepared,
            affected_device_ids: vec![DeviceId::new("device-b")],
            payload: vec![0xe5],
        },
    );
    let repository = Arc::new(MemoryLedgerRepository::new(loaded));
    let ledger = MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    );

    ledger.reset_for_space_rebuild().await.unwrap();

    assert_eq!(repository.commit_calls.load(Ordering::SeqCst), 1);
    let persisted = repository.load().await.unwrap();
    assert_eq!(persisted.revision, 9);
    assert!(persisted.lineage_id.is_none());
    assert!(persisted.membership_history.is_none());
    assert!(persisted.local_device_id.is_none());
    assert!(persisted.local_member_instance.is_none());
    assert!(!persisted.local_join_active);
    assert!(persisted.peer_reconciliation.is_empty());
    assert!(persisted.inbound_transfers.is_empty());
    assert!(persisted.completed_inbound_transfers.is_empty());
    assert!(persisted.pending_effects.is_empty());
    assert!(persisted.admission_records.is_empty());
    assert_eq!(
        persisted
            .admission_profile
            .as_ref()
            .unwrap()
            .device_trust_revision,
        persisted.revision
    );
}

struct SuccessfulActivation;

#[async_trait]
impl ActivateMembershipEffectPort for SuccessfulActivation {
    async fn activate_membership_effect(
        &self,
        _effect: &PendingMembershipEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        Ok(())
    }
}

struct RecordingRePairingResolution(AtomicUsize);

#[async_trait]
impl crate::space::membership::ResolveRePairingPort for RecordingRePairingResolution {
    async fn resolve_after_successful_pairing(
        &self,
    ) -> Result<(), crate::space::membership::RePairingStateError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn add_device_activation_resolves_the_re_pairing_requirement() {
    let resolution = Arc::new(RecordingRePairingResolution(AtomicUsize::new(0)));
    let activation =
        RePairingAwareMembershipActivation::new(Arc::new(SuccessfulActivation), resolution.clone());
    let effect = PendingMembershipEffect {
        event_id: [0xf1; 32],
        kind: MembershipEffectKind::AddDevice,
        phase: MembershipEffectPhase::SecurityApplied,
        affected_device_ids: vec![DeviceId::new("peer")],
        payload: vec![0xf2],
    };

    activation
        .activate_membership_effect(&effect)
        .await
        .unwrap();

    assert_eq!(resolution.0.load(Ordering::SeqCst), 1);
}
