use std::sync::Arc;

use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use uc_application::deps::{
    AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionStepV2,
};
use uc_core::ids::SpaceId;
use uc_core::membership::{
    ActiveRuntimeLayout, AdmissionSpaceTransitionResultV2, AdmissionSpaceTransitionV2,
    CrossSpaceControlTransitionPhaseV3, CrossSpaceControlTransitionResultV3,
    CrossSpaceControlTransitionV3, CROSS_SPACE_CONTROL_TRANSITION_FORMAT_V3,
};

use super::{
    ActiveRuntimeManifest, ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    PreparedSpaceControlGeneration, SpaceControlGeneration, SpaceControlGenerationError,
    SpaceTransitionActivation, SpaceTransitionActivationError,
};

const GENERATION_DOMAIN: &[u8] = b"uniclipboard/cross-space-control-generation/v3\0";

/// V3 admission 的 control-only CrossSpace 流程。
///
/// 该 adapter 只编排两个完整 owner：`SpaceControlGeneration` 准备不可变目标，
/// `SpaceTransitionActivation` 提升 manifest 并重绑 control runtime。它不持有
/// profile database、blob store、source/target cipher 或 payload migration port。
pub struct V3AdmissionSpaceTransition {
    profile_salt: Vec<u8>,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    control_generations: Arc<SpaceControlGeneration>,
    activation: Arc<SpaceTransitionActivation>,
}

impl V3AdmissionSpaceTransition {
    pub fn new(
        profile_salt: Vec<u8>,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        control_generations: Arc<SpaceControlGeneration>,
        activation: Arc<SpaceTransitionActivation>,
    ) -> Self {
        Self {
            profile_salt,
            manifests,
            control_generations,
            activation,
        }
    }

    fn generation(&self, attempt_id: &[u8; 32], purpose: &[u8]) -> [u8; 16] {
        let mut hasher = Sha256::new();
        hasher.update(GENERATION_DOMAIN);
        hasher.update((self.profile_salt.len() as u64).to_be_bytes());
        hasher.update(&self.profile_salt);
        hasher.update(attempt_id);
        hasher.update((purpose.len() as u64).to_be_bytes());
        hasher.update(purpose);
        let digest = hasher.finalize();
        let mut generation = [0u8; 16];
        generation.copy_from_slice(&digest[..16]);
        generation
    }

    fn source_manifest(
        transition: &CrossSpaceControlTransitionV3,
    ) -> Result<ActiveRuntimeManifestV3, AdmissionSpaceTransitionError> {
        let layout = ActiveRuntimeLayout::new(
            SpaceId::from_string(transition.source_space_id.clone()),
            transition.profile_data_generation,
            transition.source_control_generation,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        ActiveRuntimeManifestV3::new(layout, transition.source_keyslot_generation)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    fn target_manifest(
        transition: &CrossSpaceControlTransitionV3,
    ) -> Result<ActiveRuntimeManifestV3, AdmissionSpaceTransitionError> {
        let layout = ActiveRuntimeLayout::new(
            SpaceId::from_string(transition.target_space_id.clone()),
            transition.profile_data_generation,
            transition.target_control_generation,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        ActiveRuntimeManifestV3::new(layout, transition.target_keyslot_generation)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    async fn proof(
        &self,
        transition: &CrossSpaceControlTransitionV3,
    ) -> Result<PreparedSpaceControlGeneration, AdmissionSpaceTransitionError> {
        self.control_generations
            .reopen_prepared(
                &Self::target_manifest(transition)?,
                &transition.prepared_database_digest,
            )
            .await
            .map_err(map_control_generation_error)
    }

    async fn continue_activation(
        &self,
        transition: &CrossSpaceControlTransitionV3,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let source = Self::source_manifest(transition)?;
        let target = Self::target_manifest(transition)?;
        match self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?
        {
            Some(ActiveRuntimeManifest::V3(active)) if active == source => {
                let proof = self.proof(transition).await?;
                self.activation
                    .activate_cross_space(&source, &proof, &transition.target_access_state)
                    .await
                    .map_err(map_activation_error)?;
            }
            Some(ActiveRuntimeManifest::V3(active)) if active == target => {
                self.activation
                    .recover_cross_space(&target, &transition.target_access_state)
                    .await
                    .map_err(map_activation_error)?;
            }
            Some(ActiveRuntimeManifest::V2(_)) | Some(ActiveRuntimeManifest::V3(_)) | None => {
                return Err(AdmissionSpaceTransitionError::Inconsistent);
            }
        }
        Ok(())
    }

    fn advanced(
        transition: &CrossSpaceControlTransitionV3,
        phase: CrossSpaceControlTransitionPhaseV3,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        let mut next = transition.clone();
        next.phase = phase;
        transition
            .can_advance_to(&next)
            .then_some(AdmissionSpaceTransitionStepV2::Advanced(
                AdmissionSpaceTransitionV2::CrossSpaceControl(next),
            ))
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }
}

#[async_trait]
impl AdmissionSpaceTransitionPort for V3AdmissionSpaceTransition {
    async fn prepare_if_needed(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError> {
        let source = match self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?
        {
            Some(ActiveRuntimeManifest::V3(manifest)) => manifest,
            Some(ActiveRuntimeManifest::V2(_)) | None => {
                return Err(AdmissionSpaceTransitionError::Unavailable)
            }
        };
        if source.layout().space_id().as_ref() == input.target_space_id {
            // SameSpace 的数据保留与 keyslot 语义由下一独立切片实现。
            return Err(AdmissionSpaceTransitionError::Unavailable);
        }
        let target_keyslot_generation =
            self.generation(input.attempt_id.as_bytes(), b"target-keyslot");
        let target_control_generation =
            self.generation(input.attempt_id.as_bytes(), b"target-control");
        let target_layout = ActiveRuntimeLayout::new(
            SpaceId::from_string(input.target_space_id.clone()),
            *source.layout().profile_data_generation(),
            target_control_generation,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        let target = ActiveRuntimeManifestV3::new(target_layout, target_keyslot_generation)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        let prepared = self
            .control_generations
            .prepare_admission(input, &target)
            .await
            .map_err(map_control_generation_error)?;

        Ok(AdmissionSpaceTransitionV2::CrossSpaceControl(
            CrossSpaceControlTransitionV3 {
                transition_format_version: CROSS_SPACE_CONTROL_TRANSITION_FORMAT_V3,
                attempt_id: input.attempt_id,
                source_space_id: source.layout().space_id().as_ref().to_owned(),
                source_keyslot_generation: *source.keyslot_generation(),
                profile_data_generation: *source.layout().profile_data_generation(),
                source_control_generation: *source.layout().space_control_generation(),
                target_space_id: input.target_space_id.clone(),
                target_keyslot_generation,
                target_control_generation,
                target_access_state: input.target_access_state.clone(),
                prepared_database_digest: *prepared.database_digest(),
                phase: CrossSpaceControlTransitionPhaseV3::TargetPrepared,
            },
        ))
    }

    async fn advance(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        let AdmissionSpaceTransitionV2::CrossSpaceControl(transition) = transition else {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        };
        if !transition.validate() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        match transition.phase {
            CrossSpaceControlTransitionPhaseV3::TargetPrepared => Self::advanced(
                transition,
                CrossSpaceControlTransitionPhaseV3::ActivationStarted,
            ),
            CrossSpaceControlTransitionPhaseV3::ActivationStarted => {
                self.continue_activation(transition).await?;
                Self::advanced(
                    transition,
                    CrossSpaceControlTransitionPhaseV3::TargetPromoted,
                )
            }
            CrossSpaceControlTransitionPhaseV3::TargetPromoted => {
                let source = Self::source_manifest(transition)?;
                let target = Self::target_manifest(transition)?;
                self.activation
                    .recover_cross_space(&target, &transition.target_access_state)
                    .await
                    .map_err(map_activation_error)?;
                self.activation
                    .cleanup_source_control_generation(&source, &target)
                    .await
                    .map_err(map_activation_error)?;
                Self::advanced(
                    transition,
                    CrossSpaceControlTransitionPhaseV3::CleanupPending,
                )
            }
            CrossSpaceControlTransitionPhaseV3::CleanupPending => {
                let result = CrossSpaceControlTransitionResultV3::from_cleanup_pending(transition)
                    .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
                Ok(AdmissionSpaceTransitionStepV2::Finished(
                    AdmissionSpaceTransitionResultV2::CrossSpaceControl(result),
                ))
            }
        }
    }

    async fn discard_pre_activation(
        &self,
        transition: &AdmissionSpaceTransitionV2,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let AdmissionSpaceTransitionV2::CrossSpaceControl(transition) = transition else {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        };
        if transition.phase != CrossSpaceControlTransitionPhaseV3::TargetPrepared
            || !transition.validate()
        {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let source = Self::source_manifest(transition)?;
        let proof = self.proof(transition).await?;
        self.activation
            .discard_cross_space(&source, &proof)
            .await
            .map_err(map_activation_error)
    }
}

fn map_manifest_error(
    error: super::ActiveSpaceGenerationManifestStoreError,
) -> AdmissionSpaceTransitionError {
    match error {
        super::ActiveSpaceGenerationManifestStoreError::Storage => {
            AdmissionSpaceTransitionError::Storage
        }
        super::ActiveSpaceGenerationManifestStoreError::Corrupt
        | super::ActiveSpaceGenerationManifestStoreError::UnsupportedVersion => {
            AdmissionSpaceTransitionError::Inconsistent
        }
    }
}

fn map_control_generation_error(
    error: SpaceControlGenerationError,
) -> AdmissionSpaceTransitionError {
    match error {
        SpaceControlGenerationError::Busy { .. } => AdmissionSpaceTransitionError::Unavailable,
        SpaceControlGenerationError::Inconsistent { .. } => {
            AdmissionSpaceTransitionError::Inconsistent
        }
        SpaceControlGenerationError::Storage { .. } => AdmissionSpaceTransitionError::Storage,
    }
}

fn map_activation_error(error: SpaceTransitionActivationError) -> AdmissionSpaceTransitionError {
    match error {
        SpaceTransitionActivationError::Busy { .. } => AdmissionSpaceTransitionError::Unavailable,
        SpaceTransitionActivationError::Inconsistent { .. } => {
            AdmissionSpaceTransitionError::Inconsistent
        }
        SpaceTransitionActivationError::Storage { .. } => AdmissionSpaceTransitionError::Storage,
        SpaceTransitionActivationError::Recovery { .. } => {
            AdmissionSpaceTransitionError::RecoveryRequired
        }
    }
}

#[cfg(test)]
mod tests;
