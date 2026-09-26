use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use uc_core::ids::DeviceId;
use uc_core::membership::{
    LedgerInput, MembershipLedger, MembershipOperationV2, PeerRelation, PeerSyncBackoffSnapshot,
    PeerSyncOutcome, VersionedMembershipHistory,
};
use uc_core::ports::ReachabilityState;

use super::*;
use crate::space::admission::CurrentJoinStatus;
use crate::space::membership::testing::{
    append_active_peer_to_history, established_history, member_facts, record_of, started_record,
    with_peer_relation, with_peer_sync, AcceptingVerifier, EstablishedSpace, OwnerFixture,
};
use crate::space::membership::{MembershipOwner, MembershipRecord};

fn active_record() -> MembershipRecord {
    EstablishedSpace::new(&["device-a", "device-b"]).record("device-a", 8)
}

fn owner(record: MembershipRecord) -> Arc<MembershipOwner> {
    OwnerFixture::new(record).owner
}

fn peer_b() -> DeviceId {
    DeviceId::new("device-b")
}

/// `device-b` 提议移除本机，等待本机决定。
fn pending_local_removal_record() -> MembershipRecord {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let mut history = space.history.clone();
    let local_member = space.member("device-a");
    let mut removal = history
        .create_unsigned_local_removal_event(
            space.member("device-b"),
            space.credential("device-b"),
            local_member,
            [0x31; 16],
            [0x32; 32],
        )
        .unwrap();
    removal.signature = vec![0x33];
    let mut incoming = history.clone();
    incoming
        .verify_and_receive_event(removal, &AcceptingVerifier)
        .unwrap();
    history
        .merge_remote_history(&incoming, local_member, &AcceptingVerifier)
        .unwrap();
    with_peer_relation(
        started_record(history, DeviceId::new("device-a"), local_member, 8),
        &peer_b(),
        PeerRelation::AwaitingLocalDecision,
        None,
    )
}

/// 以已确认当前位置的对端结束同步。
fn converged_record() -> MembershipRecord {
    let record = active_record();
    let MembershipRecord::Space(space) = &record else {
        unreachable!("active record has a space");
    };
    let position = space.ledger.history.current_position().unwrap();
    let record = with_peer_relation(record, &peer_b(), PeerRelation::Consistent, Some(position));
    with_peer_sync(
        record,
        &peer_b(),
        PeerSyncBackoffSnapshot {
            pending_since_revision: None,
            retry_attempt: 0,
            next_attempt_at_ms: 0,
            last_outcome: PeerSyncOutcome::Acked,
        },
    )
}

/// 本机移除 `device-b` 且本地效果已全部完成，只剩一次移除通知。
fn departing_peer_record() -> MembershipRecord {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let MembershipRecord::Space(record) = space.record("device-a", 8) else {
        unreachable!("started record has a space");
    };
    let ledger = MembershipLedger::restore(record.ledger).unwrap();
    let mut history = ledger.history().clone();
    let mut removal = history
        .create_unsigned_local_removal_event(
            space.member("device-a"),
            space.credential("device-a"),
            space.member("device-b"),
            [0x41; 16],
            [0x42; 32],
        )
        .unwrap();
    removal.signature = vec![0x43];
    history
        .verify_and_receive_event(removal, &AcceptingVerifier)
        .unwrap();
    let (mut ledger, _, _) = ledger
        .apply(
            LedgerInput::LocalRemovalSigned {
                history,
                retained_device_ids: Vec::new(),
            },
            1_000,
        )
        .unwrap()
        .into_parts();
    loop {
        let Some(effect) = ledger.unfinished_effects().next().cloned() else {
            break;
        };
        ledger = ledger
            .apply(
                LedgerInput::EffectStepFinished {
                    event_id: effect.event_id(),
                    from: effect.phase(),
                },
                1_000,
            )
            .unwrap()
            .into_parts()
            .0;
    }
    record_of(ledger.snapshot())
}

struct UnexpectedObservations;

#[async_trait]
impl LoadDeviceTrustObservationsPort for UnexpectedObservations {
    async fn load(
        &self,
        _device_ids: &[uc_core::ids::DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        panic!("empty status must not read device observations")
    }
}

struct StaticObservations {
    calls: Arc<Mutex<Vec<Vec<DeviceId>>>>,
}

struct AllOfflineObservations;

struct StaticSecurityUpdates(SpaceDeviceUpdateStatus);

#[async_trait]
impl LoadSecurityDeviceUpdateStatusPort for StaticSecurityUpdates {
    async fn load_security_device_update_status(
        &self,
    ) -> Result<SpaceDeviceUpdateStatus, QueryDeviceTrustError> {
        Ok(self.0)
    }
}

#[async_trait]
impl LoadDeviceTrustObservationsPort for AllOfflineObservations {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        Ok(device_ids
            .iter()
            .map(|device_id| DeviceTrustObservation {
                device_id: *device_id,
                display_name: None,
                reachability: ReachabilityState::Offline,
            })
            .collect())
    }
}

struct StaticCurrentJoin(Option<CurrentJoinStatus>);

struct StaticPairingConfirmation(PairingConfirmationObservation);

struct StaticPendingInboundMember(crate::space::admission::PendingInboundMember);

struct StaticInboundPairings(Vec<crate::space::admission::InboundPairing>);

struct LocalOnlyObservations;

#[async_trait]
impl LoadDeviceTrustObservationsPort for LocalOnlyObservations {
    async fn load(
        &self,
        _device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        Ok(vec![DeviceTrustObservation {
            device_id: DeviceId::new("device-a"),
            display_name: Some("Local A".to_owned()),
            reachability: ReachabilityState::Online,
        }])
    }
}

#[async_trait]
impl LoadCurrentJoinStatusPort for StaticCurrentJoin {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        Ok(self.0.clone())
    }
}

#[async_trait]
impl LoadCurrentJoinStatusPort for StaticPairingConfirmation {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        Ok(None)
    }

    async fn load_admission_display(
        &self,
        targets: &[PairingConfirmationTarget],
    ) -> Result<AdmissionDisplayStatus, QueryDeviceTrustError> {
        Ok(AdmissionDisplayStatus {
            current_join: None,
            inbound_pairings: Vec::new(),
            pending_inbound_member: None,
            pairing_confirmations: targets
                .contains(&self.0.target)
                .then_some(self.0)
                .into_iter()
                .collect(),
        })
    }
}

#[async_trait]
impl LoadCurrentJoinStatusPort for StaticPendingInboundMember {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        Ok(None)
    }

    async fn load_admission_display(
        &self,
        _targets: &[PairingConfirmationTarget],
    ) -> Result<AdmissionDisplayStatus, QueryDeviceTrustError> {
        Ok(AdmissionDisplayStatus {
            current_join: None,
            inbound_pairings: Vec::new(),
            pending_inbound_member: Some(self.0.clone()),
            pairing_confirmations: Vec::new(),
        })
    }
}

#[async_trait]
impl LoadCurrentJoinStatusPort for StaticInboundPairings {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        Ok(None)
    }

    async fn load_admission_display(
        &self,
        _targets: &[PairingConfirmationTarget],
    ) -> Result<AdmissionDisplayStatus, QueryDeviceTrustError> {
        Ok(AdmissionDisplayStatus {
            current_join: None,
            inbound_pairings: self.0.clone(),
            pending_inbound_member: None,
            pairing_confirmations: Vec::new(),
        })
    }
}

#[async_trait]
impl LoadDeviceTrustObservationsPort for StaticObservations {
    async fn load(
        &self,
        device_ids: &[DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        self.calls.lock().unwrap().push(device_ids.to_vec());
        Ok(vec![
            DeviceTrustObservation {
                device_id: DeviceId::new("device-b"),
                display_name: Some("Peer B".to_owned()),
                reachability: ReachabilityState::Online,
            },
            DeviceTrustObservation {
                device_id: DeviceId::new("device-a"),
                display_name: Some("Local A".to_owned()),
                reachability: ReachabilityState::Offline,
            },
        ])
    }
}

#[tokio::test]
async fn profile_without_a_space_returns_an_explicit_empty_status() {
    let ledger = owner(MembershipRecord::NoSpace { revision: 0 });
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(UnexpectedObservations),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(status.revision, 0);
    assert_eq!(
        status.local_membership,
        DeviceTrustMembership::NoCurrentSpace
    );
    assert!(status.local_device_id.is_none());
    assert!(status.devices.is_empty());
    assert!(status.current_change.is_none());
}

#[tokio::test]
async fn departing_device_is_reported_offline_and_not_syncable() {
    let peer_device_id = peer_b();
    let ledger = owner(departing_peer_record());
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(LocalOnlyObservations),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();

    let removed = status
        .devices
        .iter()
        .find(|device| device.device_id == peer_device_id)
        .unwrap();
    assert_eq!(removed.membership, DeviceTrustMembership::Removed);
    assert_eq!(
        removed.relationship,
        DeviceTrustRelationship::AwaitingRemovalAcknowledgement
    );
    assert_eq!(removed.reachability, ReachabilityState::Offline);
    assert_eq!(
        removed.sync_state,
        DeviceTrustSyncState::Paused(
            crate::space::membership::SpaceMemberPauseReason::LocalMemberInactive
        )
    );
}

#[tokio::test]
async fn active_status_combines_verified_members_with_one_observation_read() {
    let ledger = owner(active_record());
    let calls = Arc::new(Mutex::new(Vec::new()));
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::clone(&calls),
        }),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(status.revision, 8);
    assert_eq!(status.local_device_id, Some(DeviceId::new("device-a")));
    assert_eq!(status.local_membership, DeviceTrustMembership::Active);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[vec![DeviceId::new("device-a"), DeviceId::new("device-b")]]
    );
    assert_eq!(status.devices.len(), 2);
    assert_eq!(status.devices[0].device_id, DeviceId::new("device-a"));
    assert_eq!(status.devices[0].display_name, "Local A");
    assert_eq!(
        status.devices[0].relationship,
        DeviceTrustRelationship::Local
    );
    assert_eq!(status.devices[1].device_id, DeviceId::new("device-b"));
    assert_eq!(status.devices[1].display_name, "Peer B");
    assert_eq!(
        status.devices[1].relationship,
        DeviceTrustRelationship::ConfirmationPending
    );
    assert_eq!(status.devices[1].sync_state, DeviceTrustSyncState::Usable);
    assert_eq!(
        status.space_device_update,
        SpaceDeviceUpdateStatus::updating()
    );
}

#[tokio::test]
async fn space_device_update_is_complete_only_after_every_required_fact_converges() {
    let loaded = converged_record();
    let ledger = owner(loaded);
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(AllOfflineObservations),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(
        status.space_device_update,
        SpaceDeviceUpdateStatus::completed()
    );
}

#[tokio::test]
async fn pending_security_device_update_prevents_overall_completion() {
    let loaded = converged_record();
    let ledger = owner(loaded);
    let query = QueryDeviceTrustUseCase::new(
        ledger,
        Arc::new(AllOfflineObservations),
        Arc::new(StaticCurrentJoin(None)),
        Arc::new(StaticSecurityUpdates(
            SpaceDeviceUpdateStatus::retryable_failure(60_000),
        )),
        Arc::new(super::use_case::MissingLocalIdentity),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(
        status.space_device_update,
        SpaceDeviceUpdateStatus::retryable_failure(60_000)
    );
}

#[tokio::test]
async fn deferred_history_sync_exposes_its_persisted_retry_time() {
    let loaded = with_peer_sync(
        active_record(),
        &peer_b(),
        PeerSyncBackoffSnapshot {
            pending_since_revision: Some(8),
            retry_attempt: 3,
            next_attempt_at_ms: 60_000,
            last_outcome: PeerSyncOutcome::Deferred,
        },
    );
    let ledger = owner(loaded);
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(AllOfflineObservations),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(
        status.space_device_update,
        SpaceDeviceUpdateStatus::retryable_failure(60_000)
    );
}

#[tokio::test]
async fn stable_history_rejection_exposes_a_reason_and_recovery_action() {
    let loaded = with_peer_sync(
        active_record(),
        &peer_b(),
        PeerSyncBackoffSnapshot {
            pending_since_revision: None,
            retry_attempt: 0,
            next_attempt_at_ms: 0,
            last_outcome: PeerSyncOutcome::StableRejected,
        },
    );
    let ledger = owner(loaded);
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(AllOfflineObservations),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(
        status.space_device_update,
        SpaceDeviceUpdateStatus::needs_attention(
            SpaceDeviceUpdateProblem::DeviceStateRejected,
            SpaceDeviceUpdateRecovery::ReviewDevices,
        )
    );
}

#[tokio::test]
async fn relationship_conflict_requires_attention_even_without_history_failure() {
    let loaded = with_peer_sync(
        pending_local_removal_record(),
        &peer_b(),
        PeerSyncBackoffSnapshot {
            pending_since_revision: None,
            retry_attempt: 0,
            next_attempt_at_ms: 0,
            last_outcome: PeerSyncOutcome::Acked,
        },
    );
    let ledger = owner(loaded);
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(AllOfflineObservations),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(
        status.space_device_update,
        SpaceDeviceUpdateStatus::needs_attention(
            SpaceDeviceUpdateProblem::DeviceRelationshipConflict,
            SpaceDeviceUpdateRecovery::ReviewDevices,
        )
    );
}

#[tokio::test]
async fn pairing_confirmation_is_matched_to_the_exact_active_member() {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let mut history = space.history.clone();
    let (member_instance_id, add_event_id) = append_active_peer_to_history(
        &mut history,
        space.member("device-a"),
        "device-c",
        0x43,
        0x45,
    );
    let peer_device_id = DeviceId::new("device-c");
    let loaded = started_record(
        history,
        DeviceId::new("device-a"),
        space.member("device-a"),
        8,
    );
    let ledger = owner(loaded);
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(AllOfflineObservations),
        Arc::new(StaticPairingConfirmation(PairingConfirmationObservation {
            target: PairingConfirmationTarget {
                member_instance_id,
                add_event_id,
            },
            status: PairingConfirmationStatus::Unconfirmed,
        })),
    );

    let status = query.execute().await.unwrap();
    let peer = status
        .devices
        .iter()
        .find(|device| device.device_id == peer_device_id)
        .unwrap();
    assert_eq!(
        peer.pairing_confirmation,
        Some(PairingConfirmationStatus::Unconfirmed)
    );
    assert_eq!(peer.sync_state, DeviceTrustSyncState::Usable);
}

#[tokio::test]
async fn peer_that_confirmed_the_current_position_is_reported_consistent() {
    let loaded = converged_record();
    let ledger = owner(loaded);
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(
        status.devices[1].relationship,
        DeviceTrustRelationship::Consistent
    );
}

#[tokio::test]
async fn status_exposes_the_current_pending_removal_facts() {
    let ledger = owner(pending_local_removal_record());
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
        Arc::new(StaticCurrentJoin(None)),
    );

    let status = query.execute().await.unwrap();
    let change = status.current_change.unwrap();

    assert_eq!(change.proposed_by_device_id, DeviceId::new("device-b"));
    assert_eq!(change.target_device_ids, vec![DeviceId::new("device-a")]);
    assert!(change.includes_local_device);
    assert!(change.apply_impact.usable_device_ids.is_empty());
    assert_eq!(
        change.apply_impact.member_device_ids,
        vec![DeviceId::new("device-b")]
    );
    assert_eq!(
        change.apply_impact.local_membership,
        DeviceTrustMembership::Removed
    );
    assert_eq!(
        change.keep_current_impact.usable_device_ids,
        vec![DeviceId::new("device-a")]
    );
    assert_eq!(
        change.keep_current_impact.paused_device_ids,
        vec![DeviceId::new("device-b")]
    );
    assert_eq!(
        change.keep_current_impact.local_membership,
        DeviceTrustMembership::Active
    );
    let MembershipRecord::Space(record) = pending_local_removal_record() else {
        unreachable!("pending removal record has a space");
    };
    let event = record.ledger.history.event(change.change_id).unwrap();
    assert!(matches!(
        event.operation,
        MembershipOperationV2::RemoveDevice { .. }
    ));
}

#[tokio::test]
async fn pending_peer_removal_previews_match_each_selected_history() {
    let members = vec![
        member_facts("device-a", 0x41),
        member_facts("device-b", 0x42),
        member_facts("device-c", 0x43),
        member_facts("device-d", 0x44),
    ];
    let mut history: VersionedMembershipHistory = established_history(&members);
    let local = members[0].0.member_instance;
    let mut removal = history
        .create_unsigned_local_removal_event(
            members[1].0.member_instance,
            &members[1].1,
            members[2].0.member_instance,
            [0x31; 16],
            [0x32; 32],
        )
        .unwrap();
    removal.signature = vec![0x33];
    let removal_id = removal.event_id();
    history
        .verify_and_receive_remote_event_for_local_member(removal, local, &AcceptingVerifier)
        .unwrap();
    let loaded = with_peer_relation(
        started_record(history.clone(), DeviceId::new("device-a"), local, 8),
        &peer_b(),
        PeerRelation::AwaitingLocalDecision,
        None,
    );
    let query = QueryDeviceTrustUseCase::new_for_tests(
        owner(loaded),
        Arc::new(AllOfflineObservations),
        Arc::new(StaticCurrentJoin(None)),
    );
    let change = query.execute().await.unwrap().current_change.unwrap();
    assert_eq!(
        change.apply_impact.usable_device_ids,
        ["device-a", "device-b", "device-d"].map(DeviceId::new)
    );
    assert_eq!(
        change.apply_impact.paused_device_ids,
        vec![DeviceId::new("device-c")]
    );
    assert_eq!(
        change.keep_current_impact.usable_device_ids,
        ["device-a", "device-c", "device-d"].map(DeviceId::new)
    );
    assert_eq!(
        change.keep_current_impact.paused_device_ids,
        vec![DeviceId::new("device-b")]
    );
    for (decision, preview) in [
        (
            uc_core::membership::RemovalDecision::Accept,
            change.apply_impact,
        ),
        (
            uc_core::membership::RemovalDecision::Reject,
            change.keep_current_impact,
        ),
    ] {
        let mut selected = history.clone();
        let mut signed = selected
            .create_unsigned_local_removal_decision(
                removal_id,
                local,
                &members[0].1,
                decision,
                [0x71; 16],
            )
            .unwrap();
        signed.signature = vec![0x72];
        selected
            .apply_signed_local_removal_decision(signed, local, &AcceptingVerifier)
            .unwrap();
        let mut actual = selected
            .effective_members()
            .into_iter()
            .map(|member| selected.admission_facts_for(member).unwrap().device_id)
            .collect::<Vec<_>>();
        actual.sort();
        assert_eq!(preview.member_device_ids, actual);
        assert!(preview
            .usable_device_ids
            .iter()
            .all(|id| !preview.paused_device_ids.contains(id)));
    }
}

#[tokio::test]
async fn status_uses_the_current_join_projection_from_admission_state() {
    let ledger = owner(active_record());
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
        Arc::new(StaticCurrentJoin(Some(CurrentJoinStatus::Pending {
            join_id: [0xa2; 16],
            target_space_id: Some("target-space".to_owned()),
            sponsor_device_id: None,
            sponsor_identity_fingerprint: None,
            cancel_requested: false,
            peer_upgrade_required: false,
        }))),
    );

    let status = query.execute().await.unwrap();

    assert!(matches!(
        status.current_join,
        Some(crate::space::admission::CurrentJoinStatus::Pending {
            join_id,
            target_space_id: Some(ref target),
            cancel_requested: false,
            ..
        }) if join_id == [0xa2; 16] && target == "target-space"
    ));
}

#[tokio::test]
async fn status_keeps_a_pending_inbound_member_out_of_the_formal_device_list() {
    let ledger = owner(active_record());
    let pending = crate::space::admission::PendingInboundMember {
        device_id: DeviceId::new("device-pending"),
        display_name: "Pending device".to_owned(),
    };
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
        Arc::new(StaticPendingInboundMember(pending.clone())),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(status.pending_inbound_member, Some(pending));
    assert!(status
        .devices
        .iter()
        .all(|device| device.device_id != DeviceId::new("device-pending")));
}

#[tokio::test]
async fn status_keeps_multiple_inbound_pairings_out_of_the_formal_device_list() {
    let ledger = owner(active_record());
    let pairings = vec![
        crate::space::admission::InboundPairing {
            pairing_id: [0x31; 32],
            device_id: Some(DeviceId::new("device-pending-a")),
            display_name: Some("Pending A".to_owned()),
            status: crate::space::admission::InboundPairingStatus::AwaitingConfirmation,
        },
        crate::space::admission::InboundPairing {
            pairing_id: [0x32; 32],
            device_id: Some(DeviceId::new("device-pending-b")),
            display_name: Some("Pending B".to_owned()),
            status: crate::space::admission::InboundPairingStatus::ConfirmationMissed,
        },
    ];
    let query = QueryDeviceTrustUseCase::new_for_tests(
        ledger,
        Arc::new(StaticObservations {
            calls: Arc::new(Mutex::new(Vec::new())),
        }),
        Arc::new(StaticInboundPairings(pairings.clone())),
    );

    let status = query.execute().await.unwrap();

    assert_eq!(status.inbound_pairings, pairings);
    assert!(status.devices.iter().all(|device| {
        device.device_id != DeviceId::new("device-pending-a")
            && device.device_id != DeviceId::new("device-pending-b")
    }));
}
