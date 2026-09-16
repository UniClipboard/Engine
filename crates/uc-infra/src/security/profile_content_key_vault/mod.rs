mod catalog;
mod model;
mod persistence;
mod read_state;

#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StateMutex};
use tokio::sync::Mutex;
use uc_core::membership::{ContentKeyId, GroupEpoch, SpaceKeyMaterial};
use uc_core::ports::SecureStoragePort;

use super::MasterKey;
pub(crate) use model::ProfileSearchCatalog;
pub use model::{InstalledProfileCatalog, ProfileContentKeyVaultError, ResolvedProfileContentKey};
use persistence::VaultPersistence;
pub(in crate::security) use persistence::VAULT_KEY_NAME as PROFILE_CONTENT_VAULT_KEY_NAME;
pub(crate) use read_state::ProfileKeyReadLease;
use read_state::{lock, InvalidateOnDrop, ReadState, ReadView};

/// Profile 历史密钥的唯一目录所有者；安装、并发读视图和清理均在模块内完成。
/// GUI 授权不参与本模块的后台安全运行期。
pub struct ProfileContentKeyVault {
    persistence: VaultPersistence,
    io_lock: Mutex<()>,
    reads: Arc<StateMutex<ReadState>>,
}

impl ProfileContentKeyVault {
    pub fn new(
        vault_directory: PathBuf,
        secure_storage: Arc<dyn SecureStoragePort>,
        profile_generation: [u8; 16],
    ) -> Self {
        Self {
            persistence: VaultPersistence::new(vault_directory, secure_storage, profile_generation),
            io_lock: Mutex::new(()),
            reads: Arc::new(StateMutex::new(ReadState::default())),
        }
    }

    pub(crate) fn begin_read_reuse(
        &self,
    ) -> Result<ProfileKeyReadLease, ProfileContentKeyVaultError> {
        ProfileKeyReadLease::begin(&self.reads)
    }

    pub(crate) fn close(&self) {
        lock(&self.reads).close();
    }

    // 租约与 generation 在同一临界区捕获；清理后重建的运行期不得
    // 接收持有旧租约的加载结果，否则可能出现有缓存却没有排他租约。
    fn operation_lease(
        &self,
    ) -> Result<(Arc<std::fs::File>, Arc<()>), ProfileContentKeyVaultError> {
        {
            let state = lock(&self.reads);
            state.check_open()?;
            if let Some(file) = &state.lease {
                return Ok((file.clone(), state.generation.clone()));
            }
        }
        let file = Arc::new(self.persistence.acquire_lease()?);
        let mut state = lock(&self.reads);
        state.check_open()?;
        if state.reusable {
            state.lease = Some(file.clone());
        }
        Ok((file, state.generation.clone()))
    }

    pub async fn install_verified_space_material(
        &self,
        material: &SpaceKeyMaterial,
    ) -> Result<InstalledProfileCatalog, ProfileContentKeyVaultError> {
        let _io = self.io_lock.lock().await;
        let (_lease, generation) = self.operation_lease()?;
        let group = catalog::group_from_verified_material(material)?;
        let mut vault = self
            .persistence
            .load_optional()
            .await?
            .map(|(vault, _)| vault)
            .unwrap_or_else(catalog::empty);
        if !catalog::merge(&mut vault, group)? {
            return Ok(catalog::summary(&vault, false));
        }
        vault.revision = vault
            .revision
            .checked_add(1)
            .ok_or(ProfileContentKeyVaultError::CapacityExceeded)?;
        catalog::validate(&vault)?;
        let invalidation = InvalidateOnDrop(&self.reads);
        let root = self.persistence.store(&vault).await?;
        let summary = catalog::summary(&vault, true);
        // 即使持久化成功，撤销期间也不重新发布材料。
        drop(invalidation);
        let mut state = lock(&self.reads);
        if !state.closed && state.reusable && Arc::ptr_eq(&generation, &state.generation) {
            state.view = Some(ReadView::new(vault, root));
        }
        Ok(summary)
    }

    async fn with_catalog<T>(
        &self,
        read: impl Fn(&ReadView) -> Result<T, ProfileContentKeyVaultError>,
    ) -> Result<T, ProfileContentKeyVaultError> {
        {
            let state = lock(&self.reads);
            state.check_open()?;
            if let Some(view) = &state.view {
                return read(view);
            }
        }
        let _io = self.io_lock.lock().await;
        let (_lease, generation) = self.operation_lease()?;
        {
            let state = lock(&self.reads);
            if let Some(view) = &state.view {
                return read(view);
            }
        }
        let (vault, root) = self.persistence.load().await?;
        let view = ReadView::new(vault, root);
        let mut state = lock(&self.reads);
        state.check_open()?;
        let result = read(&view);
        if state.reusable && Arc::ptr_eq(&generation, &state.generation) {
            state.view = Some(view);
        }
        result
    }

    pub async fn resolve(
        &self,
        content_key_id: &ContentKeyId,
        epoch: GroupEpoch,
    ) -> Result<ResolvedProfileContentKey, ProfileContentKeyVaultError> {
        self.with_catalog(|view| view.resolve(content_key_id, epoch))
            .await
    }

    /// 搜索能力与内容读取共用已认证目录；外层 key 不离开 persistence。
    pub(crate) async fn search_catalog(
        &self,
    ) -> Result<ProfileSearchCatalog, ProfileContentKeyVaultError> {
        self.with_catalog(|view| catalog::search_catalog(&view.vault, view.root.clone()))
            .await
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        self.persistence.path()
    }
}

#[cfg(test)]
mod tests;
