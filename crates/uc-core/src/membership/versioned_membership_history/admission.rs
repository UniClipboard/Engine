//! 准入证明、完成凭据与激活回执。

use super::{
    append_field, append_optional_event_id, AdmissionChangeFacts, AdmissionSecurityCommitmentV1,
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipAdmissionV2, MembershipCredential,
    MembershipCredentialId, MembershipEventId, MembershipEventV2, MembershipHistoryV2Error,
    MembershipOperationV2, VersionedMembershipHistory, ACTIVATION_RECEIPT_RECORD_FORMAT_V1,
    ADMISSION_COMPLETION_FORMAT_V1, MEMBERSHIP_EVENT_FORMAT_V2, PREPARED_ADMISSION_PROOF_FORMAT_V1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedAdmissionProofV1 {
    pub proof_format_version: u16,
    pub attempt_id: [u8; 32],
    pub lineage_id: String,
    pub base_history_position: BaseMembershipHistoryPosition,
    pub candidate_event_id: MembershipEventId,
    pub target_members_digest: [u8; 32],
    pub security_commitment_id: [u8; 32],
    pub joiner_member_instance_id: MemberInstanceId,
    pub joiner_credential_id: MembershipCredentialId,
    pub signature: Vec<u8>,
}

impl PreparedAdmissionProofV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        attempt_id: [u8; 32],
        lineage_id: String,
        base_history_position: BaseMembershipHistoryPosition,
        candidate_event_id: MembershipEventId,
        target_members_digest: [u8; 32],
        security_commitment_id: [u8; 32],
        joiner_member_instance_id: MemberInstanceId,
        joiner_credential_id: MembershipCredentialId,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            proof_format_version: PREPARED_ADMISSION_PROOF_FORMAT_V1,
            attempt_id,
            lineage_id,
            base_history_position,
            candidate_event_id,
            target_members_digest,
            security_commitment_id,
            joiner_member_instance_id,
            joiner_credential_id,
            signature,
        }
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard/prepared-admission-proof/v1\0");
        bytes.extend_from_slice(&self.proof_format_version.to_be_bytes());
        bytes.extend_from_slice(&self.attempt_id);
        append_field(&mut bytes, self.lineage_id.as_bytes());
        append_optional_event_id(&mut bytes, self.base_history_position.event_id);
        bytes.extend_from_slice(&self.base_history_position.depth.to_be_bytes());
        bytes.extend_from_slice(&self.base_history_position.history_digest);
        bytes.extend_from_slice(self.candidate_event_id.as_bytes());
        bytes.extend_from_slice(&self.target_members_digest);
        bytes.extend_from_slice(&self.security_commitment_id);
        bytes.extend_from_slice(self.joiner_member_instance_id.as_bytes());
        bytes.extend_from_slice(self.joiner_credential_id.as_bytes());
        bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionCompletionV1 {
    pub completion_format_version: u16,
    pub attempt_id: [u8; 32],
    pub event_id: MembershipEventId,
    pub activation_receipt_digest: [u8; 32],
    pub security_commitment_id: [u8; 32],
    pub completed_by_member_instance_id: MemberInstanceId,
    pub completed_by_credential_id: MembershipCredentialId,
    pub completed_history_position: BaseMembershipHistoryPosition,
    pub signature: Vec<u8>,
}

impl AdmissionCompletionV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        attempt_id: [u8; 32],
        event_id: MembershipEventId,
        activation_receipt_digest: [u8; 32],
        security_commitment_id: [u8; 32],
        completed_by_member_instance_id: MemberInstanceId,
        completed_by_credential_id: MembershipCredentialId,
        completed_history_position: BaseMembershipHistoryPosition,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            completion_format_version: ADMISSION_COMPLETION_FORMAT_V1,
            attempt_id,
            event_id,
            activation_receipt_digest,
            security_commitment_id,
            completed_by_member_instance_id,
            completed_by_credential_id,
            completed_history_position,
            signature,
        }
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard/admission-completion/v1\0");
        bytes.extend_from_slice(&self.completion_format_version.to_be_bytes());
        bytes.extend_from_slice(&self.attempt_id);
        bytes.extend_from_slice(self.event_id.as_bytes());
        bytes.extend_from_slice(&self.activation_receipt_digest);
        bytes.extend_from_slice(&self.security_commitment_id);
        bytes.extend_from_slice(self.completed_by_member_instance_id.as_bytes());
        bytes.extend_from_slice(self.completed_by_credential_id.as_bytes());
        append_optional_event_id(&mut bytes, self.completed_history_position.event_id);
        bytes.extend_from_slice(&self.completed_history_position.depth.to_be_bytes());
        bytes.extend_from_slice(&self.completed_history_position.history_digest);
        bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionActivationReceipt {
    pub receipt_format_version: u16,
    pub attempt_id: [u8; 32],
    pub event_id: MembershipEventId,
    pub applied_history_digest: [u8; 32],
    pub installed_security_commitment_id: [u8; 32],
    pub joiner_member_instance_id: MemberInstanceId,
    pub signature: Vec<u8>,
}

impl AdmissionActivationReceipt {
    pub fn new(
        receipt_format_version: u16,
        attempt_id: [u8; 32],
        event_id: MembershipEventId,
        applied_history_digest: [u8; 32],
        installed_security_commitment_id: [u8; 32],
        joiner_member_instance_id: MemberInstanceId,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            receipt_format_version,
            attempt_id,
            event_id,
            applied_history_digest,
            installed_security_commitment_id,
            joiner_member_instance_id,
            signature,
        }
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard/admission-activation-receipt/v1\0");
        bytes.extend_from_slice(&self.receipt_format_version.to_be_bytes());
        bytes.extend_from_slice(&self.attempt_id);
        bytes.extend_from_slice(self.event_id.as_bytes());
        bytes.extend_from_slice(&self.applied_history_digest);
        bytes.extend_from_slice(&self.installed_security_commitment_id);
        bytes.extend_from_slice(self.joiner_member_instance_id.as_bytes());
        bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipActivationReceiptRecord {
    pub receipt_record_format_version: u16,
    pub event_id: MembershipEventId,
    pub attempt_id: [u8; 32],
    pub activation_receipt: AdmissionActivationReceipt,
    pub receipt_id: [u8; 32],
}

impl MembershipActivationReceiptRecord {
    pub(super) fn new(receipt: AdmissionActivationReceipt) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/membership-activation-receipt-record/v1\0");
        hasher.update(receipt.signing_payload());
        hasher.update((receipt.signature.len() as u64).to_be_bytes());
        hasher.update(&receipt.signature);
        Self {
            receipt_record_format_version: ACTIVATION_RECEIPT_RECORD_FORMAT_V1,
            event_id: receipt.event_id,
            attempt_id: receipt.attempt_id,
            receipt_id: hasher.finalize().into(),
            activation_receipt: receipt,
        }
    }
}

impl VersionedMembershipHistory {
    /// Builds the stable AddDevice portion used as input to admission security preparation.
    #[allow(clippy::too_many_arguments)]
    pub fn create_unsigned_local_admission_event(
        &self,
        author: MemberInstanceId,
        author_credential: &MembershipCredential,
        facts: AdmissionChangeFacts,
        candidate_credential: MembershipCredential,
        resume_public_key_digest: [u8; 32],
        operation_id: [u8; 16],
    ) -> Result<MembershipEventV2, MembershipHistoryV2Error> {
        if self.credentials.get(&author) != Some(author_credential)
            || !self.active_members().contains(&author)
        {
            return Err(MembershipHistoryV2Error::UnauthorizedAuthor);
        }
        candidate_credential.validate()?;
        if facts.member_instance != candidate_credential.member_instance_id(&facts.device_id)
            || self.effective_members().contains(&facts.member_instance)
        {
            return Err(MembershipHistoryV2Error::InvalidOperation);
        }
        let position = self.current_position()?;
        let operation = MembershipOperationV2::AddDevice {
            admission: MembershipAdmissionV2 {
                facts,
                membership_credential: candidate_credential,
                resume_public_key_digest,
                security_commitment_id: [0; 32],
            },
        };
        let resulting_members_digest =
            self.expected_resulting_members_digest(position.event_id, &operation)?;
        Ok(MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            self.lineage_id.clone(),
            position.event_id,
            position.depth.saturating_add(1),
            operation_id,
            author,
            author_credential.credential_id,
            author_credential.signature_algorithm_version,
            operation,
            resulting_members_digest,
            [0; 32],
            Vec::new(),
            None,
            Vec::new(),
        ))
    }

    /// Binds an OpenMLS result to a previously created AddDevice draft.
    pub fn finalize_unsigned_local_admission_event(
        &self,
        mut event: MembershipEventV2,
        candidate_key_package: &[u8],
        commitment: &AdmissionSecurityCommitmentV1,
    ) -> Result<MembershipEventV2, MembershipHistoryV2Error> {
        if event.lineage_id != self.lineage_id
            || event.parent_event_id != self.current_head()
            || commitment.base_history_position != self.current_position()?
            || commitment.candidate_core_digest
                != event
                    .admission_candidate_core_digest(commitment.attempt_id, candidate_key_package)?
        {
            return Err(MembershipHistoryV2Error::InvalidSecurityCommitment);
        }
        let MembershipOperationV2::AddDevice { admission } = &mut event.operation else {
            return Err(MembershipHistoryV2Error::InvalidOperation);
        };
        admission.security_commitment_id = commitment.security_commitment_id;
        event.security_state_digest = commitment.security_commitment_id;
        event.admission_bundle_digest = Some(commitment.admission_bundle_digest);
        Ok(event)
    }
}
