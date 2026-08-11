//! 移除恢复适配器:从当前安全状态提供因果视图与本机成员实例。
//!
//! 意图集合决定有效成员集合,本适配器只负责把安全状态落为可验证的因果
//! 视图(ADR-015"OpenMLS 是安全状态投影")。前向恢复资料生成与应用已由
//! 受限恢复通道(`workspace-recovery/1`)取代,不再存在于此。

use std::sync::Arc;

use async_trait::async_trait;
use tracing::debug;

use super::mls_group::{MlsClientState, MlsGroupEngine};
use super::session::InMemorySession;
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    CurrentMembershipIdentityPort, MemberInstanceId, RemovalCausalProof, RemovalCausalProofMember,
    RemovalRecoveryError, RemovalRecoveryPort, RemovalViewMember, RemovalViewSnapshot,
    RevocationRepositoryPort, SpaceKeyMaterial,
};

pub struct RemovalRecoveryAdapter {
    session: InMemorySession,
    key_epoch_repository: Arc<dyn RevocationRepositoryPort>,
    current_identity: Arc<dyn CurrentMembershipIdentityPort>,
}

impl RemovalRecoveryAdapter {
    pub fn new(
        session: InMemorySession,
        key_epoch_repository: Arc<dyn RevocationRepositoryPort>,
        current_identity: Arc<dyn CurrentMembershipIdentityPort>,
    ) -> Self {
        Self {
            session,
            key_epoch_repository,
            current_identity,
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
        let instance = view
            .members
            .iter()
            .find(|member| member.device_id == own_device_id)
            .map(|member| member.instance);
        debug!(
            view_epoch = view.epoch,
            view_member_count = view.members.len(),
            instance = %instance.map_or_else(|| "none".to_owned(), |own| own.to_string()),
            "current member instance resolved from the security view"
        );
        Ok(instance)
    }
}
