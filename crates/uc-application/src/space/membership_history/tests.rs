use std::sync::Arc;

use uc_core::membership::{
    AdmissionAttemptId, MembershipHistoryV2ReceiveOutcome, VersionedMembershipHistory,
};

use super::{
    MembershipHistoryRepositoryError, MembershipHistoryRepositoryPort, MembershipHistoryStore,
};
use crate::space::workspace_membership::tests::{
    durable_admission_repository, durable_candidate_verification_fixture,
    DeterministicHistoricalVerifier, TestAdmissionRepository,
};

struct HistoryStoreHarness {
    _directory: tempfile::TempDir,
    repository: Arc<dyn TestAdmissionRepository>,
    store: MembershipHistoryStore,
    base_history: VersionedMembershipHistory,
    candidate_event: uc_core::membership::MembershipEventV2,
}

async fn history_store_harness() -> HistoryStoreHarness {
    let directory = tempfile::tempdir().unwrap();
    let repository = durable_admission_repository(&directory, [0xc6; 16]);
    let attempt_id = AdmissionAttemptId::from_bytes([0xc7; 32]);
    let (_, base_history, candidate_event, _, _) =
        durable_candidate_verification_fixture(attempt_id);
    let base_bytes = base_history.encode_persisted_v2().unwrap();
    repository
        .compare_and_replace_membership_history(None, &base_bytes)
        .await
        .unwrap();
    let history_repository: Arc<dyn MembershipHistoryRepositoryPort> = Arc::clone(&repository);
    let store = MembershipHistoryStore::new(
        history_repository,
        Arc::new(DeterministicHistoricalVerifier),
    );

    HistoryStoreHarness {
        _directory: directory,
        repository,
        store,
        base_history,
        candidate_event,
    }
}

#[tokio::test]
async fn signed_event_is_committed_against_the_loaded_history_version() {
    let harness = history_store_harness().await;
    let mut loaded = harness
        .store
        .load_verified_history()
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        loaded
            .apply_signed_event(harness.candidate_event.clone())
            .unwrap(),
        MembershipHistoryV2ReceiveOutcome::Applied
    );
    let committed = harness.store.commit(loaded).await.unwrap();

    assert_eq!(committed.revision(), 2);
    assert_eq!(
        committed
            .history()
            .event(harness.candidate_event.event_id()),
        Some(&harness.candidate_event)
    );
    let reloaded = harness
        .store
        .load_verified_history()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        reloaded.history().event(harness.candidate_event.event_id()),
        Some(&harness.candidate_event)
    );
}

#[tokio::test]
async fn commit_rejects_a_history_that_changed_after_loading() {
    let harness = history_store_harness().await;
    let loaded = harness
        .store
        .load_verified_history()
        .await
        .unwrap()
        .unwrap();
    let base_bytes = harness.base_history.encode_persisted_v2().unwrap();
    let mut concurrent_history = harness.base_history.clone();
    concurrent_history
        .verify_and_receive_event(
            harness.candidate_event.clone(),
            &DeterministicHistoricalVerifier,
        )
        .unwrap();
    let concurrent_bytes = concurrent_history.encode_persisted_v2().unwrap();
    harness
        .repository
        .compare_and_replace_membership_history(Some(&base_bytes), &concurrent_bytes)
        .await
        .unwrap();

    let error = harness.store.commit(loaded).await.unwrap_err();

    assert_eq!(error, MembershipHistoryRepositoryError::Conflict);
    let persisted = harness
        .store
        .load_verified_history()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        persisted
            .history()
            .event(harness.candidate_event.event_id()),
        Some(&harness.candidate_event)
    );
}
