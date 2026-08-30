use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MemberInstanceId, MembershipActivationBaselineV2,
    MembershipCredential, MembershipEventId, MembershipHistoryRelationship,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
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
    DeviceTrustMembership, DeviceTrustObservation, LoadDeviceTrustObservationsPort,
    QueryDeviceTrustError, QueryDeviceTrustUseCase,
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
        _device_id: &DeviceId,
    ) -> Result<MemberInstanceId, CurrentMemberSignatureError> {
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

fn pending_local_removal_ledger() -> (LoadedMembershipLedger, TestSigner, MembershipEventId) {
    let (local_facts, local_credential) = member_facts("device-a", 0x41);
    let (peer_facts, peer_credential) = member_facts("device-b", 0x42);
    let (former_facts, former_credential) = member_facts("device-c", 0x43);
    let local_member = local_facts.member_instance;
    let peer_member = peer_facts.member_instance;
    let former_member = former_facts.member_instance;
    let peer_device_id = peer_facts.device_id.clone();
    let mut history = VersionedMembershipHistory::from_activation_baseline(
        MembershipActivationBaselineV2::Established {
            lineage_id: "space-a".to_owned(),
            head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
            head_depth: 0,
            current_members: vec![
                (local_facts.clone(), local_credential.clone()),
                (peer_facts, peer_credential.clone()),
                (former_facts, former_credential),
            ],
        },
    )
    .unwrap();
    let mut prior_removal = history
        .create_unsigned_local_removal_event(
            local_member,
            &local_credential,
            former_member,
            [0x21; 16],
            [0x22; 32],
        )
        .unwrap();
    prior_removal.signature = vec![0x23];
    history
        .verify_and_receive_event(prior_removal, &AcceptingVerifier)
        .unwrap();
    let mut removal = history
        .create_unsigned_local_removal_event(
            peer_member,
            &peer_credential,
            local_member,
            [0x31; 16],
            [0x32; 32],
        )
        .unwrap();
    removal.signature = vec![0x33];
    let change_id = removal.event_id();
    let mut incoming = history.clone();
    incoming
        .verify_and_receive_event(removal, &AcceptingVerifier)
        .unwrap();
    history
        .merge_remote_history(&incoming, local_member, &AcceptingVerifier)
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
            relationship: MembershipHistoryRelationship::PendingRemovalDecision,
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
        change_id,
    )
}

#[tokio::test]
async fn accepting_local_removal_requires_explicit_confirmation_before_writing() {
    let (loaded, signer, change_id) = pending_local_removal_ledger();
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
    let decide = DecideDeviceTrustChangeUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        wake.clone(),
    );

    let result = decide
        .execute(DecideDeviceTrustChange {
            change_id,
            choice: DeviceTrustChangeChoice::ApplyChange,
            confirm_local_removal: false,
        })
        .await
        .unwrap();

    assert!(matches!(
        result,
        DecideDeviceTrustChangeResult::LocalConfirmationRequired { change_id: id, .. }
            if id == change_id
    ));
    assert_eq!(repository.commits.load(Ordering::SeqCst), 0);
    assert_eq!(wake.0.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn confirmed_acceptance_commits_the_decision_and_stops_local_access() {
    let (loaded, signer, change_id) = pending_local_removal_ledger();
    let local_member = signer.local_member;
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
    let decide = DecideDeviceTrustChangeUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        wake.clone(),
    );

    let result = decide
        .execute(DecideDeviceTrustChange {
            change_id,
            choice: DeviceTrustChangeChoice::ApplyChange,
            confirm_local_removal: true,
        })
        .await
        .unwrap();

    let status = match result {
        DecideDeviceTrustChangeResult::Applied { status, .. } => status,
        other => panic!("expected applied result, got {other:?}"),
    };
    assert_eq!(status.revision, 9);
    assert_eq!(status.local_membership, DeviceTrustMembership::Removed);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
    let persisted = repository.load().await.unwrap();
    let history = VersionedMembershipHistory::decode_persisted_v2(
        persisted.membership_history.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    assert_eq!(
        history
            .decision_for(change_id, local_member)
            .unwrap()
            .decision,
        uc_core::membership::RemovalDecision::Accept
    );
    let effect = persisted.pending_effects.get(change_id.as_bytes()).unwrap();
    assert_eq!(effect.kind, MembershipEffectKind::RemoveDevice);
    assert_eq!(effect.phase, MembershipEffectPhase::Prepared);
    let relationship = persisted
        .peer_reconciliation
        .get(&DeviceId::new("device-b"))
        .unwrap();
    assert_eq!(
        relationship.relationship,
        MembershipHistoryRelationship::Consistent
    );
    assert!(matches!(
        relationship.restricted_delivery.as_slice(),
        [RestrictedMembershipDelivery::Decision(decision)]
            if decision.removal_event_id == change_id
    ));
}

#[tokio::test]
async fn rejection_keeps_local_membership_and_diverges_only_the_proposer() {
    let (loaded, signer, change_id) = pending_local_removal_ledger();
    let local_member = signer.local_member;
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
    let decide = DecideDeviceTrustChangeUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        wake.clone(),
    );

    let result = decide
        .execute(DecideDeviceTrustChange {
            change_id,
            choice: DeviceTrustChangeChoice::KeepCurrentDeviceGroup,
            confirm_local_removal: false,
        })
        .await
        .unwrap();

    let status = match result {
        DecideDeviceTrustChangeResult::KeptCurrentDeviceGroup { status, .. } => status,
        other => panic!("expected kept-current result, got {other:?}"),
    };
    assert_eq!(status.revision, 9);
    assert_eq!(status.local_membership, DeviceTrustMembership::Active);
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
    let persisted = repository.load().await.unwrap();
    let history = VersionedMembershipHistory::decode_persisted_v2(
        persisted.membership_history.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    assert_eq!(
        history
            .decision_for(change_id, local_member)
            .unwrap()
            .decision,
        uc_core::membership::RemovalDecision::Reject
    );
    assert!(history.active_members().contains(&local_member));
    assert!(!persisted.pending_effects.contains_key(change_id.as_bytes()));
    assert_eq!(
        persisted
            .peer_reconciliation
            .get(&DeviceId::new("device-b"))
            .unwrap()
            .relationship,
        MembershipHistoryRelationship::Diverged
    );
}

#[tokio::test]
async fn repeated_decision_returns_the_original_result_without_a_second_commit() {
    let (loaded, signer, change_id) = pending_local_removal_ledger();
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
    let decide = DecideDeviceTrustChangeUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        effects.clone(),
        wake.clone(),
    );
    let input = DecideDeviceTrustChange {
        change_id,
        choice: DeviceTrustChangeChoice::ApplyChange,
        confirm_local_removal: true,
    };
    decide.execute(input).await.unwrap();

    let repeated = decide.execute(input).await.unwrap();

    assert!(matches!(
        repeated,
        DecideDeviceTrustChangeResult::AlreadyCompleted {
            change_id: id,
            choice: DeviceTrustChangeChoice::ApplyChange,
            ..
        } if id == change_id
    ));
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.0.load(Ordering::SeqCst), 2);
    assert_eq!(effects.0.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn one_decision_conflict_is_retried_from_a_fresh_snapshot() {
    let (loaded, signer, change_id) = pending_local_removal_ledger();
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
    let decide = DecideDeviceTrustChangeUseCase::new(
        ledger,
        Arc::new(signer),
        query,
        Arc::new(NoopEffects),
        wake.clone(),
    );

    let result = decide
        .execute(DecideDeviceTrustChange {
            change_id,
            choice: DeviceTrustChangeChoice::KeepCurrentDeviceGroup,
            confirm_local_removal: false,
        })
        .await
        .unwrap();

    assert!(matches!(
        result,
        DecideDeviceTrustChangeResult::KeptCurrentDeviceGroup { .. }
    ));
    assert_eq!(repository.commits.load(Ordering::SeqCst), 1);
    assert_eq!(wake.0.load(Ordering::SeqCst), 1);
}
