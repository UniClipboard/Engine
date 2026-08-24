use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    MembershipEventId, MembershipHistoryRelationship, MembershipOperationV2, RemovalDecision,
    SpaceMembershipState, VersionedMembershipHistory,
};

use super::{
    DecidePendingMembershipRemovalDeps, DecidePendingMembershipRemovalResult,
    DecidePendingMembershipRemovalUseCase,
};
use crate::space::membership_history::{MembershipHistoryRepositoryPort, MembershipHistoryStore};
use crate::space::membership_runtime::MembershipRecoveryRequests;
use crate::space::membership_state::{
    SpaceMembershipStateEvents, SpaceMembershipStateRepositoryPort,
};
use crate::space::query_space_membership_status::{
    ActiveSpaceMembershipStatusDeps, QuerySpaceMembershipStatusDeps,
    QuerySpaceMembershipStatusUseCase,
};
use crate::space::workspace_membership::tests::{
    durable_admission_repository, durable_candidate_removal_fixture,
    durable_candidate_verification_fixture, legacy_member, CredentialBackedSigner,
    DeterministicHistoricalVerifier, FixedMemberRepo, FixedPresence, MemoryWorkspaceRepository,
    TestAdmissionRepository, UnusedClock,
};

const LOCAL_DEVICE: &str = "joiner";
const REMOVAL_AUTHOR: &str = "sponsor";

struct DecisionHarness {
    _directory: tempfile::TempDir,
    use_case: Arc<DecidePendingMembershipRemovalUseCase>,
    history_repository: Arc<dyn TestAdmissionRepository>,
    state_repository: MemoryWorkspaceRepository,
    removal_event_id: MembershipEventId,
    local_member: uc_core::membership::MemberInstanceId,
}

async fn decision_harness() -> DecisionHarness {
    let directory = tempfile::tempdir().unwrap();
    let history_repository = durable_admission_repository(&directory, [0xa6; 16]);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xa7; 32]);
    let (_, mut local_history, candidate, _, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let MembershipOperationV2::AddDevice { admission } = &candidate.operation else {
        unreachable!("fixture always creates AddDevice")
    };
    let local_credential = admission.membership_credential.clone();
    let local_member = admission.facts.member_instance;
    local_history
        .verify_and_receive_event(candidate.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    local_history
        .verify_and_record_activation_receipt(activation_receipt, &DeterministicHistoricalVerifier)
        .unwrap();

    let removal = durable_candidate_removal_fixture(attempt_id);
    let mut author_history = local_history.clone();
    author_history
        .verify_and_receive_event(removal.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    local_history
        .merge_remote_history(
            &author_history,
            local_member,
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    history_repository
        .compare_and_replace_membership_history(None, &local_history.encode_persisted_v2().unwrap())
        .await
        .unwrap();

    let state_repository = MemoryWorkspaceRepository::default();
    let mut state = SpaceMembershipState::fresh(removal.lineage_id.clone(), 1);
    state.own_instance = Some(local_member);
    state_repository.save_state(&state).await.unwrap();

    let own_device = DeviceId::new(LOCAL_DEVICE);
    let member_signatures = Arc::new(CredentialBackedSigner {
        device_id: own_device.clone(),
        credential: local_credential,
    });
    let member_repo = Arc::new(FixedMemberRepo(vec![
        legacy_member(REMOVAL_AUTHOR),
        legacy_member(LOCAL_DEVICE),
    ]));
    let presence = Arc::new(FixedPresence::default());
    let clock = Arc::new(UnusedClock);
    let admission_attempts: Arc<dyn crate::deps::AdmissionAttemptRepositoryPort> =
        Arc::clone(&history_repository);
    let history_port: Arc<dyn MembershipHistoryRepositoryPort> = Arc::clone(&history_repository);
    let state_port: Arc<dyn SpaceMembershipStateRepositoryPort> =
        Arc::new(state_repository.clone());
    let membership_history = Arc::new(MembershipHistoryStore::new(
        Arc::clone(&history_port),
        Arc::new(DeterministicHistoricalVerifier),
    ));

    let status_query = Arc::new(QuerySpaceMembershipStatusUseCase::new(
        QuerySpaceMembershipStatusDeps {
            admission_attempts,
            own_device: own_device.clone(),
            clock: clock.clone(),
        },
    ));
    status_query
        .replace_active_space(Some(ActiveSpaceMembershipStatusDeps {
            state_repository: Arc::clone(&state_port),
            membership_history: Arc::clone(&membership_history),
            member_signatures: member_signatures.clone(),
            member_repo: member_repo.clone(),
            presence,
        }))
        .await;

    let use_case = Arc::new(DecidePendingMembershipRemovalUseCase::new(
        DecidePendingMembershipRemovalDeps {
            membership_history,
            state_repository: state_port,
            member_signatures,
            own_device,
            clock,
            state_write_lock: Arc::new(tokio::sync::Mutex::new(())),
            state_events: SpaceMembershipStateEvents::new(),
            recovery_requests: MembershipRecoveryRequests::new(),
            membership_status_query: status_query,
        },
    ));

    DecisionHarness {
        _directory: directory,
        use_case,
        history_repository,
        state_repository,
        removal_event_id: removal.event_id(),
        local_member,
    }
}

#[tokio::test]
async fn rejecting_a_pending_removal_records_the_decision_and_divergence() {
    let harness = decision_harness().await;

    let result = harness
        .use_case
        .execute(harness.removal_event_id, RemovalDecision::Reject, false)
        .await
        .unwrap();

    assert!(matches!(
        result,
        DecidePendingMembershipRemovalResult::Rejected { .. }
    ));
    let history = load_history(&harness).await;
    assert_eq!(
        history
            .decision_for(harness.removal_event_id, harness.local_member)
            .map(|decision| decision.decision),
        Some(RemovalDecision::Reject)
    );
    let state = harness
        .state_repository
        .load_state()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state
            .peer_history_relationships
            .get(&DeviceId::new(REMOVAL_AUTHOR)),
        Some(&MembershipHistoryRelationship::Diverged)
    );
}

#[tokio::test]
async fn accepting_self_removal_requires_confirmation_before_saving() {
    let harness = decision_harness().await;

    let first = harness
        .use_case
        .execute(harness.removal_event_id, RemovalDecision::Accept, false)
        .await
        .unwrap();
    assert!(matches!(
        first,
        DecidePendingMembershipRemovalResult::SelfRemovalConfirmationRequired { .. }
    ));
    assert!(load_history(&harness)
        .await
        .decision_for(harness.removal_event_id, harness.local_member)
        .is_none());

    let confirmed = harness
        .use_case
        .execute(harness.removal_event_id, RemovalDecision::Accept, true)
        .await
        .unwrap();
    assert!(matches!(
        confirmed,
        DecidePendingMembershipRemovalResult::Accepted { .. }
    ));
    assert!(!load_history(&harness)
        .await
        .active_members()
        .contains(&harness.local_member));
}

#[tokio::test]
async fn repeated_or_conflicting_decisions_return_the_saved_decision() {
    let harness = decision_harness().await;
    harness
        .use_case
        .execute(harness.removal_event_id, RemovalDecision::Reject, false)
        .await
        .unwrap();

    for requested in [RemovalDecision::Reject, RemovalDecision::Accept] {
        let result = harness
            .use_case
            .execute(harness.removal_event_id, requested, true)
            .await
            .unwrap();
        assert!(matches!(
            result,
            DecidePendingMembershipRemovalResult::AlreadyDecided {
                decision: RemovalDecision::Reject,
                ..
            }
        ));
    }
}

#[tokio::test]
async fn concurrent_matching_decisions_save_one_completion() {
    let harness = decision_harness().await;

    let (first, second) = tokio::join!(
        harness
            .use_case
            .execute(harness.removal_event_id, RemovalDecision::Reject, false),
        harness
            .use_case
            .execute(harness.removal_event_id, RemovalDecision::Reject, false),
    );
    let results = [first.unwrap(), second.unwrap()];
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                DecidePendingMembershipRemovalResult::Rejected { .. }
            ))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(
                result,
                DecidePendingMembershipRemovalResult::AlreadyDecided { .. }
            ))
            .count(),
        1
    );
}

async fn load_history(harness: &DecisionHarness) -> VersionedMembershipHistory {
    let encoded = harness
        .history_repository
        .load_membership_history()
        .await
        .unwrap()
        .unwrap();
    VersionedMembershipHistory::decode_persisted_v2(&encoded, &DeterministicHistoricalVerifier)
        .unwrap()
}
