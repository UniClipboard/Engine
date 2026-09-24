use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberEffectKind, MemberEffectPhase, MembershipEventId, PeerLink, PeerRelation, RemovalDecision,
};
use uc_core::ports::ReachabilityState;

use super::*;
use crate::space::membership::testing::{
    established_history, member_facts, started_record, with_peer_relation, AcceptingVerifier,
    FixedSpaceWorkMode, OwnerFixture, TestSigner,
};
use crate::space::membership::{
    DeviceTrustMembership, DeviceTrustObservation, LoadDeviceTrustObservationsPort,
    MembershipMaintenanceStepOutcome, QueryDeviceTrustError, QueryDeviceTrustUseCase,
    RecoverMembershipEffectsPort,
};

#[path = "tests/device_trust_recovery_scenario.rs"]
mod device_trust_recovery_scenario;

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
                device_id: *device_id,
                display_name: Some(device_id.as_str().to_owned()),
                reachability: ReachabilityState::Offline,
            })
            .collect())
    }
}

struct NoopEffects;

#[async_trait]
impl RecoverMembershipEffectsPort for NoopEffects {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome {
        MembershipMaintenanceStepOutcome::Completed
    }
}

struct EffectCounter(AtomicUsize);

#[async_trait]
impl RecoverMembershipEffectsPort for EffectCounter {
    async fn recover_membership_effects(&self) -> MembershipMaintenanceStepOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        MembershipMaintenanceStepOutcome::Deferred
    }
}

/// 本机 `device-a` 已移除 `device-c`，`device-b` 提议移除本机、等待本机决定。
fn pending_local_removal() -> (OwnerFixture, TestSigner, MembershipEventId) {
    let (local_facts, local_credential) = member_facts("device-a", 0x41);
    let (peer_facts, peer_credential) = member_facts("device-b", 0x42);
    let (former_facts, former_credential) = member_facts("device-c", 0x43);
    let local_member = local_facts.member_instance;
    let peer_member = peer_facts.member_instance;
    let former_member = former_facts.member_instance;
    let mut history = established_history(&[
        (local_facts.clone(), local_credential.clone()),
        (peer_facts, peer_credential.clone()),
        (former_facts, former_credential),
    ]);
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
    let record = with_peer_relation(
        started_record(history, local_facts.device_id, local_member, 8),
        &DeviceId::new("device-b"),
        PeerRelation::AwaitingLocalDecision,
        None,
    );
    (
        OwnerFixture::new(record),
        TestSigner {
            local_device_id: local_facts.device_id,
            local_member,
            credential: local_credential,
        },
        change_id,
    )
}

fn decide_case(
    fixture: &OwnerFixture,
    signer: TestSigner,
    effects: Arc<dyn RecoverMembershipEffectsPort>,
) -> (DecideDeviceTrustChangeUseCase, Arc<QueryDeviceTrustUseCase>) {
    let query = Arc::new(QueryDeviceTrustUseCase::new_for_tests(
        fixture.owner.clone(),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    (
        DecideDeviceTrustChangeUseCase::new(
            fixture.owner.clone(),
            Arc::new(signer),
            query.clone(),
            effects,
        ),
        query,
    )
}

#[tokio::test]
async fn accepting_local_removal_requires_explicit_confirmation_before_writing() {
    let (fixture, signer, change_id) = pending_local_removal();
    let (decide, _) = decide_case(&fixture, signer, Arc::new(NoopEffects));

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
    assert_eq!(fixture.records.commit_count(), 0);
    assert_eq!(fixture.wake.wake_count(), 0);
}

// ADR-027：本机接受以自身为目标的移除后进入已移除终态，不向发起方排队决定投递。
#[tokio::test]
async fn confirmed_acceptance_commits_the_decision_and_stops_local_access() {
    let (fixture, signer, change_id) = pending_local_removal();
    let local_member = signer.local_member;
    let (decide, _) = decide_case(&fixture, signer, Arc::new(NoopEffects));

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
    assert_eq!(
        status.local_membership,
        DeviceTrustMembership::PendingActivation
    );
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(fixture.wake.wake_count(), 1);
    let persisted = fixture.records.ledger();
    assert_eq!(
        persisted
            .history()
            .decision_for(change_id, local_member)
            .unwrap()
            .decision,
        RemovalDecision::Accept
    );
    let effect = persisted
        .unfinished_effects()
        .find(|effect| effect.event_id() == change_id)
        .unwrap();
    assert_eq!(effect.kind(), MemberEffectKind::RemoveDevice);
    assert_eq!(effect.phase(), MemberEffectPhase::Prepared);
    let Some(PeerLink::Member(proposer)) = persisted.peer(&DeviceId::new("device-b")) else {
        panic!("the proposer stays a member");
    };
    assert_eq!(proposer.relation(), PeerRelation::Consistent);
    assert!(proposer.outgoing_decision().is_none());
}

#[tokio::test]
async fn rejection_keeps_local_membership_and_diverges_only_the_proposer() {
    let (fixture, signer, change_id) = pending_local_removal();
    let local_member = signer.local_member;
    let (decide, _) = decide_case(&fixture, signer, Arc::new(NoopEffects));

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
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(fixture.wake.wake_count(), 1);
    let persisted = fixture.records.ledger();
    assert_eq!(
        persisted
            .history()
            .decision_for(change_id, local_member)
            .unwrap()
            .decision,
        RemovalDecision::Reject
    );
    assert!(persisted.history().active_members().contains(&local_member));
    assert!(persisted
        .unfinished_effects()
        .all(|effect| effect.event_id() != change_id));
    let Some(PeerLink::Member(proposer)) = persisted.peer(&DeviceId::new("device-b")) else {
        panic!("the proposer stays a member");
    };
    assert_eq!(proposer.relation(), PeerRelation::Diverged);
    assert!(proposer.outgoing_decision().is_none());
}

#[tokio::test]
async fn handoff_rejecting_removal_does_not_require_a_second_keep_choice() {
    use crate::space::membership::HandleMembershipHistoryMessageUseCase;
    use uc_core::membership::{MembershipConflictEvidenceV3, MembershipHistoryMessage};

    let (fixture, signer, change_id) = pending_local_removal();
    let local = fixture.records.ledger().history().clone();
    let removal = local.event(change_id).unwrap().clone();
    let prior = local
        .event(removal.parent_event_id.unwrap())
        .unwrap()
        .clone();
    let mut remote = established_history(&[
        member_facts("device-a", 0x41),
        member_facts("device-b", 0x42),
        member_facts("device-c", 0x43),
    ]);
    remote
        .verify_and_receive_event(prior, &AcceptingVerifier)
        .unwrap();
    remote
        .verify_and_receive_event(removal, &AcceptingVerifier)
        .unwrap();
    let (decide, query) = decide_case(&fixture, signer, Arc::new(NoopEffects));
    assert!(query.execute().await.unwrap().current_change.is_some());
    assert!(matches!(
        decide
            .execute(DecideDeviceTrustChange {
                change_id,
                choice: DeviceTrustChangeChoice::KeepCurrentDeviceGroup,
                confirm_local_removal: false,
            })
            .await
            .unwrap(),
        DecideDeviceTrustChangeResult::KeptCurrentDeviceGroup { .. }
    ));
    assert!(query.execute().await.unwrap().current_change.is_none());
    assert!(fixture.records.space().branch_recovery.conflicts.is_empty());
    let (peer, _) = member_facts("device-b", 0x42);
    let response = HandleMembershipHistoryMessageUseCase::new(
        fixture.owner.clone(),
        FixedSpaceWorkMode::active(),
    )
    .execute(
        &crate::space::membership::handle_history_message::AuthenticatedMember::new(peer.device_id),
        MembershipHistoryMessage::ConflictEvidenceV3(MembershipConflictEvidenceV3 {
            transfer_id: remote.current_position().unwrap().history_digest,
            pages: remote.export_conflict_evidence_pages_v2(peer).unwrap(),
        }),
    )
    .await
    .unwrap();
    assert!(matches!(
        response,
        MembershipHistoryMessage::ConflictEvidenceV3(_)
    ));
    assert!(query.execute().await.unwrap().current_change.is_none());
    let persisted = fixture.records.space().branch_recovery;
    assert_eq!(
        persisted
            .conflicts
            .values()
            .filter(|conflict| conflict.status
                != crate::space::membership::MembershipConflictStatus::Completed)
            .count(),
        0,
        "拒绝移除已完成，但同一次移除的证据又产生新的分支选择"
    );
    let reason = &persisted
        .conflict_presentations
        .values()
        .next()
        .unwrap()
        .explanation;
    assert_eq!(
        reason.reason,
        uc_core::membership::MembershipConflictReason::RemovalDecisionDisagreement
    );
    assert!(reason
        .decisions
        .iter()
        .any(|fact| fact.device.device_id == DeviceId::new("device-a")
            && fact.decision == RemovalDecision::Reject));
}

#[tokio::test]
async fn repeated_decision_returns_the_original_result_without_a_second_commit() {
    let (fixture, signer, change_id) = pending_local_removal();
    let effects = Arc::new(EffectCounter(AtomicUsize::new(0)));
    let (decide, _) = decide_case(&fixture, signer, effects.clone());
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
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(fixture.wake.wake_count(), 1);
    assert_eq!(effects.0.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn one_decision_conflict_is_retried_from_a_fresh_snapshot() {
    let (fixture, signer, change_id) = pending_local_removal();
    fixture.records.conflict_next_commits(1);
    let (decide, _) = decide_case(&fixture, signer, Arc::new(NoopEffects));

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
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(fixture.wake.wake_count(), 1);
}
