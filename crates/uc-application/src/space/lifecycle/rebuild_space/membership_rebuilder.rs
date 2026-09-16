use async_trait::async_trait;
use std::sync::Arc;
use uc_core::membership::MembershipInitializationError;
use uc_core::membership::SpaceMembershipInitializerPort;

use super::error::SpaceMembershipRebuildError;
use super::ports::SpaceMembershipRebuildPort;
use uc_core::{
    membership::{MembershipError, RelationshipStateResetError, RelationshipStateResetPort},
    DeviceId, MemberRepositoryPort, SpaceMember,
};

/// 清理旧本机准入状态
/// -> 清理成员关系
/// -> 删除所有远端成员
/// -> 保存传入的本机成员
/// -> 初始化 membership 基线
pub(crate) struct SpaceMembershipRebuilder {
    member_repo: Arc<dyn MemberRepositoryPort>,
    membership_reset: Arc<dyn RelationshipStateResetPort>,
    membership_initializer: Arc<dyn SpaceMembershipInitializerPort>,
}

impl SpaceMembershipRebuilder {
    pub(crate) fn new(
        member_repo: Arc<dyn MemberRepositoryPort>,
        membership_reset: Arc<dyn RelationshipStateResetPort>,
        membership_initializer: Arc<dyn SpaceMembershipInitializerPort>,
    ) -> Self {
        Self {
            member_repo,
            membership_reset,
            membership_initializer,
        }
    }

    async fn remove_remote_members(
        &self,
        local_device_id: &DeviceId,
    ) -> Result<(), SpaceMembershipRebuildError> {
        let members = self
            .member_repo
            .list()
            .await
            .map_err(map_membership_repository_error)?;

        for member in members {
            if &member.device_id == local_device_id {
                continue;
            }

            self.member_repo
                .remove(&member.device_id)
                .await
                .map_err(map_membership_repository_error)?;
        }

        Ok(())
    }
}

#[async_trait]
impl SpaceMembershipRebuildPort for SpaceMembershipRebuilder {
    async fn rebuild(&self, local_member: &SpaceMember) -> Result<(), SpaceMembershipRebuildError> {
        self.membership_reset
            .clear_all_relationships()
            .await
            .map_err(map_relationship_reset_error)?;

        self.remove_remote_members(&local_member.device_id).await?;

        self.member_repo
            .save(local_member)
            .await
            .map_err(map_membership_repository_error)?;

        self.membership_initializer
            .initialize()
            .await
            .map_err(map_membership_initialization_error)?;

        Ok(())
    }
}

fn map_relationship_reset_error(
    _error: RelationshipStateResetError,
) -> SpaceMembershipRebuildError {
    SpaceMembershipRebuildError::Unavailable
}

fn map_membership_repository_error(error: MembershipError) -> SpaceMembershipRebuildError {
    match error {
        MembershipError::Repository(_) => SpaceMembershipRebuildError::Unavailable,
        MembershipError::AlreadyAdmitted(_) | MembershipError::NotFound(_) => {
            SpaceMembershipRebuildError::Inconsistent
        }
    }
}

fn map_membership_initialization_error(
    error: MembershipInitializationError,
) -> SpaceMembershipRebuildError {
    match error {
        MembershipInitializationError::Unavailable => SpaceMembershipRebuildError::Unavailable,
        MembershipInitializationError::Inconsistent => SpaceMembershipRebuildError::Inconsistent,
    }
}
