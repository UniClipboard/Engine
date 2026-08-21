use std::sync::Arc;

use async_trait::async_trait;
use uc_core::{
    membership::{
        RelationshipStateResetPort, SpaceMembershipInitializerPort, SpaceMembershipRebuildError,
        SpaceMembershipRebuildPort,
    },
    ports::space::SpaceRebuildAdmissionStatePort,
    DeviceId, MemberRepositoryPort, SpaceMember,
};

/// 清理旧本机准入状态
/// -> 清理成员关系
/// -> 删除所有远端成员
/// -> 保存传入的本机成员
/// -> 初始化 membership 基线
pub(crate) struct SpaceMembershipRebuilder {
    admission_state: Arc<dyn SpaceRebuildAdmissionStatePort>,
    relationship_reset: Arc<dyn RelationshipStateResetPort>,
    member_repo: Arc<dyn MemberRepositoryPort>,
    membership_initializer: Arc<dyn SpaceMembershipInitializerPort>,
}

#[async_trait]
impl SpaceMembershipRebuildPort for SpaceMembershipRebuilder {
    async fn rebuild(&self, local_member: &SpaceMember) -> Result<(), SpaceMembershipRebuildError> {
        self.admission_state
            .clear_prior_space_admission_state()
            .await
            .map_err(SpaceMembershipRebuildError::unavailable)?;

        self.relationship_reset
            .clear_all_relationships()
            .await
            .map_err(SpaceMembershipRebuildError::unavailable)?;

        self.remove_remote_members(&local_member.device_id).await?;

        self.member_repo
            .save(local_member)
            .await
            .map_err(SpaceMembershipRebuildError::unavailable)?;

        self.membership_initializer
            .initialize()
            .await
            .map_err(SpaceMembershipRebuildError::unavailable)?;

        Ok(())
    }
}

impl SpaceMembershipRebuilder {
    async fn remove_remote_members(
        &self,
        local_device_id: &DeviceId,
    ) -> Result<(), SpaceMembershipRebuildError> {
        let members = self
            .member_repo
            .list()
            .await
            .map_err(map_membership_initialization_error)?;

        for member in members {
            if &member.device_id == local_device_id {
                continue;
            }

            self.member_repo
                .remove(&member.device_id)
                .await
                .map_err(map_membership_initialization_error)?;
        }

        Ok(())
    }
}

fn map_admission_state_error(
    error: SpaceRebuildAdmissionStateError,
) -> SpaceMembershipRebuildError {
    match error {
        SpaceRebuildAdmissionStateError::Unavailable => SpaceMembershipRebuildError::Unavailable,
        SpaceRebuildAdmissionStateError::Inconsistent => SpaceMembershipRebuildError::Inconsistent,
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
