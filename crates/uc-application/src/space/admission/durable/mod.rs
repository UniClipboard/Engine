mod completion_recovery;
mod flow;
mod transaction;

use async_trait::async_trait;

pub(in crate::space) use transaction::DurableAdmissionCandidatePayloadV1;
pub(in crate::space) use transaction::{
    map_repository_error, DurableAdmissionProjection, DurableAdmissionTransaction,
};

#[cfg(test)]
pub(in crate::space) use transaction::DurableAdmissionCandidateV1;

#[cfg(test)]
pub(super) use transaction::{
    admission_acknowledgment, durable_admission_message, verify_candidate_preparation,
    DurableJoinRecoveryMaterialV1, InvitationConsumeResultV1, JoinerActivationOutcomeV1,
    PendingMemberRemovalOutcomeV1,
};

use super::*;
use crate::space::workspace_membership::*;

#[async_trait]
impl SpaceTransitionRecoveryPort for crate::space::admission::SpaceAdmission {
    async fn requires_session_transition(&self) -> Result<bool, WorkspaceConvergenceError> {
        crate::space::admission::SpaceAdmission::requires_session_transition(self).await
    }

    async fn recover_after_session_drain(&self) -> Result<usize, WorkspaceConvergenceError> {
        self.recover_space_transition_after_session_drain().await
    }
}

fn admission_invitation_digest(invitation: &str) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"uniclipboard/admission-invitation-claim/v1\0");
    hasher.update((invitation.len() as u64).to_be_bytes());
    hasher.update(invitation.as_bytes());
    hasher.finalize().into()
}

pub(in crate::space) fn admission_resume_public_key_digest(public_key: &[u8]) -> [u8; 32] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"uniclipboard/admission-resume-public-key/v1\0");
    hasher.update((public_key.len() as u64).to_be_bytes());
    hasher.update(public_key);
    hasher.finalize().into()
}

fn admission_operation_id(attempt_id: uc_core::membership::AdmissionAttemptId) -> [u8; 16] {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"uniclipboard/admission-operation/v1\0");
    hasher.update(attempt_id.as_bytes());
    let digest = hasher.finalize();
    let mut operation_id = [0; 16];
    operation_id.copy_from_slice(&digest[..16]);
    operation_id
}

fn common_existing_member_delivery_payload(
    deliveries: &[uc_core::membership::SponsorAdmissionSecurityDelivery],
) -> Result<Vec<u8>, WorkspaceConvergenceError> {
    let Some(first) = deliveries.first() else {
        return Ok(Vec::new());
    };
    if first.payload.is_empty()
        || deliveries
            .iter()
            .skip(1)
            .any(|delivery| delivery.payload != first.payload)
    {
        return Err(WorkspaceConvergenceError::Inconsistent(
            "existing members received incompatible security updates".to_owned(),
        ));
    }
    Ok(first.payload.clone())
}

fn validate_candidate_request(
    candidate: &transaction::DurableAdmissionCandidateV1,
    request: &uc_core::pairing::JoinerRequest,
) -> Result<(), WorkspaceConvergenceError> {
    let event: uc_core::membership::MembershipEventV2 =
        postcard::from_bytes(&candidate.candidate_event)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
    let uc_core::membership::MembershipOperationV2::AddDevice { admission } = event.operation
    else {
        return Err(WorkspaceConvergenceError::InvalidConfirmation);
    };
    if admission.facts != request.admission
        || admission.membership_credential != request.membership_credential
        || admission.resume_public_key_digest
            != admission_resume_public_key_digest(&request.resume_public_key)
        || candidate.candidate_key_package != request.key_package
        || candidate.resume_public_key != request.resume_public_key
    {
        return Err(WorkspaceConvergenceError::InvalidConfirmation);
    }
    Ok(())
}

fn candidate_frame(
    attempt_id: uc_core::membership::AdmissionAttemptId,
    message: &uc_core::membership::AdmissionOutboxMessageV1,
) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
    if message.purpose != uc_core::membership::AdmissionOutboxPurposeV1::Candidate {
        return Err(WorkspaceConvergenceError::Inconsistent(
            "candidate outbox has the wrong purpose".to_owned(),
        ));
    }
    Ok(uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *attempt_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::Candidate,
        message_id: message.message_id,
        predecessor_message_id: message.predecessor_message_id,
        payload: message.payload.clone(),
    })
}

fn durable_frame_from_outbox(
    attempt_id: uc_core::membership::AdmissionAttemptId,
    kind: uc_core::pairing::DurableAdmissionMessageKind,
    purpose: uc_core::membership::AdmissionOutboxPurposeV1,
    message: &uc_core::membership::AdmissionOutboxMessageV1,
) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
    if message.purpose != purpose {
        return Err(WorkspaceConvergenceError::Inconsistent(
            "durable admission outbox has the wrong purpose".to_owned(),
        ));
    }
    Ok(uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *attempt_id.as_bytes(),
        kind,
        message_id: message.message_id,
        predecessor_message_id: message.predecessor_message_id,
        payload: message.payload.clone(),
    })
}

pub(in crate::space) fn complete_ack_frame(
    attempt_id: uc_core::membership::AdmissionAttemptId,
    complete_message_id: [u8; 32],
    payload: Vec<u8>,
) -> uc_core::pairing::DurableAdmissionFrame {
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"uniclipboard/admission-complete-ack/v1\0");
    hasher.update(attempt_id.as_bytes());
    hasher.update(complete_message_id);
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(&payload);
    uc_core::pairing::DurableAdmissionFrame {
        attempt_id: *attempt_id.as_bytes(),
        kind: uc_core::pairing::DurableAdmissionMessageKind::CompleteAck,
        message_id: hasher.finalize().into(),
        predecessor_message_id: Some(complete_message_id),
        payload,
    }
}

#[async_trait]
impl uc_core::membership::MembershipAdmissionGatePort for crate::space::admission::SpaceAdmission {
    async fn admission_decision(
        &self,
        invitation_generation: u64,
    ) -> uc_core::membership::MembershipAdmissionDecision {
        let state = match self.membership.load_state().await {
            Ok(state) => state,
            Err(_) => return uc_core::membership::MembershipAdmissionDecision::Unavailable,
        };
        WorkspaceMembership::admission_decision_for_state(&state, invitation_generation)
    }

    async fn invitation_generation(
        &self,
    ) -> Result<u64, uc_core::membership::MembershipAdmissionDecision> {
        let state = self
            .membership
            .load_state()
            .await
            .map_err(|_| uc_core::membership::MembershipAdmissionDecision::Unavailable)?;
        Ok(WorkspaceMembership::admission_generation(&state))
    }
}
