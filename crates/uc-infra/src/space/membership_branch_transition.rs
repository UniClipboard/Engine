use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore;
use uc_application::deps::{
    PrepareMembershipBranchTransitionError, PrepareMembershipBranchTransitionInput,
    PrepareMembershipBranchTransitionPort,
};
use uc_core::membership::{ActiveSpaceGenerationManifestV2, MembershipBranchTransitionV1};

use crate::security::{
    ActiveSpaceGenerationManifestStore, ActiveSpaceGenerationManifestStoreError,
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
            .load()
            .await
            .map_err(map_manifest_error)?
            .ok_or_else(|| invalid("active generation manifest is missing"))?;
        prepare_transition(input, &manifest, random_target_generation(&manifest))
    }
}

fn random_target_generation(manifest: &ActiveSpaceGenerationManifestV2) -> [u8; 16] {
    loop {
        let mut generation = [0; 16];
        rand::rng().fill_bytes(&mut generation);
        if generation != [0; 16] && generation != manifest.database_generation {
            return generation;
        }
    }
}

fn prepare_transition(
    input: PrepareMembershipBranchTransitionInput,
    manifest: &ActiveSpaceGenerationManifestV2,
    target_generation: [u8; 16],
) -> Result<MembershipBranchTransitionV1, PrepareMembershipBranchTransitionError> {
    if !manifest.validate() || target_generation == [0; 16] {
        return Err(invalid("generation transition input is invalid"));
    }
    MembershipBranchTransitionV1::new(
        input.transition_id,
        input.conflict_id,
        input.target_branch_id,
        manifest.database_generation,
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
        ActiveSpaceGenerationManifestStoreError::Corrupt => {
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
        let manifest = ActiveSpaceGenerationManifestV2::new(
            "space-a".to_owned(),
            [0x21; 16],
            [0x22; 16],
            [0x23; 16],
        )
        .unwrap();

        let prepared = prepare_transition(input(), &manifest, [0x24; 16]).unwrap();

        assert_eq!(prepared.source_generation(), &[0x22; 16]);
        assert_eq!(prepared.target_generation(), &[0x24; 16]);
    }

    #[test]
    fn invalid_generation_keeps_stable_classification_and_source() {
        let manifest = ActiveSpaceGenerationManifestV2::new(
            "space-a".to_owned(),
            [0x21; 16],
            [0x22; 16],
            [0x23; 16],
        )
        .unwrap();

        let error = prepare_transition(input(), &manifest, [0; 16]).unwrap_err();

        assert!(matches!(
            error,
            PrepareMembershipBranchTransitionError::Invalid { .. }
        ));
        assert!(error.source().is_some());
    }
}
