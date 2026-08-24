use async_trait::async_trait;

use uc_core::ids::DeviceId;
use uc_core::membership::MembershipAdmissionDecision;
use uc_core::pairing::JoinerRequest;

use crate::space::workspace_membership::WorkspaceConvergenceError;

/// Durable admission capabilities required by the sponsor-side runtime.
#[async_trait]
pub(crate) trait SponsorAdmissionOwnerPort: Send + Sync {
    /// Validate the exact J0 member identity before an invitation is consumed.
    async fn validate_join_request(
        &self,
        request: &JoinerRequest,
    ) -> Result<(), WorkspaceConvergenceError> {
        request
            .validate_durable_identity()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_owned()))
    }
    async fn reject_superseded_join_cleanup(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }

    async fn confirm_superseded_join_cleanup_sent(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }

    /// Whether the workspace currently allows this device to join through
    /// an invitation bound to the given generation.
    async fn admission_decision_for_joiner(
        &self,
        invitation_generation: u64,
        joiner_device_id: &DeviceId,
    ) -> MembershipAdmissionDecision;

    /// Pull the local chain head up to the newest known member before an
    /// admission is committed. Best effort and bounded; the admission may
    /// continue on the local head when the sync cannot complete.
    async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError>;

    /// Build and durably save the exact Candidate before the channel sends it.
    async fn prepare_sponsor_candidate(
        &self,
        _request: &JoinerRequest,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }
    async fn commit_sponsor_prepared(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }
    async fn complete_sponsor_applied(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }
    async fn confirm_sponsor_complete_ack(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }
}
#[async_trait]
impl SponsorAdmissionOwnerPort for crate::space::admission::SpaceAdmission {
    async fn validate_join_request(
        &self,
        request: &JoinerRequest,
    ) -> Result<(), WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::validate_join_request(self, request).await
    }
    async fn reject_superseded_join_cleanup(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::reject_superseded_join_cleanup(self, frame).await
    }

    async fn confirm_superseded_join_cleanup_sent(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::confirm_superseded_join_cleanup_sent(self, frame)
            .await
    }

    async fn admission_decision_for_joiner(
        &self,
        invitation_generation: u64,
        joiner_device_id: &DeviceId,
    ) -> MembershipAdmissionDecision {
        self.membership
            .admission_decision_for_joiner(invitation_generation, joiner_device_id)
            .await
    }

    async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError> {
        self.membership.synchronize_chain().await
    }

    async fn prepare_sponsor_candidate(
        &self,
        request: &JoinerRequest,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::prepare_sponsor_candidate(self, request).await
    }
    async fn commit_sponsor_prepared(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::commit_sponsor_prepared(self, frame).await
    }
    async fn complete_sponsor_applied(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::complete_sponsor_applied(self, frame).await
    }
    async fn confirm_sponsor_complete_ack(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::confirm_sponsor_complete_ack(self, frame).await
    }
}
