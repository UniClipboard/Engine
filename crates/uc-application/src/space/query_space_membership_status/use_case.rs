use super::deps::{ActiveSpaceMembershipStatusDeps, QuerySpaceMembershipStatusDeps};
use super::{build_active_space_status, ActiveSpaceStatusFacts};
use super::{
    ActionUnavailableReason, DeviceMembership, QuerySpaceMembershipStatusError,
    RecoveryAvailability, SpaceMembershipStatus,
};
use crate::space::admission::durable::{self as admission, DurableAdmissionProjection};
use crate::space::membership_state::SpaceMembershipStateRepositoryError;
use crate::space::workspace_membership::WorkspaceConvergenceError;
use std::sync::Arc;

use uc_core::membership::{
    CurrentMemberSignatureError, MemberInstanceId, SpaceMembershipState, VersionedMembershipHistory,
};

/// 生成当前 Space 面向产品展示的成员状态。
///
/// 该查询将持久化的成员流程进度、已验证的成员历史、设备资料、在线状态和
/// 正在进行的准入状态组合成一份临时视图。持久化状态不会直接返回给调用方；
/// 在线状态只描述连接情况，不能授予或移除成员资格。
pub(crate) struct QuerySpaceMembershipStatusUseCase {
    admission: DurableAdmissionProjection,
    deps: QuerySpaceMembershipStatusDeps,
    active_space: tokio::sync::RwLock<Option<ActiveSpaceMembershipStatusDeps>>,
}

impl QuerySpaceMembershipStatusUseCase {
    pub(crate) fn new(deps: QuerySpaceMembershipStatusDeps) -> Self {
        let admission = DurableAdmissionProjection::new(Arc::clone(&deps.admission_attempts));
        Self {
            admission,
            deps,
            active_space: tokio::sync::RwLock::new(None),
        }
    }

    pub(crate) async fn replace_active_space(
        &self,
        active_space: Option<ActiveSpaceMembershipStatusDeps>,
    ) {
        *self.active_space.write().await = active_space;
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<SpaceMembershipStatus, QuerySpaceMembershipStatusError> {
        let Some(active_space) = self.active_space.read().await.clone() else {
            return self.query_status_without_active_space().await;
        };

        let mut active_status = match self.query_active_space_status(&active_space).await {
            Ok(active_status) => active_status,
            Err(QuerySpaceMembershipStatusError::Unavailable) => {
                return Ok(self.build_unavailable_status());
            }
            Err(error) => return Err(error),
        };
        let revision = self.load_profile_membership_revision().await?;
        active_status.status.revision = active_status.status.revision.max(revision);
        active_status.status.current_join = self
            .admission
            .current_local_join()
            .await
            .map_err(map_workspace_membership_error)?;
        active_status.status.pending_inbound_member = self
            .admission
            .pending_inbound_member(&active_status.space_lineage)
            .await
            .map_err(map_workspace_membership_error)?;
        Ok(active_status.status)
    }

    async fn query_status_without_active_space(
        &self,
    ) -> Result<SpaceMembershipStatus, QuerySpaceMembershipStatusError> {
        let revision = match self.load_profile_membership_revision().await {
            Ok(revision) => revision,
            Err(QuerySpaceMembershipStatusError::Unavailable) => {
                return Ok(self.build_unavailable_status());
            }
            Err(error) => return Err(error),
        };
        let current_join = self
            .admission
            .current_local_join()
            .await
            .map_err(map_workspace_membership_error)?;
        Ok(SpaceMembershipStatus {
            revision,
            local_device_id: self.deps.own_device.clone(),
            local_membership: DeviceMembership::Unavailable,
            current_change: None,
            current_join,
            pending_inbound_member: None,
            devices: Vec::new(),
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions: Vec::new(),
            blocked_reason: None,
            updated_at_ms: self.deps.clock.now_ms(),
        })
    }

    async fn load_profile_membership_revision(
        &self,
    ) -> Result<u64, QuerySpaceMembershipStatusError> {
        self.deps
            .admission_attempts
            .profile_metadata()
            .await
            .map(|metadata| metadata.device_trust_revision)
            .map_err(admission::map_repository_error)
            .map_err(map_workspace_membership_error)
    }

    pub(super) async fn query_active_space_status(
        &self,
        active_space: &ActiveSpaceMembershipStatusDeps,
    ) -> Result<super::ActiveSpaceStatusResult, QuerySpaceMembershipStatusError> {
        let state = self
            .load_current_space_membership_state(active_space)
            .await?;
        let history = self
            .load_current_verified_membership_history(active_space)
            .await?;
        let (own_instance, local_membership) = self
            .determine_local_membership_from_history(active_space, &history)
            .await?;
        let roster = active_space
            .member_repo
            .list()
            .await
            .map_err(|_| QuerySpaceMembershipStatusError::Failed)?;

        Ok(build_active_space_status(ActiveSpaceStatusFacts {
            state,
            history,
            own_instance,
            local_membership,
            roster,
            presence: Arc::clone(&active_space.presence),
            local_device_id: self.deps.own_device.clone(),
        })
        .await)
    }

    async fn load_current_space_membership_state(
        &self,
        active_space: &ActiveSpaceMembershipStatusDeps,
    ) -> Result<SpaceMembershipState, QuerySpaceMembershipStatusError> {
        match active_space.state_repository.load_state().await {
            Ok(Some(state)) => Ok(state),
            Ok(None) | Err(SpaceMembershipStateRepositoryError::Locked) => {
                Err(QuerySpaceMembershipStatusError::Unavailable)
            }
            Err(SpaceMembershipStateRepositoryError::Corrupt) => {
                Err(QuerySpaceMembershipStatusError::Corrupt)
            }
            Err(SpaceMembershipStateRepositoryError::Unavailable) => {
                Err(QuerySpaceMembershipStatusError::Failed)
            }
        }
    }

    async fn load_current_verified_membership_history(
        &self,
        active_space: &ActiveSpaceMembershipStatusDeps,
    ) -> Result<VersionedMembershipHistory, QuerySpaceMembershipStatusError> {
        let encoded = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission::map_repository_error)
            .map_err(map_workspace_membership_error)?
            .ok_or(QuerySpaceMembershipStatusError::Unavailable)?;

        VersionedMembershipHistory::decode_persisted_v2(
            &encoded,
            active_space.historical_signatures.as_ref(),
        )
        .map_err(|_| QuerySpaceMembershipStatusError::Corrupt)
    }

    async fn determine_local_membership_from_history(
        &self,
        active_space: &ActiveSpaceMembershipStatusDeps,
        history: &VersionedMembershipHistory,
    ) -> Result<(MemberInstanceId, DeviceMembership), QuerySpaceMembershipStatusError> {
        let own_instance = active_space
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|error| match error {
                CurrentMemberSignatureError::Unavailable => {
                    QuerySpaceMembershipStatusError::Unavailable
                }
                CurrentMemberSignatureError::InvalidState => {
                    QuerySpaceMembershipStatusError::Corrupt
                }
                CurrentMemberSignatureError::Repository(_) => {
                    QuerySpaceMembershipStatusError::Failed
                }
            })?;
        let membership = if history.active_members().contains(&own_instance) {
            DeviceMembership::Active
        } else {
            DeviceMembership::Removed
        };

        Ok((own_instance, membership))
    }

    fn build_unavailable_status(&self) -> SpaceMembershipStatus {
        SpaceMembershipStatus {
            revision: 0,
            local_device_id: self.deps.own_device.clone(),
            local_membership: DeviceMembership::Unavailable,
            current_change: None,
            current_join: None,
            pending_inbound_member: None,
            devices: Vec::new(),
            recovery: RecoveryAvailability::NotAvailableInThisVersion,
            allowed_actions: Vec::new(),
            blocked_reason: Some(ActionUnavailableReason::EngineUnavailable),
            updated_at_ms: self.deps.clock.now_ms(),
        }
    }
}

fn map_workspace_membership_error(
    error: WorkspaceConvergenceError,
) -> QuerySpaceMembershipStatusError {
    if error.is_corrupt() {
        QuerySpaceMembershipStatusError::Corrupt
    } else if error.is_locked() {
        QuerySpaceMembershipStatusError::Unavailable
    } else {
        QuerySpaceMembershipStatusError::Failed
    }
}
