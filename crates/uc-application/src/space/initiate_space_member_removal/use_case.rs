use uc_core::ids::DeviceId;
use uc_core::membership::{
    CurrentMemberSignatureError, MembershipHistoryV2ReceiveOutcome, SpaceMembershipState,
    VersionedMembershipHistory, WorkspaceDigest, WorkspaceSnapshot,
};

use super::{
    InitiateSpaceMemberRemovalDeps, InitiateSpaceMemberRemovalError,
    InitiateSpaceMemberRemovalResult,
};

/// 在本机当前成员分支中正式发起一次成员移除。
///
/// 成功表示签名移除已经保存，并已请求后台尽快传播。其他成员是否接受、何时
/// 上线以及最终是否与本机一致，不属于本次调用的成功条件。
pub(crate) struct InitiateSpaceMemberRemovalUseCase {
    deps: InitiateSpaceMemberRemovalDeps,
}

impl InitiateSpaceMemberRemovalUseCase {
    pub(crate) fn new(deps: InitiateSpaceMemberRemovalDeps) -> Self {
        Self { deps }
    }

    pub(crate) async fn execute(
        &self,
        target_device: &DeviceId,
    ) -> Result<InitiateSpaceMemberRemovalResult, InitiateSpaceMemberRemovalError> {
        let _guard = self.deps.state_write_lock.lock().await;
        let state = self.load_state().await?;
        if state.removed {
            return Err(InitiateSpaceMemberRemovalError::LocalMemberRemoved);
        }

        let mut loaded_history = self
            .deps
            .membership_history
            .load_verified_history()
            .await
            .map_err(map_history_repository_error)?
            .ok_or(InitiateSpaceMemberRemovalError::Unavailable)?;
        if state.space_lineage != loaded_history.history().lineage_id() {
            return Err(InitiateSpaceMemberRemovalError::Corrupt);
        }

        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(map_member_signature_error)?;
        let own_member = own_credential.member_instance_id(&self.deps.own_device);
        if !loaded_history
            .history()
            .active_members()
            .contains(&own_member)
        {
            return Err(InitiateSpaceMemberRemovalError::LocalMemberRemoved);
        }

        let target_member = loaded_history
            .history()
            .effective_member_for_device(target_device)
            .ok_or(InitiateSpaceMemberRemovalError::TargetNotFound)?;
        if target_member == own_member {
            return Err(InitiateSpaceMemberRemovalError::SelfTarget);
        }

        let security_state_digest = state
            .current_digest()
            .map(|digest| *digest.as_bytes())
            .unwrap_or([0; 32]);
        let mut removal = loaded_history
            .history()
            .create_unsigned_local_removal_event(
                own_member,
                &own_credential,
                target_member,
                uuid::Uuid::new_v4().into_bytes(),
                security_state_digest,
            )
            .map_err(|_| InitiateSpaceMemberRemovalError::Corrupt)?;
        removal.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&removal.signing_payload())
            .await
            .map_err(map_member_signature_error)?;
        let removal_event_id = removal.event_id();
        if loaded_history
            .apply_signed_event(removal)
            .map_err(|_| InitiateSpaceMemberRemovalError::Corrupt)?
            != MembershipHistoryV2ReceiveOutcome::Applied
        {
            return Err(InitiateSpaceMemberRemovalError::Corrupt);
        }

        let committed_history = self
            .deps
            .membership_history
            .commit(loaded_history)
            .await
            .map_err(map_history_repository_error)?;

        self.deps.recovery_requests.request();
        self.deps.state_events.publish(&state);
        drop(_guard);

        let snapshot = build_workspace_snapshot(
            &state,
            committed_history.history(),
            own_member,
            committed_history.revision(),
        )?;
        Ok(InitiateSpaceMemberRemovalResult {
            removal_event_id,
            snapshot,
        })
    }

    async fn load_state(&self) -> Result<SpaceMembershipState, InitiateSpaceMemberRemovalError> {
        self.deps
            .state_repo
            .load_state()
            .await
            .map_err(map_state_repository_error)?
            .ok_or(InitiateSpaceMemberRemovalError::Unavailable)
    }
}

fn build_workspace_snapshot(
    state: &SpaceMembershipState,
    history: &VersionedMembershipHistory,
    own_member: uc_core::membership::MemberInstanceId,
    profile_revision: u64,
) -> Result<WorkspaceSnapshot, InitiateSpaceMemberRemovalError> {
    let position = history
        .current_position()
        .map_err(|_| InitiateSpaceMemberRemovalError::Corrupt)?;
    let mut snapshot = state.snapshot();
    snapshot.revision = snapshot.revision.max(profile_revision);
    snapshot.history_event_count =
        usize::try_from(position.depth.saturating_add(1)).unwrap_or(usize::MAX);
    snapshot.effective_member_count = history.active_members().len();
    snapshot.convergence_digest = Some(WorkspaceDigest::from_bytes(position.history_digest));
    snapshot.pending_removal_decision_event_id = history.pending_removal_decision(own_member);
    snapshot.removed = !history.active_members().contains(&own_member);
    Ok(snapshot)
}

fn map_history_repository_error(
    error: crate::space::membership_history::MembershipHistoryRepositoryError,
) -> InitiateSpaceMemberRemovalError {
    match error {
        crate::space::membership_history::MembershipHistoryRepositoryError::Locked => {
            InitiateSpaceMemberRemovalError::Unavailable
        }
        crate::space::membership_history::MembershipHistoryRepositoryError::Corrupt => {
            InitiateSpaceMemberRemovalError::Corrupt
        }
        crate::space::membership_history::MembershipHistoryRepositoryError::Conflict
        | crate::space::membership_history::MembershipHistoryRepositoryError::Unavailable => {
            InitiateSpaceMemberRemovalError::Failed
        }
    }
}

fn map_member_signature_error(
    error: CurrentMemberSignatureError,
) -> InitiateSpaceMemberRemovalError {
    match error {
        CurrentMemberSignatureError::Unavailable => InitiateSpaceMemberRemovalError::Unavailable,
        CurrentMemberSignatureError::InvalidState => InitiateSpaceMemberRemovalError::Corrupt,
        CurrentMemberSignatureError::Repository(_) => InitiateSpaceMemberRemovalError::Failed,
    }
}

fn map_state_repository_error(
    error: crate::space::membership_state::SpaceMembershipStateRepositoryError,
) -> InitiateSpaceMemberRemovalError {
    match error {
        crate::space::membership_state::SpaceMembershipStateRepositoryError::Locked => {
            InitiateSpaceMemberRemovalError::Unavailable
        }
        crate::space::membership_state::SpaceMembershipStateRepositoryError::Corrupt => {
            InitiateSpaceMemberRemovalError::Corrupt
        }
        crate::space::membership_state::SpaceMembershipStateRepositoryError::Unavailable => {
            InitiateSpaceMemberRemovalError::Failed
        }
    }
}
