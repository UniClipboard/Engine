use super::*;
use crate::space::membership::{
    DeviceTrustMembership, DeviceTrustStatus, QueryDeviceTrustError,
    QueryMembershipConflictStatusPort, ResolveMembershipConflictInput,
    ResolveMembershipConflictResult, ResolveMembershipConflictUseCase,
};

struct QueryStatus;

#[async_trait]
impl QueryMembershipConflictStatusPort for QueryStatus {
    async fn query_status(&self) -> Result<DeviceTrustStatus, QueryDeviceTrustError> {
        Ok(DeviceTrustStatus {
            revision: 0,
            local_device_id: Some(DeviceId::new("device-a")),
            local_membership: DeviceTrustMembership::Active,
            current_change: None,
            current_join: None,
            pending_inbound_member: None,
            devices: Vec::new(),
        })
    }
}

struct Fixture {
    repository: Arc<MemoryLedgerRepository>,
    ledger: Arc<MembershipLedger>,
    remote: VersionedMembershipHistory,
    peer: AdmissionChangeFacts,
    relay: AdmissionChangeFacts,
    local_removal: MembershipEventV2,
}

impl Fixture {
    fn new() -> Self {
        let members = [
            member_facts("device-a", 0x41),
            member_facts("device-b", 0x42),
            member_facts("device-c", 0x43),
            member_facts("device-d", 0x44),
        ];
        let base = VersionedMembershipHistory::from_activation_baseline(
            MembershipActivationBaselineV2::Established {
                lineage_id: "space-a".to_owned(),
                head_event_id: MembershipEventId::from_hex(&"11".repeat(32)).unwrap(),
                head_depth: 0,
                current_members: members.to_vec(),
            },
        )
        .unwrap();
        let author = |index: usize| MembershipAdmissionV2 {
            facts: members[index].0.clone(),
            membership_credential: members[index].1.clone(),
            resume_public_key_digest: [7; 32],
            security_commitment_id: [8; 32],
        };
        let local_removal = remove_event(&base, &author(0), members[2].0.member_instance, 0x51);
        let remote_removal = remove_event(&base, &author(1), members[3].0.member_instance, 0x61);
        let mut local = base.clone();
        local
            .verify_and_receive_event(local_removal.clone(), &AcceptingVerifier)
            .unwrap();
        let mut remote = base;
        remote
            .verify_and_receive_event(remote_removal, &AcceptingVerifier)
            .unwrap();
        let mut loaded = LoadedMembershipLedger::no_current_space();
        loaded.revision = 30;
        loaded.lineage_id = Some("space-a".to_owned());
        loaded.membership_history = Some(local.encode_persisted_v2().unwrap());
        loaded.local_device_id = Some(members[0].0.device_id.clone());
        loaded.local_member_instance = Some(members[0].0.member_instance);
        loaded.local_join_active = true;
        for (facts, _) in &members[1..] {
            loaded.peer_reconciliation.insert(
                facts.device_id.clone(),
                PeerReconciliationRecord {
                    peer_device_id: facts.device_id.clone(),
                    relationship: MembershipHistoryRelationship::Consistent,
                    confirmed_position: None,
                    sync_state: Default::default(),
                    restricted_delivery: Vec::new(),
                    updated_at_ms: 1,
                },
            );
        }
        let repository = Arc::new(MemoryLedgerRepository {
            loaded: Mutex::new(loaded),
            commits: AtomicUsize::new(0),
            fail_on_commit: None,
        });
        let ledger = Arc::new(MembershipLedger::new(
            repository.clone(),
            repository.clone(),
            Arc::new(AcceptingVerifier),
        ));
        Self {
            repository,
            ledger,
            remote,
            peer: members[1].0.clone(),
            relay: members[2].0.clone(),
            local_removal,
        }
    }

    async fn deliver(&self, sender: &AdmissionChangeFacts) {
        let response = HandleMembershipHistoryMessageUseCase::new(self.ledger.clone())
            .execute(
                &AuthenticatedMember::new(sender.device_id.clone()),
                MembershipHistoryMessage::ConflictEvidenceV3(MembershipConflictEvidenceV3 {
                    transfer_id: self.remote.current_position().unwrap().history_digest,
                    pages: self
                        .remote
                        .export_conflict_evidence_pages_v2(sender.clone())
                        .unwrap(),
                }),
            )
            .await
            .unwrap();
        assert!(matches!(
            response,
            MembershipHistoryMessage::ConflictEvidenceV3(_)
        ));
    }

    fn resolver(&self) -> ResolveMembershipConflictUseCase {
        ResolveMembershipConflictUseCase::new(self.ledger.clone(), Arc::new(QueryStatus))
    }

    async fn keep_local(&self) {
        let view = self.resolver().query().await.unwrap();
        assert_eq!(view.conflicts.len(), 1);
        let conflict = &view.conflicts[0];
        let input = ResolveMembershipConflictInput {
            conflict_id: conflict.conflict_id,
            target_branch_id: conflict
                .branches
                .iter()
                .find(|branch| branch.is_local)
                .unwrap()
                .branch_id,
        };
        assert!(matches!(
            self.resolver().execute(input).await.unwrap(),
            ResolveMembershipConflictResult::Completed { .. }
        ));
        self.assert_no_prompt().await;
    }

    async fn assert_no_prompt(&self) {
        let view = self.resolver().query().await.unwrap();
        assert_eq!(
            view.conflicts
                .iter()
                .filter(|conflict| !conflict.local_resolution_completed)
                .count(),
            0,
            "已完成选择后，相同设备分组的证据再次产生待处理事项"
        );
    }
}

#[tokio::test]
async fn handoff_identical_evidence_after_choice_does_not_reopen() {
    let fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.keep_local().await;
    let revision = fixture.repository.load().await.unwrap().revision;
    for _ in 0..3 {
        fixture.deliver(&fixture.peer).await;
    }
    fixture.assert_no_prompt().await;
    assert_eq!(fixture.repository.load().await.unwrap().revision, revision);
}

#[tokio::test]
async fn handoff_same_branch_from_another_peer_does_not_reopen() {
    let fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.keep_local().await;
    fixture.deliver(&fixture.relay).await;
    fixture.assert_no_prompt().await;
    assert_eq!(
        fixture
            .repository
            .load()
            .await
            .unwrap()
            .membership_conflicts
            .len(),
        1
    );
}

#[tokio::test]
async fn handoff_reconstructed_owner_preserves_completed_choice() {
    let mut fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.keep_local().await;
    fixture.ledger = Arc::new(MembershipLedger::new(
        fixture.repository.clone(),
        fixture.repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    fixture.deliver(&fixture.peer).await;
    fixture.assert_no_prompt().await;
}

#[tokio::test]
async fn handoff_late_known_sibling_evidence_does_not_reopen_completed_choice() {
    let mut fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.keep_local().await;
    let original = fixture
        .repository
        .load()
        .await
        .unwrap()
        .membership_conflicts
        .into_values()
        .next()
        .unwrap();
    let before = fixture.remote.current_position().unwrap();
    let members = fixture.remote.effective_members();
    // 只补入本机已经知道的另一分支事件，远端没有执行新的成员变更。
    fixture
        .remote
        .verify_and_receive_event(fixture.local_removal.clone(), &AcceptingVerifier)
        .unwrap();
    let after = fixture.remote.current_position().unwrap();
    assert_eq!(before.event_id, after.event_id);
    assert_eq!(members, fixture.remote.effective_members());
    assert_ne!(before.history_digest, after.history_digest);
    fixture.deliver(&fixture.peer).await;
    let persisted = fixture.repository.load().await.unwrap();
    assert_eq!(
        persisted.membership_conflicts[&original.conflict_id].status,
        crate::space::membership::MembershipConflictStatus::Completed
    );
    assert_eq!(persisted.membership_conflicts.len(), 1);
    fixture.assert_no_prompt().await;
}

#[tokio::test]
async fn same_applied_branch_with_extra_evidence_is_consistent() {
    let mut fixture = Fixture::new();
    let loaded = fixture.repository.load().await.unwrap();
    let old_remote = fixture.remote.clone();
    fixture.remote = VersionedMembershipHistory::decode_persisted_v2(
        loaded.membership_history.as_deref().unwrap(),
        &AcceptingVerifier,
    )
    .unwrap();
    fixture
        .remote
        .verify_and_receive_event(
            old_remote
                .event(old_remote.current_head().unwrap())
                .unwrap()
                .clone(),
            &AcceptingVerifier,
        )
        .unwrap();
    fixture
        .repository
        .loaded
        .lock()
        .unwrap()
        .peer_reconciliation
        .get_mut(&fixture.peer.device_id)
        .unwrap()
        .relationship = MembershipHistoryRelationship::Diverged;
    fixture.deliver(&fixture.peer).await;
    let stored = fixture.repository.load().await.unwrap();
    assert!(stored.membership_conflicts.is_empty());
    assert_eq!(
        stored.peer_reconciliation[&fixture.peer.device_id].relationship,
        MembershipHistoryRelationship::Consistent
    );
    assert!(stored.peer_reconciliation[&fixture.peer.device_id]
        .confirmed_position
        .is_none());
}

#[tokio::test]
async fn verified_legacy_records_preserve_choices_without_duplicate_prompts() {
    for completed in [false, true] {
        let fixture = Fixture::new();
        fixture.deliver(&fixture.peer).await;
        if completed {
            fixture.keep_local().await;
        }
        let loaded = fixture.repository.load().await.unwrap();
        let local = VersionedMembershipHistory::decode_persisted_v2(
            loaded.membership_history.as_deref().unwrap(),
            &AcceptingVerifier,
        )
        .unwrap();
        let legacy = uc_core::membership::MembershipConflictPolicy::legacy_description(
            &local,
            &fixture.remote,
            loaded.local_member_instance.unwrap(),
        )
        .unwrap();
        let mut old = loaded.membership_conflicts.into_values().next().unwrap();
        old.conflict_id = legacy.conflict_id;
        old.local_branch_id = legacy.local_branch_id;
        old.remote_branch_id = legacy.remote_branch_id;
        old.selected_branch_id = completed.then_some(legacy.local_branch_id);
        {
            let mut stored = fixture.repository.loaded.lock().unwrap();
            stored.membership_conflicts.clear();
            stored.membership_conflict_presentations.clear();
            stored
                .membership_conflicts
                .insert(old.conflict_id, old.clone());
        }
        fixture.deliver(&fixture.peer).await;
        let view = fixture.resolver().query().await.unwrap();
        assert_eq!(
            view.conflicts
                .iter()
                .filter(|conflict| !conflict.local_resolution_completed)
                .count(),
            usize::from(!completed)
        );
        assert_eq!(
            fixture
                .repository
                .load()
                .await
                .unwrap()
                .membership_conflicts[&legacy.conflict_id],
            old
        );
    }
}

#[tokio::test]
async fn verified_candidates_explain_changes_and_keep_remote_sync_pending() {
    use uc_core::membership::{MembershipChangeSide, MembershipConflictReason};
    let mut fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.ledger = Arc::new(MembershipLedger::new(
        fixture.repository.clone(),
        fixture.repository.clone(),
        Arc::new(AcceptingVerifier),
    ));
    let view = fixture.resolver().query().await.unwrap();
    let conflict = &view.conflicts[0];
    assert_eq!(
        conflict.explanation.reason,
        MembershipConflictReason::DifferentRemovals
    );
    assert!(conflict.explanation.details_complete);
    let local_change = conflict
        .explanation
        .changes
        .iter()
        .find(|change| change.side == MembershipChangeSide::Local)
        .unwrap();
    assert_eq!(local_change.actor.device_id, DeviceId::new("device-a"));
    assert_eq!(local_change.target.device_id, DeviceId::new("device-c"));
    let remote_change = conflict
        .explanation
        .changes
        .iter()
        .find(|change| change.side == MembershipChangeSide::Remote)
        .unwrap();
    assert_eq!(remote_change.actor.device_id, DeviceId::new("device-b"));
    assert_eq!(remote_change.target.device_id, DeviceId::new("device-d"));
    let remote = &conflict.branches[1];
    let members = remote.members.as_ref().unwrap();
    assert_eq!(
        members
            .iter()
            .map(|member| member.device.device_id.as_str())
            .collect::<Vec<_>>(),
        ["device-a", "device-b", "device-c"]
    );
    assert!(members
        .iter()
        .all(|member| member.device.display_name == member.device.device_id.as_str()));
    assert_eq!(remote.source_device_ids, vec![DeviceId::new("device-b")]);
    let impact = remote.impact.as_ref().unwrap();
    assert_eq!(
        impact.sync_scope_device_ids,
        ["device-b", "device-c"].map(DeviceId::new)
    );
    assert_eq!(
        impact.pending_confirmation_device_ids,
        impact.sync_scope_device_ids
    );
    assert_eq!(impact.paused_device_ids, vec![DeviceId::new("device-d")]);
    assert_eq!(
        impact.requires_rejoin_device_ids,
        vec![DeviceId::new("device-d")]
    );
    assert!(impact
        .sync_scope_device_ids
        .iter()
        .all(|id| !impact.paused_device_ids.contains(id)));
}

#[tokio::test]
async fn old_conflict_without_presentation_stays_explicitly_unknown() {
    let fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture
        .repository
        .loaded
        .lock()
        .unwrap()
        .membership_conflict_presentations
        .clear();
    let view = fixture.resolver().query().await.unwrap();
    let conflict = &view.conflicts[0];
    assert_eq!(
        conflict.explanation.reason,
        uc_core::membership::MembershipConflictReason::Unknown
    );
    assert!(!conflict.explanation.details_complete);
    assert!(conflict.branches[0].members.is_some());
    assert!(conflict.branches[1].members.is_none());
    assert!(conflict.branches[1].impact.is_none());
    fixture.deliver(&fixture.peer).await;
    assert!(
        fixture.resolver().query().await.unwrap().conflicts[0].branches[1]
            .members
            .is_some()
    );
}

#[tokio::test]
async fn rejected_evidence_does_not_create_display_facts() {
    let fixture = Fixture::new();
    let response = HandleMembershipHistoryMessageUseCase::new(fixture.ledger.clone())
        .execute(
            &AuthenticatedMember::new(fixture.peer.device_id.clone()),
            MembershipHistoryMessage::ConflictEvidenceV3(MembershipConflictEvidenceV3 {
                transfer_id: [0; 32],
                pages: fixture
                    .remote
                    .export_conflict_evidence_pages_v2(fixture.peer.clone())
                    .unwrap(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        response,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid)
    );
    let stored = fixture.repository.load().await.unwrap();
    assert!(stored.membership_conflict_presentations.is_empty());
    assert!(stored.membership_conflicts.is_empty());
}

#[tokio::test]
async fn target_without_local_membership_requires_rejoin_and_has_no_sync_scope() {
    let mut fixture = Fixture::new();
    let (local, _) = member_facts("device-a", 0x41);
    let (_, credential) = member_facts("device-b", 0x42);
    let author = MembershipAdmissionV2 {
        facts: fixture.peer.clone(),
        membership_credential: credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let event = remove_event(&fixture.remote, &author, local.member_instance, 0x79);
    fixture
        .remote
        .verify_and_receive_event(event, &AcceptingVerifier)
        .unwrap();
    fixture.deliver(&fixture.peer).await;
    let view = fixture.resolver().query().await.unwrap();
    let remote = &view.conflicts[0].branches[1];
    assert_eq!(
        remote.choice,
        uc_core::membership::MembershipConflictChoice::RePairingRequired
    );
    let impact = remote.impact.as_ref().unwrap();
    assert!(impact.sync_scope_device_ids.is_empty());
    assert_eq!(
        impact.requires_rejoin_device_ids,
        vec![local.device_id, DeviceId::new("device-d")]
    );
    assert_eq!(impact.local_membership, DeviceTrustMembership::Removed);
    assert!(!view.conflicts[0].explanation.details_complete);
}

#[tokio::test]
async fn handoff_new_remote_removal_is_a_distinct_choice() {
    let mut fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.keep_local().await;
    let (_, credential) = member_facts("device-b", 0x42);
    let author = MembershipAdmissionV2 {
        facts: fixture.peer.clone(),
        membership_credential: credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let event = remove_event(
        &fixture.remote,
        &author,
        fixture.relay.member_instance,
        0x71,
    );
    let before = fixture.remote.current_head();
    fixture
        .remote
        .verify_and_receive_event(event, &AcceptingVerifier)
        .unwrap();
    assert_ne!(fixture.remote.current_head(), before);
    fixture.deliver(&fixture.peer).await;
    let view = fixture.resolver().query().await.unwrap();
    assert_eq!(
        view.conflicts
            .iter()
            .filter(|conflict| !conflict.local_resolution_completed)
            .count(),
        1
    );
}
