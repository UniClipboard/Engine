use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, HistoricalMembershipSignatureError,
    HistoricalMembershipSignatureVerifier, MembershipBranchId, MembershipConflictChoice,
    MembershipConflictId, MembershipCredential, MembershipHistoryRelationship,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use super::*;
use crate::space::membership::{
    CommitMembershipLedgerPort, DeviceTrustMembership, LoadMembershipLedgerPort,
    LoadedMembershipLedger, MembershipConflictRecord, MembershipConflictStatus, MembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation, PeerReconciliationRecord,
};

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

struct MemoryLedger(Mutex<LoadedMembershipLedger>);

#[async_trait]
impl LoadMembershipLedgerPort for MemoryLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        self.0
            .lock()
            .map(|record| record.clone())
            .map_err(|_| MembershipLedgerError::Unavailable)
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for MemoryLedger {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let mut current = self
            .0
            .lock()
            .map_err(|_| MembershipLedgerError::Unavailable)?;
        let digest = current
            .membership_history
            .as_deref()
            .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
        if current.revision != mutation.expected_revision
            || digest != mutation.expected_history_digest
        {
            return Err(MembershipLedgerError::Conflict);
        }
        *current = mutation.replacement;
        Ok(current.clone())
    }
}

struct FixedQuery;

#[async_trait]
impl QueryMembershipConflictStatusPort for FixedQuery {
    async fn query_status(
        &self,
    ) -> Result<
        crate::space::membership::DeviceTrustStatus,
        crate::space::membership::QueryDeviceTrustError,
    > {
        Ok(crate::space::membership::DeviceTrustStatus {
            revision: 12,
            local_device_id: Some(DeviceId::new("local")),
            local_membership: DeviceTrustMembership::Active,
            current_change: None,
            current_join: None,
            pending_inbound_member: None,
            devices: Vec::new(),
        })
    }
}

fn fixture(
    remote_choice: MembershipConflictChoice,
) -> (
    Arc<MemoryLedger>,
    ResolveMembershipConflictUseCase,
    MembershipConflictId,
    MembershipBranchId,
    MembershipBranchId,
) {
    let device_id = DeviceId::new("local");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x41; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    let history = VersionedMembershipHistory::new_single_member_root(
        "space-a".to_owned(),
        AdmissionChangeFacts {
            member_instance,
            device_id: device_id.clone(),
            device_name: "Local".to_owned(),
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
    .unwrap();
    let conflict_id = MembershipConflictId::from_bytes([0x81; 32]);
    let local_branch_id = MembershipBranchId::from_bytes([0x82; 32]);
    let remote_branch_id = MembershipBranchId::from_bytes([0x83; 32]);
    let peer_id = DeviceId::new("peer");
    let mut record = LoadedMembershipLedger::no_current_space();
    record.revision = 11;
    record.lineage_id = Some("space-a".to_owned());
    record.membership_history = Some(history.encode_persisted_v2().unwrap());
    record.local_device_id = Some(device_id);
    record.local_member_instance = Some(member_instance);
    record.local_join_active = true;
    record.peer_reconciliation = BTreeMap::from([(
        peer_id.clone(),
        PeerReconciliationRecord {
            peer_device_id: peer_id.clone(),
            relationship: MembershipHistoryRelationship::Diverged,
            confirmed_position: None,
            sync_state: Default::default(),
            restricted_delivery: Vec::new(),
            updated_at_ms: 1,
        },
    )]);
    record.membership_conflicts.insert(
        conflict_id,
        MembershipConflictRecord {
            conflict_id,
            local_branch_id,
            remote_branch_id,
            local_choice: MembershipConflictChoice::ActiveMemberRecovery,
            remote_choice,
            evidence_peer_device_ids: BTreeSet::from([peer_id]),
            detected_at_revision: 11,
            status: MembershipConflictStatus::Unresolved,
            selected_branch_id: None,
            transition_id: None,
        },
    );
    let repository = Arc::new(MemoryLedger(Mutex::new(record)));
    let ledger = Arc::new(MembershipLedger::new(
        repository.clone(),
        repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let use_case = ResolveMembershipConflictUseCase::new(ledger, Arc::new(FixedQuery));
    (
        repository,
        use_case,
        conflict_id,
        local_branch_id,
        remote_branch_id,
    )
}

#[tokio::test]
async fn keeping_local_branch_completes_once_and_repeats_idempotently() {
    let (repository, use_case, conflict_id, local_branch_id, _) =
        fixture(MembershipConflictChoice::ActiveMemberRecovery);
    let input = ResolveMembershipConflictInput {
        conflict_id,
        target_branch_id: local_branch_id,
    };

    assert!(matches!(
        use_case.execute(input).await.unwrap(),
        ResolveMembershipConflictResult::Completed { .. }
    ));
    assert!(matches!(
        use_case.execute(input).await.unwrap(),
        ResolveMembershipConflictResult::AlreadyCompleted { .. }
    ));
    let persisted = repository.load().await.unwrap();
    let conflict = persisted.membership_conflicts.get(&conflict_id).unwrap();
    assert_eq!(conflict.status, MembershipConflictStatus::Completed);
    assert_eq!(conflict.selected_branch_id, Some(local_branch_id));
    assert_eq!(persisted.revision, 12, "the repeated call does not commit");
}

#[tokio::test]
async fn query_returns_complete_branch_choices_without_claiming_global_resolution() {
    let (_, use_case, conflict_id, local_branch_id, remote_branch_id) =
        fixture(MembershipConflictChoice::RePairingRequired);

    let view = use_case.query().await.unwrap();

    assert_eq!(view.revision, 11);
    assert_eq!(view.conflicts.len(), 1);
    let conflict = &view.conflicts[0];
    assert_eq!(conflict.conflict_id, conflict_id);
    assert_eq!(conflict.status, MembershipConflictStatus::Unresolved);
    assert!(!conflict.local_resolution_completed);
    assert_eq!(conflict.evidence_peer_count, 1);
    assert_eq!(conflict.branches[0].branch_id, local_branch_id);
    assert!(conflict.branches[0].is_local);
    assert_eq!(conflict.branches[1].branch_id, remote_branch_id);
    assert_eq!(
        conflict.branches[1].choice,
        MembershipConflictChoice::RePairingRequired
    );
}

struct LockedLedger;

#[async_trait]
impl LoadMembershipLedgerPort for LockedLedger {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Err(MembershipLedgerError::Locked)
    }
}

#[async_trait]
impl CommitMembershipLedgerPort for LockedLedger {
    async fn compare_and_commit(
        &self,
        _mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        Err(MembershipLedgerError::Locked)
    }
}

#[tokio::test]
async fn query_error_preserves_stable_classification_and_source() {
    let repository = Arc::new(LockedLedger);
    let use_case = ResolveMembershipConflictUseCase::new(
        Arc::new(MembershipLedger::new(
            repository.clone(),
            repository,
            Arc::new(AcceptingVerifier),
        )),
        Arc::new(FixedQuery),
    );

    let error = use_case.query().await.unwrap_err();

    assert!(matches!(
        error,
        QueryMembershipConflictsError::Locked { .. }
    ));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn removed_target_requires_re_pairing_and_rejects_a_later_opposite_choice() {
    let (repository, use_case, conflict_id, local_branch_id, remote_branch_id) =
        fixture(MembershipConflictChoice::RePairingRequired);

    assert_eq!(
        use_case
            .execute(ResolveMembershipConflictInput {
                conflict_id,
                target_branch_id: remote_branch_id,
            })
            .await
            .unwrap(),
        ResolveMembershipConflictResult::RePairingRequired { conflict_id }
    );
    assert_eq!(
        use_case
            .execute(ResolveMembershipConflictInput {
                conflict_id,
                target_branch_id: local_branch_id,
            })
            .await
            .unwrap(),
        ResolveMembershipConflictResult::StateChanged {
            current_conflict_id: Some(conflict_id),
        }
    );
    let persisted = repository.load().await.unwrap();
    assert_eq!(
        persisted.membership_conflicts[&conflict_id].status,
        MembershipConflictStatus::RePairingRequired
    );
}

#[tokio::test]
async fn recoverable_remote_choice_persists_one_stable_transition_intent() {
    let (repository, use_case, conflict_id, _, remote_branch_id) =
        fixture(MembershipConflictChoice::ActiveMemberRecovery);
    let input = ResolveMembershipConflictInput {
        conflict_id,
        target_branch_id: remote_branch_id,
    };

    assert_eq!(
        use_case.execute(input).await.unwrap(),
        ResolveMembershipConflictResult::Pending { conflict_id }
    );
    let first = repository.load().await.unwrap();
    let first_transition_id = first.membership_conflicts[&conflict_id]
        .transition_id
        .expect("remote recovery gets a durable transition id");
    assert_eq!(
        use_case.execute(input).await.unwrap(),
        ResolveMembershipConflictResult::Pending { conflict_id }
    );
    let repeated = repository.load().await.unwrap();
    assert_eq!(
        repeated.membership_conflicts[&conflict_id].transition_id,
        Some(first_transition_id)
    );
    assert_eq!(repeated.revision, first.revision);
}

#[tokio::test]
async fn concurrent_opposite_choices_commit_exactly_one_immutable_intent() {
    let (repository, use_case, conflict_id, local_branch_id, remote_branch_id) =
        fixture(MembershipConflictChoice::ActiveMemberRecovery);
    let use_case = Arc::new(use_case);

    let (local_result, remote_result) = tokio::join!(
        use_case.execute(ResolveMembershipConflictInput {
            conflict_id,
            target_branch_id: local_branch_id,
        }),
        use_case.execute(ResolveMembershipConflictInput {
            conflict_id,
            target_branch_id: remote_branch_id,
        }),
    );
    let local_result = local_result.unwrap();
    let remote_result = remote_result.unwrap();
    assert!(matches!(
        (&local_result, &remote_result),
        (
            ResolveMembershipConflictResult::Completed { .. },
            ResolveMembershipConflictResult::StateChanged { .. }
        ) | (
            ResolveMembershipConflictResult::StateChanged { .. },
            ResolveMembershipConflictResult::Pending { .. }
        )
    ));
    let persisted = repository.load().await.unwrap();
    let conflict = &persisted.membership_conflicts[&conflict_id];
    assert!(matches!(
        conflict.selected_branch_id,
        Some(selected) if selected == local_branch_id || selected == remote_branch_id
    ));
    assert_eq!(persisted.revision, 12);
}

#[tokio::test]
async fn a_later_distinct_conflict_allows_another_explicit_branch_choice() {
    let (repository, use_case, first_conflict_id, local_branch_id, _) =
        fixture(MembershipConflictChoice::ActiveMemberRecovery);
    assert!(matches!(
        use_case
            .execute(ResolveMembershipConflictInput {
                conflict_id: first_conflict_id,
                target_branch_id: local_branch_id,
            })
            .await
            .unwrap(),
        ResolveMembershipConflictResult::Completed { .. }
    ));

    let second_conflict_id = MembershipConflictId::from_bytes([0x91; 32]);
    let second_remote_branch_id = MembershipBranchId::from_bytes([0x92; 32]);
    {
        let mut record = repository.0.lock().unwrap();
        let detected_at_revision = record.revision;
        record.membership_conflicts.insert(
            second_conflict_id,
            MembershipConflictRecord {
                conflict_id: second_conflict_id,
                local_branch_id,
                remote_branch_id: second_remote_branch_id,
                local_choice: MembershipConflictChoice::ActiveMemberRecovery,
                remote_choice: MembershipConflictChoice::ActiveMemberRecovery,
                evidence_peer_device_ids: BTreeSet::from([DeviceId::new("later-peer")]),
                detected_at_revision,
                status: MembershipConflictStatus::Unresolved,
                selected_branch_id: None,
                transition_id: None,
            },
        );
    }

    assert_eq!(
        use_case
            .execute(ResolveMembershipConflictInput {
                conflict_id: second_conflict_id,
                target_branch_id: second_remote_branch_id,
            })
            .await
            .unwrap(),
        ResolveMembershipConflictResult::Pending {
            conflict_id: second_conflict_id,
        }
    );
    let persisted = repository.load().await.unwrap();
    assert_eq!(
        persisted.membership_conflicts[&first_conflict_id].status,
        MembershipConflictStatus::Completed
    );
    assert_eq!(
        persisted.membership_conflicts[&second_conflict_id].selected_branch_id,
        Some(second_remote_branch_id)
    );
}
