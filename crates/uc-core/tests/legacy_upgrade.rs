use uc_core::ids::DeviceId;
use uc_core::ids::SpaceId;
use uc_core::membership::{
    decide_legacy_upgrade, AdmissionReplayId, ContentKeyId, LegacyProtectionCommand,
    LegacyProtectionResult, LegacyProtectionSnapshot, LegacyRequestInspection, LegacyUpgradeAction,
    LegacyUpgradeDescriptor, LegacyUpgradeId, ProtectionGroupAdmission, ProtectionGroupId,
    SpaceKeyMaterial, SpaceKeyState,
};
use uc_core::space_access::GroupAdmission;

fn upgrade_id(seed: u8) -> LegacyUpgradeId {
    LegacyUpgradeId::from_bytes([seed; 32])
}

fn group(id: &str) -> ProtectionGroupId {
    ProtectionGroupId::from_string(id).unwrap()
}

#[test]
fn a_ready_device_admits_a_legacy_peer() {
    let local = LegacyUpgradeDescriptor::ready(upgrade_id(1), group("group-a"));
    let remote = LegacyUpgradeDescriptor::legacy(upgrade_id(1));

    assert_eq!(
        decide_legacy_upgrade(
            &DeviceId::new("device-a"),
            &local,
            &DeviceId::new("device-b"),
            &remote,
        ),
        LegacyUpgradeAction::AdmitRemote
    );
}

#[test]
fn a_legacy_device_joins_a_ready_peer() {
    let local = LegacyUpgradeDescriptor::legacy(upgrade_id(1));
    let remote = LegacyUpgradeDescriptor::ready(upgrade_id(1), group("group-a"));

    assert_eq!(
        decide_legacy_upgrade(
            &DeviceId::new("device-b"),
            &local,
            &DeviceId::new("device-a"),
            &remote,
        ),
        LegacyUpgradeAction::JoinRemote
    );
}

#[test]
fn divergent_groups_converge_on_the_lexicographically_smaller_group_id() {
    let winner = LegacyUpgradeDescriptor::ready(upgrade_id(1), group("group-a"));
    let loser = LegacyUpgradeDescriptor::ready(upgrade_id(1), group("group-b"));

    assert_eq!(
        decide_legacy_upgrade(
            &DeviceId::new("device-a"),
            &winner,
            &DeviceId::new("device-b"),
            &loser,
        ),
        LegacyUpgradeAction::AdmitRemote
    );
    assert_eq!(
        decide_legacy_upgrade(
            &DeviceId::new("device-b"),
            &loser,
            &DeviceId::new("device-a"),
            &winner,
        ),
        LegacyUpgradeAction::JoinRemote
    );
}

#[test]
fn devices_from_different_legacy_spaces_are_rejected() {
    let local = LegacyUpgradeDescriptor::legacy(upgrade_id(1));
    let remote = LegacyUpgradeDescriptor::ready(upgrade_id(2), group("group-a"));

    assert_eq!(
        decide_legacy_upgrade(
            &DeviceId::new("device-a"),
            &local,
            &DeviceId::new("device-b"),
            &remote,
        ),
        LegacyUpgradeAction::Reject
    );
}

#[test]
fn two_legacy_devices_choose_one_bootstrap_owner() {
    let local = LegacyUpgradeDescriptor::legacy(upgrade_id(1));
    let remote = LegacyUpgradeDescriptor::legacy(upgrade_id(1));

    assert_eq!(
        decide_legacy_upgrade(
            &DeviceId::new("device-a"),
            &local,
            &DeviceId::new("device-b"),
            &remote,
        ),
        LegacyUpgradeAction::CreateLocalGroup
    );
    assert_eq!(
        decide_legacy_upgrade(
            &DeviceId::new("device-b"),
            &remote,
            &DeviceId::new("device-a"),
            &local,
        ),
        LegacyUpgradeAction::AwaitRemote
    );
}

#[test]
fn devices_already_in_the_same_group_do_nothing() {
    let local = LegacyUpgradeDescriptor::ready(upgrade_id(1), group("group-a"));
    let remote = local.clone();

    assert_eq!(
        decide_legacy_upgrade(
            &DeviceId::new("device-a"),
            &local,
            &DeviceId::new("device-b"),
            &remote,
        ),
        LegacyUpgradeAction::NoAction
    );
}

#[test]
fn ready_space_material_carries_its_protection_group_identity() {
    let mut state = SpaceKeyState::legacy(SpaceId::from("space-a"));
    state.mark_migrating().unwrap();
    state
        .mark_ready(ContentKeyId::generate(), group("group-a"))
        .unwrap();

    let encoded = serde_json::to_vec(&state).unwrap();
    let restored: SpaceKeyState = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(restored.protection_group_id(), Some(&group("group-a")));
}

#[test]
fn admission_response_is_cached_by_an_opaque_replay_id_across_restart() {
    let recipient = DeviceId::new("device-b");
    let replay_id = AdmissionReplayId::from_bytes([3; 32]);
    let admission = ProtectionGroupAdmission {
        protection_group_id: group("group-a"),
        admission: GroupAdmission {
            welcome: vec![7],
            encrypted_key_catalog: vec![8],
            existing_member_updates: Vec::new(),
            group_epoch: 2,
        },
    };
    let mut state = SpaceKeyState::legacy(SpaceId::from("space-a"));
    state.mark_migrating().unwrap();
    state
        .mark_ready(ContentKeyId::generate(), group("group-a"))
        .unwrap();
    let mut material = SpaceKeyMaterial::new(state, vec![1], vec![2], 100);

    material.cache_group_admission(recipient, replay_id, admission.clone(), 110);
    let encoded = serde_json::to_vec(&material).unwrap();
    let restored: SpaceKeyMaterial = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(
        restored.cached_group_admission(&recipient, replay_id),
        Some(&admission)
    );
    assert!(restored
        .cached_group_admission(&recipient, AdmissionReplayId::from_bytes([4; 32]))
        .is_none());
}

#[test]
fn protection_contract_exposes_complete_commands_without_private_join_state() {
    let descriptor = LegacyUpgradeDescriptor::legacy(upgrade_id(1));
    let snapshot = LegacyProtectionSnapshot {
        descriptor: descriptor.clone(),
        protected_members: vec![DeviceId::new("device-a")],
        pending_readmission_members: vec![DeviceId::new("device-b")],
    };
    assert_eq!(snapshot.descriptor, descriptor);
    assert_eq!(snapshot.protected_members, vec![DeviceId::new("device-a")]);
    assert_eq!(
        snapshot.pending_readmission_members,
        vec![DeviceId::new("device-b")]
    );

    let command = LegacyProtectionCommand::CreateGroup {
        sponsor: DeviceId::new("device-a"),
        retained_members: vec![DeviceId::new("device-b")],
    };
    assert!(matches!(
        command,
        LegacyProtectionCommand::CreateGroup { .. }
    ));
    assert!(matches!(
        LegacyRequestInspection::Verified,
        LegacyRequestInspection::Verified
    ));
    assert!(matches!(
        LegacyProtectionResult::GroupReady(LegacyUpgradeDescriptor::legacy(upgrade_id(1))),
        LegacyProtectionResult::GroupReady(_)
    ));
}
