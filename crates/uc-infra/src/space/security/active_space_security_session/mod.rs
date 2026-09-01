use std::sync::Arc;

use tokio::sync::Mutex;
use uc_core::ids::SpaceId;
use uc_core::membership::SpaceKeyMaterial;

use crate::security::{MasterKey, ProfileContentKeyVault};

use super::InMemorySession;

/// 原子安装活动 Space 的持久 catalog 与进程内安全会话。
///
/// 调用方只提交已经由 repository 验证的完整 material。本模块独占
/// “先耐久安装 catalog、再切换 session”以及 session 失败恢复规则；vault
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
        if let Some(material) = material {
            self.vault
                .install_verified_space_material(material)
                .await
                .map_err(|source| ActiveSpaceSecuritySessionError::Vault {
                    source: anyhow::Error::new(source),
                })?;
        }

        let previous = self.session.snapshot();
        self.session
            .set_master_key_for_space(space_id.clone(), master_key);
        if let Some(material) = material {
            if let Err(source) = self.session.install_space_material(material) {
                self.session.restore(previous);
                return Err(ActiveSpaceSecuritySessionError::Session {
                    source: anyhow::Error::new(source),
                });
            }
        }
        Ok(())
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
    #[error("active space security session could not be installed")]
    Session {
        #[source]
        source: anyhow::Error,
    },
}

#[cfg(test)]
mod tests;
