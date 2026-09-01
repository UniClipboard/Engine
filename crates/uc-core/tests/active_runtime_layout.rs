use uc_core::ids::SpaceId;
use uc_core::membership::{ActiveRuntimeLayout, ActiveRuntimeLayoutError};

#[test]
fn cross_space_layout_reuses_profile_data_and_replaces_control_generation() {
    let profile_data_generation = [0x21; 16];
    let source = ActiveRuntimeLayout::new(
        SpaceId::from_str("space-a"),
        profile_data_generation,
        [0x31; 16],
    )
    .unwrap();
    let target = ActiveRuntimeLayout::new(
        SpaceId::from_str("space-b"),
        profile_data_generation,
        [0x32; 16],
    )
    .unwrap();

    assert_eq!(source.profile_data_generation(), &profile_data_generation);
    assert_eq!(target.profile_data_generation(), &profile_data_generation);
    assert_ne!(source.space_id(), target.space_id());
    assert_ne!(
        source.space_control_generation(),
        target.space_control_generation()
    );
}

#[test]
fn active_runtime_layout_rejects_invalid_ownership_references() {
    assert_eq!(
        ActiveRuntimeLayout::new(SpaceId::from_str(""), [0x21; 16], [0x31; 16]),
        Err(ActiveRuntimeLayoutError::EmptySpaceId)
    );
    assert_eq!(
        ActiveRuntimeLayout::new(SpaceId::from_str("space-a"), [0; 16], [0x31; 16]),
        Err(ActiveRuntimeLayoutError::ReservedGeneration)
    );
    assert_eq!(
        ActiveRuntimeLayout::new(SpaceId::from_str("space-a"), [0x21; 16], [0; 16]),
        Err(ActiveRuntimeLayoutError::ReservedGeneration)
    );
    assert_eq!(
        ActiveRuntimeLayout::new(SpaceId::from_str("space-a"), [0x21; 16], [0x21; 16]),
        Err(ActiveRuntimeLayoutError::AliasedGenerations)
    );
}

#[test]
fn active_runtime_layout_debug_redacts_identifiers() {
    let layout = ActiveRuntimeLayout::new(
        SpaceId::from_str("private-space-id"),
        [0x21; 16],
        [0x31; 16],
    )
    .unwrap();

    let debug = format!("{layout:?}");
    assert!(!debug.contains("private-space-id"));
    assert!(!debug.contains("21"));
    assert!(!debug.contains("31"));
}
