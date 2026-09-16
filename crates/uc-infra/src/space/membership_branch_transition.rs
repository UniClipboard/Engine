use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use uc_application::deps::{
    PrepareMembershipBranchTransitionError, PrepareMembershipBranchTransitionInput,
    PrepareMembershipBranchTransitionPort,
};
use uc_core::membership::MembershipBranchTransitionV1;

use crate::security::{
    ActiveRuntimeManifest, ActiveSpaceGenerationManifestStore,
    ActiveSpaceGenerationManifestStoreError,
};

/// 从当前加密 manifest 生成无磁盘副作用的分支切换计划。
pub struct DefaultMembershipBranchTransitionPreparation {
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
}

impl DefaultMembershipBranchTransitionPreparation {
    pub fn new(manifests: Arc<ActiveSpaceGenerationManifestStore>) -> Self {
        Self { manifests }
    }
}

#[async_trait]
impl PrepareMembershipBranchTransitionPort for DefaultMembershipBranchTransitionPreparation {
    async fn prepare_membership_branch_transition(
        &self,
        input: PrepareMembershipBranchTransitionInput,
    ) -> Result<MembershipBranchTransitionV1, PrepareMembershipBranchTransitionError> {
        if input.package.conflict_id() != input.conflict_id
            || input.package.target_branch_id() != input.target_branch_id
        {
            return Err(invalid("recovery package binding is inconsistent"));
        }
        let manifest = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?
            .ok_or_else(|| invalid("active generation manifest is missing"))?;
        let (source_generation, forbidden_generation) = match &manifest {
            ActiveRuntimeManifest::V2(manifest) => (manifest.database_generation, None),
            ActiveRuntimeManifest::V3(manifest) => (
                *manifest.layout().space_control_generation(),
                Some(*manifest.layout().profile_data_generation()),
            ),
        };
        let target_generation =
            random_target_generation(source_generation, forbidden_generation.as_ref());
        prepare_transition(input, source_generation, target_generation)
    }
}

fn random_target_generation(
    source_generation: [u8; 16],
    forbidden_generation: Option<&[u8; 16]>,
) -> [u8; 16] {
    loop {
        let mut generation = [0; 16];
        rand::rng().fill_bytes(&mut generation);
        if generation != [0; 16]
            && generation != source_generation
            && forbidden_generation != Some(&generation)
        {
            return generation;
        }
    }
}

fn prepare_transition(
    input: PrepareMembershipBranchTransitionInput,
    source_generation: [u8; 16],
    target_generation: [u8; 16],
) -> Result<MembershipBranchTransitionV1, PrepareMembershipBranchTransitionError> {
    if source_generation == [0; 16] || target_generation == [0; 16] {
        return Err(invalid("generation transition input is invalid"));
    }
    MembershipBranchTransitionV1::new(
        input.transition_id,
        input.conflict_id,
        input.target_branch_id,
        source_generation,
        target_generation,
    )
    .ok_or_else(|| invalid("generation transition plan is invalid"))
}

fn map_manifest_error(
    error: ActiveSpaceGenerationManifestStoreError,
) -> PrepareMembershipBranchTransitionError {
    match error {
        ActiveSpaceGenerationManifestStoreError::Storage => {
            PrepareMembershipBranchTransitionError::Unavailable {
                source: anyhow::Error::new(error),
            }
        }
        ActiveSpaceGenerationManifestStoreError::Corrupt
        | ActiveSpaceGenerationManifestStoreError::UnsupportedVersion => {
            PrepareMembershipBranchTransitionError::Invalid {
                source: anyhow::Error::new(error),
            }
        }
    }
}

fn invalid(context: &'static str) -> PrepareMembershipBranchTransitionError {
    PrepareMembershipBranchTransitionError::Invalid {
        source: anyhow::anyhow!(context),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use uc_core::membership::{
        MemberInstanceId, MembershipBranchId, MembershipBranchRecoveryPackageV1,
        MembershipConflictId,
    };

    use super::*;

    fn input() -> PrepareMembershipBranchTransitionInput {
        let conflict_id = MembershipConflictId::from_bytes([0x11; 32]);
        let target_branch_id = MembershipBranchId::from_bytes([0x12; 32]);
        PrepareMembershipBranchTransitionInput {
            transition_id: [0x13; 32],
            conflict_id,
            target_branch_id,
            package: MembershipBranchRecoveryPackageV1::new_unsigned(
                conflict_id,
                target_branch_id,
                MemberInstanceId::from_bytes([0x14; 32]),
                MemberInstanceId::from_bytes([0x15; 32]),
                100,
                [0x16; 32],
                vec![1],
                vec![2],
                vec![3],
            )
            .unwrap(),
        }
    }

    #[test]
    fn prepared_plan_uses_active_database_generation_and_fresh_target() {
        let prepared = prepare_transition(input(), [0x22; 16], [0x24; 16]).unwrap();

        assert_eq!(prepared.source_generation(), &[0x22; 16]);
        assert_eq!(prepared.target_generation(), &[0x24; 16]);
    }

    #[test]
    fn invalid_generation_keeps_stable_classification_and_source() {
        let error = prepare_transition(input(), [0x22; 16], [0; 16]).unwrap_err();

        assert!(matches!(
            error,
            PrepareMembershipBranchTransitionError::Invalid { .. }
        ));
        assert!(error.source().is_some());
    }
}
