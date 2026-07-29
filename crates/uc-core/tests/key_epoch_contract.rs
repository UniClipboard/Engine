use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    ContentKeyId, ContentKeyPurpose, GroupEpoch, KeyEpochError, RevocationId,
    RevocationOutboxMessage, RevocationRecord, RevocationStage, RevocationStatus, SpaceKeyMaterial,
    SpaceKeyState, SpaceSecurityMode,
};

#[test]
fn group_epoch_only_advances() {
    let epoch = GroupEpoch::new(7);
    assert_eq!(epoch.value(), 7);
    assert_eq!(epoch.next().unwrap().value(), 8);
    assert_eq!(
        GroupEpoch::new(u64::MAX).next(),
        Err(KeyEpochError::EpochOverflow)
    );
}

#[test]
fn revocation_follows_the_forward_only_state_machine() {
    let mut record = RevocationRecord::prepare(
        RevocationId::from_string("revocation-1").unwrap(),
        SpaceId::from_str("space-1"),
        DeviceId::new("removed-device"),
        GroupEpoch::new(4),
        100,
    )
    .unwrap();

    assert_eq!(record.status(), RevocationStatus::Prepared);
    assert_eq!(record.previous_epoch(), GroupEpoch::new(4));
    assert_eq!(record.next_epoch(), GroupEpoch::new(5));
    record.transition_to(RevocationStatus::Staged, 110).unwrap();
    record
        .transition_to(RevocationStatus::Activated, 120)
        .unwrap();
    record
        .transition_to(RevocationStatus::Distributing, 130)
        .unwrap();
    record
        .transition_to(RevocationStatus::Complete, 140)
        .unwrap();

    assert_eq!(record.status(), RevocationStatus::Complete);
    assert_eq!(record.updated_at_ms(), 140);
}

#[test]
fn activated_revocation_cannot_move_backwards() {
    let mut record = RevocationRecord::prepare(
        RevocationId::from_string("revocation-2").unwrap(),
        SpaceId::from_str("space-1"),
        DeviceId::new("removed-device"),
        GroupEpoch::new(1),
        100,
    )
    .unwrap();
    record.transition_to(RevocationStatus::Staged, 110).unwrap();
    record
        .transition_to(RevocationStatus::Activated, 120)
        .unwrap();

    assert_eq!(
        record.transition_to(RevocationStatus::Staged, 130),
        Err(KeyEpochError::InvalidRevocationTransition {
            from: RevocationStatus::Activated,
            to: RevocationStatus::Staged,
        })
    );
    assert_eq!(record.status(), RevocationStatus::Activated);
}

#[test]
fn legacy_space_migrates_once_then_rotates_forward() {
    let mut state = SpaceKeyState::legacy(SpaceId::from_str("space-1"));
    assert_eq!(state.mode(), SpaceSecurityMode::Legacy);
    assert_eq!(state.epoch(), GroupEpoch::new(0));
    assert_eq!(state.current_content_key_id(), &ContentKeyId::legacy_v1());

    state.mark_migrating().unwrap();
    let first_current = ContentKeyId::from_string("current-1").unwrap();
    state.mark_ready(first_current.clone()).unwrap();
    assert_eq!(state.mode(), SpaceSecurityMode::Ready);
    assert_eq!(state.epoch(), GroupEpoch::new(1));
    assert_eq!(state.current_content_key_id(), &first_current);

    let next = ContentKeyId::from_string("current-2").unwrap();
    state.rotate(next.clone()).unwrap();
    assert_eq!(state.epoch(), GroupEpoch::new(2));
    assert_eq!(state.current_content_key_id(), &next);
    assert_eq!(ContentKeyPurpose::Content.as_str(), "content");
    assert_eq!(ContentKeyPurpose::Transport.as_str(), "transport");
    assert_eq!(ContentKeyPurpose::Search.as_str(), "search");
}

#[test]
fn ready_space_cannot_return_to_migration_or_reuse_a_key_id() {
    let mut state = SpaceKeyState::legacy(SpaceId::from_str("space-1"));
    state.mark_migrating().unwrap();
    let current = ContentKeyId::from_string("current-1").unwrap();
    state.mark_ready(current.clone()).unwrap();

    assert_eq!(
        state.mark_migrating(),
        Err(KeyEpochError::InvalidSpaceSecurityTransition {
            from: SpaceSecurityMode::Ready,
            to: SpaceSecurityMode::Migrating,
        })
    );
    assert_eq!(state.rotate(current), Err(KeyEpochError::ContentKeyReuse));
}

#[test]
fn staged_revocation_redacts_payloads_and_excludes_removed_member() {
    let target = DeviceId::new("removed-device");
    let mut record = RevocationRecord::prepare(
        RevocationId::from_string("revocation-3").unwrap(),
        SpaceId::from_str("space-1"),
        target.clone(),
        GroupEpoch::new(1),
        100,
    )
    .unwrap();
    record.transition_to(RevocationStatus::Staged, 110).unwrap();
    let mut state = SpaceKeyState::legacy(SpaceId::from_str("space-1"));
    state.mark_migrating().unwrap();
    state
        .mark_ready(ContentKeyId::from_string("current-1").unwrap())
        .unwrap();
    state
        .rotate(ContentKeyId::from_string("current-2").unwrap())
        .unwrap();
    let outbox = vec![RevocationOutboxMessage::new(
        DeviceId::new("retained-device"),
        b"secret commit bytes".to_vec(),
    )];
    let stage = RevocationStage::new(
        record.clone(),
        state.clone(),
        b"secret group state".to_vec(),
        b"secret key catalog".to_vec(),
        outbox,
    )
    .unwrap();

    let debug = format!("{stage:?}");
    assert!(!debug.contains("secret"));
    assert_eq!(stage.record(), &record);
    assert_eq!(stage.next_space_state(), &state);

    let invalid = RevocationStage::new(
        record,
        state,
        vec![1],
        vec![2],
        vec![RevocationOutboxMessage::new(target, vec![3])],
    );
    assert_eq!(invalid, Err(KeyEpochError::RemovedMemberInOutbox));
}

#[test]
fn persisted_space_key_material_redacts_sensitive_bytes() {
    let state = SpaceKeyState::legacy(SpaceId::from_str("space-1"));
    let material = SpaceKeyMaterial::new(
        state.clone(),
        b"secret group state".to_vec(),
        b"secret key catalog".to_vec(),
        200,
    );

    let debug = format!("{material:?}");
    assert!(!debug.contains("secret"));
    assert_eq!(material.state(), &state);
    assert_eq!(material.group_state(), b"secret group state");
    assert_eq!(material.key_catalog(), b"secret key catalog");
    assert_eq!(material.updated_at_ms(), 200);
}
