use super::*;
use crate::space::membership::{
    DeviceTrustMembership, DeviceTrustStatus, QueryDeviceTrustError,
    QueryMembershipConflictStatusPort, ResolveMembershipConflictInput,
    ResolveMembershipConflictResult, ResolveMembershipConflictUseCase,
};
use async_trait::async_trait;

#[path = "handoff_reproduction/legacy_candidate_convergence_scenario.rs"]
mod legacy_candidate_convergence_scenario;

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
            inbound_pairings: Vec::new(),
            pending_inbound_member: None,
            space_device_update: crate::space::membership::SpaceDeviceUpdateStatus::completed(),
            devices: Vec::new(),
        })
    }
}

struct Fixture {
    owner_fixture: OwnerFixture,
    ledger: Arc<crate::space::membership::MembershipOwner>,
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
        let base = established_history(&members);
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
        let owner_fixture = OwnerFixture::new(started_record(
            local,
            members[0].0.device_id,
            members[0].0.member_instance,
            30,
        ));
        let ledger = owner_fixture.owner.clone();
        Self {
            owner_fixture,
            ledger,
            remote,
            peer: members[1].0.clone(),
            relay: members[2].0.clone(),
            local_removal,
        }
    }

    async fn deliver(&self, sender: &AdmissionChangeFacts) {
        let response = HandleMembershipHistoryMessageUseCase::new(
            self.ledger.clone(),
            FixedSpaceWorkMode::active(),
        )
        .execute(
            &AuthenticatedMember::new(sender.device_id),
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
    let revision = fixture.owner_fixture.records.record().revision();
    for _ in 0..3 {
        fixture.deliver(&fixture.peer).await;
    }
    fixture.assert_no_prompt().await;
    assert_eq!(fixture.owner_fixture.records.record().revision(), revision);
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
            .owner_fixture
            .records
            .space()
            .branch_recovery
            .conflicts
            .len(),
        1
    );
}

#[tokio::test]
async fn handoff_reconstructed_owner_preserves_completed_choice() {
    let mut fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.keep_local().await;
    fixture.ledger = fixture.owner_fixture.reopen();
    fixture.deliver(&fixture.peer).await;
    fixture.assert_no_prompt().await;
}

#[tokio::test]
async fn handoff_late_known_sibling_evidence_does_not_reopen_completed_choice() {
    let mut fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.keep_local().await;
    let original = fixture
        .owner_fixture
        .records
        .space()
        .branch_recovery
        .conflicts
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
    let persisted = fixture.owner_fixture.records.space().branch_recovery;
    assert_eq!(
        persisted.conflicts[&original.conflict_id].status,
        crate::space::membership::MembershipConflictStatus::Completed
    );
    assert_eq!(persisted.conflicts.len(), 1);
    fixture.assert_no_prompt().await;
}

#[tokio::test]
async fn same_applied_branch_with_extra_evidence_is_consistent() {
    let mut fixture = Fixture::new();
    let old_remote = fixture.remote.clone();
    fixture.remote = fixture.owner_fixture.records.space().ledger.history;
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
    let peer = fixture.peer.device_id;
    fixture.owner_fixture.edit(|space| {
        let Some(uc_core::membership::PeerLinkSnapshot::Member(member)) =
            space.ledger.peers.get_mut(&peer)
        else {
            panic!("the peer is a member");
        };
        member.relation = PeerRelation::Diverged;
    });
    fixture.deliver(&fixture.peer).await;
    assert!(fixture
        .owner_fixture
        .records
        .space()
        .branch_recovery
        .conflicts
        .is_empty());
    assert_eq!(
        relation(&fixture.owner_fixture, &peer),
        Some(PeerRelation::Consistent)
    );
    assert!(confirmed_position(&fixture.owner_fixture, &peer).is_none());
}

#[tokio::test]
async fn verified_legacy_records_preserve_choices_without_duplicate_prompts() {
    for completed in [false, true] {
        let fixture = Fixture::new();
        fixture.deliver(&fixture.peer).await;
        if completed {
            fixture.keep_local().await;
        }
        let loaded = fixture.owner_fixture.records.space();
        let legacy = uc_core::membership::MembershipConflictPolicy::legacy_description(
            &loaded.ledger.history,
            &fixture.remote,
            loaded.ledger.local_member,
        )
        .unwrap();
        let mut old = loaded
            .branch_recovery
            .conflicts
            .into_values()
            .next()
            .unwrap();
        old.conflict_id = legacy.conflict_id;
        old.local_branch_id = legacy.local_branch_id;
        old.remote_branch_id = legacy.remote_branch_id;
        old.selected_branch_id = completed.then_some(legacy.local_branch_id);
        fixture.owner_fixture.edit(|stored| {
            stored.branch_recovery.conflicts.clear();
            stored.branch_recovery.conflict_presentations.clear();
            stored
                .branch_recovery
                .conflicts
                .insert(old.conflict_id, old.clone());
        });
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
                .owner_fixture
                .records
                .space()
                .branch_recovery
                .conflicts[&legacy.conflict_id],
            old
        );
    }
}

#[tokio::test]
async fn verified_candidates_explain_changes_and_keep_remote_sync_pending() {
    use uc_core::membership::{MembershipChangeSide, MembershipConflictReason};
    let mut fixture = Fixture::new();
    fixture.deliver(&fixture.peer).await;
    fixture.ledger = fixture.owner_fixture.reopen();
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
    fixture.owner_fixture.edit(|loaded| {
        loaded.branch_recovery.conflict_presentations.clear();
        loaded.ledger.revision += 1;
    });
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
    let response = HandleMembershipHistoryMessageUseCase::new(
        fixture.ledger.clone(),
        FixedSpaceWorkMode::active(),
    )
    .execute(
        &AuthenticatedMember::new(fixture.peer.device_id),
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
    let stored = fixture.owner_fixture.records.space().branch_recovery;
    assert!(stored.conflict_presentations.is_empty());
    assert!(stored.conflicts.is_empty());
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
