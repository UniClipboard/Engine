use std::sync::Arc;

use async_trait::async_trait;
use uc_application::deps::{
    AdvanceMembershipBranchTransitionError, AdvanceMembershipBranchTransitionInput,
    AdvanceMembershipBranchTransitionPort,
};
use uc_core::membership::{
    ActiveRuntimeLayout, MembershipBranchTransitionPhaseV1, MembershipBranchTransitionV1,
};

use super::{
    ActiveRuntimeManifest, ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    ActiveSpaceGenerationManifestStoreError, SpaceControlGeneration, SpaceControlGenerationError,
    SpaceTransitionActivation, SpaceTransitionActivationError,
};
use crate::db::pool::DbPool;

/// Membership branch 的完整 V3 control-generation owner。
///
/// Core transition 中的 generation 在 V3 下只表示 control generation。模块内部
/// 独占 control seed、recovery material 验证、目标 security/relationships/ledger、
/// manifest promotion、运行期前向恢复与 source cleanup；profile payload 不参与。
pub struct V3MembershipBranchTransition {
    control_pool: DbPool,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    control_generations: Arc<SpaceControlGeneration>,
    activation: Arc<SpaceTransitionActivation>,
    operation_lock: tokio::sync::Mutex<()>,
}

impl V3MembershipBranchTransition {
    pub fn new(
        control_pool: DbPool,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        control_generations: Arc<SpaceControlGeneration>,
        activation: Arc<SpaceTransitionActivation>,
    ) -> Self {
        Self {
            control_pool,
            manifests,
            control_generations,
            activation,
            operation_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn transition_manifests(
        active: &ActiveRuntimeManifestV3,
        transition: &MembershipBranchTransitionV1,
    ) -> Result<
        (ActiveRuntimeManifestV3, ActiveRuntimeManifestV3),
        AdvanceMembershipBranchTransitionError,
    > {
        let active_generation = active.layout().space_control_generation();
        if active_generation != transition.source_generation()
            && active_generation != transition.target_generation()
        {
            return Err(invalid(anyhow::anyhow!(
                "active control generation does not match branch transition"
            )));
        }
        let source = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(
                active.layout().space_id().clone(),
                *active.layout().profile_data_generation(),
                *transition.source_generation(),
            )
            .map_err(|source| invalid(anyhow::Error::new(source)))?,
            *active.keyslot_generation(),
        )
        .ok_or_else(|| invalid(anyhow::anyhow!("branch source manifest is invalid")))?;
        let target = ActiveRuntimeManifestV3::new(
            ActiveRuntimeLayout::new(
                active.layout().space_id().clone(),
                *active.layout().profile_data_generation(),
                *transition.target_generation(),
            )
            .map_err(|source| invalid(anyhow::Error::new(source)))?,
            *active.keyslot_generation(),
        )
        .ok_or_else(|| invalid(anyhow::anyhow!("branch target manifest is invalid")))?;
        Ok((source, target))
    }

    fn advance(
        transition: &MembershipBranchTransitionV1,
        phase: MembershipBranchTransitionPhaseV1,
    ) -> Result<MembershipBranchTransitionV1, AdvanceMembershipBranchTransitionError> {
        transition
            .advance(phase)
            .ok_or_else(|| invalid(anyhow::anyhow!("branch transition cannot advance")))
    }
}

#[async_trait]
impl AdvanceMembershipBranchTransitionPort for V3MembershipBranchTransition {
    async fn advance_membership_branch_transition(
        &self,
        input: AdvanceMembershipBranchTransitionInput,
    ) -> Result<MembershipBranchTransitionV1, AdvanceMembershipBranchTransitionError> {
        let _guard = self.operation_lock.lock().await;
        let transition = &input.transition;
        if !transition.validate()
            || input.recovery_package.conflict_id() != transition.conflict_id()
            || input.recovery_package.target_branch_id() != transition.target_branch_id()
        {
            return Err(invalid(anyhow::anyhow!(
                "branch transition binding is invalid"
            )));
        }
        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?
            .ok_or_else(|| invalid(anyhow::anyhow!("active runtime manifest is missing")))?;
        let ActiveRuntimeManifest::V3(active) = active else {
            return Err(invalid(anyhow::anyhow!(
                "branch transition requires a V3 runtime"
            )));
        };
        let (source, target) = Self::transition_manifests(&active, transition)?;
        let source_is_active = active == source;
        let target_is_active = active == target;

        match transition.phase() {
            MembershipBranchTransitionPhaseV1::Prepared => {
                if !source_is_active {
                    return Err(invalid(anyhow::anyhow!(
                        "branch source changed before control seed"
                    )));
                }
                self.control_generations
                    .prepare_membership_branch_snapshot(&source, &target, &self.control_pool)
                    .await
                    .map_err(map_generation_error)?;
                Self::advance(
                    transition,
                    MembershipBranchTransitionPhaseV1::SourceBackedUp,
                )
            }
            MembershipBranchTransitionPhaseV1::SourceBackedUp => {
                if !source_is_active {
                    return Err(invalid(anyhow::anyhow!(
                        "branch source changed before target verification"
                    )));
                }
                self.control_generations
                    .verify_membership_branch_recovery(&input)
                    .map_err(map_generation_error)?;
                Self::advance(
                    transition,
                    MembershipBranchTransitionPhaseV1::TargetVerified,
                )
            }
            MembershipBranchTransitionPhaseV1::TargetVerified => {
                if !source_is_active {
                    return Err(invalid(anyhow::anyhow!(
                        "branch source changed before target staging"
                    )));
                }
                self.control_generations
                    .stage_membership_branch_target(&input, &source, &target)
                    .await
                    .map_err(map_generation_error)?;
                Self::advance(transition, MembershipBranchTransitionPhaseV1::TargetStaged)
            }
            MembershipBranchTransitionPhaseV1::TargetStaged => {
                if source_is_active {
                    let prepared = self
                        .control_generations
                        .finalize_membership_branch_target(&input, &source, &target)
                        .await
                        .map_err(map_generation_error)?;
                    self.activation
                        .activate_membership_branch(&source, &prepared)
                        .await
                        .map_err(map_activation_error)?;
                } else if target_is_active {
                    self.activation
                        .recover_membership_branch(&target)
                        .await
                        .map_err(map_activation_error)?;
                } else {
                    return Err(invalid(anyhow::anyhow!(
                        "branch runtime changed before promotion"
                    )));
                }
                Self::advance(transition, MembershipBranchTransitionPhaseV1::Promoted)
            }
            MembershipBranchTransitionPhaseV1::Promoted => {
                if !target_is_active {
                    return Err(recovery(anyhow::anyhow!(
                        "branch target is not active after promotion"
                    )));
                }
                self.activation
                    .recover_membership_branch(&target)
                    .await
                    .map_err(map_activation_error)?;
                Self::advance(
                    transition,
                    MembershipBranchTransitionPhaseV1::RuntimeRestored,
                )
            }
            MembershipBranchTransitionPhaseV1::RuntimeRestored => {
                if !target_is_active {
                    return Err(recovery(anyhow::anyhow!(
                        "branch target is not active during cleanup"
                    )));
                }
                self.activation
                    .cleanup_source_control_generation(&source, &target)
                    .await
                    .map_err(map_activation_error)?;
                Self::advance(transition, MembershipBranchTransitionPhaseV1::Completed)
            }
            MembershipBranchTransitionPhaseV1::Completed => Err(invalid(anyhow::anyhow!(
                "branch transition is already complete"
            ))),
        }
    }
}

fn map_manifest_error(
    source: ActiveSpaceGenerationManifestStoreError,
) -> AdvanceMembershipBranchTransitionError {
    match source {
        ActiveSpaceGenerationManifestStoreError::Storage => unavailable(anyhow::Error::new(source)),
        ActiveSpaceGenerationManifestStoreError::Corrupt
        | ActiveSpaceGenerationManifestStoreError::UnsupportedVersion => {
            invalid(anyhow::Error::new(source))
        }
    }
}

fn map_generation_error(
    source: SpaceControlGenerationError,
) -> AdvanceMembershipBranchTransitionError {
    match source {
        SpaceControlGenerationError::Busy { .. } | SpaceControlGenerationError::Storage { .. } => {
            unavailable(anyhow::Error::new(source))
        }
        SpaceControlGenerationError::Inconsistent { .. } => invalid(anyhow::Error::new(source)),
    }
}

fn map_activation_error(
    source: SpaceTransitionActivationError,
) -> AdvanceMembershipBranchTransitionError {
    match source {
        SpaceTransitionActivationError::Busy { .. }
        | SpaceTransitionActivationError::Storage { .. } => unavailable(anyhow::Error::new(source)),
        SpaceTransitionActivationError::Inconsistent { .. } => invalid(anyhow::Error::new(source)),
        SpaceTransitionActivationError::Recovery { .. } => recovery(anyhow::Error::new(source)),
    }
}

fn unavailable(source: anyhow::Error) -> AdvanceMembershipBranchTransitionError {
    AdvanceMembershipBranchTransitionError::Unavailable { source }
}

fn invalid(source: anyhow::Error) -> AdvanceMembershipBranchTransitionError {
    AdvanceMembershipBranchTransitionError::Invalid { source }
}

fn recovery(source: anyhow::Error) -> AdvanceMembershipBranchTransitionError {
    AdvanceMembershipBranchTransitionError::RecoveryRequired { source }
}

#[cfg(test)]
mod tests;
