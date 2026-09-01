mod catalog;
mod model;
mod persistence;

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;
use uc_core::membership::{ContentKeyId, GroupEpoch, SpaceKeyMaterial};
use uc_core::ports::SecureStoragePort;

pub use model::{InstalledProfileCatalog, ProfileContentKeyVaultError, ResolvedProfileContentKey};
pub(in crate::security) use persistence::VAULT_KEY_NAME as PROFILE_CONTENT_VAULT_KEY_NAME;

use persistence::VaultPersistence;

/// Profile 级历史内容密钥目录。
///
/// 调用方只负责安装一份已经验证的完整 Space material，或按不可变的
/// content-key identity 精确解析。格式升级、冲突判断、密钥保管与崩溃安全写入
/// 均由本模块内部完成。
pub struct ProfileContentKeyVault {
    persistence: VaultPersistence,
    write_lock: Mutex<()>,
}

impl ProfileContentKeyVault {
    pub fn new(
        vault_directory: PathBuf,
        secure_storage: Arc<dyn SecureStoragePort>,
        profile_generation: [u8; 16],
    ) -> Self {
        Self {
            persistence: VaultPersistence::new(vault_directory, secure_storage, profile_generation),
            write_lock: Mutex::new(()),
        }
    }

    pub async fn install_verified_space_material(
        &self,
        material: &SpaceKeyMaterial,
    ) -> Result<InstalledProfileCatalog, ProfileContentKeyVaultError> {
        let _guard = self.write_lock.lock().await;
        let group = catalog::group_from_verified_material(material)?;
        let mut vault = self
            .persistence
            .load_optional()
            .await?
            .unwrap_or_else(catalog::empty);
        if !catalog::merge(&mut vault, group)? {
            return Ok(catalog::summary(&vault, false));
        }
        vault.revision = vault
            .revision
            .checked_add(1)
            .ok_or(ProfileContentKeyVaultError::CapacityExceeded)?;
        catalog::validate(&vault)?;
        self.persistence.store(&vault).await?;
        Ok(catalog::summary(&vault, true))
    }

    pub async fn resolve(
        &self,
        content_key_id: &ContentKeyId,
        epoch: GroupEpoch,
    ) -> Result<ResolvedProfileContentKey, ProfileContentKeyVaultError> {
        let vault = self.persistence.load().await?;
        catalog::resolve(&vault, content_key_id, epoch)
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        self.persistence.path()
    }
}

#[cfg(test)]
mod tests;
