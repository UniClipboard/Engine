//! The workspace admission adapter seam (ADR-017).
//!
//! `WorkspaceAdmissionOwnerPort` is the private interface the pairing
//! channel (this module's sponsor/joiner implementations) uses to reach the
//! workspace convergence owner. It carries exactly the three interactions
//! the owner allows the channel to make:
//!
//! - admission decisions (verify the joiner, allow/reject the join);
//! - readiness submission (hand the saved joiner facts back, get the
//!   "admission change saved" confirmation);
//! - workspace decisions (the channel executes accept/reject/close exactly
//!   as the owner decides — it never generates them).
//!
//! The channel side test surface uses a double of this port, so the
//! communication and verification behaviour can be verified without a real
//! workspace owner.

use async_trait::async_trait;

use uc_core::ids::DeviceId;
use uc_core::membership::MembershipAdmissionDecision;
use uc_core::pairing::JoinerRequest;
use uc_core::ports::space::GroupAdmissionPort;
use uc_core::space_access::PreparedGroupJoin;

use crate::space::convergence::{WorkspaceConvergence, WorkspaceConvergenceError};

pub(crate) fn stable_join_request_binding(
    device_id: &DeviceId,
    identity_fingerprint: &uc_core::security::IdentityFingerprint,
) -> Vec<u8> {
    let mut binding = b"uniclipboard/join-request-binding/v1\0".to_vec();
    let device = device_id.as_str().as_bytes();
    binding.extend_from_slice(&(device.len() as u64).to_be_bytes());
    binding.extend_from_slice(device);
    let fingerprint = identity_fingerprint.as_display().as_bytes();
    binding.extend_from_slice(&(fingerprint.len() as u64).to_be_bytes());
    binding.extend_from_slice(fingerprint);
    binding
}

#[derive(Debug)]
pub(crate) struct DurableLocalJoinPreparation {
    pub attempt_id: [u8; 32],
    pub join_id: [u8; 16],
    pub request_message_id: [u8; 32],
    pub resume_public_key: Vec<u8>,
    pub prepared_group_join: PreparedGroupJoin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DurableJoinerCompletion {
    Active(uc_core::pairing::DurableAdmissionFrame),
    SpaceTransitionRequired,
}

/// The workspace-owner side of the admission seam. Implemented by
/// [`WorkspaceConvergence`]; consumed only by the pairing channel inside
/// this module.
#[async_trait]
pub(crate) trait WorkspaceAdmissionOwnerPort: Send + Sync {
    async fn preflight_local_join_source(
        &self,
        _preserve_unreadable_history: bool,
    ) -> Result<(), WorkspaceConvergenceError> {
        Ok(())
    }

    /// Validate the exact J0 member identity before an invitation is consumed.
    async fn validate_join_request(
        &self,
        request: &JoinerRequest,
    ) -> Result<(), WorkspaceConvergenceError> {
        request
            .validate_durable_identity()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_owned()))
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

    async fn commit_sponsor_prepared(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
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

    async fn complete_sponsor_applied(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        Err(WorkspaceConvergenceError::Unavailable)
    }

    async fn activate_joiner_complete(
        &self,
        _frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<DurableJoinerCompletion, WorkspaceConvergenceError> {
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
impl WorkspaceAdmissionOwnerPort for WorkspaceConvergence {
    async fn preflight_local_join_source(
        &self,
        preserve_unreadable_history: bool,
    ) -> Result<(), WorkspaceConvergenceError> {
        WorkspaceConvergence::preflight_local_join_source(self, preserve_unreadable_history).await
    }

    async fn validate_join_request(
        &self,
        request: &JoinerRequest,
    ) -> Result<(), WorkspaceConvergenceError> {
        WorkspaceConvergence::validate_join_request(self, request).await
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
        WorkspaceConvergence::prepare_local_join_before_network(
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
        WorkspaceConvergence::reject_local_join_before_candidate(self, attempt_id, reason).await
    }

    async fn reject_superseded_join_cleanup(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        WorkspaceConvergence::reject_superseded_join_cleanup(self, frame).await
    }

    async fn confirm_superseded_join_cleanup_sent(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        WorkspaceConvergence::confirm_superseded_join_cleanup_sent(self, frame).await
    }

    async fn admission_decision_for_joiner(
        &self,
        invitation_generation: u64,
        joiner_device_id: &DeviceId,
    ) -> MembershipAdmissionDecision {
        WorkspaceConvergence::admission_decision_for_joiner(
            self,
            invitation_generation,
            joiner_device_id,
        )
        .await
    }

    async fn synchronize_chain(&self) -> Result<(), WorkspaceConvergenceError> {
        WorkspaceConvergence::synchronize_chain(self).await
    }

    async fn prepare_sponsor_candidate(
        &self,
        request: &JoinerRequest,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        WorkspaceConvergence::prepare_sponsor_candidate(self, request).await
    }

    async fn prepare_joiner_candidate(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        proof_signer: &(dyn GroupAdmissionPort + Send + Sync),
        target_access: &(dyn uc_core::ports::space::PrepareAdmissionTargetAccessPort + Send + Sync),
        passphrase: &uc_core::crypto::domain::Passphrase,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        WorkspaceConvergence::prepare_joiner_candidate(
            self,
            frame,
            proof_signer,
            target_access,
            passphrase,
        )
        .await
    }

    async fn commit_sponsor_prepared(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        WorkspaceConvergence::commit_sponsor_prepared(self, frame).await
    }

    async fn apply_joiner_commit(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        receipt_signer: &(dyn GroupAdmissionPort + Send + Sync),
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        WorkspaceConvergence::apply_joiner_commit(self, frame, receipt_signer).await
    }

    async fn complete_sponsor_applied(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        WorkspaceConvergence::complete_sponsor_applied(self, frame).await
    }

    async fn activate_joiner_complete(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<DurableJoinerCompletion, WorkspaceConvergenceError> {
        WorkspaceConvergence::activate_joiner_complete(self, frame).await
    }

    async fn confirm_sponsor_complete_ack(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        WorkspaceConvergence::confirm_sponsor_complete_ack(self, frame).await
    }
}
