//! 成员事件、决定与准入承诺的唯一记录定义。

use super::{
    append_field, append_operation, append_optional_digest, append_optional_event_id,
    AdmissionChangeFacts, MemberInstanceId, MembershipCredential, MembershipCredentialId,
    MembershipDecisionId, MembershipEventId, MembershipHistoryV2Error, RemovalDecision,
    ADMISSION_SECURITY_COMMITMENT_FORMAT_V1,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseMembershipHistoryPosition {
    pub event_id: Option<MembershipEventId>,
    pub depth: u64,
    pub history_digest: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionSecurityCommitmentV1 {
    pub commitment_format_version: u16,
    pub lineage_id: String,
    pub mls_group_id: Vec<u8>,
    pub attempt_id: [u8; 32],
    pub base_history_position: BaseMembershipHistoryPosition,
    pub candidate_core_digest: [u8; 32],
    pub ciphersuite: u16,
    pub base_epoch: u64,
    pub target_epoch: u64,
    pub commit_digest: [u8; 32],
    pub group_context_digest: [u8; 32],
    pub member_credentials_digest: [u8; 32],
    pub key_catalog_digest: [u8; 32],
    pub admission_bundle_digest: [u8; 32],
    pub security_commitment_id: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "baseline_kind")]
pub enum MembershipActivationBaselineV2 {
    Established {
        lineage_id: String,
        head_event_id: MembershipEventId,
        head_depth: u64,
        current_members: Vec<(AdmissionChangeFacts, MembershipCredential)>,
    },
}

impl MembershipActivationBaselineV2 {
    pub(super) fn lineage_id(&self) -> &str {
        match self {
            Self::Established { lineage_id, .. } => lineage_id,
        }
    }

    pub(super) fn head_and_depth(&self) -> (MembershipEventId, u64) {
        match self {
            Self::Established {
                head_event_id,
                head_depth,
                ..
            } => (*head_event_id, *head_depth),
        }
    }

    pub(super) fn current_member_ids(&self) -> BTreeSet<MemberInstanceId> {
        match self {
            Self::Established {
                current_members, ..
            } => current_members
                .iter()
                .map(|(facts, _)| facts.member_instance)
                .collect(),
        }
    }
}

impl AdmissionSecurityCommitmentV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        commitment_format_version: u16,
        lineage_id: String,
        mls_group_id: Vec<u8>,
        attempt_id: [u8; 32],
        base_history_position: BaseMembershipHistoryPosition,
        candidate_core_digest: [u8; 32],
        ciphersuite: u16,
        base_epoch: u64,
        target_epoch: u64,
        commit_digest: [u8; 32],
        group_context_digest: [u8; 32],
        member_credentials_digest: [u8; 32],
        key_catalog_digest: [u8; 32],
        admission_bundle_digest: [u8; 32],
    ) -> Result<Self, MembershipHistoryV2Error> {
        if commitment_format_version != ADMISSION_SECURITY_COMMITMENT_FORMAT_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if lineage_id.is_empty()
            || mls_group_id.is_empty()
            || target_epoch != base_epoch.saturating_add(1)
        {
            return Err(MembershipHistoryV2Error::InvalidSecurityCommitment);
        }
        let mut commitment = Self {
            commitment_format_version,
            lineage_id,
            mls_group_id,
            attempt_id,
            base_history_position,
            candidate_core_digest,
            ciphersuite,
            base_epoch,
            target_epoch,
            commit_digest,
            group_context_digest,
            member_credentials_digest,
            key_catalog_digest,
            admission_bundle_digest,
            security_commitment_id: [0; 32],
        };
        commitment.security_commitment_id = Sha256::digest(commitment.canonical_content()).into();
        commitment.validate()?;
        Ok(commitment)
    }

    pub fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.commitment_format_version != ADMISSION_SECURITY_COMMITMENT_FORMAT_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.lineage_id.is_empty()
            || self.mls_group_id.is_empty()
            || self.target_epoch != self.base_epoch.saturating_add(1)
            || self.security_commitment_id
                != <[u8; 32]>::from(Sha256::digest(self.canonical_content()))
        {
            return Err(MembershipHistoryV2Error::InvalidSecurityCommitment);
        }
        Ok(())
    }

    pub(super) fn canonical_content(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard/admission-security-commitment/v1\0");
        bytes.extend_from_slice(&self.commitment_format_version.to_be_bytes());
        append_field(&mut bytes, self.lineage_id.as_bytes());
        append_field(&mut bytes, &self.mls_group_id);
        bytes.extend_from_slice(&self.attempt_id);
        append_optional_event_id(&mut bytes, self.base_history_position.event_id);
        bytes.extend_from_slice(&self.base_history_position.depth.to_be_bytes());
        bytes.extend_from_slice(&self.base_history_position.history_digest);
        bytes.extend_from_slice(&self.candidate_core_digest);
        bytes.extend_from_slice(&self.ciphersuite.to_be_bytes());
        bytes.extend_from_slice(&self.base_epoch.to_be_bytes());
        bytes.extend_from_slice(&self.target_epoch.to_be_bytes());
        bytes.extend_from_slice(&self.commit_digest);
        bytes.extend_from_slice(&self.group_context_digest);
        bytes.extend_from_slice(&self.member_credentials_digest);
        bytes.extend_from_slice(&self.key_catalog_digest);
        bytes.extend_from_slice(&self.admission_bundle_digest);
        bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipAdmissionV2 {
    pub facts: AdmissionChangeFacts,
    pub membership_credential: MembershipCredential,
    pub resume_public_key_digest: [u8; 32],
    pub security_commitment_id: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipOperationV2 {
    AddDevice { admission: MembershipAdmissionV2 },
    RemoveDevice { member: MemberInstanceId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipEventV2 {
    pub event_format_version: u16,
    pub lineage_id: String,
    pub parent_event_id: Option<MembershipEventId>,
    pub parent_depth: u64,
    pub operation_id: [u8; 16],
    pub author_member_instance_id: MemberInstanceId,
    pub author_credential_id: MembershipCredentialId,
    pub author_signature_algorithm_version: u16,
    pub operation: MembershipOperationV2,
    pub resulting_members_digest: [u8; 32],
    pub security_state_digest: [u8; 32],
    pub security_update_payload: Vec<u8>,
    pub admission_bundle_digest: Option<[u8; 32]>,
    pub signature: Vec<u8>,
}

impl MembershipEventV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_format_version: u16,
        lineage_id: String,
        parent_event_id: Option<MembershipEventId>,
        parent_depth: u64,
        operation_id: [u8; 16],
        author_member_instance_id: MemberInstanceId,
        author_credential_id: MembershipCredentialId,
        author_signature_algorithm_version: u16,
        operation: MembershipOperationV2,
        resulting_members_digest: [u8; 32],
        security_state_digest: [u8; 32],
        security_update_payload: Vec<u8>,
        admission_bundle_digest: Option<[u8; 32]>,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            event_format_version,
            lineage_id,
            parent_event_id,
            parent_depth,
            operation_id,
            author_member_instance_id,
            author_credential_id,
            author_signature_algorithm_version,
            operation,
            resulting_members_digest,
            security_state_digest,
            security_update_payload,
            admission_bundle_digest,
            signature,
        }
    }

    pub fn event_id(&self) -> MembershipEventId {
        MembershipEventId::from_bytes(Sha256::digest(self.signing_payload()).into())
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard-membership-event/v2\0");
        bytes.extend_from_slice(&self.event_format_version.to_be_bytes());
        append_field(&mut bytes, self.lineage_id.as_bytes());
        append_optional_event_id(&mut bytes, self.parent_event_id);
        bytes.extend_from_slice(&self.parent_depth.to_be_bytes());
        bytes.extend_from_slice(&self.operation_id);
        bytes.extend_from_slice(self.author_member_instance_id.as_bytes());
        bytes.extend_from_slice(self.author_credential_id.as_bytes());
        bytes.extend_from_slice(&self.author_signature_algorithm_version.to_be_bytes());
        append_operation(&mut bytes, &self.operation);
        bytes.extend_from_slice(&self.resulting_members_digest);
        bytes.extend_from_slice(&self.security_state_digest);
        append_field(&mut bytes, &self.security_update_payload);
        append_optional_digest(&mut bytes, self.admission_bundle_digest);
        bytes
    }

    /// Stable candidate input shared by sponsor security preparation and
    /// joiner verification. Security outputs and the final signature are
    /// deliberately excluded so the commitment can be formed without a
    /// circular event-id dependency.
    pub fn admission_candidate_core_digest(
        &self,
        attempt_id: [u8; 32],
        candidate_key_package: &[u8],
    ) -> Result<[u8; 32], MembershipHistoryV2Error> {
        let MembershipOperationV2::AddDevice { admission } = &self.operation else {
            return Err(MembershipHistoryV2Error::InvalidOperation);
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard/admission-candidate-core/v1\0");
        bytes.extend_from_slice(&attempt_id);
        bytes.extend_from_slice(&self.event_format_version.to_be_bytes());
        append_field(&mut bytes, self.lineage_id.as_bytes());
        append_optional_event_id(&mut bytes, self.parent_event_id);
        bytes.extend_from_slice(&self.parent_depth.to_be_bytes());
        bytes.extend_from_slice(&self.operation_id);
        bytes.extend_from_slice(self.author_member_instance_id.as_bytes());
        bytes.extend_from_slice(self.author_credential_id.as_bytes());
        bytes.extend_from_slice(&self.author_signature_algorithm_version.to_be_bytes());
        append_field(&mut bytes, &admission.facts.signing_payload());
        bytes.extend_from_slice(
            &admission
                .membership_credential
                .credential_format_version
                .to_be_bytes(),
        );
        bytes.extend_from_slice(
            &admission
                .membership_credential
                .signature_algorithm_version
                .to_be_bytes(),
        );
        append_field(&mut bytes, &admission.membership_credential.public_key);
        bytes.extend_from_slice(admission.membership_credential.credential_id.as_bytes());
        bytes.extend_from_slice(&admission.resume_public_key_digest);
        bytes.extend_from_slice(&self.resulting_members_digest);
        append_field(&mut bytes, candidate_key_package);
        Ok(Sha256::digest(bytes).into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipDecisionV2 {
    pub decision_format_version: u16,
    pub lineage_id: String,
    pub removal_event_id: MembershipEventId,
    pub decided_by_member_instance_id: MemberInstanceId,
    pub decider_credential_id: MembershipCredentialId,
    pub signature_algorithm_version: u16,
    pub decision: RemovalDecision,
    pub observed_applied_head: Option<MembershipEventId>,
    pub resulting_members_digest: [u8; 32],
    pub decision_nonce: [u8; 16],
    pub signature: Vec<u8>,
}

impl MembershipDecisionV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        decision_format_version: u16,
        lineage_id: String,
        removal_event_id: MembershipEventId,
        decided_by_member_instance_id: MemberInstanceId,
        decider_credential_id: MembershipCredentialId,
        signature_algorithm_version: u16,
        decision: RemovalDecision,
        observed_applied_head: Option<MembershipEventId>,
        resulting_members_digest: [u8; 32],
        decision_nonce: [u8; 16],
        signature: Vec<u8>,
    ) -> Self {
        Self {
            decision_format_version,
            lineage_id,
            removal_event_id,
            decided_by_member_instance_id,
            decider_credential_id,
            signature_algorithm_version,
            decision,
            observed_applied_head,
            resulting_members_digest,
            decision_nonce,
            signature,
        }
    }

    pub fn decision_id(&self) -> MembershipDecisionId {
        MembershipDecisionId::from_bytes(Sha256::digest(self.signing_payload()).into())
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard-membership-decision/v2\0");
        bytes.extend_from_slice(&self.decision_format_version.to_be_bytes());
        append_field(&mut bytes, self.lineage_id.as_bytes());
        bytes.extend_from_slice(self.removal_event_id.as_bytes());
        bytes.extend_from_slice(self.decided_by_member_instance_id.as_bytes());
        bytes.extend_from_slice(self.decider_credential_id.as_bytes());
        bytes.extend_from_slice(&self.signature_algorithm_version.to_be_bytes());
        bytes.push(match self.decision {
            RemovalDecision::Accept => 1,
            RemovalDecision::Reject => 2,
        });
        append_optional_event_id(&mut bytes, self.observed_applied_head);
        bytes.extend_from_slice(&self.resulting_members_digest);
        bytes.extend_from_slice(&self.decision_nonce);
        bytes
    }
}
