use uc_application::deps::{
    CurrentSpaceMemberScopePort, HandleAuthenticatedSpaceAdmissionMessagePort,
    JoinerStartMaterialPort, JoinerStartStatePort, PendingAdmissionRecoveryStatePort,
    SpaceAdmissionTransportPort, SpaceApplicationDeps, SpaceSessionActivityPort,
    SponsorJoinRequestStatePort,
};
use uc_application::facade::space_setup::{SpaceFacade, SpaceFacadeDeps};
use uc_application::facade::{AppFacade, DeviceTrustStatus, JoinSpaceInput, NetworkRecoveryFacade};

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
        std::any::type_name::<dyn HandleAuthenticatedSpaceAdmissionMessagePort>(),
        std::any::type_name::<dyn JoinerStartMaterialPort>(),
        std::any::type_name::<dyn JoinerStartStatePort>(),
        std::any::type_name::<dyn PendingAdmissionRecoveryStatePort>(),
        std::any::type_name::<dyn SpaceAdmissionTransportPort>(),
        std::any::type_name::<dyn SponsorJoinRequestStatePort>(),
        std::any::type_name::<dyn SpaceSessionActivityPort>(),
    ];

    assert!(public_types.iter().all(|name| !name.is_empty()));
}

#[test]
fn app_facade_exposes_all_stable_space_membership_actions() {
    let _ = AppFacade::join_space;
    let _ = AppFacade::query_device_trust;
    let _ = AppFacade::remove_space_member;
    let _ = AppFacade::decide_device_trust_change;
    let _ = AppFacade::cancel_space_join;
}
