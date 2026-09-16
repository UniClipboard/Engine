use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionChangeFacts, AdmissionMemberBindingV2,
    HistoricalMembershipSignatureError, HistoricalMembershipSignatureVerifier, MemberInstanceId,
    MembershipActivationBaselineV2, MembershipAdmissionV2, MembershipCredential, MembershipEventId,
    MembershipEventV2, MembershipHistoryRelationship, MembershipOperationV2, SpaceAdmissionId,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1, MEMBERSHIP_EVENT_FORMAT_V2,
};
use uc_core::ports::ReachabilityState;

use super::*;
use crate::space::membership::WakeSpaceMembershipMaintenancePort;
use crate::space::membership::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipEffectKind, MembershipEffectPhase, MembershipLedger, MembershipLedgerError,
    MembershipLedgerMutation, PeerReconciliationRecord, RestrictedMembershipDelivery,
};
use crate::space::membership::{CurrentMemberSignatureError, CurrentMemberSignaturePort};
use crate::space::membership::{
    DeviceTrustMembership, DeviceTrustObservation, DeviceTrustSyncState,
    LoadDeviceTrustObservationsPort, QueryDeviceTrustError, QueryDeviceTrustUseCase,
};

struct MemoryLedgerRepository {
    loaded: Mutex<LoadedMembershipLedger>,
    commits: AtomicUsize,
    remaining_conflicts: AtomicUsize,
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
        if self
            .remaining_conflicts
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(MembershipLedgerError::Conflict);
        }
        let mut loaded = self
            .loaded
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)?;
        let digest = loaded
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        if loaded.revision != mutation.expected_revision
            || digest != mutation.expected_history_digest
        {
            return Err(MembershipLedgerError::Conflict);
        }
        self.commits.fetch_add(1, Ordering::SeqCst);
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

struct TestSigner {
    local_device_id: DeviceId,
    local_member: MemberInstanceId,
    credential: MembershipCredential,
}

#[async_trait]
impl CurrentMemberSignaturePort for TestSigner {
    async fn current_member_epoch(&self) -> Result<u64, CurrentMemberSignatureError> {
        Ok(1)
    }

    async fn current_membership_credential(
        &self,
        device_id: &DeviceId,
    ) -> Result<MembershipCredential, CurrentMemberSignatureError> {
        assert_eq!(device_id, &self.local_device_id);
        Ok(self.credential.clone())
    }

    async fn current_member_instance(
        &self,
        device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError> {
        assert_eq!(device_id, &self.local_device_id);
        Ok(self.local_member)
    }

    async fn sign_current_member_payload(
        &self,
        _payload: &[u8],
    ) -> Result<Vec<u8>, CurrentMemberSignatureError> {
        Ok(vec![0x91])
    }

    async fn verify_current_member_payload(
        &self,
        _member: &DeviceId,
        _payload: &[u8],
        _signature: &[u8],
    ) -> Result<bool, CurrentMemberSignatureError> {
        Ok(true)
    }
}

struct OfflineObservations;

#[async_trait]
impl LoadDeviceTrustObservationsPort for OfflineObservations {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        Ok(device_ids
            .iter()
            .map(|device_id| DeviceTrustObservation {
                device_id: device_id.clone(),
                display_name: Some(device_id.as_str().to_owned()),
                reachability: ReachabilityState::Offline,
            })
            .collect())
    }
}

struct WakeCounter(AtomicUsize);

impl WakeSpaceMembershipMaintenancePort for WakeCounter {
    fn wake(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn schedule_at(&self, _expires_at_ms: i64, _now_ms: i64) {}
}

struct NoopEffects;

#[async_trait]
impl crate::space::membership::RecoverMembershipEffectsPort for NoopEffects {
    async fn recover_membership_effects(
        &self,
    ) -> crate::space::membership::MembershipMaintenanceStepOutcome {
        crate::space::membership::MembershipMaintenanceStepOutcome::Completed
    }
}

struct EffectCounter(AtomicUsize);

#[async_trait]
impl crate::space::membership::RecoverMembershipEffectsPort for EffectCounter {
    async fn recover_membership_effects(
        &self,
    ) -> crate::space::membership::MembershipMaintenanceStepOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        crate::space::membership::MembershipMaintenanceStepOutcome::Deferred
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

fn active_ledger() -> (LoadedMembershipLedger, TestSigner) {
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
                (local_facts.clone(), local_credential.clone()),
                (peer_facts, peer_credential),
            ],
        },
    )
    .unwrap();
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 8;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local_facts.device_id.clone());
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
    (
        loaded,
        TestSigner {
            local_device_id: local_facts.device_id,
            local_member,
            credential: local_credential,
        },
    )
}

#[tokio::test]
async fn removal_commits_all_local_facts_once_before_returning_success() {
    let (loaded, signer) = active_ledger();
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
        remaining_conflicts: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let query = Arc::new(QueryDeviceTrustUseCase::new(
        Arc::clone(&ledger),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let effects = Arc::new(EffectCounter(AtomicUsize::new(0)));
    let remove = RemoveSpaceMemberUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        effects.clone(),
        wake.clone(),
    );

    let result = remove.execute(&DeviceId::new("device-b")).await.unwrap();

    assert_eq!(result.commit.revision, 9);
    assert_eq!(result.status.revision, 9);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
    assert_eq!(effects.0.load(Ordering::SeqCst), 1);
    let removed = result
        .status
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-b"))
        .unwrap();
    assert_eq!(removed.membership, DeviceTrustMembership::Removed);
    assert_eq!(
        removed.sync_state,
        DeviceTrustSyncState::Paused(
            crate::space::membership::SpaceMemberPauseReason::LocalMemberInactive
        )
    );
    let persisted = repository.load().await.unwrap();
    let history = VersionedMembershipHistory::decode_persisted_v2(
        persisted.membership_history.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    assert!(history
        .effective_member_for_device(&DeviceId::new("device-b"))
        .is_none());
    let effect = persisted
        .effect_journal
        .get(result.change_id.as_bytes())
        .unwrap();
    assert_eq!(effect.kind, MembershipEffectKind::RemoveDevice);
    assert_eq!(effect.phase, MembershipEffectPhase::Prepared);
    let initiated = effect.initiated_removal().unwrap();
    assert_eq!(initiated.event.event_id(), result.change_id);
    assert!(initiated.retained_device_ids.is_empty());
    let relationship = persisted
        .peer_reconciliation
        .get(&DeviceId::new("device-b"))
        .unwrap();
    assert_eq!(
        relationship.relationship,
        MembershipHistoryRelationship::PendingRemovalDecision
    );
    assert!(matches!(
        relationship.restricted_delivery.as_slice(),
        [RestrictedMembershipDelivery::Event(event)] if event.event_id() == result.change_id
    ));
}

#[tokio::test]
async fn one_persistence_conflict_is_retried_from_a_fresh_snapshot() {
    let (loaded, signer) = active_ledger();
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
        remaining_conflicts: AtomicUsize::new(1),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let query = Arc::new(QueryDeviceTrustUseCase::new(
        Arc::clone(&ledger),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let remove = RemoveSpaceMemberUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        wake.clone(),
    );

    let result = remove.execute(&DeviceId::new("device-b")).await.unwrap();

    assert_eq!(result.commit.revision, 9);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replaying_an_old_admission_revocation_does_not_remove_the_repaired_instance() {
    let (loaded, signer, old_member, old_add_event_id) = admitted_peer_ledger();
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
        remaining_conflicts: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let query = Arc::new(QueryDeviceTrustUseCase::new(
        Arc::clone(&ledger),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    let remove = RemoveSpaceMemberUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        Arc::new(WakeCounter(AtomicUsize::new(0))),
    );
    let admission_id = SpaceAdmissionId::from_bytes([0x61; 32]).unwrap();
    let binding = AdmissionMemberBindingV2::new(
        [0x62; 32],
        SpaceId::from_str("space-a"),
        old_member,
        old_add_event_id,
    )
    .unwrap();
    let target = AdmissionRevocationTarget::new(admission_id, binding);

    let first = remove.revoke_admission(target.clone()).await.unwrap();
    let AdmissionRevocationResult::Removed {
        change_id: first_change_id,
    } = first
    else {
        panic!("the first exact revocation must remove its target");
    };

    let new_member = {
        let mut loaded = repository.loaded.lock().unwrap();
        append_active_peer(&mut loaded, "device-b", 0x52, 0x71)
    };
    let replay = remove.revoke_admission(target).await.unwrap();

    assert_eq!(
        replay,
        AdmissionRevocationResult::AlreadyAbsent {
            change_id: first_change_id,
        }
    );
    let persisted = repository.load().await.unwrap();
    let history = VersionedMembershipHistory::decode_persisted_v2(
        persisted.membership_history.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    assert!(history.active_members().contains(&new_member));
    assert_eq!(
        history.effective_member_for_device(&DeviceId::new("device-b")),
        Some(new_member)
    );
}

#[tokio::test]
async fn abandoned_admission_lookup_resolves_to_the_exact_original_member() {
    let (loaded, signer, old_member, old_add_event_id) = admitted_peer_ledger();
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
        remaining_conflicts: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let query = Arc::new(QueryDeviceTrustUseCase::new(
        Arc::clone(&ledger),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    let remove = RemoveSpaceMemberUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        Arc::new(WakeCounter(AtomicUsize::new(0))),
    );
    let target = AdmissionAbandonmentRevocationTarget::new(
        SpaceAdmissionId::from_bytes([0x63; 32]).unwrap(),
        [0x64; 32],
        old_member,
        old_add_event_id,
    );

    assert!(matches!(
        remove.revoke_abandoned_admission(target).await.unwrap(),
        AdmissionRevocationResult::Removed { .. }
    ));

    let new_member = {
        let mut loaded = repository.loaded.lock().unwrap();
        append_active_peer(&mut loaded, "device-b", 0x65, 0x72)
    };
    let history = VersionedMembershipHistory::decode_persisted_v2(
        repository
            .load()
            .await
            .unwrap()
            .membership_history
            .as_deref()
            .unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    assert!(history.active_members().contains(&new_member));
    assert!(!history.active_members().contains(&old_member));
}

#[tokio::test]
async fn exact_revocation_rejects_wrong_space_and_unknown_add_without_committing() {
    let (loaded, signer, member, add_event_id) = admitted_peer_ledger();
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
        remaining_conflicts: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let query = Arc::new(QueryDeviceTrustUseCase::new(
        Arc::clone(&ledger),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    let remove = RemoveSpaceMemberUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        Arc::new(WakeCounter(AtomicUsize::new(0))),
    );
    let admission_id = SpaceAdmissionId::from_bytes([0x63; 32]).unwrap();
    let wrong_space = AdmissionMemberBindingV2::new(
        [0x64; 32],
        SpaceId::from_str("space-b"),
        member,
        add_event_id,
    )
    .unwrap();
    let wrong_add = AdmissionMemberBindingV2::new(
        [0x64; 32],
        SpaceId::from_str("space-a"),
        member,
        MembershipEventId::from_hex(&"65".repeat(32)).unwrap(),
    )
    .unwrap();

    let space_error = remove
        .revoke_admission(AdmissionRevocationTarget::new(admission_id, wrong_space))
        .await
        .unwrap_err();
    let add_error = remove
        .revoke_admission(AdmissionRevocationTarget::new(admission_id, wrong_add))
        .await
        .unwrap_err();

    assert!(matches!(
        space_error,
        RemoveSpaceMemberError::TargetNotFound
    ));
    assert!(matches!(add_error, RemoveSpaceMemberError::TargetNotFound));
    assert_eq!(repository.commits.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exact_revocation_reports_pending_local_effects_after_the_remove_is_saved() {
    let (loaded, signer, member, add_event_id) = admitted_peer_ledger();
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
        remaining_conflicts: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let query = Arc::new(QueryDeviceTrustUseCase::new(
        Arc::clone(&ledger),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    let remove = RemoveSpaceMemberUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(EffectCounter(AtomicUsize::new(0))),
        Arc::new(WakeCounter(AtomicUsize::new(0))),
    );
    let binding = AdmissionMemberBindingV2::new(
        [0x66; 32],
        SpaceId::from_str("space-a"),
        member,
        add_event_id,
    )
    .unwrap();

    let result = remove
        .revoke_admission(AdmissionRevocationTarget::new(
            SpaceAdmissionId::from_bytes([0x67; 32]).unwrap(),
            binding,
        ))
        .await
        .unwrap();

    assert!(matches!(
        result,
        AdmissionRevocationResult::LocalEffectsPending { .. }
    ));
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn exact_revocation_without_the_current_member_credential_never_reports_removed() {
    let (loaded, mut signer, member, add_event_id) = admitted_peer_ledger();
    signer.credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x7a; 32]);
    let repository = Arc::new(MemoryLedgerRepository {
        loaded: Mutex::new(loaded),
        commits: AtomicUsize::new(0),
        remaining_conflicts: AtomicUsize::new(0),
    });
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let query = Arc::new(QueryDeviceTrustUseCase::new(
        Arc::clone(&ledger),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    let remove = RemoveSpaceMemberUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        Arc::new(WakeCounter(AtomicUsize::new(0))),
    );
    let binding = AdmissionMemberBindingV2::new(
        [0x7b; 32],
        SpaceId::from_str("space-a"),
        member,
        add_event_id,
    )
    .unwrap();

    let error = remove
        .revoke_admission(AdmissionRevocationTarget::new(
            SpaceAdmissionId::from_bytes([0x7c; 32]).unwrap(),
            binding,
        ))
        .await
        .unwrap_err();

    assert!(matches!(error, RemoveSpaceMemberError::RecoveryRequired));
    assert_eq!(repository.commits.load(Ordering::SeqCst), 0);
}

fn admitted_peer_ledger() -> (
    LoadedMembershipLedger,
    TestSigner,
    MemberInstanceId,
    MembershipEventId,
) {
    let (local_facts, local_credential) = member_facts("device-a", 0x41);
    let local_member = local_facts.member_instance;
    let mut history = VersionedMembershipHistory::new_single_member_root(
        "space-a".to_owned(),
        local_facts.clone(),
        local_credential.clone(),
    )
    .unwrap();
    let (peer_member, add_event_id) =
        append_active_peer_to_history(&mut history, local_member, "device-b", 0x42, 0x51);
    let mut loaded = LoadedMembershipLedger::no_current_space();
    loaded.revision = 8;
    loaded.lineage_id = Some("space-a".to_owned());
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    loaded.local_device_id = Some(local_facts.device_id.clone());
    loaded.local_member_instance = Some(local_member);
    loaded.local_join_active = true;
    loaded.peer_reconciliation.insert(
        DeviceId::new("device-b"),
        PeerReconciliationRecord {
            peer_device_id: DeviceId::new("device-b"),
            relationship: MembershipHistoryRelationship::Consistent,
            confirmed_position: None,
            sync_state: Default::default(),
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    );
    (
        loaded,
        TestSigner {
            local_device_id: local_facts.device_id,
            local_member,
            credential: local_credential,
        },
        peer_member,
        add_event_id,
    )
}

fn append_active_peer(
    loaded: &mut LoadedMembershipLedger,
    device: &str,
    credential_byte: u8,
    marker: u8,
) -> MemberInstanceId {
    let mut history = VersionedMembershipHistory::decode_persisted_v2(
        loaded.membership_history.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    let local_member = loaded.local_member_instance.unwrap();
    let (member, _) =
        append_active_peer_to_history(&mut history, local_member, device, credential_byte, marker);
    loaded.membership_history = Some(history.encode_persisted_v2().unwrap());
    if let Some(record) = loaded.peer_reconciliation.get_mut(&DeviceId::new(device)) {
        record.relationship = MembershipHistoryRelationship::Consistent;
        record.restricted_delivery.clear();
    }
    member
}

fn append_active_peer_to_history(
    history: &mut VersionedMembershipHistory,
    author: MemberInstanceId,
    device: &str,
    credential_byte: u8,
    marker: u8,
) -> (MemberInstanceId, MembershipEventId) {
    let (facts, credential) = member_facts(device, credential_byte);
    let member = facts.member_instance;
    let author_credential = history.credential_for(author).unwrap();
    let parent = history.current_head();
    let operation = MembershipOperationV2::AddDevice {
        admission: MembershipAdmissionV2 {
            facts,
            membership_credential: credential,
            resume_public_key_digest: [marker; 32],
            security_commitment_id: [marker.wrapping_add(1); 32],
        },
    };
    let resulting_members_digest = history
        .expected_resulting_members_digest(parent, &operation)
        .unwrap();
    let event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        "space-a".to_owned(),
        parent,
        parent.map(|id| history.depth(id).unwrap() + 1).unwrap_or(0),
        [marker; 16],
        author,
        author_credential.credential_id,
        author_credential.signature_algorithm_version,
        operation,
        resulting_members_digest,
        [marker.wrapping_add(1); 32],
        vec![marker],
        Some([marker.wrapping_add(2); 32]),
        vec![marker.wrapping_add(3)],
    );
    let event_id = event.event_id();
    let receipt = AdmissionActivationReceipt::new(
        1,
        [marker.wrapping_add(4); 32],
        event_id,
        event.resulting_members_digest,
        [marker.wrapping_add(1); 32],
        member,
        vec![marker.wrapping_add(5)],
    );
    history
        .verify_and_receive_event(event, &AcceptingVerifier)
        .unwrap();
    history
        .verify_and_record_activation_receipt(receipt, &AcceptingVerifier)
        .unwrap();
    (member, event_id)
}
