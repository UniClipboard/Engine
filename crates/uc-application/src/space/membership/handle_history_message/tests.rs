use std::sync::Arc;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionChangeFacts, MemberEffectKind, MemberEffectPhase, MembershipAdmissionV2,
    MembershipConflictEvidenceV3, MembershipCredential, MembershipEventV2, MembershipHistoryAckV3,
    MembershipHistoryMessage, MembershipHistorySummaryV3, MembershipOperationV2, PeerLink,
    PeerRelation, VersionedMembershipHistory, ED25519_SIGNATURE_ALGORITHM_V1,
    MEMBERSHIP_EVENT_FORMAT_V2,
};

use super::use_case::{remember_completed_inbound_transfer, MAX_COMPLETED_INBOUND_TRANSFERS};
use super::*;
mod handoff_reproduction;
use crate::space::membership::testing::{
    established_history, started_record, with_peer_relation, AcceptingVerifier, FixedSpaceWorkMode,
    OwnerFixture,
};
use crate::space::membership::{MembershipHistoryExchangeRecord, MembershipRecord, SpaceWorkMode};

fn handler(fixture: &OwnerFixture) -> HandleMembershipHistoryMessageUseCase {
    HandleMembershipHistoryMessageUseCase::new(fixture.owner.clone(), FixedSpaceWorkMode::active())
}

fn relation(fixture: &OwnerFixture, peer: &DeviceId) -> Option<PeerRelation> {
    match fixture.records.ledger().peer(peer) {
        Some(PeerLink::Member(member)) => Some(member.relation()),
        _ => None,
    }
}

fn confirmed_position(
    fixture: &OwnerFixture,
    peer: &DeviceId,
) -> Option<uc_core::membership::BaseMembershipHistoryPosition> {
    match fixture.records.ledger().peer(peer) {
        Some(PeerLink::Member(member)) => member.confirmed_position().cloned(),
        _ => None,
    }
}

#[test]
fn completed_inbound_transfer_retention_is_bounded_and_keeps_the_latest_ack() {
    let mut record = MembershipHistoryExchangeRecord::default();
    let source = DeviceId::new("device-b");

    for marker in 0..=MAX_COMPLETED_INBOUND_TRANSFERS {
        let mut transfer_id = [0u8; 32];
        transfer_id[..8].copy_from_slice(&(marker as u64).to_be_bytes());
        remember_completed_inbound_transfer(
            &mut record,
            source,
            transfer_id,
            MembershipHistoryAckV3::Invalid,
        );
    }

    let mut latest_transfer_id = [0u8; 32];
    latest_transfer_id[..8]
        .copy_from_slice(&(MAX_COMPLETED_INBOUND_TRANSFERS as u64).to_be_bytes());
    assert_eq!(
        record.completed_inbound_transfers.len(),
        MAX_COMPLETED_INBOUND_TRANSFERS
    );
    assert!(record
        .completed_inbound_transfers
        .contains_key(&(source, latest_transfer_id)));
}

#[tokio::test]
async fn pairing_rejects_inbound_history_as_retryable_before_ledger_access() {
    let (loaded, peer_device_id, pages) = two_page_extension();
    let fixture = OwnerFixture::new(loaded);
    let handler = HandleMembershipHistoryMessageUseCase::new(
        fixture.owner.clone(),
        Arc::new(FixedSpaceWorkMode(SpaceWorkMode::Pairing)),
    );

    let result = uc_core::membership::MembershipHistoryExchangeEndpointPort::handle_membership_history_exchange(
        &handler,
        &peer_device_id,
        pages[0].clone(),
    )
    .await;

    assert!(matches!(
        result,
        Err(uc_core::membership::MembershipHistoryExchangeError::PairingInProgress)
    ));
    assert_eq!(fixture.records.load_count(), 0);
    assert_eq!(fixture.records.commit_count(), 0);
}

#[tokio::test]
async fn sibling_summary_requests_verified_evidence_and_records_one_conflict() {
    let (local, local_credential) = member_facts("device-a", 0x41);
    let (peer, peer_credential) = member_facts("device-b", 0x42);
    let (removed, removed_credential) = member_facts("device-c", 0x43);
    let base = established_history(&[
        (local.clone(), local_credential.clone()),
        (peer.clone(), peer_credential.clone()),
        (removed.clone(), removed_credential),
    ]);
    let local_author = MembershipAdmissionV2 {
        facts: local.clone(),
        membership_credential: local_credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let peer_author = MembershipAdmissionV2 {
        facts: peer.clone(),
        membership_credential: peer_credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let mut local_history = base.clone();
    local_history
        .verify_and_receive_event(
            remove_event(&local_history, &local_author, removed.member_instance, 0x51),
            &AcceptingVerifier,
        )
        .unwrap();
    let mut remote_history = base;
    remote_history
        .verify_and_receive_event(
            add_event(
                &remote_history,
                &peer_author,
                admission("device-d", MembershipCredential::new(1, vec![0x44; 32])),
                0x61,
            ),
            &AcceptingVerifier,
        )
        .unwrap();
    let remote_position = remote_history.current_position().unwrap();
    let pages = remote_history
        .export_conflict_evidence_pages_v2(peer.clone())
        .unwrap();
    let fixture = OwnerFixture::new(started_record(
        local_history,
        local.device_id,
        local.member_instance,
        5,
    ));
    let handler = handler(&fixture);
    let source = AuthenticatedMember::new(peer.device_id);

    let request = handler
        .execute(
            &source,
            MembershipHistoryMessage::SummaryV3(MembershipHistorySummaryV3 {
                lineage_id: "space-a".to_owned(),
                current_position: remote_position.clone(),
                transfer_id: remote_position.history_digest,
                sender_admission: peer.clone(),
            }),
        )
        .await
        .unwrap();
    assert!(matches!(
        request,
        MembershipHistoryMessage::RequestConflictEvidenceV3(_)
    ));
    assert_eq!(fixture.records.commit_count(), 0);

    let response = handler
        .execute(
            &source,
            MembershipHistoryMessage::ConflictEvidenceV3(MembershipConflictEvidenceV3 {
                transfer_id: remote_position.history_digest,
                pages,
            }),
        )
        .await
        .unwrap();

    assert!(matches!(
        response,
        MembershipHistoryMessage::ConflictEvidenceV3(_)
    ));
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(fixture.records.space().branch_recovery.conflicts.len(), 1);
    assert_eq!(
        relation(&fixture, &peer.device_id),
        Some(PeerRelation::Diverged)
    );

    let repeated = handler
        .execute(
            &source,
            MembershipHistoryMessage::ConflictEvidenceV3(MembershipConflictEvidenceV3 {
                transfer_id: remote_position.history_digest,
                pages: remote_history
                    .export_conflict_evidence_pages_v2(peer.clone())
                    .unwrap(),
            }),
        )
        .await
        .unwrap();
    assert!(matches!(
        repeated,
        MembershipHistoryMessage::ConflictEvidenceV3(_)
    ));
    assert_eq!(fixture.records.commit_count(), 1);
}

fn member_facts(device: &str, credential_byte: u8) -> (AdmissionChangeFacts, MembershipCredential) {
    let device_id = DeviceId::new(device);
    let credential =
        MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![credential_byte; 32]);
    let member_instance = credential.member_instance_id(&device_id);
    (
        AdmissionChangeFacts {
            member_instance,
            device_id,
            device_name: device.to_owned(),
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
}

fn admission(device: &str, credential: MembershipCredential) -> MembershipAdmissionV2 {
    let (facts, _) = member_facts(device, 0x55);
    let mut facts = facts;
    facts.member_instance = credential.member_instance_id(&facts.device_id);
    MembershipAdmissionV2 {
        facts,
        membership_credential: credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    }
}

fn add_event(
    history: &VersionedMembershipHistory,
    author: &MembershipAdmissionV2,
    added: MembershipAdmissionV2,
    marker: u8,
) -> MembershipEventV2 {
    let parent = history.current_head();
    let operation = MembershipOperationV2::AddDevice { admission: added };
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        history.lineage_id().to_owned(),
        parent,
        parent
            .map(|parent| history.depth(parent).unwrap() + 1)
            .unwrap_or(0),
        [marker; 16],
        author.facts.member_instance,
        author.membership_credential.credential_id,
        author.membership_credential.signature_algorithm_version,
        operation.clone(),
        history
            .expected_resulting_members_digest(parent, &operation)
            .unwrap(),
        [marker.wrapping_add(1); 32],
        vec![marker],
        Some([marker.wrapping_add(2); 32]),
        vec![marker],
    );
    event.signature = vec![marker];
    event
}

fn remove_event(
    history: &VersionedMembershipHistory,
    author: &MembershipAdmissionV2,
    removed_member: uc_core::membership::MemberInstanceId,
    marker: u8,
) -> MembershipEventV2 {
    let parent = history.current_head();
    let operation = MembershipOperationV2::RemoveDevice {
        member: removed_member,
    };
    let mut event = MembershipEventV2::new(
        MEMBERSHIP_EVENT_FORMAT_V2,
        history.lineage_id().to_owned(),
        parent,
        parent.map(|id| history.depth(id).unwrap() + 1).unwrap_or(0),
        [marker; 16],
        author.facts.member_instance,
        author.membership_credential.credential_id,
        author.membership_credential.signature_algorithm_version,
        operation.clone(),
        history
            .expected_resulting_members_digest(parent, &operation)
            .unwrap(),
        [marker.wrapping_add(1); 32],
        Vec::new(),
        None,
        vec![marker],
    );
    event.signature = vec![marker];
    event
}

fn two_page_extension() -> (MembershipRecord, DeviceId, Vec<MembershipHistoryMessage>) {
    let (local, local_credential) = member_facts("device-a", 0x41);
    let (peer, peer_credential) = member_facts("device-b", 0x42);
    let (observer, observer_credential) = member_facts("device-observer", 0x45);
    let base = established_history(&[
        (local.clone(), local_credential),
        (peer.clone(), peer_credential.clone()),
        (observer.clone(), observer_credential),
    ]);
    let author = MembershipAdmissionV2 {
        facts: peer.clone(),
        membership_credential: peer_credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let mut incoming = base.clone();
    let add_c = add_event(
        &incoming,
        &author,
        admission(
            "device-large-c",
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x43; 2_100_000]),
        ),
        0x51,
    );
    incoming
        .verify_and_receive_event(add_c, &AcceptingVerifier)
        .unwrap();
    let add_d = add_event(
        &incoming,
        &author,
        admission(
            "device-large-d",
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x44; 2_100_000]),
        ),
        0x52,
    );
    incoming
        .verify_and_receive_event(add_d, &AcceptingVerifier)
        .unwrap();
    let pages = incoming
        .export_suffix_pages_v4(peer.clone(), base.current_position().unwrap())
        .unwrap();
    assert_eq!(pages.len(), 2);
    let peer_device_id = peer.device_id;
    let loaded = with_peer_relation(
        started_record(base.clone(), local.device_id, local.member_instance, 10),
        &observer.device_id,
        PeerRelation::Consistent,
        base.current_position().ok(),
    );
    (
        loaded,
        peer_device_id,
        pages
            .into_iter()
            .map(MembershipHistoryMessage::SuffixPageV4)
            .collect(),
    )
}

#[tokio::test]
async fn restricted_event_applies_only_the_authenticated_signed_event() {
    let (local, local_credential) = member_facts("device-a", 0x41);
    let (peer, peer_credential) = member_facts("device-b", 0x42);
    let base = established_history(&[
        (local.clone(), local_credential),
        (peer.clone(), peer_credential.clone()),
    ]);
    let author = MembershipAdmissionV2 {
        facts: peer.clone(),
        membership_credential: peer_credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let event = add_event(
        &base,
        &author,
        admission(
            "device-c",
            MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x43; 32]),
        ),
        0x51,
    );
    let event_id = event.event_id();
    let fixture = OwnerFixture::new(started_record(
        base,
        local.device_id,
        local.member_instance,
        3,
    ));

    let response = handler(&fixture)
        .execute(
            &AuthenticatedMember::new(peer.device_id),
            MembershipHistoryMessage::RestrictedEventV3(event),
        )
        .await
        .unwrap();

    assert_eq!(
        response,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::RestrictedApplied)
    );
    assert!(fixture
        .records
        .ledger()
        .unfinished_effects()
        .any(|effect| effect.event_id() == event_id));
    assert_eq!(
        confirmed_position(&fixture, &peer.device_id),
        None,
        "受限事件 ACK 不能伪造完整历史确认水位"
    );
}

#[tokio::test]
async fn restricted_remote_removal_is_persisted_without_advancing_the_local_branch() {
    let (local, local_credential) = member_facts("device-a", 0x41);
    let (peer, peer_credential) = member_facts("device-b", 0x42);
    let (removed, removed_credential) = member_facts("device-c", 0x43);
    let base = established_history(&[
        (local.clone(), local_credential),
        (peer.clone(), peer_credential.clone()),
        (removed.clone(), removed_credential),
    ]);
    let peer_author = MembershipAdmissionV2 {
        facts: peer.clone(),
        membership_credential: peer_credential,
        resume_public_key_digest: [7; 32],
        security_commitment_id: [8; 32],
    };
    let removal = remove_event(&base, &peer_author, removed.member_instance, 0x51);
    let removal_id = removal.event_id();
    let common_position = base.current_position().unwrap();
    let fixture = OwnerFixture::new(started_record(
        base,
        local.device_id,
        local.member_instance,
        3,
    ));

    let response = handler(&fixture)
        .execute(
            &AuthenticatedMember::new(peer.device_id),
            MembershipHistoryMessage::RestrictedEventV3(removal),
        )
        .await
        .unwrap();

    assert_eq!(
        response,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::RestrictedApplied)
    );
    let persisted = fixture.records.ledger();
    let history = persisted.history();
    let persisted_position = history.current_position().unwrap();
    assert_eq!(persisted_position.event_id, common_position.event_id);
    assert_eq!(persisted_position.depth, common_position.depth);
    assert_ne!(
        persisted_position.history_digest,
        common_position.history_digest
    );
    assert_eq!(history.effective_members().len(), 3);
    assert_eq!(
        history.pending_removal_decision(local.member_instance),
        Some(removal_id)
    );
    assert!(persisted.unfinished_effects().next().is_none());
}

#[tokio::test]
async fn two_page_transfer_persists_each_page_and_applies_only_when_complete() {
    let (loaded, peer_device_id, pages) = two_page_extension();
    let fixture = OwnerFixture::new(loaded);
    let handler = handler(&fixture);
    let source = AuthenticatedMember::new(peer_device_id);

    let first = handler.execute(&source, pages[0].clone()).await.unwrap();
    let repeated = handler.execute(&source, pages[0].clone()).await.unwrap();
    let after_first = fixture.records.space();

    assert!(matches!(
        first,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Continue {
            next_page_index: 1,
            ..
        })
    ));
    assert_eq!(repeated, first);
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(
        after_first
            .history_exchange
            .inbound_transfers
            .get(&peer_device_id)
            .unwrap()
            .pages
            .len(),
        1
    );
    let base_history = after_first.ledger.history.clone();

    let final_ack = handler.execute(&source, pages[1].clone()).await.unwrap();

    assert!(matches!(
        final_ack,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Confirmed { .. })
    ));
    assert_eq!(fixture.records.commit_count(), 2);
    let persisted = fixture.records.space();
    assert!(persisted.history_exchange.inbound_transfers.is_empty());
    assert_ne!(persisted.ledger.history, base_history);
    let current_position = persisted.ledger.history.current_position().unwrap();
    assert_eq!(
        confirmed_position(&fixture, &peer_device_id),
        Some(current_position.clone())
    );
    assert_ne!(
        confirmed_position(&fixture, &DeviceId::new("device-observer")),
        Some(current_position)
    );
    let prepared_add_devices = fixture
        .records
        .ledger()
        .unfinished_effects()
        .filter(|effect| {
            effect.kind() == MemberEffectKind::AddDevice
                && effect.phase() == MemberEffectPhase::Prepared
        })
        .flat_map(|effect| effect.affected_device_ids().to_vec())
        .collect::<Vec<_>>();
    assert!(prepared_add_devices.contains(&DeviceId::new("device-large-c")));
    assert!(prepared_add_devices.contains(&DeviceId::new("device-large-d")));
}

#[tokio::test]
async fn final_page_commit_failure_preserves_staged_state_and_does_not_wake_maintenance() {
    let (loaded, peer_device_id, pages) = two_page_extension();
    let fixture = OwnerFixture::new(loaded);
    let handler = handler(&fixture);
    let source = AuthenticatedMember::new(peer_device_id);

    handler.execute(&source, pages[0].clone()).await.unwrap();
    let staged = fixture.records.record();
    fixture.records.fail_next_commits(1);
    let error = handler
        .execute(&source, pages[1].clone())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        HandleMembershipHistoryMessageError::Unavailable
    ));
    assert_eq!(fixture.records.commit_count(), 1);
    assert_eq!(fixture.wake.wake_count(), 0);
    assert_eq!(fixture.records.record(), staged);
}

#[tokio::test]
async fn out_of_order_page_requests_the_missing_page_without_persisting() {
    let (loaded, peer_device_id, pages) = two_page_extension();
    let fixture = OwnerFixture::new(loaded);
    let handler = handler(&fixture);
    let source = AuthenticatedMember::new(peer_device_id);

    let response = handler.execute(&source, pages[1].clone()).await.unwrap();

    assert!(matches!(
        response,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Continue {
            next_page_index: 0,
            ..
        })
    ));
    assert_eq!(fixture.records.commit_count(), 0);
    assert!(fixture
        .records
        .space()
        .history_exchange
        .inbound_transfers
        .is_empty());
}

#[tokio::test]
async fn unknown_sender_requires_complete_evidence_without_committing_unrelated_history() {
    let (_, peer_device_id, pages) = two_page_extension();
    let (local, local_credential) = member_facts("device-a", 0x41);
    let fixture = OwnerFixture::new(started_record(
        VersionedMembershipHistory::new_single_member_root(
            "space-a".to_owned(),
            local.clone(),
            local_credential,
        )
        .unwrap(),
        local.device_id,
        local.member_instance,
        10,
    ));
    let handler = handler(&fixture);

    let source = AuthenticatedMember::new(peer_device_id);
    let first = handler.execute(&source, pages[0].clone()).await.unwrap();

    assert!(matches!(
        first,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Continue { .. })
    ));
    let final_ack = handler.execute(&source, pages[1].clone()).await.unwrap();
    assert_eq!(
        final_ack,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::NeedsEvidence)
    );
    let persisted = fixture.records.space();
    assert!(persisted.history_exchange.inbound_transfers.is_empty());
    assert_eq!(persisted.ledger.history.effective_members().len(), 1);
    assert_eq!(relation(&fixture, &peer_device_id), None);
}

#[tokio::test]
async fn changed_transfer_replaces_staged_pages_without_quarantining_the_member() {
    let (loaded, peer, old_pages) = two_page_extension();
    let MembershipRecord::Space(space) = &loaded else {
        unreachable!("extension record has a space");
    };
    let mut base = space.ledger.history.clone();
    let local_member = space.ledger.local_member;
    let base_position = base.current_position().unwrap();
    let old_frames = old_pages
        .iter()
        .map(|message| match message {
            MembershipHistoryMessage::SuffixPageV4(page) => page.clone(),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    let mut sender = base
        .apply_suffix_pages_v4(&old_frames, local_member, &AcceptingVerifier)
        .unwrap();
    let author = sender.effective_member_for_device(&peer).unwrap();
    let removed = sender
        .effective_member_for_device(&DeviceId::new("device-observer"))
        .unwrap();
    let mut event = sender
        .create_unsigned_local_removal_event(
            author,
            sender.credential_for(author).unwrap(),
            removed,
            [91; 16],
            [92; 32],
        )
        .unwrap();
    event.signature = vec![93];
    sender
        .verify_and_receive_event(event, &AcceptingVerifier)
        .unwrap();
    let next = sender
        .export_suffix_pages_v4(
            sender.admission_facts_for(author).unwrap().clone(),
            base_position,
        )
        .unwrap();
    let fixture = OwnerFixture::new(loaded);
    let handler = handler(&fixture);
    let source = AuthenticatedMember::new(peer);
    handler
        .execute(&source, old_pages[0].clone())
        .await
        .unwrap();
    let response = handler
        .execute(
            &source,
            MembershipHistoryMessage::SuffixPageV4(next[0].clone()),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            response,
            MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Continue {
                next_page_index: 1,
                ..
            })
        ),
        "新资料的首帧应取代旧暂存，而不是永久隔离成员"
    );
    let late = handler
        .execute(&source, old_pages[1].clone())
        .await
        .unwrap();
    assert!(matches!(
        late,
        MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Continue {
            next_page_index: 0,
            ..
        })
    ));
    let state = fixture.records.space();
    assert_eq!(
        state.history_exchange.inbound_transfers[&peer].transfer_id,
        next[0].transfer_id()
    );
    assert_ne!(relation(&fixture, &peer), Some(PeerRelation::Invalid));
}
