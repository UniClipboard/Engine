use std::collections::BTreeSet;
use std::error::Error;
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, MembershipBranchId, MembershipConflictChoice, MembershipConflictId,
    MembershipCredential, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};

use super::*;
use crate::space::membership::testing::{started_record, MemoryMembershipRecords, OwnerFixture};
use crate::space::membership::{
    DeviceTrustMembership, MembershipConflictRecord, MembershipConflictStatus,
    MembershipLedgerError, MembershipRecord, MembershipRecordCommit, MembershipRecordStorePort,
};

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
            inbound_pairings: Vec::new(),
            pending_inbound_member: None,
            space_device_update: crate::space::membership::SpaceDeviceUpdateStatus::completed(),
            devices: Vec::new(),
        })
    }
}

fn fixture(
    remote_choice: MembershipConflictChoice,
) -> (
    Arc<MemoryMembershipRecords>,
    ResolveMembershipConflictUseCase,
    MembershipConflictId,
    MembershipBranchId,
    MembershipBranchId,
    OwnerFixture,
) {
    let device_id = DeviceId::new("local");
    let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x41; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    let history = VersionedMembershipHistory::new_single_member_root(
        "space-a".to_owned(),
        AdmissionChangeFacts {
            member_instance,
            device_id,
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
    let local_branch_id =
        uc_core::membership::MembershipConflictPolicy::branch_id(&history).unwrap();
    let remote_branch_id = MembershipBranchId::from_bytes([0x83; 32]);
    let MembershipRecord::Space(mut record) =
        started_record(history, device_id, member_instance, 11)
    else {
        unreachable!("started record has a space");
    };
    record.branch_recovery.conflicts.insert(
        conflict_id,
        MembershipConflictRecord {
            conflict_id,
            local_branch_id,
            remote_branch_id,
            local_choice: MembershipConflictChoice::ActiveMemberRecovery,
            remote_choice,
            evidence_peer_device_ids: BTreeSet::from([DeviceId::new("peer")]),
            detected_at_revision: 11,
            status: MembershipConflictStatus::Unresolved,
            selected_branch_id: None,
            transition_id: None,
        },
    );
    let fixture = OwnerFixture::new(MembershipRecord::Space(record));
    let use_case =
        ResolveMembershipConflictUseCase::new(fixture.owner.clone(), Arc::new(FixedQuery));
    (
        fixture.records.clone(),
        use_case,
        conflict_id,
        local_branch_id,
        remote_branch_id,
        fixture,
    )
}

#[tokio::test]
async fn keeping_local_branch_completes_once_and_repeats_idempotently() {
    let (repository, use_case, conflict_id, local_branch_id, _, _fixture) =
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
    let persisted = repository.space();
    let conflict = persisted
        .branch_recovery
        .conflicts
        .get(&conflict_id)
        .unwrap();
    assert_eq!(conflict.status, MembershipConflictStatus::Completed);
    assert_eq!(conflict.selected_branch_id, Some(local_branch_id));
    assert_eq!(
        persisted.ledger.revision, 12,
        "the repeated call does not commit"
    );
}

#[tokio::test]
async fn query_returns_complete_branch_choices_without_claiming_global_resolution() {
    let (_, use_case, conflict_id, local_branch_id, remote_branch_id, _fixture) =
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

struct LockedRecords;

#[async_trait]
impl MembershipRecordStorePort for LockedRecords {
    async fn load(&self) -> Result<MembershipRecord, MembershipLedgerError> {
        Err(MembershipLedgerError::Locked)
    }

    async fn commit(&self, _commit: MembershipRecordCommit) -> Result<(), MembershipLedgerError> {
        Err(MembershipLedgerError::Locked)
    }
}

#[tokio::test]
async fn query_error_preserves_stable_classification_and_source() {
    let owner = Arc::new(crate::space::membership::MembershipOwner::new(
        Arc::new(LockedRecords),
        Arc::new(crate::space::membership::testing::AcceptingVerifier),
        crate::space::membership::testing::TestClock::at(0),
        Arc::new(crate::space::membership::testing::RecordingHostEvents::default()),
        Arc::new(crate::space::membership::testing::RecordingWake::default()),
    ));
    let use_case = ResolveMembershipConflictUseCase::new(owner, Arc::new(FixedQuery));

    let error = use_case.query().await.unwrap_err();

    assert!(matches!(
        error,
        QueryMembershipConflictsError::Locked { .. }
    ));
    assert!(error.source().is_some());
}

#[tokio::test]
async fn removed_target_requires_re_pairing_and_rejects_a_later_opposite_choice() {
    let (repository, use_case, conflict_id, local_branch_id, remote_branch_id, _fixture) =
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
    assert_eq!(
        repository.space().branch_recovery.conflicts[&conflict_id].status,
        MembershipConflictStatus::RePairingRequired
    );
}

#[tokio::test]
async fn recoverable_remote_choice_persists_one_stable_transition_intent() {
    let (repository, use_case, conflict_id, _, remote_branch_id, _fixture) =
        fixture(MembershipConflictChoice::ActiveMemberRecovery);
    let input = ResolveMembershipConflictInput {
        conflict_id,
        target_branch_id: remote_branch_id,
    };

    assert_eq!(
        use_case.execute(input).await.unwrap(),
        ResolveMembershipConflictResult::Pending { conflict_id }
    );
    let first = repository.space();
    let first_transition_id = first.branch_recovery.conflicts[&conflict_id]
        .transition_id
        .expect("remote recovery gets a durable transition id");
    assert_eq!(
        use_case.execute(input).await.unwrap(),
        ResolveMembershipConflictResult::Pending { conflict_id }
    );
    let repeated = repository.space();
    assert_eq!(
        repeated.branch_recovery.conflicts[&conflict_id].transition_id,
        Some(first_transition_id)
    );
    assert_eq!(repeated.ledger.revision, first.ledger.revision);
}

#[tokio::test]
async fn concurrent_opposite_choices_commit_exactly_one_immutable_intent() {
    let (repository, use_case, conflict_id, local_branch_id, remote_branch_id, _fixture) =
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
    let persisted = repository.space();
    let conflict = &persisted.branch_recovery.conflicts[&conflict_id];
    assert!(matches!(
        conflict.selected_branch_id,
        Some(selected) if selected == local_branch_id || selected == remote_branch_id
    ));
    assert_eq!(persisted.ledger.revision, 12);
}

#[tokio::test]
async fn a_later_distinct_conflict_allows_another_explicit_branch_choice() {
    let (repository, use_case, first_conflict_id, local_branch_id, _, fixture) =
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
        let mut record = repository.space();
        let detected_at_revision = record.ledger.revision;
        record.ledger.revision += 1;
        record.branch_recovery.conflicts.insert(
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
        repository.replace(MembershipRecord::Space(Box::new(record)));
        fixture.owner.reload_for_test();
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
    let persisted = repository.space().branch_recovery;
    assert_eq!(
        persisted.conflicts[&first_conflict_id].status,
        MembershipConflictStatus::Completed
    );
    assert_eq!(
        persisted.conflicts[&second_conflict_id].selected_branch_id,
        Some(second_remote_branch_id)
    );
}
