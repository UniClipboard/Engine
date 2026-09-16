use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use rand::RngCore as _;
use uc_application::deps::{AdmissionSpaceTransitionError, DeviceManagementResetDataPort};
use uc_core::ids::SpaceId;
use uc_core::membership::ActiveRuntimeLayout;

use super::active_space_generation_manifest_store::{
    DeviceManagementResetJournalV3, DeviceManagementResetPhaseV3,
};
use super::{
    ActiveRuntimeManifest, ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    ActiveSpaceGenerationManifestStoreError, ProfileRuntimeLayout, SpaceControlGeneration,
    SpaceControlGenerationError, SpaceTransitionActivation, SpaceTransitionActivationError,
};
use crate::db::pool::DbPool;

/// Device Reset 的完整 V3 control-generation adapter。
///
/// Application 仍只执行既有四步动作；本模块独占 target 分配、control snapshot、
/// mutable staging、promotion proof、manifest 前向恢复和 source cleanup。profile
/// database/blob 与 payload cipher 不进入依赖图。
pub struct V3DeviceManagementReset {
    profile_root: PathBuf,
    control_pool: DbPool,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    control_generations: Arc<SpaceControlGeneration>,
    activation: Arc<SpaceTransitionActivation>,
    operation_lock: tokio::sync::Mutex<()>,
}

impl V3DeviceManagementReset {
    pub fn new(
        profile_root: PathBuf,
        control_pool: DbPool,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        control_generations: Arc<SpaceControlGeneration>,
        activation: Arc<SpaceTransitionActivation>,
    ) -> Self {
        Self {
            profile_root,
            control_pool,
            manifests,
            control_generations,
            activation,
            operation_lock: tokio::sync::Mutex::new(()),
        }
    }

    fn target_manifest(
        journal: &DeviceManagementResetJournalV3,
    ) -> Result<ActiveRuntimeManifestV3, AdmissionSpaceTransitionError> {
        let layout = ActiveRuntimeLayout::new(
            SpaceId::from_string(journal.target_space_id.clone()),
            journal.profile_data_generation,
            journal.target_control_generation,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        ActiveRuntimeManifestV3::new(layout, journal.source_keyslot_generation)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    fn source_manifest(
        journal: &DeviceManagementResetJournalV3,
    ) -> Result<ActiveRuntimeManifestV3, AdmissionSpaceTransitionError> {
        let layout = ActiveRuntimeLayout::new(
            SpaceId::from_string(journal.source_space_id.clone()),
            journal.profile_data_generation,
            journal.source_control_generation,
        )
        .map_err(|_| AdmissionSpaceTransitionError::Inconsistent)?;
        ActiveRuntimeManifestV3::new(layout, journal.source_keyslot_generation)
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)
    }

    fn allocate_target_generation(source: &ActiveRuntimeManifestV3) -> [u8; 16] {
        loop {
            let mut generation = [0u8; 16];
            rand::rng().fill_bytes(&mut generation);
            if generation != [0; 16]
                && &generation != source.layout().profile_data_generation()
                && &generation != source.layout().space_control_generation()
            {
                return generation;
            }
        }
    }

    fn journal_matches(
        journal: &DeviceManagementResetJournalV3,
        source: &ActiveRuntimeManifestV3,
        target_space: &SpaceId,
    ) -> bool {
        journal.validate()
            && journal.source_space_id == source.layout().space_id().as_ref()
            && journal.source_keyslot_generation == *source.keyslot_generation()
            && journal.profile_data_generation == *source.layout().profile_data_generation()
            && journal.source_control_generation == *source.layout().space_control_generation()
            && journal.target_space_id == target_space.as_ref()
    }

    async fn active_runtime(
        &self,
    ) -> Result<Option<ActiveRuntimeManifest>, AdmissionSpaceTransitionError> {
        self.manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)
    }

    async fn journal(
        &self,
    ) -> Result<Option<DeviceManagementResetJournalV3>, AdmissionSpaceTransitionError> {
        self.manifests
            .load_device_reset_journal_v3()
            .await
            .map_err(map_manifest_error)
    }

    async fn save_journal(
        &self,
        journal: &DeviceManagementResetJournalV3,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        self.manifests
            .save_device_reset_journal_v3(journal)
            .await
            .map_err(map_manifest_error)
    }
}

#[async_trait]
impl DeviceManagementResetDataPort for V3DeviceManagementReset {
    async fn prepare_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let _guard = self.operation_lock.lock().await;
        let active = self
            .active_runtime()
            .await?
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        let ActiveRuntimeManifest::V3(source) = active else {
            return Err(AdmissionSpaceTransitionError::Unavailable);
        };
        if source.layout().space_id() == target_space_id {
            return Ok(());
        }
        let mut journal = match self.journal().await? {
            Some(journal) if Self::journal_matches(&journal, &source, target_space_id) => journal,
            Some(_) => return Err(AdmissionSpaceTransitionError::Inconsistent),
            None => {
                let journal = DeviceManagementResetJournalV3 {
                    format_version: 3,
                    phase: DeviceManagementResetPhaseV3::Allocated,
                    source_space_id: source.layout().space_id().as_ref().to_owned(),
                    source_keyslot_generation: *source.keyslot_generation(),
                    profile_data_generation: *source.layout().profile_data_generation(),
                    source_control_generation: *source.layout().space_control_generation(),
                    target_space_id: target_space_id.as_ref().to_owned(),
                    target_control_generation: Self::allocate_target_generation(&source),
                    prepared_database_digest: [0; 32],
                };
                self.save_journal(&journal).await?;
                journal
            }
        };
        match journal.phase {
            DeviceManagementResetPhaseV3::Allocated => {
                let target = Self::target_manifest(&journal)?;
                let prepared = self
                    .control_generations
                    .prepare_device_reset_snapshot(&source, &target, &self.control_pool)
                    .await
                    .map_err(map_generation_error)?;
                journal.prepared_database_digest = *prepared.database_digest();
                journal.phase = DeviceManagementResetPhaseV3::Prepared;
                self.save_journal(&journal).await
            }
            DeviceManagementResetPhaseV3::Prepared => {
                let target = Self::target_manifest(&journal)?;
                self.control_generations
                    .reopen_prepared(&target, &journal.prepared_database_digest)
                    .await
                    .map_err(map_generation_error)?;
                Ok(())
            }
            DeviceManagementResetPhaseV3::Staged => Ok(()),
            DeviceManagementResetPhaseV3::Promoted
            | DeviceManagementResetPhaseV3::CleanupPending => {
                Err(AdmissionSpaceTransitionError::Inconsistent)
            }
        }
    }

    async fn stage_device_management_reset_mutations(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let _guard = self.operation_lock.lock().await;
        if self.active_runtime().await?.is_some_and(|active| {
            matches!(active, ActiveRuntimeManifest::V3(manifest) if manifest.layout().space_id() == target_space_id)
        }) {
            return Ok(());
        }
        let mut journal = self
            .journal()
            .await?
            .ok_or(AdmissionSpaceTransitionError::Inconsistent)?;
        if journal.target_space_id != target_space_id.as_ref() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let target = Self::target_manifest(&journal)?;
        match journal.phase {
            DeviceManagementResetPhaseV3::Prepared => {
                self.control_generations
                    .reopen_prepared(&target, &journal.prepared_database_digest)
                    .await
                    .map_err(map_generation_error)?;
                let database = ProfileRuntimeLayout::v3(&self.profile_root, &target);
                self.control_pool
                    .replace_database(
                        database
                            .control_database()
                            .to_str()
                            .ok_or(AdmissionSpaceTransitionError::Storage)?,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)?;
                journal.phase = DeviceManagementResetPhaseV3::Staged;
                self.save_journal(&journal).await
            }
            DeviceManagementResetPhaseV3::Staged => {
                let database = ProfileRuntimeLayout::v3(&self.profile_root, &target);
                self.control_pool
                    .replace_database(
                        database
                            .control_database()
                            .to_str()
                            .ok_or(AdmissionSpaceTransitionError::Storage)?,
                    )
                    .map_err(|_| AdmissionSpaceTransitionError::RecoveryRequired)
            }
            DeviceManagementResetPhaseV3::Allocated
            | DeviceManagementResetPhaseV3::Promoted
            | DeviceManagementResetPhaseV3::CleanupPending => {
                Err(AdmissionSpaceTransitionError::Inconsistent)
            }
        }
    }

    async fn promote_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let _guard = self.operation_lock.lock().await;
        let mut journal = match self.journal().await? {
            Some(journal) => journal,
            None => {
                return self
                    .active_runtime()
                    .await?
                    .is_some_and(|active| {
                        matches!(active, ActiveRuntimeManifest::V3(manifest) if manifest.layout().space_id() == target_space_id)
                    })
                    .then_some(())
                    .ok_or(AdmissionSpaceTransitionError::Inconsistent);
            }
        };
        if journal.target_space_id != target_space_id.as_ref() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let source = Self::source_manifest(&journal)?;
        let target = Self::target_manifest(&journal)?;
        match self.active_runtime().await? {
            Some(ActiveRuntimeManifest::V3(active)) if active == target => {
                self.activation
                    .recover_device_reset(&target)
                    .await
                    .map_err(map_activation_error)?;
                journal.phase = DeviceManagementResetPhaseV3::Promoted;
                self.save_journal(&journal).await
            }
            Some(ActiveRuntimeManifest::V3(active)) if active == source => {
                if journal.phase != DeviceManagementResetPhaseV3::Staged {
                    return Err(AdmissionSpaceTransitionError::Inconsistent);
                }
                let prepared = self
                    .control_generations
                    .finalize_device_reset_target(&source, &target, &self.control_pool)
                    .await
                    .map_err(map_generation_error)?;
                self.activation
                    .activate_device_reset(&source, &prepared)
                    .await
                    .map_err(map_activation_error)?;
                journal.phase = DeviceManagementResetPhaseV3::Promoted;
                self.save_journal(&journal).await
            }
            _ => Err(AdmissionSpaceTransitionError::Inconsistent),
        }
    }

    async fn finalize_device_management_reset(
        &self,
        target_space_id: &SpaceId,
    ) -> Result<(), AdmissionSpaceTransitionError> {
        let _guard = self.operation_lock.lock().await;
        let Some(mut journal) = self.journal().await? else {
            return self
                .active_runtime()
                .await?
                .is_some_and(|active| {
                    matches!(active, ActiveRuntimeManifest::V3(manifest) if manifest.layout().space_id() == target_space_id)
                })
                .then_some(())
                .ok_or(AdmissionSpaceTransitionError::Inconsistent);
        };
        if journal.target_space_id != target_space_id.as_ref() {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        let source = Self::source_manifest(&journal)?;
        let target = Self::target_manifest(&journal)?;
        if self.active_runtime().await? != Some(ActiveRuntimeManifest::V3(target.clone())) {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        if journal.phase == DeviceManagementResetPhaseV3::Promoted {
            journal.phase = DeviceManagementResetPhaseV3::CleanupPending;
            self.save_journal(&journal).await?;
        }
        if journal.phase != DeviceManagementResetPhaseV3::CleanupPending {
            return Err(AdmissionSpaceTransitionError::Inconsistent);
        }
        self.activation
            .cleanup_source_control_generation(&source, &target)
            .await
            .map_err(map_activation_error)?;
        self.manifests
            .clear_device_reset_journal()
            .await
            .map_err(map_manifest_error)
    }
}

fn map_manifest_error(
    error: ActiveSpaceGenerationManifestStoreError,
) -> AdmissionSpaceTransitionError {
    match error {
        ActiveSpaceGenerationManifestStoreError::Storage => AdmissionSpaceTransitionError::Storage,
        ActiveSpaceGenerationManifestStoreError::Corrupt
        | ActiveSpaceGenerationManifestStoreError::UnsupportedVersion => {
            AdmissionSpaceTransitionError::Inconsistent
        }
    }
}

fn map_generation_error(error: SpaceControlGenerationError) -> AdmissionSpaceTransitionError {
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
