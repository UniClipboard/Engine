//! 移除恢复适配器:把有效成员集合落实为新的统一安全状态。
//!
//! 意图集合决定有效成员集合,本适配器只负责把该集合落实为 OpenMLS 群组
//! 状态与新一轮内容密钥目录(ADR-015"OpenMLS 是安全状态投影")。执行者
//! 从自己的分叉成员集合生成恢复资料;其他有效成员应用完全匹配目标集合与
//! 收敛摘要的资料。本机生成恢复资料或应用恢复资料后立即持久化并安装新
//! 状态,绝不恢复旧密钥或重新加入已移除成员。

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{info, warn};

use super::space_access_adapter::PortableKeyCatalog;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    CurrentMembershipIdentityPort, GroupEpoch, MemberInstanceId, RemovalCausalProof,
    RemovalCausalProofMember, RemovalPendingJoinStorePort, RemovalPreparedRecovery,
    RemovalRecoveryError, RemovalRecoveryMaterial, RemovalRecoveryPort, RemovalViewMember,
    RemovalViewSnapshot, RevocationRepositoryPort, SpaceKeyMaterial,
};

use super::mls_group::{MlsClientState, MlsGroupEngine, PendingMlsJoin};
use super::session::InMemorySession;
use super::space_access_adapter::{open_group_catalog, seal_group_catalog};

pub struct RemovalRecoveryAdapter {
    session: InMemorySession,
    key_epoch_repository: Arc<dyn RevocationRepositoryPort>,
    current_identity: Arc<dyn CurrentMembershipIdentityPort>,
    pending_join: Arc<dyn RemovalPendingJoinStorePort>,
    pending_join_gate: Arc<Mutex<()>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PendingRemovalJoin {
    key_package: Vec<u8>,
    client_state: Vec<u8>,
}

impl RemovalRecoveryAdapter {
    pub fn new(
        session: InMemorySession,
        key_epoch_repository: Arc<dyn RevocationRepositoryPort>,
        current_identity: Arc<dyn CurrentMembershipIdentityPort>,
        pending_join: Arc<dyn RemovalPendingJoinStorePort>,
    ) -> Self {
        Self {
            session,
            key_epoch_repository,
            current_identity,
            pending_join,
            pending_join_gate: Arc::new(Mutex::new(())),
        }
    }

    fn recovery_error(error: impl std::fmt::Display) -> RemovalRecoveryError {
        RemovalRecoveryError::Repository(error.to_string())
    }

    fn current_space_id(&self) -> Result<SpaceId, RemovalRecoveryError> {
        self.session
            .current_space_id()
            .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))
    }

    async fn load_material(
        &self,
        space_id: &SpaceId,
    ) -> Result<SpaceKeyMaterial, RemovalRecoveryError> {
        self.key_epoch_repository
            .load_space_material(space_id)
            .await
            .map_err(Self::recovery_error)?
            .ok_or_else(|| {
                RemovalRecoveryError::Repository("space key material unavailable".into())
            })
    }

    async fn own_device_id(&self) -> Result<DeviceId, RemovalRecoveryError> {
        self.current_identity
            .current_membership_identity()
            .await
            .map(|identity| identity.device_id)
            .map_err(|_| RemovalRecoveryError::Unavailable)
    }
}

#[async_trait]
impl RemovalRecoveryPort for RemovalRecoveryAdapter {
    async fn current_view(&self) -> Result<RemovalViewSnapshot, RemovalRecoveryError> {
        let space_id = self.current_space_id()?;
        let material = self.load_material(&space_id).await?;
        if material.group_state().is_empty() {
            return Err(RemovalRecoveryError::Unavailable);
        }
        let state = MlsClientState::from_bytes(material.group_state().to_vec());
        let epoch = MlsGroupEngine::current_epoch(&state).map_err(Self::recovery_error)?;
        let identities = MlsGroupEngine::view_members(&state).map_err(Self::recovery_error)?;
        let members = identities
            .into_iter()
            .filter_map(|identity| {
                let device_id =
                    DeviceId::try_new(String::from_utf8_lossy(&identity.device_identity))?;
                let instance =
                    MemberInstanceId::derive(device_id.as_str(), &identity.signature_key);
                Some(RemovalViewMember {
                    device_id,
                    instance,
                    signing_public_key: identity.signature_key,
                })
            })
            .collect::<Vec<_>>();
        let causal_proof = RemovalCausalProof::new(
            epoch,
            members
                .iter()
                .map(|member| RemovalCausalProofMember {
                    device_id: member.device_id.clone(),
                    instance: member.instance,
                    signing_public_key: member.signing_public_key.clone(),
                })
                .collect(),
        );
        Ok(RemovalViewSnapshot {
            epoch,
            members,
            causal_proof,
        })
    }

    async fn own_instance(&self) -> Result<Option<MemberInstanceId>, RemovalRecoveryError> {
        let own_device_id = self.own_device_id().await?;
        let view = self.current_view().await?;
        Ok(view
            .members
            .into_iter()
            .find(|member| member.device_id == own_device_id)
            .map(|member| member.instance))
    }

    async fn prepare_key_package(&self) -> Result<Vec<u8>, RemovalRecoveryError> {
        // 同一摘要的重复网络请求必须复用同一份 key package。若覆盖已保存的
        // 私有状态，执行者持有的旧 key package 将再也无法打开之后的 welcome。
        let _pending_join_guard = self.pending_join_gate.lock().await;
        let space_id = self.current_space_id()?;
        match self.pending_join.load(space_id.as_ref()).await {
            Ok(Some(saved)) => {
                let pending: PendingRemovalJoin = postcard::from_bytes(&saved)
                    .map_err(|_| RemovalRecoveryError::InvalidMaterial)?;
                return Ok(pending.key_package);
            }
            Ok(None) => {}
            Err(error) => return Err(RemovalRecoveryError::Repository(error.to_string())),
        }
        let own_device_id = self.own_device_id().await?;
        let pending = MlsGroupEngine::prepare_join(own_device_id.as_str().as_bytes())
            .map_err(Self::recovery_error)?;
        let stored = postcard::to_stdvec(&PendingRemovalJoin {
            key_package: pending.key_package.clone(),
            client_state: pending.client_state.into_bytes(),
        })
        .map_err(|_| RemovalRecoveryError::InvalidMaterial)?;
        self.pending_join
            .save(space_id.as_ref(), stored)
            .await
            .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))?;
        Ok(pending.key_package)
    }

    async fn prepare_forward_recovery(
        &self,
        convergence_digest: &[u8; 32],
        effective_members: &[MemberInstanceId],
        key_packages: &[(MemberInstanceId, Vec<u8>)],
    ) -> Result<RemovalPreparedRecovery, RemovalRecoveryError> {
        let space_id = self.current_space_id()?;
        let material = self.load_material(&space_id).await?;
        let view = self.current_view().await?;
        // 把成员实例映射到设备标识,并按设备标识整理备用 key package。
        let mut packages = Vec::with_capacity(key_packages.len());
        for (instance, key_package) in key_packages {
            let member = view
                .members
                .iter()
                .find(|member| &member.instance == instance)
                .ok_or(RemovalRecoveryError::InvalidMaterial)?;
            packages.push((member.device_id.as_str().as_bytes(), key_package.clone()));
        }
        let sponsor = MlsClientState::from_bytes(material.group_state().to_vec());
        let recovery =
            MlsGroupEngine::recover_forward(&sponsor, &packages).map_err(Self::recovery_error)?;
        let epoch = GroupEpoch::new(recovery.epoch);
        let now_ms = chrono::Utc::now().timestamp_millis();
        let next = self
            .session
            .rotate_space_material(
                &material,
                recovery.sponsor_state.into_bytes(),
                epoch,
                now_ms,
            )
            .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))?;
        let encrypted_key_catalog = seal_group_catalog(&recovery.wrapping_key, &next)
            .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))?;
        let local_checkpoint = postcard::to_stdvec(&next)
            .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))?;
        Ok(RemovalPreparedRecovery {
            material: RemovalRecoveryMaterial {
                convergence_digest: *convergence_digest,
                effective_members: effective_members.to_vec(),
                epoch: recovery.epoch,
                commit: recovery.commit,
                welcome: recovery.welcome,
                encrypted_key_catalog,
            },
            local_checkpoint,
        })
    }

    async fn install_prepared_forward_recovery(
        &self,
        local_checkpoint: &[u8],
    ) -> Result<(), RemovalRecoveryError> {
        let next: SpaceKeyMaterial = postcard::from_bytes(local_checkpoint)
            .map_err(|_| RemovalRecoveryError::InvalidMaterial)?;
        let space_id = self.current_space_id()?;
        let validator = InMemorySession::new();
        validator.set_master_key_for_space(
            space_id,
            self.session
                .get_master_key()
                .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))?,
        );
        validator
            .install_space_material(&next)
            .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))?;
        self.key_epoch_repository
            .save_space_material(&next)
            .await
            .map_err(Self::recovery_error)?;
        self.session
            .install_space_material(&next)
            .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))?;
        info!(
            epoch = next.state().epoch().value(),
            "forward recovery material applied locally"
        );
        Ok(())
    }

    async fn apply_forward_recovery(
        &self,
        material: &RemovalRecoveryMaterial,
        expected_convergence_digest: &[u8; 32],
        expected_effective_members: &[MemberInstanceId],
    ) -> Result<(), RemovalRecoveryError> {
        if material.convergence_digest != *expected_convergence_digest {
            return Err(RemovalRecoveryError::InvalidMaterial);
        }
        let mut actual = material.effective_members.clone();
        actual.sort_unstable();
        let mut expected = expected_effective_members.to_vec();
        expected.sort_unstable();
        if actual != expected {
            return Err(RemovalRecoveryError::InvalidMaterial);
        }
        let space_id = self.current_space_id()?;
        let welcome = material
            .welcome
            .as_deref()
            .ok_or(RemovalRecoveryError::InvalidMaterial)?;
        let pending = match self.pending_join.load(space_id.as_ref()).await {
            Ok(Some(pending)) => pending,
            Ok(None) => {
                warn!(
                    failure = "pending_key_package_missing",
                    "forward recovery rejected"
                );
                return Err(RemovalRecoveryError::InvalidMaterial);
            }
            Err(_) => {
                warn!(
                    failure = "pending_key_package_load_failed",
                    "forward recovery deferred"
                );
                return Err(RemovalRecoveryError::Repository(
                    "pending key package unavailable".to_owned(),
                ));
            }
        };
        let pending: PendingRemovalJoin = postcard::from_bytes(&pending).map_err(|_| {
            warn!(
                failure = "pending_key_package_invalid",
                "forward recovery rejected"
            );
            RemovalRecoveryError::InvalidMaterial
        })?;
        let completed = MlsGroupEngine::complete_recovery_join(
            PendingMlsJoin {
                key_package: pending.key_package,
                client_state: MlsClientState::from_bytes(pending.client_state),
            },
            space_id.as_ref().as_bytes(),
            welcome,
        )
        .map_err(|_| {
            warn!(failure = "welcome_rejected", "forward recovery rejected");
            RemovalRecoveryError::InvalidMaterial
        })?;
        if completed.epoch != material.epoch {
            warn!(failure = "epoch_mismatch", "forward recovery rejected");
            return Err(RemovalRecoveryError::OutOfOrder);
        }
        let portable: PortableKeyCatalog = open_group_catalog(
            &completed.wrapping_key,
            &space_id,
            material.epoch,
            &material.encrypted_key_catalog,
        )
        .map_err(|_| {
            warn!(
                failure = "key_catalog_rejected",
                "forward recovery rejected"
            );
            RemovalRecoveryError::InvalidMaterial
        })?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let next = SpaceKeyMaterial::new(
            portable.state,
            completed.client_state.into_bytes(),
            portable.key_catalog,
            now_ms,
        );
        let validator = InMemorySession::new();
        validator.set_master_key_for_space(
            space_id.clone(),
            self.session
                .get_master_key()
                .map_err(|error| RemovalRecoveryError::Repository(error.to_string()))?,
        );
        validator.install_space_material(&next).map_err(|_| {
            warn!(
                failure = "material_validation_failed",
                "forward recovery rejected"
            );
            RemovalRecoveryError::InvalidMaterial
        })?;
        self.key_epoch_repository
            .save_space_material(&next)
            .await
            .map_err(|_| {
                warn!(
                    failure = "material_save_failed",
                    "forward recovery deferred"
                );
                RemovalRecoveryError::Repository("forward material save failed".to_owned())
            })?;
        self.session.install_space_material(&next).map_err(|_| {
            warn!(
                failure = "material_install_failed",
                "forward recovery rejected"
            );
            RemovalRecoveryError::InvalidMaterial
        })?;
        self.pending_join
            .clear(space_id.as_ref())
            .await
            .map_err(|_| {
                warn!(
                    failure = "pending_key_package_clear_failed",
                    "forward recovery deferred"
                );
                RemovalRecoveryError::Repository("pending key package clear failed".to_owned())
            })?;
        info!(epoch = material.epoch, "forward recovery material applied");
        Ok(())
    }
}
