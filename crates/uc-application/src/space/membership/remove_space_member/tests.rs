use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    AdmissionMemberBindingV2, MemberEffectKind, MemberEffectMaterial, MemberEffectPhase,
    MemberInstanceId, MembershipCredential, MembershipEventId, PeerLink, SpaceAdmissionId,
    VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
};
use uc_core::ports::ReachabilityState;

use super::*;
use crate::space::membership::testing::{
    append_active_peer_to_history, member_facts, replace_history, started_record, EstablishedSpace,
    OwnerFixture, TestSigner,
};
use crate::space::membership::{
    DeviceTrustMembership, DeviceTrustObservation, DeviceTrustRelationship, DeviceTrustSyncState,
    LoadDeviceTrustObservationsPort, MembershipMaintenanceStepOutcome, QueryDeviceTrustError,
    QueryDeviceTrustUseCase, RecoverMembershipEffectsPort, SpaceMemberPauseReason,
};

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

fn remove_case(
    fixture: &OwnerFixture,
    signer: TestSigner,
    effects: Arc<dyn RecoverMembershipEffectsPort>,
) -> RemoveSpaceMemberUseCase {
    let query = Arc::new(QueryDeviceTrustUseCase::new_for_tests(
        fixture.owner.clone(),
        Arc::new(OfflineObservations),
        Arc::new(crate::space::membership::query_device_trust::NoCurrentJoinStatus),
    ));
    RemoveSpaceMemberUseCase::new(fixture.owner.clone(), Arc::new(signer), query, effects)
}

fn active_space() -> (OwnerFixture, TestSigner) {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    (
        OwnerFixture::new(space.record("device-a", 8)),
        space.signer("device-a"),
    )
}

#[tokio::test]
async fn removal_commits_all_local_facts_once_before_returning_success() {
    let (fixture, signer) = active_space();
    let effects = Arc::new(EffectCounter(AtomicUsize::new(0)));
    let remove = remove_case(&fixture, signer, effects.clone());

    let result = remove.execute(&DeviceId::new("device-b")).await.unwrap();

    assert_eq!(result.commit.revision, 9);
    assert_eq!(result.status.revision, 9);
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(fixture.wake.wake_count(), 1);
    assert_eq!(fixture.events.committed_revisions(), vec![9]);
    assert_eq!(effects.0.load(Ordering::SeqCst), 1);
    let removed = result
        .status
        .devices
        .iter()
        .find(|device| device.device_id == DeviceId::new("device-b"))
        .unwrap();
    assert_eq!(removed.membership, DeviceTrustMembership::PendingActivation);
    assert_eq!(
        removed.relationship,
        DeviceTrustRelationship::AwaitingRemovalAcknowledgement
    );
    assert_eq!(
        removed.sync_state,
        DeviceTrustSyncState::Paused(SpaceMemberPauseReason::LocalMemberInactive)
    );
    let persisted = fixture.records.ledger();
    assert!(persisted
        .history()
        .effective_member_for_device(&DeviceId::new("device-b"))
        .is_none());
    let effect = persisted
        .unfinished_effects()
        .find(|effect| effect.event_id() == result.change_id)
        .unwrap();
    assert_eq!(effect.kind(), MemberEffectKind::RemoveDevice);
    assert_eq!(effect.phase(), MemberEffectPhase::Prepared);
    assert!(matches!(
        effect.material(),
        MemberEffectMaterial::InitiatedRemoval { event, retained_device_ids }
            if event.event_id() == result.change_id && retained_device_ids.is_empty()
    ));
    assert!(matches!(
        persisted.peer(&DeviceId::new("device-b")),
        Some(PeerLink::Departing(departing)) if departing.notice().event_id() == result.change_id
    ));
    // 成员读模型随记录同一次提交维护，仍保留正在离开的设备资料以便投递通知。
    let projection = fixture.records.last_projection().unwrap();
    assert!(projection
        .members
        .iter()
        .any(|facts| facts.device_id == DeviceId::new("device-b")));
    assert!(!projection
        .trusted_device_ids
        .contains(&DeviceId::new("device-b")));
}

#[tokio::test]
async fn repeating_a_committed_removal_returns_the_same_change() {
    let (fixture, signer) = active_space();
    let remove = remove_case(&fixture, signer, Arc::new(NoopEffects));

    let first = remove.execute(&DeviceId::new("device-b")).await.unwrap();
    let repeated = remove.execute(&DeviceId::new("device-b")).await.unwrap();

    assert_eq!(repeated.change_id, first.change_id);
    assert_eq!(repeated.commit, first.commit);
    assert_eq!(fixture.records.commit_count(), 1);
}

#[tokio::test]
async fn one_persistence_conflict_is_retried_from_a_fresh_snapshot() {
    let (fixture, signer) = active_space();
    fixture.records.conflict_next_commits(1);
    let remove = remove_case(&fixture, signer, Arc::new(NoopEffects));

    let result = remove.execute(&DeviceId::new("device-b")).await.unwrap();

    assert_eq!(result.commit.revision, 9);
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(fixture.wake.wake_count(), 1);
}

#[tokio::test]
async fn removing_the_local_device_is_rejected_without_writing() {
    let (fixture, signer) = active_space();
    let remove = remove_case(&fixture, signer, Arc::new(NoopEffects));

    let error = remove
        .execute(&DeviceId::new("device-a"))
        .await
        .unwrap_err();

    assert!(matches!(error, RemoveSpaceMemberError::SelfTarget));
    assert_eq!(fixture.records.commit_count(), 0);
}

#[tokio::test]
async fn replaying_an_old_admission_revocation_does_not_remove_the_repaired_instance() {
    let (fixture, signer, old_member, old_add_event_id) = admitted_peer();
    let remove = remove_case(&fixture, signer, Arc::new(NoopEffects));
    let admission_id = SpaceAdmissionId::from_bytes([0x61; 32]).unwrap();
    let binding = AdmissionMemberBindingV2::new(
        [0x62; 32],
        SpaceId::from_str("space-a"),
        old_member,
        old_add_event_id,
    )
    .unwrap();
    let target = AdmissionRevocationTarget::new(admission_id, binding);

    let first = remove.revoke_admission(target.clone()).await.unwrap();
    let AdmissionRevocationResult::Removed {
        change_id: first_change_id,
    } = first
    else {
        panic!("the first exact revocation must remove its target");
    };

    let new_member = append_active_peer(&fixture, "device-b", 0x52, 0x71);
    let replay = remove.revoke_admission(target).await.unwrap();

    assert_eq!(
        replay,
        AdmissionRevocationResult::AlreadyAbsent {
            change_id: first_change_id,
        }
    );
    let persisted = fixture.records.ledger();
    assert!(persisted.history().active_members().contains(&new_member));
    assert_eq!(
        persisted
            .history()
            .effective_member_for_device(&DeviceId::new("device-b")),
        Some(new_member)
    );
}

#[tokio::test]
async fn abandoned_admission_lookup_resolves_to_the_exact_original_member() {
    let (fixture, signer, old_member, old_add_event_id) = admitted_peer();
    let remove = remove_case(&fixture, signer, Arc::new(NoopEffects));
    let target = AdmissionAbandonmentRevocationTarget::new(
        SpaceAdmissionId::from_bytes([0x63; 32]).unwrap(),
        [0x64; 32],
        old_member,
        old_add_event_id,
    );

    assert!(matches!(
        remove.revoke_abandoned_admission(target).await.unwrap(),
        AdmissionRevocationResult::Removed { .. }
    ));

    let new_member = append_active_peer(&fixture, "device-b", 0x65, 0x72);
    let persisted = fixture.records.ledger();
    assert!(persisted.history().active_members().contains(&new_member));
    assert!(!persisted.history().active_members().contains(&old_member));
}

#[tokio::test]
async fn exact_revocation_rejects_wrong_space_and_unknown_add_without_committing() {
    let (fixture, signer, member, add_event_id) = admitted_peer();
    let remove = remove_case(&fixture, signer, Arc::new(NoopEffects));
    let admission_id = SpaceAdmissionId::from_bytes([0x63; 32]).unwrap();
    let wrong_space = AdmissionMemberBindingV2::new(
        [0x64; 32],
        SpaceId::from_str("space-b"),
        member,
        add_event_id,
    )
    .unwrap();
    let wrong_add = AdmissionMemberBindingV2::new(
        [0x64; 32],
        SpaceId::from_str("space-a"),
        member,
        MembershipEventId::from_hex(&"65".repeat(32)).unwrap(),
    )
    .unwrap();

    let space_error = remove
        .revoke_admission(AdmissionRevocationTarget::new(admission_id, wrong_space))
        .await
        .unwrap_err();
    let add_error = remove
        .revoke_admission(AdmissionRevocationTarget::new(admission_id, wrong_add))
        .await
        .unwrap_err();

    assert!(matches!(
        space_error,
        RemoveSpaceMemberError::TargetNotFound
    ));
    assert!(matches!(add_error, RemoveSpaceMemberError::TargetNotFound));
    assert_eq!(fixture.records.commit_count(), 0);
}

#[tokio::test]
async fn exact_revocation_reports_pending_local_effects_after_the_remove_is_saved() {
    let (fixture, signer, member, add_event_id) = admitted_peer();
    let remove = remove_case(
        &fixture,
        signer,
        Arc::new(EffectCounter(AtomicUsize::new(0))),
    );
    let binding = AdmissionMemberBindingV2::new(
        [0x66; 32],
        SpaceId::from_str("space-a"),
        member,
        add_event_id,
    )
    .unwrap();

    let result = remove
        .revoke_admission(AdmissionRevocationTarget::new(
            SpaceAdmissionId::from_bytes([0x67; 32]).unwrap(),
            binding,
        ))
        .await
        .unwrap();

    assert!(matches!(
        result,
        AdmissionRevocationResult::LocalEffectsPending { .. }
    ));
    assert_eq!(fixture.records.commit_count(), 1);
}

#[tokio::test]
async fn exact_revocation_without_the_current_member_credential_never_reports_removed() {
    let (fixture, mut signer, member, add_event_id) = admitted_peer();
    signer.credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x7a; 32]);
    let remove = remove_case(&fixture, signer, Arc::new(NoopEffects));
    let binding = AdmissionMemberBindingV2::new(
        [0x7b; 32],
        SpaceId::from_str("space-a"),
        member,
        add_event_id,
    )
    .unwrap();

    let error = remove
        .revoke_admission(AdmissionRevocationTarget::new(
            SpaceAdmissionId::from_bytes([0x7c; 32]).unwrap(),
            binding,
        ))
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        RemoveSpaceMemberError::RecoveryRequired { .. }
    ));
    assert_eq!(fixture.records.commit_count(), 0);
}

fn admitted_peer() -> (
    OwnerFixture,
    TestSigner,
    MemberInstanceId,
    MembershipEventId,
) {
    let (local_facts, local_credential) = member_facts("device-a", 0x41);
    let local_member = local_facts.member_instance;
    let mut history = VersionedMembershipHistory::new_single_member_root(
        "space-a".to_owned(),
        local_facts.clone(),
        local_credential.clone(),
    )
    .unwrap();
    let (peer_member, add_event_id) =
        append_active_peer_to_history(&mut history, local_member, "device-b", 0x42, 0x51);
    let fixture = OwnerFixture::new(started_record(
        history,
        local_facts.device_id,
        local_member,
        8,
    ));
    (
        fixture,
        TestSigner {
            local_device_id: local_facts.device_id,
            local_member,
            credential: local_credential,
        },
        peer_member,
        add_event_id,
    )
}

/// 其他来源已提交的重新加入：直接写入持久记录，Owner 下次读取时看到。
fn append_active_peer(
    fixture: &OwnerFixture,
    device: &str,
    credential_byte: u8,
    marker: u8,
) -> MemberInstanceId {
    let local_member = fixture.records.ledger().local_member();
    let mut member = None;
    replace_history(&fixture.records, |history| {
        member = Some(
            append_active_peer_to_history(history, local_member, device, credential_byte, marker).0,
        );
    });
    fixture.owner.reload_for_test();
    member.unwrap()
}
