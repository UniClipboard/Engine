use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    ContentKeyId, ContentKeyPurpose, GroupEpoch, KeyEpochError, ProtectionGroupId, RevocationId,
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
    state
        .mark_ready(first_current.clone(), ProtectionGroupId::generate())
        .unwrap();
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
    state
        .mark_ready(current.clone(), ProtectionGroupId::generate())
        .unwrap();

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
        .mark_ready(
            ContentKeyId::from_string("current-1").unwrap(),
            ProtectionGroupId::generate(),
        )
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

#[test]
fn recovery_required_is_terminal_and_reachable_from_active_states() {
    for status in [
        RevocationStatus::Prepared,
        RevocationStatus::Staged,
        RevocationStatus::Activated,
        RevocationStatus::Distributing,
    ] {
        let mut record = RevocationRecord::prepare(
            RevocationId::from_string(format!("recovery-{status:?}")).unwrap(),
            SpaceId::from_str("space-1"),
            DeviceId::new("removed-device"),
            GroupEpoch::new(1),
            100,
        )
        .unwrap();
        for next in [
            RevocationStatus::Staged,
            RevocationStatus::Activated,
            RevocationStatus::Distributing,
        ] {
            if record.status() == status {
                break;
            }
            record.transition_to(next, 101).unwrap();
        }
        record
            .transition_to(RevocationStatus::RecoveryRequired, 102)
            .unwrap();
        assert!(record.status().is_terminal());
    }
    assert!(RevocationStatus::Complete.is_terminal());
    assert!(!RevocationStatus::Distributing.is_terminal());
}

#[test]
fn retained_recipients_are_deduplicated_independent_of_order() {
    let record = RevocationRecord::prepare_with_recipients(
        RevocationId::from_string("deduplicated-recipients").unwrap(),
        SpaceId::from_str("space-1"),
        DeviceId::new("removed-device"),
        vec![
            DeviceId::new("alice"),
            DeviceId::new("bob"),
            DeviceId::new("alice"),
        ],
        GroupEpoch::new(1),
        100,
    )
    .unwrap();

    assert_eq!(
        record.retained_recipients(),
        &[DeviceId::new("alice"), DeviceId::new("bob")]
    );
}

#[test]
fn revocation_stage_tracks_each_recipient_once_and_rejects_unknown_ack() {
    let mut record = RevocationRecord::prepare_with_recipients(
        RevocationId::from_string("recipient-acks").unwrap(),
        SpaceId::from_str("space-1"),
        DeviceId::new("removed-device"),
        vec![DeviceId::new("alice"), DeviceId::new("bob")],
        GroupEpoch::new(1),
        100,
    )
    .unwrap();
    record.transition_to(RevocationStatus::Staged, 101).unwrap();
    let mut state = SpaceKeyState::legacy(SpaceId::from_str("space-1"));
    state.mark_migrating().unwrap();
    state
        .mark_ready(
            ContentKeyId::from_string("current-1").unwrap(),
            ProtectionGroupId::generate(),
        )
        .unwrap();
    state
        .rotate(ContentKeyId::from_string("current-2").unwrap())
        .unwrap();
    let mut stage = RevocationStage::new(
        record,
        state,
        vec![1],
        vec![2],
        vec![
            RevocationOutboxMessage::new(DeviceId::new("alice"), vec![3]),
            RevocationOutboxMessage::new(DeviceId::new("bob"), vec![4]),
        ],
    )
    .unwrap();

    assert!(!stage.all_recipients_confirmed());
    assert_eq!(
        stage.acknowledge_recipient(&DeviceId::new("unknown"), 102),
        Err(KeyEpochError::RevocationRecipientNotFound)
    );
    stage
        .acknowledge_recipient(&DeviceId::new("alice"), 103)
        .unwrap();
    assert!(!stage.all_recipients_confirmed());
    stage
        .acknowledge_recipient(&DeviceId::new("bob"), 104)
        .unwrap();
    assert!(stage.all_recipients_confirmed());
}

#[test]
fn permanent_loss_recovery_only_excludes_waiting_devices_and_appends_a_generation() {
    let mut record = RevocationRecord::prepare_with_recipients(
        RevocationId::from_string("permanent-loss-recovery").unwrap(),
        SpaceId::from_str("space-1"),
        DeviceId::new("removed-device"),
        vec![DeviceId::new("alice"), DeviceId::new("bob")],
        GroupEpoch::new(1),
        100,
    )
    .unwrap();
    record.transition_to(RevocationStatus::Staged, 101).unwrap();
    let mut first_state = SpaceKeyState::legacy(SpaceId::from_str("space-1"));
    first_state.mark_migrating().unwrap();
    first_state
        .mark_ready(
            ContentKeyId::from_string("current-1").unwrap(),
            ProtectionGroupId::generate(),
        )
        .unwrap();
    first_state
        .rotate(ContentKeyId::from_string("current-2").unwrap())
        .unwrap();
    let mut stage = RevocationStage::new(
        record,
        first_state.clone(),
        vec![1],
        vec![2],
        vec![
            RevocationOutboxMessage::new(DeviceId::new("alice"), vec![3]),
            RevocationOutboxMessage::new(DeviceId::new("bob"), vec![4]),
        ],
    )
    .unwrap();
    stage
        .transition_to(RevocationStatus::Activated, 102)
        .unwrap();
    stage
        .transition_to(RevocationStatus::Distributing, 103)
        .unwrap();
    stage
        .acknowledge_recipient(&DeviceId::new("bob"), 104)
        .unwrap();
    let before_invalid_recovery = stage.clone();
    assert_eq!(
        stage.append_recovery_generation(
            &DeviceId::new("alice"),
            first_state.clone(),
            vec![5],
            vec![6],
            vec![RevocationOutboxMessage::new(DeviceId::new("bob"), vec![7])],
            105,
        ),
        Err(KeyEpochError::InvalidRevocationStage)
    );
    assert_eq!(stage.record(), before_invalid_recovery.record());
    assert_eq!(
        stage.generation_count(),
        before_invalid_recovery.generation_count()
    );
    assert_eq!(
        stage.pending_recipient_device_ids(),
        before_invalid_recovery.pending_recipient_device_ids()
    );
    let mut second_state = first_state;
    second_state
        .rotate(ContentKeyId::from_string("current-3").unwrap())
        .unwrap();

    assert_eq!(
        stage.append_recovery_generation(
            &DeviceId::new("bob"),
            second_state.clone(),
            vec![5],
            vec![6],
            vec![],
            106,
        ),
        Err(KeyEpochError::PermanentLossRecipientNotPending)
    );
    stage
        .append_recovery_generation(
            &DeviceId::new("alice"),
            second_state,
            vec![5],
            vec![6],
            vec![RevocationOutboxMessage::new(DeviceId::new("bob"), vec![7])],
            107,
        )
        .unwrap();

    assert_eq!(stage.generation_count(), 2);
    assert_eq!(stage.record().previous_epoch(), GroupEpoch::new(2));
    assert_eq!(stage.record().next_epoch(), GroupEpoch::new(3));
    assert_eq!(stage.record().status(), RevocationStatus::Distributing);
    assert_eq!(
        stage.removed_device_ids(),
        vec![DeviceId::new("removed-device"), DeviceId::new("alice")]
    );
    assert_eq!(
        stage.pending_recipient_device_ids(),
        vec![DeviceId::new("bob")]
    );
    stage
        .acknowledge_recipient(&DeviceId::new("bob"), 108)
        .unwrap();
    assert!(stage.all_recipients_confirmed());
}

#[test]
fn absent_permanent_loss_recipient_finishes_the_old_revocation_without_another_generation() {
    let mut record = RevocationRecord::prepare_with_recipients(
        RevocationId::from_string("absent-permanent-loss-recipient").unwrap(),
        SpaceId::from_str("space-1"),
        DeviceId::new("removed-device"),
        vec![DeviceId::new("already-absent-device")],
        GroupEpoch::new(1),
        100,
    )
    .unwrap();
    record.transition_to(RevocationStatus::Staged, 101).unwrap();
    let mut state = SpaceKeyState::legacy(SpaceId::from_str("space-1"));
    state.mark_migrating().unwrap();
    state
        .mark_ready(
            ContentKeyId::from_string("current-1").unwrap(),
            ProtectionGroupId::generate(),
        )
        .unwrap();
    state
        .rotate(ContentKeyId::from_string("current-2").unwrap())
        .unwrap();
    let mut stage = RevocationStage::new(
        record,
        state,
        vec![1],
        vec![2],
        vec![RevocationOutboxMessage::new(
            DeviceId::new("already-absent-device"),
            vec![3],
        )],
    )
    .unwrap();
    stage
        .transition_to(RevocationStatus::Activated, 102)
        .unwrap();
    stage
        .transition_to(RevocationStatus::Distributing, 103)
        .unwrap();

    stage
        .finish_absent_recipients(&[DeviceId::new("already-absent-device")], 104)
        .unwrap();

    assert_eq!(stage.generation_count(), 1);
    assert_eq!(stage.record().status(), RevocationStatus::Complete);
    assert_eq!(
        stage.removed_device_ids(),
        vec![
            DeviceId::new("removed-device"),
            DeviceId::new("already-absent-device")
        ]
    );
    assert!(stage.pending_recipient_device_ids().is_empty());
}

#[test]
fn space_material_pending_updates_are_acknowledged_by_id() {
    let mut material = SpaceKeyMaterial::new(
        SpaceKeyState::legacy(SpaceId::from_str("space-1")),
        vec![1],
        vec![2],
        100,
    );
    let first =
        uc_core::membership::PendingGroupUpdate::persistent(DeviceId::new("alice"), vec![3]);
    let first_id = first.update_id().to_owned();
    let second = uc_core::membership::PendingGroupUpdate::persistent(DeviceId::new("bob"), vec![4]);
    material.add_pending_group_updates([first, second], 101);

    assert_eq!(material.pending_group_updates().len(), 2);
    assert!(!material.acknowledge_group_update("unknown", 102));
    assert!(material.acknowledge_group_update(&first_id, 103));
    assert_eq!(material.pending_group_updates().len(), 1);
}

#[test]
fn content_key_id_rejects_invalid_strings() {
    assert_eq!(
        ContentKeyId::from_string(""),
        Err(KeyEpochError::InvalidContentKeyId)
    );
    assert_eq!(
        ContentKeyId::from_string("a".repeat(129)),
        Err(KeyEpochError::InvalidContentKeyId)
    );
    assert_eq!(
        ContentKeyId::from_string("non-ascii-key-密钥"),
        Err(KeyEpochError::InvalidContentKeyId)
    );
}

#[test]
fn persisted_revocation_record_rejects_broken_epoch_invariant() {
    let record = RevocationRecord::prepare(
        RevocationId::from_string("invalid-persisted-record").unwrap(),
        SpaceId::from_str("space-1"),
        DeviceId::new("removed-device"),
        GroupEpoch::new(1),
        100,
    )
    .unwrap();
    let mut value = serde_json::to_value(record).unwrap();
    value["next_epoch"] = serde_json::json!(9);

    assert!(serde_json::from_value::<RevocationRecord>(value).is_err());
}
