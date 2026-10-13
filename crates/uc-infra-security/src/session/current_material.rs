//! 当前 Space 的材料更新在独立快照中准备；vault I/O 不撤下已提交的会话。
use std::sync::Arc;

use uc_core::crypto::model::EncryptionError;
use uc_core::membership::SpaceKeyMaterial;

use super::{InMemorySession, State};
use crate::active_space_security_session::ActiveSpaceSecuritySessionError;
use crate::ProfileContentKeyVault;

pub(crate) struct CurrentMaterialUpdate {
    generation: Arc<()>,
    material: State,
}

impl InMemorySession {
    pub(crate) fn prepare_current_material(
        &self,
        material: &SpaceKeyMaterial,
    ) -> Result<CurrentMaterialUpdate, EncryptionError> {
        let state = self.lock_state();
        if state.closed || state.pending_material.is_some() {
            return Err(EncryptionError::NotInitialized);
        }
        let generation = state.generation.clone();
        let mut candidate = state.material.clone();
        drop(state);
        Self::apply_space_material(&mut candidate, material)?;
        Ok(CurrentMaterialUpdate {
            generation,
            material: candidate,
        })
    }

    pub(crate) fn commit_current_material(
        &self,
        update: CurrentMaterialUpdate,
        vault: &ProfileContentKeyVault,
    ) -> Result<(), ActiveSpaceSecuritySessionError> {
        let mut state = self.lock_state();
        // clear/close 或另一 Space 的激活不能被慢速 vault 写入完成后覆盖。
        if state.closed
            || state.pending_material.is_some()
            || !Arc::ptr_eq(&update.generation, &state.generation)
            || state.space_id != update.material.space_id
        {
            return Err(ActiveSpaceSecuritySessionError::Session {
                source: anyhow::Error::new(EncryptionError::NotInitialized),
            });
        }
        if state.allow_reuse && state.lease.is_none() {
            state.lease = Some(vault.begin_read_reuse().map_err(|source| {
                ActiveSpaceSecuritySessionError::Vault {
                    source: anyhow::Error::new(source),
                }
            })?);
        }
        state.material = update.material;
        drop(state);
        self.ready.notify_waiters();
        Ok(())
    }
}
