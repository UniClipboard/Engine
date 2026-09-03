use std::fs::{File, OpenOptions, TryLockError};
use std::path::PathBuf;
use std::sync::Arc;

use super::active_space_generation_manifest_store::V3ManifestPromotionOutcome;
use super::{
    ActiveRuntimeManifest, ActiveRuntimeManifestV3, ActiveSpaceGenerationManifestStore,
    ActiveSpaceGenerationManifestStoreError, PreparedSpaceControlGeneration, ProfileRuntimeLayout,
    SpaceControlGeneration,
};
use crate::db::pool::DbPool;
use crate::space::DefaultSpaceAccessAdapter;

/// V3 control-generation 的唯一 manifest 生效与进程内重绑入口。
///
/// 线性化点只有 active manifest 替换；替换后发生的 pool/keyslot/session 故障
/// 只能向同一 target 前向恢复，不能回滚到 source。profile data generation 从输入
/// 到结果始终不变，也没有 payload store 依赖。
pub struct SpaceTransitionActivation {
    profile_root: PathBuf,
    control_pool: DbPool,
    manifests: Arc<ActiveSpaceGenerationManifestStore>,
    control_generations: Arc<SpaceControlGeneration>,
    space_access: Arc<DefaultSpaceAccessAdapter>,
    activation_lock: tokio::sync::Mutex<()>,
}

impl SpaceTransitionActivation {
    pub fn new(
        profile_root: PathBuf,
        control_pool: DbPool,
        manifests: Arc<ActiveSpaceGenerationManifestStore>,
        control_generations: Arc<SpaceControlGeneration>,
        space_access: Arc<DefaultSpaceAccessAdapter>,
    ) -> Self {
        Self {
            profile_root,
            control_pool,
            manifests,
            control_generations,
            space_access,
            activation_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn activate_cross_space(
        &self,
        expected_source: &ActiveRuntimeManifestV3,
        prepared: &PreparedSpaceControlGeneration,
        target_access_state: &[u8],
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        let target = prepared.manifest();
        if target_access_state.is_empty()
            || expected_source.layout().space_id() == target.layout().space_id()
            || expected_source.layout().profile_data_generation()
                != target.layout().profile_data_generation()
            || expected_source.layout().space_control_generation()
                == target.layout().space_control_generation()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "cross-space control activation input is inconsistent"
            )));
        }

        // 在 manifest 线性化点前重新认证介质，避免消费陈旧或伪造的 proof。
        self.control_generations
            .reopen_prepared(target, prepared.database_digest())
            .await
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;

        let promotion = self
            .manifests
            .promote_v3_control_generation(expected_source, target)
            .await
            .map_err(map_manifest_error)?;
        if promotion == V3ManifestPromotionOutcome::SourceChanged {
            return Err(inconsistent(anyhow::anyhow!(
                "active runtime manifest changed before control activation"
            )));
        }

        self.rebind_target(target, target_access_state).await?;
        Ok(match promotion {
            V3ManifestPromotionOutcome::Promoted => SpaceTransitionActivationOutcome::Promoted,
            V3ManifestPromotionOutcome::AlreadyActive => {
                SpaceTransitionActivationOutcome::Recovered
            }
            V3ManifestPromotionOutcome::SourceChanged => {
                return Err(inconsistent(anyhow::anyhow!(
                    "active runtime manifest changed during control activation"
                )))
            }
        })
    }

    /// manifest 已指向 target 后，只向前恢复 control pool、keyslot 与 session。
    ///
    /// prepared generation 在 promotion 后会成为可写运行库，原始介质摘要不再稳定，
    /// 因此恢复只能以已认证 active manifest 为事实，不能重新伪装成 pre-activation proof。
    pub async fn recover_cross_space(
        &self,
        target: &ActiveRuntimeManifestV3,
        target_access_state: &[u8],
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        if target_access_state.is_empty() {
            return Err(inconsistent(anyhow::anyhow!(
                "cross-space recovery access state is missing"
            )));
        }
        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        if active.as_ref() != Some(&ActiveRuntimeManifest::V3(target.clone())) {
            return Err(inconsistent(anyhow::anyhow!(
                "cross-space recovery target is not active"
            )));
        }
        self.rebind_target(target, target_access_state).await?;
        Ok(SpaceTransitionActivationOutcome::Recovered)
    }

    pub async fn activate_same_space(
        &self,
        expected_source: &ActiveRuntimeManifestV3,
        prepared: &PreparedSpaceControlGeneration,
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        self.activate_same_space_retained_control(expected_source, prepared)
            .await
    }

    pub async fn activate_membership_branch(
        &self,
        expected_source: &ActiveRuntimeManifestV3,
        prepared: &PreparedSpaceControlGeneration,
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        self.activate_same_space_retained_control(expected_source, prepared)
            .await
    }

    async fn activate_same_space_retained_control(
        &self,
        expected_source: &ActiveRuntimeManifestV3,
        prepared: &PreparedSpaceControlGeneration,
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        let target = prepared.manifest();
        if expected_source.layout().space_id() != target.layout().space_id()
            || expected_source.keyslot_generation() != target.keyslot_generation()
            || expected_source.layout().profile_data_generation()
                != target.layout().profile_data_generation()
            || expected_source.layout().space_control_generation()
                == target.layout().space_control_generation()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "same-space retained control activation input is inconsistent"
            )));
        }
        self.control_generations
            .reopen_prepared(target, prepared.database_digest())
            .await
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let promotion = self
            .manifests
            .promote_v3_control_generation(expected_source, target)
            .await
            .map_err(map_manifest_error)?;
        if promotion == V3ManifestPromotionOutcome::SourceChanged {
            return Err(inconsistent(anyhow::anyhow!(
                "active runtime manifest changed before retained control activation"
            )));
        }
        self.rebind_retained_target(target).await?;
        Ok(match promotion {
            V3ManifestPromotionOutcome::Promoted => SpaceTransitionActivationOutcome::Promoted,
            V3ManifestPromotionOutcome::AlreadyActive => {
                SpaceTransitionActivationOutcome::Recovered
            }
            V3ManifestPromotionOutcome::SourceChanged => {
                return Err(inconsistent(anyhow::anyhow!(
                    "active runtime manifest changed during retained control activation"
                )))
            }
        })
    }

    pub async fn activate_fresh(
        &self,
        prepared: &PreparedSpaceControlGeneration,
        target_access_state: &[u8],
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        let target = prepared.manifest();
        if target_access_state.is_empty() {
            return Err(inconsistent(anyhow::anyhow!(
                "fresh control activation access state is missing"
            )));
        }
        self.validate_fresh_profile_layout(target)?;
        self.control_generations
            .reopen_prepared(target, prepared.database_digest())
            .await
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let promotion = self
            .manifests
            .promote_initial_v3(target)
            .await
            .map_err(map_manifest_error)?;
        if promotion == V3ManifestPromotionOutcome::SourceChanged {
            return Err(inconsistent(anyhow::anyhow!(
                "an active runtime appeared before fresh activation"
            )));
        }
        self.rebind_target(target, target_access_state).await?;
        Ok(match promotion {
            V3ManifestPromotionOutcome::Promoted => SpaceTransitionActivationOutcome::Promoted,
            V3ManifestPromotionOutcome::AlreadyActive => {
                SpaceTransitionActivationOutcome::Recovered
            }
            V3ManifestPromotionOutcome::SourceChanged => {
                return Err(inconsistent(anyhow::anyhow!(
                    "active runtime changed during fresh activation"
                )))
            }
        })
    }

    /// Fresh manifest 已写入后的运行期恢复只接受同一 target。
    pub async fn recover_fresh(
        &self,
        target: &ActiveRuntimeManifestV3,
        target_access_state: &[u8],
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        if target_access_state.is_empty() {
            return Err(inconsistent(anyhow::anyhow!(
                "fresh recovery access state is missing"
            )));
        }
        self.validate_fresh_profile_layout(target)?;
        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        if active.as_ref() != Some(&ActiveRuntimeManifest::V3(target.clone())) {
            return Err(inconsistent(anyhow::anyhow!(
                "fresh recovery target is not active"
            )));
        }
        self.rebind_target(target, target_access_state).await?;
        Ok(SpaceTransitionActivationOutcome::Recovered)
    }

    /// SameSpace manifest 已指向 target 后，只向前恢复 control pool 与安全 session。
    pub async fn recover_same_space(
        &self,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        self.recover_retained_control(target).await
    }

    pub async fn recover_membership_branch(
        &self,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        self.recover_retained_control(target).await
    }

    async fn recover_retained_control(
        &self,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        if active.as_ref() != Some(&ActiveRuntimeManifest::V3(target.clone())) {
            return Err(inconsistent(anyhow::anyhow!(
                "retained control recovery target is not active"
            )));
        }
        self.rebind_retained_target(target).await?;
        Ok(SpaceTransitionActivationOutcome::Recovered)
    }

    /// Device Reset 保留 profile data、MasterKey 与 keyslot，但更换 Space 和
    /// control generation。manifest 是唯一线性化点；之后只向 target 前向恢复。
    pub async fn activate_device_reset(
        &self,
        expected_source: &ActiveRuntimeManifestV3,
        prepared: &PreparedSpaceControlGeneration,
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        let target = prepared.manifest();
        if expected_source.layout().space_id() == target.layout().space_id()
            || expected_source.keyslot_generation() != target.keyslot_generation()
            || expected_source.layout().profile_data_generation()
                != target.layout().profile_data_generation()
            || expected_source.layout().space_control_generation()
                == target.layout().space_control_generation()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "device reset control activation input is inconsistent"
            )));
        }
        self.control_generations
            .reopen_prepared(target, prepared.database_digest())
            .await
            .map_err(|source| inconsistent(anyhow::Error::new(source)))?;
        let promotion = self
            .manifests
            .promote_v3_control_generation(expected_source, target)
            .await
            .map_err(map_manifest_error)?;
        if promotion == V3ManifestPromotionOutcome::SourceChanged {
            return Err(inconsistent(anyhow::anyhow!(
                "active runtime changed before device reset activation"
            )));
        }
        self.rebind_retained_target(target).await?;
        Ok(match promotion {
            V3ManifestPromotionOutcome::Promoted => SpaceTransitionActivationOutcome::Promoted,
            V3ManifestPromotionOutcome::AlreadyActive => {
                SpaceTransitionActivationOutcome::Recovered
            }
            V3ManifestPromotionOutcome::SourceChanged => {
                return Err(inconsistent(anyhow::anyhow!(
                    "active runtime changed during device reset activation"
                )))
            }
        })
    }

    pub async fn recover_device_reset(
        &self,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<SpaceTransitionActivationOutcome, SpaceTransitionActivationError> {
        self.recover_retained_control(target).await
    }

    pub async fn discard_prepared_control(
        &self,
        expected_source: &ActiveRuntimeManifestV3,
        prepared: &PreparedSpaceControlGeneration,
    ) -> Result<(), SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        if active.as_ref() != Some(&ActiveRuntimeManifest::V3(expected_source.clone())) {
            return Err(inconsistent(anyhow::anyhow!(
                "prepared control generation is no longer pre-activation"
            )));
        }
        self.control_generations
            .discard_prepared(prepared)
            .map_err(|source| storage(anyhow::Error::new(source)))
    }

    pub async fn discard_fresh(
        &self,
        prepared: &PreparedSpaceControlGeneration,
    ) -> Result<(), SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        if self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?
            .is_some()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "fresh control generation is no longer pre-activation"
            )));
        }
        self.control_generations
            .discard_prepared(prepared)
            .map_err(|source| storage(anyhow::Error::new(source)))
    }

    async fn rebind_target(
        &self,
        target: &ActiveRuntimeManifestV3,
        target_access_state: &[u8],
    ) -> Result<(), SpaceTransitionActivationError> {
        let layout = ProfileRuntimeLayout::v3(&self.profile_root, target);
        let database = layout
            .control_database()
            .to_str()
            .ok_or_else(|| recovery(anyhow::anyhow!("control database path is invalid")))?;
        self.control_pool
            .replace_database(database)
            .map_err(recovery)?;
        self.space_access
            .activate_prepared_control_generation(target.layout().space_id(), target_access_state)
            .await
            .map_err(|source| recovery(anyhow::Error::new(source)))?;

        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        if active.as_ref() != Some(&ActiveRuntimeManifest::V3(target.clone())) {
            return Err(recovery(anyhow::anyhow!(
                "target runtime manifest is not active after rebind"
            )));
        }
        Ok(())
    }

    async fn rebind_retained_target(
        &self,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<(), SpaceTransitionActivationError> {
        let layout = ProfileRuntimeLayout::v3(&self.profile_root, target);
        let database = layout
            .control_database()
            .to_str()
            .ok_or_else(|| recovery(anyhow::anyhow!("control database path is invalid")))?;
        self.control_pool
            .replace_database(database)
            .map_err(recovery)?;
        self.space_access
            .activate_retained_control_generation(target.layout().space_id())
            .await
            .map_err(|source| recovery(anyhow::Error::new(source)))?;
        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        if active.as_ref() != Some(&ActiveRuntimeManifest::V3(target.clone())) {
            return Err(recovery(anyhow::anyhow!(
                "retained control target is not active after rebind"
            )));
        }
        Ok(())
    }

    fn validate_fresh_profile_layout(
        &self,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<(), SpaceTransitionActivationError> {
        let layout = ProfileRuntimeLayout::v3(&self.profile_root, target);
        if !layout.profile_database().is_file() || !layout.blob_root().is_dir() {
            return Err(inconsistent(anyhow::anyhow!(
                "fresh profile data generation is not prepared"
            )));
        }
        Ok(())
    }

    pub async fn cleanup_source_control_generation(
        &self,
        expected_source: &ActiveRuntimeManifestV3,
        target: &ActiveRuntimeManifestV3,
    ) -> Result<(), SpaceTransitionActivationError> {
        let _guard = self.activation_lock.lock().await;
        let _lease = acquire_activation_lease(&self.profile_root)?;
        let active = self
            .manifests
            .load_runtime()
            .await
            .map_err(map_manifest_error)?;
        if active.as_ref() != Some(&ActiveRuntimeManifest::V3(target.clone()))
            || expected_source.layout().profile_data_generation()
                != target.layout().profile_data_generation()
        {
            return Err(inconsistent(anyhow::anyhow!(
                "source control cleanup does not match the active target"
            )));
        }
        let source = ProfileRuntimeLayout::v3(&self.profile_root, expected_source);
        let target_layout = ProfileRuntimeLayout::v3(&self.profile_root, target);
        let source_directory = source.control_database().parent().ok_or_else(|| {
            storage(anyhow::anyhow!(
                "source control generation directory is missing"
            ))
        })?;
        let target_directory = target_layout.control_database().parent().ok_or_else(|| {
            storage(anyhow::anyhow!(
                "target control generation directory is missing"
            ))
        })?;
        if source_directory == target_directory {
            return Err(inconsistent(anyhow::anyhow!(
                "source and target control generations are identical"
            )));
        }
        match std::fs::remove_dir_all(source_directory) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(storage(anyhow::Error::new(source))),
        }
        let parent = source_directory
            .parent()
            .ok_or_else(|| storage(anyhow::anyhow!("control generation parent is missing")))?;
        sync_directory(parent).map_err(|source| storage(anyhow::Error::new(source)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceTransitionActivationOutcome {
    Promoted,
    Recovered,
}

#[derive(Debug, thiserror::Error)]
pub enum SpaceTransitionActivationError {
    #[error("space transition activation is busy")]
    Busy {
        #[source]
        source: anyhow::Error,
    },
    #[error("space transition activation input is inconsistent")]
    Inconsistent {
        #[source]
        source: anyhow::Error,
    },
    #[error("space transition activation storage is unavailable")]
    Storage {
        #[source]
        source: anyhow::Error,
    },
    #[error("space transition activation requires forward recovery")]
    Recovery {
        #[source]
        source: anyhow::Error,
    },
}

struct ActivationLease {
    _file: File,
}

fn acquire_activation_lease(
    profile_root: &std::path::Path,
) -> Result<ActivationLease, SpaceTransitionActivationError> {
    std::fs::create_dir_all(profile_root).map_err(|source| storage(anyhow::Error::new(source)))?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(profile_root.join(".space-transition-activation.lease"))
        .map_err(|source| storage(anyhow::Error::new(source)))?;
    match file.try_lock() {
        Ok(()) => Ok(ActivationLease { _file: file }),
        Err(TryLockError::WouldBlock) => Err(SpaceTransitionActivationError::Busy {
            source: anyhow::anyhow!("space transition activation lease is held"),
        }),
        Err(TryLockError::Error(source)) => Err(storage(anyhow::Error::new(source))),
    }
}

fn map_manifest_error(
    source: ActiveSpaceGenerationManifestStoreError,
) -> SpaceTransitionActivationError {
    match source {
        ActiveSpaceGenerationManifestStoreError::Storage => storage(anyhow::Error::new(source)),
        ActiveSpaceGenerationManifestStoreError::Corrupt
        | ActiveSpaceGenerationManifestStoreError::UnsupportedVersion => {
            inconsistent(anyhow::Error::new(source))
        }
    }
}

fn inconsistent(source: anyhow::Error) -> SpaceTransitionActivationError {
    SpaceTransitionActivationError::Inconsistent { source }
}

fn storage(source: anyhow::Error) -> SpaceTransitionActivationError {
    SpaceTransitionActivationError::Storage { source }
}

fn recovery(source: anyhow::Error) -> SpaceTransitionActivationError {
    SpaceTransitionActivationError::Recovery { source }
}

fn sync_directory(directory: &std::path::Path) -> std::io::Result<()> {
    #[cfg(not(windows))]
    {
        std::fs::File::open(directory)?.sync_all()?;
    }
    Ok(())
}
