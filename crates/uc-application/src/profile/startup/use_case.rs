use std::sync::Arc;

use super::{
    ProfileStartupError, ProfileStartupStoragePort, ProfileUpgradeBackupError,
    ProfileUpgradeBackupPort, ProfileUpgradeVersions,
};
use crate::profile::factory_reset::{
    ProfileGeneration, ProfileLifecycle, ProfileLifecycleRepositoryPort, ProfileLifecycleState,
};

/// 唯一升级前准备负责人；调用方不接触内部备份记录，也不补排后续准备步骤。
pub struct PrepareProfileStartupUseCase {
    backup: Arc<dyn ProfileUpgradeBackupPort>,
    storage: Arc<dyn ProfileStartupStoragePort>,
    lifecycle: Arc<dyn ProfileLifecycleRepositoryPort>,
    target: ProfileUpgradeVersions,
}

impl PrepareProfileStartupUseCase {
    pub fn new(
        backup: Arc<dyn ProfileUpgradeBackupPort>,
        storage: Arc<dyn ProfileStartupStoragePort>,
        lifecycle: Arc<dyn ProfileLifecycleRepositoryPort>,
        target: ProfileUpgradeVersions,
    ) -> Self {
        Self {
            backup,
            storage,
            lifecycle,
            target,
        }
    }

    pub async fn execute(&self) -> Result<ProfileLifecycle, ProfileStartupError> {
        let backed_up = self.ensure_backup().await?;
        let existing = self.lifecycle.load()?;
        if let Some(lifecycle) = &existing {
            // 原样现场保留后，清理中的资料交回恢复出厂负责人，不导入或创建新生命周期。
            if lifecycle.state() != ProfileLifecycleState::Ready {
                return Ok(lifecycle.clone());
            }
        }
        if backed_up {
            self.backup
                .preserve_security_materials(&self.target)
                .await?;
        }
        self.storage.adopt_legacy_layout()?;
        self.storage.apply_pending_import()?;
        if let Some(lifecycle) = existing {
            return Ok(lifecycle);
        }
        let lifecycle = ProfileLifecycle::new(ProfileGeneration::new());
        self.lifecycle.compare_and_swap(None, &lifecycle)?;
        Ok(lifecycle)
    }

    async fn ensure_backup(&self) -> Result<bool, ProfileUpgradeBackupError> {
        let state = self.backup.read_source()?;
        let has_known_version = state.source_product.is_some() || state.source_engine.is_some();
        let known_versions_are_current = state
            .source_product
            .as_deref()
            .is_none_or(|version| version == self.target.product)
            && state
                .source_engine
                .as_deref()
                .is_none_or(|version| version == self.target.engine);
        if !state.has_data || (has_known_version && known_versions_are_current) {
            return Ok(false);
        }
        if self.backup.read_prepared_target()?.as_ref() == Some(&self.target) {
            self.backup.verify_prepared(&self.target).await?;
        } else {
            self.backup.capture_verified(&self.target).await?;
        }
        Ok(true)
    }
}
