use uc_core::ids::DeviceId;
use uc_core::membership::{
    MembershipAdmissionDecision, PeerRelation, PeerSyncBackoffSnapshot, PeerSyncOutcome,
};

use super::*;
use crate::space::membership::testing::{
    with_peer_relation, with_peer_sync, EstablishedSpace, OwnerFixture,
};

fn active_record() -> crate::space::membership::MembershipRecord {
    EstablishedSpace::new(&["device-a", "device-b"]).record("device-a", 7)
}

#[tokio::test]
async fn admission_snapshot_returns_generation_and_decision_from_one_load() {
    let fixture = OwnerFixture::new(active_record());
    let query = QueryMembershipAdmissionUseCase::new(fixture.owner.clone());

    let snapshot = query.query_membership_admission(Some(7)).await.unwrap();

    assert_eq!(snapshot.current_generation, 7);
    assert_eq!(snapshot.decision, MembershipAdmissionDecision::Allowed);
    assert_eq!(fixture.records.load_count(), 1);
}

#[tokio::test]
async fn superseded_invitation_generation_is_reported() {
    let fixture = OwnerFixture::new(active_record());
    let query = QueryMembershipAdmissionUseCase::new(fixture.owner.clone());

    let snapshot = query.query_membership_admission(Some(6)).await.unwrap();

    assert_eq!(
        snapshot.decision,
        MembershipAdmissionDecision::SupersededInvitation
    );
}

#[tokio::test]
async fn offline_peer_awaiting_the_latest_history_does_not_block_a_new_invitation() {
    let record = with_peer_sync(
        active_record(),
        &DeviceId::new("device-b"),
        PeerSyncBackoffSnapshot {
            pending_since_revision: Some(7),
            retry_attempt: 3,
            next_attempt_at_ms: 60_000,
            last_outcome: PeerSyncOutcome::Deferred,
        },
    );
    let fixture = OwnerFixture::new(record);
    let query = QueryMembershipAdmissionUseCase::new(fixture.owner.clone());

    let snapshot = query.query_membership_admission(Some(7)).await.unwrap();

    assert_eq!(snapshot.decision, MembershipAdmissionDecision::Allowed);
}

#[tokio::test]
async fn unsafe_active_peer_relationships_block_a_new_invitation() {
    for relation in [
        PeerRelation::Unconfirmed,
        PeerRelation::AwaitingLocalDecision,
        PeerRelation::Diverged,
        PeerRelation::Invalid,
        PeerRelation::UpgradeRequired,
    ] {
        let record =
            with_peer_relation(active_record(), &DeviceId::new("device-b"), relation, None);
        let fixture = OwnerFixture::new(record);
        let query = QueryMembershipAdmissionUseCase::new(fixture.owner.clone());

        let snapshot = query.query_membership_admission(Some(7)).await.unwrap();

        assert_eq!(
            snapshot.decision,
            MembershipAdmissionDecision::AwaitingConvergence,
            "relation {relation:?} must keep admission closed"
        );
    }
}
