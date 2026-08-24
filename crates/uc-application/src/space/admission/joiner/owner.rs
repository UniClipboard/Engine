use async_trait::async_trait;

use crate::deps::GroupAdmissionPort;
use uc_core::ids::DeviceId;

use crate::space::admission::adapter::{DurableJoinerCompletion, DurableLocalJoinPreparation};
use crate::space::workspace_membership::WorkspaceConvergenceError;

/// Durable admission capabilities required by the joiner-side handshake.
#[async_trait]
pub(crate) trait JoinerAdmissionOwnerPort: Send + Sync {
    async fn preflight_local_join_source(
        &self,
        _preserve_unreadable_history: bool,
    ) -> Result<(), WorkspaceConvergenceError> {
        Ok(())
    }

    /// Persist or reopen the exact local member material before the first
    /// JoinRequest is sent. Implementations that do not own the durable
    /// profile admission store must fail closed.
    async fn prepare_local_join_before_network(
        &self,
        _preparation: &(dyn GroupAdmissionPort + Send + Sync),
        _local_device_id: &DeviceId,
        _sponsor: &[u8],
        _sponsor_continuation_address: &[u8],
        _stable_request_binding: &[u8],
        _preserve_unreadable_history: bool,
    ) -> Result<DurableLocalJoinPreparation, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }

    /// Save an explicit sponsor rejection before the durable Candidate exists.
    async fn reject_local_join_before_candidate(
        &self,
        _attempt_id: [u8; 32],
        _reason: uc_core::membership::AdmissionRejectionReasonV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }
    async fn prepare_joiner_candidate(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
        _proof_signer: &(dyn GroupAdmissionPort + Send + Sync),
        _target_access: &(dyn uc_core::ports::space::PrepareAdmissionTargetAccessPort
              + Send
              + Sync),
        _passphrase: &uc_core::crypto::domain::Passphrase,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }
    async fn apply_joiner_commit(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
        _receipt_signer: &(dyn GroupAdmissionPort + Send + Sync),
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }

    async fn activate_joiner_complete(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<DurableJoinerCompletion, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }
}
#[async_trait]
impl JoinerAdmissionOwnerPort for crate::space::admission::SpaceAdmission {
    async fn preflight_local_join_source(
        &self,
        preserve_unreadable_history: bool,
    ) -> Result<(), WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::preflight_local_join_source(
            self,
            preserve_unreadable_history,
        )
        .await
    }
    async fn prepare_local_join_before_network(
        &self,
        preparation: &(dyn GroupAdmissionPort + Send + Sync),
        local_device_id: &DeviceId,
        sponsor: &[u8],
        sponsor_continuation_address: &[u8],
        stable_request_binding: &[u8],
        preserve_unreadable_history: bool,
    ) -> Result<DurableLocalJoinPreparation, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::prepare_local_join_before_network(
            self,
            preparation,
            local_device_id,
            sponsor,
            sponsor_continuation_address,
            stable_request_binding,
            preserve_unreadable_history,
        )
        .await
    }

    async fn reject_local_join_before_candidate(
        &self,
        attempt_id: [u8; 32],
        reason: uc_core::membership::AdmissionRejectionReasonV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::reject_local_join_before_candidate(
            self, attempt_id, reason,
        )
        .await
    }
    async fn prepare_joiner_candidate(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        proof_signer: &(dyn GroupAdmissionPort + Send + Sync),
        target_access: &(dyn uc_core::ports::space::PrepareAdmissionTargetAccessPort + Send + Sync),
        passphrase: &uc_core::crypto::domain::Passphrase,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::prepare_joiner_candidate(
            self,
            frame,
            proof_signer,
            target_access,
            passphrase,
        )
        .await
    }
    async fn apply_joiner_commit(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        receipt_signer: &(dyn GroupAdmissionPort + Send + Sync),
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::apply_joiner_commit(self, frame, receipt_signer)
            .await
    }

    async fn activate_joiner_complete(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<DurableJoinerCompletion, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::activate_joiner_complete(self, frame).await
    }
}
