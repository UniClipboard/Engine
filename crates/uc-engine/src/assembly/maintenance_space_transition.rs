use async_trait::async_trait;
use uc_application::deps::{
    AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
    AdvanceMembershipBranchTransitionError, AdvanceMembershipBranchTransitionInput,
    AdvanceMembershipBranchTransitionPort, CurrentSpaceIdentityError,
    DeviceManagementResetDataPort, InitialSpaceActivationPort,
};
use uc_core::ids::SpaceId;
use uc_core::membership::{AdmissionSpaceTransitionV2, MembershipBranchTransitionV1};

/// Profile lifecycle 未 Ready 时的显式能力门禁。
///
/// 该 adapter 不持有任何存储或安全依赖，所有 Space transition 都在 I/O 前
/// 失败关闭；Profile Factory Reset 通过独立 lifecycle 能力继续恢复。
pub(crate) struct MaintenanceOnlySpaceTransitionPorts;

#[derive(Debug, thiserror::Error)]
#[error("profile lifecycle is not ready for Space transitions")]
struct MaintenanceOnlyTransitionUnavailable;

#[async_trait]
impl AdmissionSpaceTransitionPort for MaintenanceOnlySpaceTransitionPorts {
    async fn preflight_source_history(
        &self,
        _preserve_unreadable_history: bool,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Locked)
    }

    async fn prepare_if_needed(
        &self,
        _input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Locked)
    }

    async fn advance(
        &self,
        _transition: &AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Locked)
    }

    async fn discard_pre_activation(
        &self,
        _transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Locked)
    }
}

#[async_trait]
impl DeviceManagementResetDataPort for MaintenanceOnlySpaceTransitionPorts {
    async fn prepare_device_management_reset(
        &self,
        _target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Locked)
    }

    async fn stage_device_management_reset_mutations(
        &self,
        _target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Locked)
    }

    async fn promote_device_management_reset(
        &self,
        _target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Locked)
    }

    async fn finalize_device_management_reset(
        &self,
        _target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        Err(AdmissionSpaceTransitionError::Locked)
    }
}

#[async_trait]
impl InitialSpaceActivationPort for MaintenanceOnlySpaceTransitionPorts {
    async fn activate_initial_space(
        &self,
        _space_id: &SpaceId,
    ) -> Result<(), CurrentSpaceIdentityError> {
        Err(CurrentSpaceIdentityError::Unavailable)
    }
}

#[async_trait]
impl AdvanceMembershipBranchTransitionPort for MaintenanceOnlySpaceTransitionPorts {
    async fn advance_membership_branch_transition(
        &self,
        _input: AdvanceMembershipBranchTransitionInput,
    ) -> Result<MembershipBranchTransitionV1, AdvanceMembershipBranchTransitionError> {
        Err(AdvanceMembershipBranchTransitionError::Unavailable {
            source: anyhow::Error::new(MaintenanceOnlyTransitionUnavailable),
        })
    }
}

#[cfg(test)]
mod tests {
    use uc_application::deps::{
        AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
        AdmissionSpaceTransitionPreparationV2, AdvanceMembershipBranchTransitionError,
        AdvanceMembershipBranchTransitionInput, AdvanceMembershipBranchTransitionPort,
        CurrentSpaceIdentityError, DeviceManagementResetDataPort, InitialSpaceActivationPort,
    };
    use uc_core::ids::{DeviceId, SpaceId};
    use uc_core::membership::{
        AdmissionChangeFacts, AdmissionSecurityCommitmentV1, AdmissionSpaceTransitionV2,
        BaseMembershipHistoryPosition, FreshSpaceControlTransitionPhaseV3,
        FreshSpaceControlTransitionV3, MemberInstanceId, MembershipBranchId,
        MembershipBranchRecoveryPackageV1, MembershipBranchTransitionV1, MembershipConflictId,
        MembershipCredential, SpaceAdmissionId, VersionedMembershipHistory,
        ADMISSION_SECURITY_COMMITMENT_FORMAT_V1, ED25519_SIGNATURE_ALGORITHM_V1,
        FRESH_SPACE_CONTROL_TRANSITION_FORMAT_V3,
    };
    use uc_core::security::IdentityFingerprint;

    use super::MaintenanceOnlySpaceTransitionPorts;

    #[tokio::test]
    async fn maintenance_profile_rejects_every_admission_action_as_locked() {
        let ports = MaintenanceOnlySpaceTransitionPorts;
        let transition = fresh_transition();

        assert_eq!(
            ports.preflight_source_history(false).await,
            Err(AdmissionSpaceTransitionError::Locked)
        );
        assert!(matches!(
            ports.prepare_if_needed(&preparation()).await,
            Err(AdmissionSpaceTransitionError::Locked)
        ));
        assert!(matches!(
            ports.advance(&transition).await,
            Err(AdmissionSpaceTransitionError::Locked)
        ));
        assert_eq!(
            ports.discard_pre_activation(&transition).await,
            Err(AdmissionSpaceTransitionError::Locked)
        );
    }

    #[tokio::test]
    async fn maintenance_profile_rejects_every_device_reset_action_as_locked() {
        let ports = MaintenanceOnlySpaceTransitionPorts;
        let target = SpaceId::from_str("maintenance-target");

        assert_eq!(
            ports.prepare_device_management_reset(&target).await,
            Err(AdmissionSpaceTransitionError::Locked)
        );
        assert_eq!(
            ports.stage_device_management_reset_mutations(&target).await,
            Err(AdmissionSpaceTransitionError::Locked)
        );
        assert_eq!(
            ports.promote_device_management_reset(&target).await,
            Err(AdmissionSpaceTransitionError::Locked)
        );
        assert_eq!(
            ports.finalize_device_management_reset(&target).await,
            Err(AdmissionSpaceTransitionError::Locked)
        );
    }

    #[tokio::test]
    async fn maintenance_profile_rejects_initial_activation_before_io() {
        let ports = MaintenanceOnlySpaceTransitionPorts;

        assert_eq!(
            ports
                .activate_initial_space(&SpaceId::from_str("maintenance-target"))
                .await,
            Err(CurrentSpaceIdentityError::Unavailable)
        );
    }

    #[tokio::test]
    async fn maintenance_profile_rejects_branch_transition_with_a_source() {
        let ports = MaintenanceOnlySpaceTransitionPorts;

        let error = ports
            .advance_membership_branch_transition(branch_input())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AdvanceMembershipBranchTransitionError::Unavailable { .. }
        ));
        assert!(std::error::Error::source(&error).is_some());
    }

    fn preparation() -> AdmissionSpaceTransitionPreparationV2 {
        let attempt = [0x11; 32];
        AdmissionSpaceTransitionPreparationV2 {
            attempt_id: SpaceAdmissionId::from_bytes(attempt).unwrap(),
            target_space_id: "maintenance-target".to_owned(),
            target_security_commitment: AdmissionSecurityCommitmentV1::new(
                ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
                "maintenance-target".to_owned(),
                vec![0x12],
                attempt,
                BaseMembershipHistoryPosition {
                    event_id: None,
                    depth: 0,
                    history_digest: [0x13; 32],
                },
                [0x14; 32],
                1,
                0,
                1,
                [0x15; 32],
                [0x16; 32],
                [0x17; 32],
                [0x18; 32],
                [0x19; 32],
            )
            .unwrap(),
            target_membership_history: vec![0x1a],
            target_security_state: vec![0x1b],
            target_protection_group_id: "maintenance-group".to_owned(),
            target_key_catalog: vec![0x1c],
            local_device_id: DeviceId::new("maintenance-device"),
            target_relationships: Vec::new(),
            relayed_group_updates: Vec::new(),
            target_access_state: vec![0x1d],
            target_admission_credentials: vec![0x1e],
            preserve_unreadable_history: false,
        }
    }

    fn fresh_transition() -> AdmissionSpaceTransitionV2 {
        AdmissionSpaceTransitionV2::FreshControl(FreshSpaceControlTransitionV3 {
            transition_format_version: FRESH_SPACE_CONTROL_TRANSITION_FORMAT_V3,
            attempt_id: SpaceAdmissionId::from_bytes([0x21; 32]).unwrap(),
            target_space_id: "maintenance-target".to_owned(),
            target_keyslot_generation: [0x22; 16],
            profile_data_generation: [0x23; 16],
            target_control_generation: [0x24; 16],
            target_access_state: vec![0x25],
            prepared_database_digest: [0x26; 32],
            phase: FreshSpaceControlTransitionPhaseV3::TargetPrepared,
        })
    }

    fn branch_input() -> AdvanceMembershipBranchTransitionInput {
        let device = DeviceId::new("maintenance-branch-device");
        let credential = MembershipCredential::new(ED25519_SIGNATURE_ALGORITHM_V1, vec![0x31; 32]);
        let member = credential.member_instance_id(&device);
        let history = VersionedMembershipHistory::new_single_member_root(
            "maintenance-branch-space".to_owned(),
            AdmissionChangeFacts {
                member_instance: member,
                device_id: device,
                device_name: "maintenance branch device".to_owned(),
                identity_fingerprint: IdentityFingerprint::from_display_string(
                    "ABCD-EFGH-IJKL-MNOP",
                )
                .unwrap(),
                transport_public_key: vec![0x32],
                transport_address_blob: vec![0x33],
                identity_signature: vec![0x34],
            },
            credential,
        )
        .unwrap();
        let conflict = MembershipConflictId::from_bytes([0x35; 32]);
        let branch = MembershipBranchId::from_bytes([0x36; 32]);
        let transition_id = MembershipBranchTransitionV1::derive_id(conflict, branch);
        let transition = MembershipBranchTransitionV1::new(
            transition_id,
            conflict,
            branch,
            [0x37; 16],
            [0x38; 16],
        )
        .unwrap();
        let recovery_package = MembershipBranchRecoveryPackageV1::new_unsigned(
            conflict,
            branch,
            member,
            MemberInstanceId::from_bytes(*member.as_bytes()),
            i64::MAX,
            [0x39; 32],
            history.encode_persisted_v2().unwrap(),
            vec![0x3a],
            vec![0x3b],
        )
        .unwrap();
        AdvanceMembershipBranchTransitionInput {
            transition,
            recipient_staged_mls_state: vec![0x3c],
            recovery_package,
            target_history: history,
        }
    }
}
