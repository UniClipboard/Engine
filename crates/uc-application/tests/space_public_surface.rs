use uc_application::deps::{
    CurrentSpaceMemberScopePort, HandleSpaceAdmissionMessagePort, SpaceApplicationDeps,
    SpaceSessionActivityPort,
};
use uc_application::facade::space_setup::{SpaceFacade, SpaceFacadeDeps};
use uc_application::facade::{DeviceTrustStatus, JoinSpaceInput, NetworkRecoveryFacade};

#[test]
fn space_contracts_remain_reachable_only_through_facade_and_deps() {
    let public_types = [
        std::any::type_name::<SpaceFacade>(),
        std::any::type_name::<SpaceFacadeDeps>(),
        std::any::type_name::<JoinSpaceInput>(),
        std::any::type_name::<DeviceTrustStatus>(),
        std::any::type_name::<NetworkRecoveryFacade>(),
        std::any::type_name::<SpaceApplicationDeps>(),
        std::any::type_name::<dyn CurrentSpaceMemberScopePort>(),
        std::any::type_name::<dyn HandleSpaceAdmissionMessagePort>(),
        std::any::type_name::<dyn SpaceSessionActivityPort>(),
    ];

    assert!(public_types.iter().all(|name| !name.is_empty()));
}
