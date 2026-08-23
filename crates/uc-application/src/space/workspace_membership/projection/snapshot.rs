use super::super::*;

impl WorkspaceMembership {
    /// Load the current workspace snapshot without changing any state.
    pub async fn query(&self) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        let state = self.load_state().await?;
        let mut snapshot = state.snapshot();
        let Some(scope) = self
            .v2_current_peer_snapshot(&state)
            .await
            .map_err(|error| {
                WorkspaceConvergenceError::Inconsistent(format!(
                    "current V2 member scope is unavailable: {error:?}"
                ))
            })?
        else {
            return Ok(snapshot);
        };
        let encoded_history = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(admission::map_repository_error)?
            .ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "current V2 member history disappeared during query".to_owned(),
                )
            })?;
        let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
            &encoded_history,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| {
            WorkspaceConvergenceError::Inconsistent(format!(
                "current V2 member history is invalid: {error}"
            ))
        })?;
        let position = history.current_position().map_err(|error| {
            WorkspaceConvergenceError::Inconsistent(format!(
                "current V2 member history position is invalid: {error}"
            ))
        })?;
        let metadata = self
            .deps
            .admission_attempts
            .profile_metadata()
            .await
            .map_err(admission::map_repository_error)?;
        snapshot.revision = snapshot.revision.max(metadata.device_trust_revision);
        snapshot.history_event_count =
            usize::try_from(position.depth.saturating_add(1)).unwrap_or(usize::MAX);
        snapshot.effective_member_count = history.active_members().len();
        snapshot.convergence_digest = Some(uc_core::membership::WorkspaceDigest::from_bytes(
            position.history_digest,
        ));
        let own_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        snapshot.pending_removal_decision_event_id = history.pending_removal_decision(own_instance);
        snapshot.removed =
            scope.local_membership == uc_core::membership::CurrentWorkspaceLocalMembership::Removed;
        Ok(snapshot)
    }
}
