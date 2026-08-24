use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    MembershipOperationV2, SpaceMembershipState, VersionedMembershipHistory,
};

use super::{
    InitiateSpaceMemberRemovalDeps, InitiateSpaceMemberRemovalError,
    InitiateSpaceMemberRemovalUseCase,
};
use crate::space::membership_history::{MembershipHistoryRepositoryPort, MembershipHistoryStore};
use crate::space::membership_runtime::MembershipRecoveryRequests;
use crate::space::membership_state::{
    SpaceMembershipStateEvents, SpaceMembershipStateRepositoryPort,
};
use crate::space::workspace_membership::tests::{
    durable_admission_repository, durable_candidate_verification_fixture, CredentialBackedSigner,
    DeterministicHistoricalVerifier, MemoryWorkspaceRepository, TestAdmissionRepository,
};

const LOCAL_DEVICE: &str = "joiner";
const TARGET_DEVICE: &str = "sponsor";

struct RemovalHarness {
    _directory: tempfile::TempDir,
    use_case: InitiateSpaceMemberRemovalUseCase,
    history_repository: Arc<dyn TestAdmissionRepository>,
    recovery_requests: MembershipRecoveryRequests,
    state_events: tokio::sync::broadcast::Receiver<uc_core::membership::WorkspaceSnapshot>,
}

async fn removal_harness() -> RemovalHarness {
    let directory = tempfile::tempdir().unwrap();
    let history_repository = durable_admission_repository(&directory, [0xb6; 16]);
    let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes([0xb7; 32]);
    let (_, mut history, candidate, _, activation_receipt) =
        durable_candidate_verification_fixture(attempt_id);
    let MembershipOperationV2::AddDevice { admission } = &candidate.operation else {
        unreachable!("fixture always creates AddDevice")
    };
    let local_credential = admission.membership_credential.clone();
    let local_member = admission.facts.member_instance;
    history
        .verify_and_receive_event(candidate.clone(), &DeterministicHistoricalVerifier)
        .unwrap();
    history
        .verify_and_record_activation_receipt(activation_receipt, &DeterministicHistoricalVerifier)
        .unwrap();
    history_repository
        .compare_and_replace_membership_history(None, &history.encode_persisted_v2().unwrap())
        .await
        .unwrap();

    let state_repository = MemoryWorkspaceRepository::default();
    let mut state = SpaceMembershipState::fresh(candidate.lineage_id.clone(), 1);
    state.own_instance = Some(local_member);
    state_repository.save_state(&state).await.unwrap();

    let own_device = DeviceId::new(LOCAL_DEVICE);
    let member_signatures = Arc::new(CredentialBackedSigner {
        device_id: own_device.clone(),
        credential: local_credential,
    });
    let history_port: Arc<dyn MembershipHistoryRepositoryPort> = Arc::clone(&history_repository);
    let state_port: Arc<dyn SpaceMembershipStateRepositoryPort> =
        Arc::new(state_repository.clone());
    let membership_history = Arc::new(MembershipHistoryStore::new(
        Arc::clone(&history_port),
        Arc::new(DeterministicHistoricalVerifier),
    ));

    let state_events = SpaceMembershipStateEvents::new();
    let state_event_receiver = state_events.subscribe();
    let recovery_requests = MembershipRecoveryRequests::new();
    let use_case = InitiateSpaceMemberRemovalUseCase::new(InitiateSpaceMemberRemovalDeps {
        membership_history,
        state_repo: state_port,
        member_signatures,
        own_device,
        state_write_lock: Arc::new(tokio::sync::Mutex::new(())),
        state_events,
        recovery_requests: recovery_requests.clone(),
    });

    RemovalHarness {
        _directory: directory,
        use_case,
        history_repository,
        recovery_requests,
        state_events: state_event_receiver,
    }
}

#[tokio::test]
async fn initiating_removal_saves_the_event_and_requests_delivery() {
    let mut harness = removal_harness().await;

    let result = harness
        .use_case
        .execute(&DeviceId::new(TARGET_DEVICE))
        .await
        .unwrap();

    let history = load_history(&harness).await;
    let removal = history.event(result.removal_event_id).unwrap();
    assert!(matches!(
        removal.operation,
        MembershipOperationV2::RemoveDevice { .. }
    ));
    assert_eq!(history.active_members().len(), 1);
    assert_eq!(result.snapshot.effective_member_count, 1);
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        harness.recovery_requests.notified(),
    )
    .await
    .expect("saved removal requests background delivery");
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        harness.state_events.recv(),
    )
    .await
    .expect("saved removal publishes membership invalidation")
    .expect("membership state event channel remains open");
}

#[tokio::test]
async fn invalid_targets_do_not_change_membership_history() {
    let harness = removal_harness().await;
    let before = harness
        .history_repository
        .load_membership_history()
        .await
        .unwrap();

    assert!(matches!(
        harness.use_case.execute(&DeviceId::new("missing")).await,
        Err(InitiateSpaceMemberRemovalError::TargetNotFound)
    ));
    assert!(matches!(
        harness.use_case.execute(&DeviceId::new(LOCAL_DEVICE)).await,
        Err(InitiateSpaceMemberRemovalError::SelfTarget)
    ));
    assert_eq!(
        harness
            .history_repository
            .load_membership_history()
            .await
            .unwrap(),
        before
    );
}

async fn load_history(harness: &RemovalHarness) -> VersionedMembershipHistory {
    let encoded = harness
        .history_repository
        .load_membership_history()
        .await
        .unwrap()
        .unwrap();
    VersionedMembershipHistory::decode_persisted_v2(&encoded, &DeterministicHistoricalVerifier)
        .unwrap()
}
