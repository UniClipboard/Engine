use std::sync::Arc;

use tokio::sync::Mutex;
use uc_core::ids::SpaceId;
use uc_core::membership::{GroupEpoch, RevocationRepositoryPort, SpaceKeyMaterial};

use crate::security::{MasterKey, ProfileContentKeyVault};

use super::InMemorySession;

/// 原子安装活动 Space 的持久 catalog 与进程内安全会话。
///
/// 调用方只提交完整 material，或请求从 MasterKey 加密 repository 恢复。
/// 本模块独占 catalog/session 顺序、恢复所需的临时密钥访问和失败回滚；vault
/// 中已经成功追加但尚未被活动 session 引用的 catalog 保持安全且可幂等复用。
pub(crate) struct ActiveSpaceSecuritySession {
    session: Arc<InMemorySession>,
    vault: Arc<ProfileContentKeyVault>,
    activation_lock: Mutex<()>,
}

impl ActiveSpaceSecuritySession {
    pub(crate) fn new(session: Arc<InMemorySession>, vault: Arc<ProfileContentKeyVault>) -> Self {
        Self {
            session,
            vault,
            activation_lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    pub(crate) async fn activate(
        &self,
        space_id: &SpaceId,
        master_key: MasterKey,
        material: Option<&SpaceKeyMaterial>,
    ) -> Result<(), ActiveSpaceSecuritySessionError> {
        if material.is_some_and(|material| material.state().space_id() != space_id) {
            return Err(ActiveSpaceSecuritySessionError::InvalidMaterial {
                source: anyhow::anyhow!("active security material belongs to another space"),
            });
        }

        let _guard = self.activation_lock.lock().await;
        let transaction = self
            .session
            .begin_transaction(Some((space_id.clone(), master_key)))
            .map_err(session_error)?;
        if let Some(material) = material {
            self.vault
                .install_verified_space_material(material)
                .await
                .map_err(vault_error)?;
            self.session
                .install_space_material(material)
                .map_err(session_error)?;
        }
        transaction.commit(&self.vault)
    }

    /// 从由当前 MasterKey 加密的 repository 恢复活动 Space。
    ///
    /// repository 读取依赖共享 session，因此先在锁内临时装入目标 Space 的
    /// MasterKey；读取、材料归属校验、vault 安装或 session 安装任一步失败时，
    /// 都恢复进入本操作前的完整 session 快照。
    pub(crate) async fn restore_from_repository(
        &self,
        space_id: &SpaceId,
        master_key: MasterKey,
        repository: &dyn RevocationRepositoryPort,
    ) -> Result<Option<GroupEpoch>, ActiveSpaceSecuritySessionError> {
        let _guard = self.activation_lock.lock().await;
        let transaction = self
            .session
            .begin_transaction(Some((space_id.clone(), master_key)))
            .map_err(session_error)?;
        let material = repository
            .load_space_material(space_id)
            .await
            .map_err(|source| ActiveSpaceSecuritySessionError::Repository {
                source: anyhow::Error::new(source),
            })?;
        if let Some(material) = &material {
            if material.state().space_id() != space_id {
                return Err(ActiveSpaceSecuritySessionError::InvalidMaterial {
                    source: anyhow::anyhow!(
                        "repository security material belongs to another space"
                    ),
                });
            }
            self.vault
                .install_verified_space_material(material)
                .await
                .map_err(vault_error)?;
            self.session
                .install_space_material(material)
                .map_err(session_error)?;
        }
        transaction.commit(&self.vault)?;
        Ok(material.map(|material| material.state().epoch()))
    }

    /// 推进当前 Space 的完整安全材料。
    ///
    /// repository 已持久化的当前状态只通过这个入口安装：
    /// catalog 先进入 profile vault，成功后才更新进程内 session。
    pub(crate) async fn install_current_material(
        &self,
        material: &SpaceKeyMaterial,
    ) -> Result<(), ActiveSpaceSecuritySessionError> {
        let _guard = self.activation_lock.lock().await;
        let transaction = self
            .session
            .begin_transaction(None)
            .map_err(session_error)?;
        let current_space_id = self.session.current_space_id().map_err(session_error)?;
        if material.state().space_id() != &current_space_id {
            return Err(ActiveSpaceSecuritySessionError::InvalidMaterial {
                source: anyhow::anyhow!("security material does not belong to the active space"),
            });
        }
        self.vault
            .install_verified_space_material(material)
            .await
            .map_err(vault_error)?;
        self.session
            .install_space_material(material)
            .map_err(session_error)?;
        transaction.commit(&self.vault)
    }

    pub(crate) fn close(&self) {
        self.session.close();
        self.vault.close();
    }
}

fn session_error(
    source: uc_core::crypto::model::EncryptionError,
) -> ActiveSpaceSecuritySessionError {
    ActiveSpaceSecuritySessionError::Session {
        source: anyhow::Error::new(source),
    }
}
fn vault_error(
    source: crate::security::ProfileContentKeyVaultError,
) -> ActiveSpaceSecuritySessionError {
    ActiveSpaceSecuritySessionError::Vault {
        source: anyhow::Error::new(source),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ActiveSpaceSecuritySessionError {
    #[error("active space security material is invalid")]
    InvalidMaterial {
        #[source]
        source: anyhow::Error,
    },
    #[error("active space content key catalog could not be installed")]
    Vault {
        #[source]
        source: anyhow::Error,
    },
    #[error("active space security material could not be loaded")]
    Repository {
        #[source]
        source: anyhow::Error,
    },
    #[error("active space security session could not be installed")]
    Session {
        #[source]
        source: anyhow::Error,
    },
}

#[cfg(test)]
mod tests;
