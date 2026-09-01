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
    CrossSpaceControlTransitionV3, FreshSpaceControlTransitionPhaseV3,
    FreshSpaceControlTransitionResultV3, FreshSpaceControlTransitionV3,
    SameSpaceControlTransitionPhaseV3, SameSpaceControlTransitionResultV3,
    SameSpaceControlTransitionV3, CROSS_SPACE_CONTROL_TRANSITION_FORMAT_V3,
    FRESH_SPACE_CONTROL_TRANSITION_FORMAT_V3, SAME_SPACE_CONTROL_TRANSITION_FORMAT_V3,
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
    fresh_profile_data_generation: Option<[u8; 16]>,
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
            fresh_profile_data_generation: None,
            manifests,
            control_generations,
            activation,
        }
    }

    /// 构造已经由 profile 初始化/升级负责人准备好空数据世代的 Fresh adapter。
    pub fn new_with_fresh_profile_generation(
        profile_salt: Vec<u8>,
        profile_data_generation: [u8; 16],
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        control_generations: Arc<SpaceControlGeneration>,
        activation: Arc<SpaceTransitionActivation>,
    ) -> Self {
        Self {
            profile_salt,
            fresh_profile_data_generation: (profile_data_generation != [0; 16])
                .then_some(profile_data_generation),
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

    fn same_space_source_manifest(
        transition: &SameSpaceControlTransitionV3,
    ) -> Result<ActiveRuntimeManifestV3, AdmissionSpaceTransitionError> {
        let layout = ActiveRuntimeLayout::new(
            SpaceId::from_string(transition.space_id.clone()),
            transition.profile_data_generation,
            transition.source_control_generation,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        ActiveRuntimeManifestV3::new(layout, transition.retained_keyslot_generation)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    fn same_space_target_manifest(
        transition: &SameSpaceControlTransitionV3,
    ) -> Result<ActiveRuntimeManifestV3, AdmissionSpaceTransitionError> {
        let layout = ActiveRuntimeLayout::new(
            SpaceId::from_string(transition.space_id.clone()),
            transition.profile_data_generation,
            transition.target_control_generation,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        ActiveRuntimeManifestV3::new(layout, transition.retained_keyslot_generation)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    fn fresh_target_manifest(
        transition: &FreshSpaceControlTransitionV3,
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

    async fn continue_same_space_activation(
        &self,
        transition: &SameSpaceControlTransitionV3,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let source = Self::same_space_source_manifest(transition)?;
        let target = Self::same_space_target_manifest(transition)?;
        match self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?
        {
            Some(ActiveRuntimeManifest::V3(active)) if active == source => {
                let proof = self
                    .control_generations
                    .reopen_prepared(&target, &transition.prepared_database_digest)
                    .await
                    .map_err(map_control_generation_error)?;
                self.activation
                    .activate_same_space(&source, &proof)
                    .await
                    .map_err(map_activation_error)?;
            }
            Some(ActiveRuntimeManifest::V3(active)) if active == target => {
                self.activation
                    .recover_same_space(&target)
                    .await
                    .map_err(map_activation_error)?;
            }
            Some(ActiveRuntimeManifest::V2(_)) | Some(ActiveRuntimeManifest::V3(_)) | None => {
                return Err(AdmissionSpaceTransitionError::Inconsistent);
            }
        }
        Ok(())
    }

    async fn continue_fresh_activation(
        &self,
        transition: &FreshSpaceControlTransitionV3,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let target = Self::fresh_target_manifest(transition)?;
        match self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?
        {
            None => {
                let proof = self
                    .control_generations
                    .reopen_prepared(&target, &transition.prepared_database_digest)
                    .await
                    .map_err(map_control_generation_error)?;
                self.activation
                    .activate_fresh(&proof, &transition.target_access_state)
                    .await
                    .map_err(map_activation_error)?;
            }
            Some(ActiveRuntimeManifest::V3(active)) if active == target => {
                self.activation
                    .recover_fresh(&target, &transition.target_access_state)
                    .await
                    .map_err(map_activation_error)?;
            }
            Some(ActiveRuntimeManifest::V2(_)) | Some(ActiveRuntimeManifest::V3(_)) => {
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

    fn same_space_advanced(
        transition: &SameSpaceControlTransitionV3,
        phase: SameSpaceControlTransitionPhaseV3,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        let mut next = transition.clone();
        next.phase = phase;
        transition
            .can_advance_to(&next)
            .then_some(AdmissionSpaceTransitionStepV2::Advanced(
                AdmissionSpaceTransitionV2::SameSpaceControl(next),
            ))
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    async fn advance_same_space(
        &self,
        transition: &SameSpaceControlTransitionV3,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        if !transition.validate() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        match transition.phase {
            SameSpaceControlTransitionPhaseV3::TargetPrepared => Self::same_space_advanced(
                transition,
                SameSpaceControlTransitionPhaseV3::ActivationStarted,
            ),
            SameSpaceControlTransitionPhaseV3::ActivationStarted => {
                self.continue_same_space_activation(transition).await?;
                Self::same_space_advanced(
                    transition,
                    SameSpaceControlTransitionPhaseV3::TargetPromoted,
                )
            }
            SameSpaceControlTransitionPhaseV3::TargetPromoted => {
                let source = Self::same_space_source_manifest(transition)?;
                let target = Self::same_space_target_manifest(transition)?;
                self.activation
                    .recover_same_space(&target)
                    .await
                    .map_err(map_activation_error)?;
                self.activation
                    .cleanup_source_control_generation(&source, &target)
                    .await
                    .map_err(map_activation_error)?;
                Self::same_space_advanced(
                    transition,
                    SameSpaceControlTransitionPhaseV3::CleanupPending,
                )
            }
            SameSpaceControlTransitionPhaseV3::CleanupPending => {
                let result = SameSpaceControlTransitionResultV3::from_cleanup_pending(transition)
                    .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
                Ok(AdmissionSpaceTransitionStepV2::Finished(
                    AdmissionSpaceTransitionResultV2::SameSpaceControl(result),
                ))
            }
        }
    }

    fn fresh_advanced(
        transition: &FreshSpaceControlTransitionV3,
        phase: FreshSpaceControlTransitionPhaseV3,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        let mut next = transition.clone();
        next.phase = phase;
        transition
            .can_advance_to(&next)
            .then_some(AdmissionSpaceTransitionStepV2::Advanced(
                AdmissionSpaceTransitionV2::FreshControl(next),
            ))
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    async fn advance_fresh(
        &self,
        transition: &FreshSpaceControlTransitionV3,
    ) -> Result<AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionError> {
        if !transition.validate() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        match transition.phase {
            FreshSpaceControlTransitionPhaseV3::TargetPrepared => Self::fresh_advanced(
                transition,
                FreshSpaceControlTransitionPhaseV3::ActivationStarted,
            ),
            FreshSpaceControlTransitionPhaseV3::ActivationStarted => {
                self.continue_fresh_activation(transition).await?;
                Self::fresh_advanced(
                    transition,
                    FreshSpaceControlTransitionPhaseV3::TargetPromoted,
                )
            }
            FreshSpaceControlTransitionPhaseV3::TargetPromoted => {
                let target = Self::fresh_target_manifest(transition)?;
                self.activation
                    .recover_fresh(&target, &transition.target_access_state)
                    .await
                    .map_err(map_activation_error)?;
                Self::fresh_advanced(
                    transition,
                    FreshSpaceControlTransitionPhaseV3::CleanupPending,
                )
            }
            FreshSpaceControlTransitionPhaseV3::CleanupPending => {
                let result = FreshSpaceControlTransitionResultV3::from_cleanup_pending(transition)
                    .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
                Ok(AdmissionSpaceTransitionStepV2::Finished(
                    AdmissionSpaceTransitionResultV2::FreshControl(result),
                ))
            }
        }
    }
}

#[async_trait]
impl AdmissionSpaceTransitionPort for V3AdmissionSpaceTransition {
    async fn prepare_if_needed(
        &self,
        input: &AdmissionSpaceTransitionPreparationV2,
    ) -> Result<AdmissionSpaceTransitionV2, AdmissionSpaceTransitionError> {
        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        let target_control_generation =
            self.generation(input.attempt_id.as_bytes(), b"target-control");
        if active.is_none() {
            let profile_data_generation = self
                .fresh_profile_data_generation
                .ok_or(AdmissionSpaceTransitionError::Unavailable)?;
            let target_keyslot_generation =
                self.generation(input.attempt_id.as_bytes(), b"target-keyslot");
            let target_layout = ActiveRuntimeLayout::new(
                SpaceId::from_string(input.target_space_id.clone()),
                profile_data_generation,
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
            return Ok(AdmissionSpaceTransitionV2::FreshControl(
                FreshSpaceControlTransitionV3 {
                    transition_format_version: FRESH_SPACE_CONTROL_TRANSITION_FORMAT_V3,
                    attempt_id: input.attempt_id,
                    target_space_id: input.target_space_id.clone(),
                    target_keyslot_generation,
                    profile_data_generation,
                    target_control_generation,
                    target_access_state: input.target_access_state.clone(),
                    prepared_database_digest: *prepared.database_digest(),
                    phase: FreshSpaceControlTransitionPhaseV3::TargetPrepared,
                },
            ));
        }
        let source = match active {
            Some(ActiveRuntimeManifest::V3(manifest)) => manifest,
            Some(ActiveRuntimeManifest::V2(_)) => {
                return Err(AdmissionSpaceTransitionError::Unavailable)
            }
            None => return Err(AdmissionSpaceTransitionError::Inconsistent),
        };
        if source.layout().space_id().as_ref() == input.target_space_id {
            let target_layout = ActiveRuntimeLayout::new(
                source.layout().space_id().clone(),
                *source.layout().profile_data_generation(),
                target_control_generation,
            )
            .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
            let target = ActiveRuntimeManifestV3::new(target_layout, *source.keyslot_generation())
                .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
            let prepared = self
                .control_generations
                .prepare_same_space_admission(input, &source, &target)
                .await
                .map_err(map_control_generation_error)?;
            return Ok(AdmissionSpaceTransitionV2::SameSpaceControl(
                SameSpaceControlTransitionV3 {
                    transition_format_version: SAME_SPACE_CONTROL_TRANSITION_FORMAT_V3,
                    attempt_id: input.attempt_id,
                    space_id: input.target_space_id.clone(),
                    retained_keyslot_generation: *source.keyslot_generation(),
                    profile_data_generation: *source.layout().profile_data_generation(),
                    source_control_generation: *source.layout().space_control_generation(),
                    target_control_generation,
                    prepared_database_digest: *prepared.database_digest(),
                    phase: SameSpaceControlTransitionPhaseV3::TargetPrepared,
                },
            ));
        }
        let target_keyslot_generation =
            self.generation(input.attempt_id.as_bytes(), b"target-keyslot");
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
        if let AdmissionSpaceTransitionV2::SameSpaceControl(transition) = transition {
            return self.advance_same_space(transition).await;
        }
        if let AdmissionSpaceTransitionV2::FreshControl(transition) = transition {
            return self.advance_fresh(transition).await;
        }
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
        if let AdmissionSpaceTransitionV2::FreshControl(transition) = transition {
            if transition.phase != FreshSpaceControlTransitionPhaseV3::TargetPrepared
                || !transition.validate()
            {
                return Err(AdmissionSpaceTransitionError::Inconsistent);
            }
            let target = Self::fresh_target_manifest(transition)?;
            let proof = self
                .control_generations
                .reopen_prepared(&target, &transition.prepared_database_digest)
                .await
                .map_err(map_control_generation_error)?;
            return self
                .activation
                .discard_fresh(&proof)
                .await
                .map_err(map_activation_error);
        }
        if let AdmissionSpaceTransitionV2::SameSpaceControl(transition) = transition {
            if transition.phase != SameSpaceControlTransitionPhaseV3::TargetPrepared
                || !transition.validate()
            {
                return Err(AdmissionSpaceTransitionError::Inconsistent);
            }
            let source = Self::same_space_source_manifest(transition)?;
            let target = Self::same_space_target_manifest(transition)?;
            let proof = self
                .control_generations
                .reopen_prepared(&target, &transition.prepared_database_digest)
                .await
                .map_err(map_control_generation_error)?;
            return self
                .activation
                .discard_prepared_control(&source, &proof)
                .await
                .map_err(map_activation_error);
        }
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
            .discard_prepared_control(&source, &proof)
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
