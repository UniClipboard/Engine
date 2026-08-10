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
use uc_core::membership::{
    AdmissionChangeFacts, AdmissionCommittedFacts, MemberInstanceId, RemovalAdmissionDecision,
    WorkspaceSnapshot,
};
use uc_core::ports::pairing::PairingSessionId;

use crate::space::convergence::{WorkspaceConvergence, WorkspaceConvergenceError};

/// The workspace-owner side of the admission seam. Implemented by
/// [`WorkspaceConvergence`]; consumed only by the pairing channel inside
/// this module.
#[async_trait]
pub(crate) trait WorkspaceAdmissionOwnerPort: Send + Sync {
    /// Whether the workspace currently allows an admission bound to the
    /// given invitation generation.
    async fn admission_decision(&self, invitation_generation: u64) -> RemovalAdmissionDecision;

    /// Save the in-flight sponsor admission record before waiting for the
    /// joiner's readiness.
    async fn begin_admission(
        &self,
        session: &PairingSessionId,
        joiner_device_id: &DeviceId,
        invitation_generation: u64,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError>;

    /// Commit the readiness-confirmed joiner facts in one save commit and
    /// return the confirmation material for the "admission change saved"
    /// reply.
    async fn commit_joiner_admission(
        &self,
        session: &PairingSessionId,
        joiner: AdmissionChangeFacts,
    ) -> Result<AdmissionCommittedFacts, WorkspaceConvergenceError>;

    /// Locally signed facts the joiner returns after its group session is
    /// active.
    async fn local_admission_facts(
        &self,
    ) -> Result<AdmissionChangeFacts, WorkspaceConvergenceError>;

    /// Save the joiner's local readiness facts before it sends its
    /// readiness reply.
    async fn record_local_readiness(
        &self,
        own_instance: MemberInstanceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError>;

    /// Record the sponsor's "admission change saved" confirmation once the
    /// joiner received it.
    async fn record_admission_committed(
        &self,
        confirmation: AdmissionCommittedFacts,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError>;
}

#[async_trait]
impl WorkspaceAdmissionOwnerPort for WorkspaceConvergence {
    async fn admission_decision(&self, invitation_generation: u64) -> RemovalAdmissionDecision {
        uc_core::membership::RemovalAdmissionGatePort::admission_decision(
            self,
            invitation_generation,
        )
        .await
    }

    async fn begin_admission(
        &self,
        session: &PairingSessionId,
        joiner_device_id: &DeviceId,
        invitation_generation: u64,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        WorkspaceConvergence::begin_admission(
            self,
            session,
            joiner_device_id,
            invitation_generation,
        )
        .await
    }

    async fn commit_joiner_admission(
        &self,
        session: &PairingSessionId,
        joiner: AdmissionChangeFacts,
    ) -> Result<AdmissionCommittedFacts, WorkspaceConvergenceError> {
        WorkspaceConvergence::commit_joiner_admission(self, session, joiner).await
    }

    async fn local_admission_facts(
        &self,
    ) -> Result<AdmissionChangeFacts, WorkspaceConvergenceError> {
        WorkspaceConvergence::local_admission_facts(self).await
    }

    async fn record_local_readiness(
        &self,
        own_instance: MemberInstanceId,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        WorkspaceConvergence::record_local_readiness(self, own_instance).await
    }

    async fn record_admission_committed(
        &self,
        confirmation: AdmissionCommittedFacts,
    ) -> Result<WorkspaceSnapshot, WorkspaceConvergenceError> {
        WorkspaceConvergence::record_admission_committed(self, confirmation).await
    }
}
