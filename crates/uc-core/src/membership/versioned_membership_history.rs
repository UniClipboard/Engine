use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::DeviceId;

use super::{
    AdmissionChangeFacts, MemberInstanceId, MembershipDecisionId, MembershipEventId,
    RemovalDecision,
};

pub const MEMBERSHIP_CREDENTIAL_FORMAT_V1: u16 = 1;
pub const ED25519_SIGNATURE_ALGORITHM_V1: u16 = 1;
pub const MEMBERSHIP_EVENT_FORMAT_V2: u16 = 2;
pub const MEMBERSHIP_DECISION_FORMAT_V2: u16 = 2;
pub const ADMISSION_SECURITY_COMMITMENT_FORMAT_V1: u16 = 1;
pub const PREPARED_ADMISSION_PROOF_FORMAT_V1: u16 = 1;
pub const ADMISSION_COMPLETION_FORMAT_V1: u16 = 1;
pub const MEMBERSHIP_HISTORY_EXCHANGE_FORMAT_V2: u16 = 2;
pub const MAX_MEMBERSHIP_HISTORY_FRAME_SIZE: usize = 4 * 1024 * 1024;
pub const MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE: usize = 256;
const MEMBERSHIP_HISTORY_PAGE_FRAME_OVERHEAD: usize = 2;
const ACTIVATION_RECEIPT_FORMAT_V1: u16 = 1;
const ACTIVATION_RECEIPT_RECORD_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MembershipCredentialId([u8; 32]);

impl MembershipCredentialId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipCredential {
    pub credential_format_version: u16,
    pub signature_algorithm_version: u16,
    pub public_key: Vec<u8>,
    pub credential_id: MembershipCredentialId,
}

impl MembershipCredential {
    pub fn new(signature_algorithm_version: u16, public_key: Vec<u8>) -> Self {
        let credential_format_version = MEMBERSHIP_CREDENTIAL_FORMAT_V1;
        let credential_id = credential_id(
            credential_format_version,
            signature_algorithm_version,
            &public_key,
        );
        Self {
            credential_format_version,
            signature_algorithm_version,
            public_key,
            credential_id,
        }
    }

    pub fn member_instance_id(&self, device_id: &DeviceId) -> MemberInstanceId {
        MemberInstanceId::derive(device_id.as_str(), &self.public_key)
    }

    pub fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.credential_format_version != MEMBERSHIP_CREDENTIAL_FORMAT_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.public_key.is_empty()
            || self.credential_id
                != credential_id(
                    self.credential_format_version,
                    self.signature_algorithm_version,
                    &self.public_key,
                )
        {
            return Err(MembershipHistoryV2Error::InvalidCredential);
        }
        Ok(())
    }
}

fn credential_id(
    credential_format_version: u16,
    signature_algorithm_version: u16,
    public_key: &[u8],
) -> MembershipCredentialId {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-credential/v1\0");
    hasher.update(credential_format_version.to_be_bytes());
    hasher.update(signature_algorithm_version.to_be_bytes());
    hasher.update((public_key.len() as u64).to_be_bytes());
    hasher.update(public_key);
    MembershipCredentialId(hasher.finalize().into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoricalMembershipSignatureError {
    UnsupportedAlgorithm,
    VerificationFailed,
}

impl fmt::Display for HistoricalMembershipSignatureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedAlgorithm => "membership signature algorithm is not supported",
            Self::VerificationFailed => "membership signature verification failed",
        })
    }
}

impl std::error::Error for HistoricalMembershipSignatureError {}

pub trait HistoricalMembershipSignatureVerifier: Send + Sync {
    fn verify(
        &self,
        signature_algorithm_version: u16,
        public_key: &[u8],
        payload: &[u8],
        signature: &[u8],
    ) -> Result<bool, HistoricalMembershipSignatureError>;
}

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
    fn lineage_id(&self) -> &str {
        match self {
            Self::Established { lineage_id, .. } => lineage_id,
        }
    }

    fn head_and_depth(&self) -> (MembershipEventId, u64) {
        match self {
            Self::Established {
                head_event_id,
                head_depth,
                ..
            } => (*head_event_id, *head_depth),
        }
    }

    fn current_member_ids(&self) -> BTreeSet<MemberInstanceId> {
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

    fn canonical_content(&self) -> Vec<u8> {
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
    fn new(receipt: AdmissionActivationReceipt) -> Self {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipHistoryV2ReceiveOutcome {
    Applied,
    AlreadyKnown,
    Diverged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipActivationReceiptStoreOutcome {
    Stored,
    AlreadyKnown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipDecisionStoreOutcome {
    Stored,
    AlreadyKnown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MembershipHistoryPageRecordCountsV2 {
    pub events: usize,
    pub activation_receipts: usize,
    pub decisions: usize,
}

impl MembershipHistoryPageRecordCountsV2 {
    fn total(self) -> usize {
        self.events + self.activation_receipts + self.decisions
    }
}

/// One deterministic, bounded fragment of a complete V2 history image.
/// Pages remain untrusted until the complete transfer is reassembled and
/// verified through [`VersionedMembershipHistory::import_exchange_pages_v2`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipHistoryPageV2 {
    exchange_format_version: u16,
    transfer_id: [u8; 32],
    page_index: u32,
    page_count: u32,
    lineage_id: String,
    position: BaseMembershipHistoryPosition,
    sender_admission: AdmissionChangeFacts,
    events: Vec<MembershipEventV2>,
    activation_receipts: Vec<AdmissionActivationReceipt>,
    decisions: Vec<MembershipDecisionV2>,
    activation_baseline: Option<PersistedActivationBaselineV2>,
    known_head: Option<MembershipEventId>,
}

const MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V3: u16 = 3;
pub const MAX_MEMBERSHIP_HISTORY_SUFFIX_PAGES: usize = 64;

/// V3 只携带 `base_position` 之后的连续记录；接收方必须从完全匹配的 base 原子应用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipHistorySuffixPageV3 {
    format_version: u16,
    transfer_id: [u8; 32],
    page_index: u32,
    page_count: u32,
    lineage_id: String,
    base_position: BaseMembershipHistoryPosition,
    target_position: BaseMembershipHistoryPosition,
    sender_admission: AdmissionChangeFacts,
    events: Vec<MembershipEventV2>,
    activation_receipts: Vec<AdmissionActivationReceipt>,
    decisions: Vec<MembershipDecisionV2>,
}

impl MembershipHistorySuffixPageV3 {
    pub fn transfer_id(&self) -> [u8; 32] {
        self.transfer_id
    }

    pub fn page_index(&self) -> u32 {
        self.page_index
    }

    pub fn page_count(&self) -> u32 {
        self.page_count
    }

    pub fn base_position(&self) -> &BaseMembershipHistoryPosition {
        &self.base_position
    }

    pub fn target_position(&self) -> &BaseMembershipHistoryPosition {
        &self.target_position
    }

    pub fn sender_admission(&self) -> &AdmissionChangeFacts {
        &self.sender_admission
    }

    pub fn validate_envelope(&self) -> Result<(), MembershipHistoryV2Error> {
        let record_count =
            self.events.len() + self.activation_receipts.len() + self.decisions.len();
        if self.format_version != MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V3
            || self.page_count == 0
            || self.page_index >= self.page_count
            || record_count != 1
            || postcard::to_stdvec(self)
                .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?
                .len()
                > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        Ok(())
    }
}

impl MembershipHistoryPageV2 {
    pub fn validate_envelope(&self) -> Result<(), MembershipHistoryV2Error> {
        let counts = self.record_counts();
        if self.exchange_format_version != MEMBERSHIP_HISTORY_EXCHANGE_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.page_count == 0
            || self.page_index >= self.page_count
            || counts.events > MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE
            || counts.activation_receipts > MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE
            || counts.decisions > MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE
            || self.encoded_frame_size()? > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE
            || (self.page_index != 0
                && (self.activation_baseline.is_some() || self.known_head.is_some()))
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        Ok(())
    }

    fn encoded_frame_size(&self) -> Result<usize, MembershipHistoryV2Error> {
        postcard::to_stdvec(self)
            .map(|page| page.len() + MEMBERSHIP_HISTORY_PAGE_FRAME_OVERHEAD)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)
    }

    pub fn transfer_id(&self) -> [u8; 32] {
        self.transfer_id
    }

    pub fn page_index(&self) -> u32 {
        self.page_index
    }

    pub fn page_count(&self) -> u32 {
        self.page_count
    }

    pub fn sender_admission(&self) -> &AdmissionChangeFacts {
        &self.sender_admission
    }

    pub fn record_counts(&self) -> MembershipHistoryPageRecordCountsV2 {
        MembershipHistoryPageRecordCountsV2 {
            events: self.events.len(),
            activation_receipts: self.activation_receipts.len(),
            decisions: self.decisions.len(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MembershipHistoryV2Ack {
    Continue {
        transfer_id: [u8; 32],
        next_page_index: u32,
    },
    Consistent,
    UpdatesApplied,
    Diverged,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipHistoryV2Error {
    UpgradeRequired,
    InvalidLineage,
    InvalidGenesis,
    UnknownParent,
    InvalidParentDepth,
    OperationReplay,
    UnauthorizedAuthor,
    AwaitingActivationReceipt,
    InvalidCredential,
    CredentialConflict,
    InvalidSignature,
    UnsupportedSignatureAlgorithm,
    InvalidSecurityCommitment,
    InvalidActivationBaseline,
    InvalidOperation,
    ResultingMembersDigestMismatch,
    MissingMembershipEvent(MembershipEventId),
    InvalidActivationReceipt,
    ActivationReceiptConflict,
    UnknownRemoval,
    InvalidDecision,
    DecisionConflict,
    InvalidPersistedHistory,
}

impl fmt::Display for MembershipHistoryV2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UpgradeRequired => "membership history version requires an upgrade",
            Self::InvalidLineage => "membership history lineage is invalid",
            Self::InvalidGenesis => "membership history genesis is invalid",
            Self::UnknownParent => "membership history parent is unknown",
            Self::InvalidParentDepth => "membership history parent depth is invalid",
            Self::OperationReplay => "membership operation identifier was already used",
            Self::UnauthorizedAuthor => "membership event author was not authorized at the parent",
            Self::AwaitingActivationReceipt => {
                "membership event author is awaiting activation proof"
            }
            Self::InvalidCredential => "membership credential is invalid",
            Self::CredentialConflict => "membership credential conflicts with retained history",
            Self::InvalidSignature => "membership history signature is invalid",
            Self::UnsupportedSignatureAlgorithm => {
                "membership history signature algorithm is not supported"
            }
            Self::InvalidSecurityCommitment => "admission security commitment is invalid",
            Self::InvalidActivationBaseline => "membership activation baseline is invalid",
            Self::InvalidOperation => "membership history operation is invalid at the parent",
            Self::ResultingMembersDigestMismatch => {
                "membership event resulting members digest does not match"
            }
            Self::MissingMembershipEvent(_) => {
                "membership activation receipt references an unknown event"
            }
            Self::InvalidActivationReceipt => "membership activation receipt is invalid",
            Self::ActivationReceiptConflict => {
                "membership activation receipt conflicts with retained history"
            }
            Self::UnknownRemoval => "membership decision references an unknown removal",
            Self::InvalidDecision => "membership decision is invalid at the removal parent",
            Self::DecisionConflict => "membership decision conflicts with retained history",
            Self::InvalidPersistedHistory => "persisted membership history is invalid",
        })
    }
}

impl std::error::Error for MembershipHistoryV2Error {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MembershipHistorySnapshot {
    members: BTreeSet<MemberInstanceId>,
    active_members: BTreeSet<MemberInstanceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionedMembershipHistory {
    lineage_id: String,
    events: BTreeMap<MembershipEventId, MembershipEventV2>,
    snapshots: BTreeMap<MembershipEventId, MembershipHistorySnapshot>,
    credentials: BTreeMap<MemberInstanceId, MembershipCredential>,
    operation_ids: BTreeSet<[u8; 16]>,
    activation_receipts: BTreeMap<MembershipEventId, MembershipActivationReceiptRecord>,
    peer_decisions: BTreeMap<(MembershipEventId, MemberInstanceId), MembershipDecisionV2>,
    activation_baseline: Option<MembershipActivationBaselineV2>,
    known_head: Option<MembershipEventId>,
}

const PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2: u16 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum PersistedActivationBaselineV2 {
    Established {
        lineage_id: String,
        head_event_id: MembershipEventId,
        head_depth: u64,
        current_members: Vec<(AdmissionChangeFacts, MembershipCredential)>,
    },
}

impl From<MembershipActivationBaselineV2> for PersistedActivationBaselineV2 {
    fn from(value: MembershipActivationBaselineV2) -> Self {
        match value {
            MembershipActivationBaselineV2::Established {
                lineage_id,
                head_event_id,
                head_depth,
                current_members,
            } => Self::Established {
                lineage_id,
                head_event_id,
                head_depth,
                current_members,
            },
        }
    }
}

impl From<PersistedActivationBaselineV2> for MembershipActivationBaselineV2 {
    fn from(value: PersistedActivationBaselineV2) -> Self {
        match value {
            PersistedActivationBaselineV2::Established {
                lineage_id,
                head_event_id,
                head_depth,
                current_members,
            } => Self::Established {
                lineage_id,
                head_event_id,
                head_depth,
                current_members,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PersistedMembershipHistoryV2 {
    format_version: u16,
    lineage_id: String,
    events: Vec<MembershipEventV2>,
    activation_receipts: Vec<AdmissionActivationReceipt>,
    peer_decisions: Vec<MembershipDecisionV2>,
    activation_baseline: Option<PersistedActivationBaselineV2>,
    known_head: Option<MembershipEventId>,
}

impl VersionedMembershipHistory {
    pub fn lineage_id(&self) -> &str {
        &self.lineage_id
    }

    pub fn encode_persisted_v2(&self) -> Result<Vec<u8>, MembershipHistoryV2Error> {
        let persisted = PersistedMembershipHistoryV2 {
            format_version: PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2,
            lineage_id: self.lineage_id.clone(),
            events: self.events.values().cloned().collect(),
            activation_receipts: self
                .activation_receipts
                .values()
                .map(|record| record.activation_receipt.clone())
                .collect(),
            peer_decisions: self.peer_decisions.values().cloned().collect(),
            activation_baseline: self.activation_baseline.clone().map(Into::into),
            known_head: self.known_head,
        };
        postcard::to_stdvec(&persisted)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)
    }

    fn validate_exchange_sender(
        &self,
        sender_admission: &AdmissionChangeFacts,
    ) -> Result<BaseMembershipHistoryPosition, MembershipHistoryV2Error> {
        let sender_member_instance_id = sender_admission.member_instance;
        let credential = self
            .credential_for(sender_member_instance_id)
            .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
        if credential.member_instance_id(&sender_admission.device_id) != sender_member_instance_id
            || (!self.active_members().contains(&sender_member_instance_id)
                && !self.has_removal_decision_by(sender_member_instance_id))
        {
            return Err(MembershipHistoryV2Error::UnauthorizedAuthor);
        }
        self.current_position()
    }

    pub fn export_reconciliation_pages_v2(
        &self,
        sender_admission: AdmissionChangeFacts,
    ) -> Result<Vec<MembershipHistoryPageV2>, MembershipHistoryV2Error> {
        let sender_member = sender_admission.member_instance;
        let mut exchange_history = self.clone();
        exchange_history
            .peer_decisions
            .retain(|_, decision| decision.decided_by_member_instance_id == sender_member);
        exchange_history.export_pages_v2(sender_admission)
    }

    pub fn export_suffix_pages_v3(
        &self,
        sender_admission: AdmissionChangeFacts,
        base_position: BaseMembershipHistoryPosition,
    ) -> Result<Vec<MembershipHistorySuffixPageV3>, MembershipHistoryV2Error> {
        let target_position = self.validate_exchange_sender(&sender_admission)?;
        if base_position == target_position {
            return Ok(Vec::new());
        }
        let base_event = base_position
            .event_id
            .ok_or(MembershipHistoryV2Error::UnknownParent)?;
        if self.depth(base_event) != Some(base_position.depth) {
            return Err(MembershipHistoryV2Error::UnknownParent);
        }
        let mut suffix_events = Vec::new();
        let mut cursor = self.known_head;
        while cursor != Some(base_event) {
            let event_id = cursor.ok_or(MembershipHistoryV2Error::UnknownParent)?;
            let event = self
                .events
                .get(&event_id)
                .cloned()
                .ok_or(MembershipHistoryV2Error::UnknownParent)?;
            cursor = event.parent_event_id;
            suffix_events.push(event);
        }
        suffix_events.reverse();

        let mut records = Vec::new();
        for event in suffix_events {
            let event_id = event.event_id();
            records.push(MembershipHistorySuffixRecordV3::Event(event));
            if let Some(receipt) = self.activation_receipts.get(&event_id) {
                records.push(MembershipHistorySuffixRecordV3::ActivationReceipt(
                    receipt.activation_receipt.clone(),
                ));
            }
        }
        records.extend(
            self.peer_decisions
                .values()
                .filter(|decision| {
                    decision.decided_by_member_instance_id == sender_admission.member_instance
                })
                .cloned()
                .map(MembershipHistorySuffixRecordV3::Decision),
        );
        if records.is_empty() || records.len() > MAX_MEMBERSHIP_HISTORY_SUFFIX_PAGES {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        let transfer_id = suffix_transfer_id_v3(
            &self.lineage_id,
            &base_position,
            &target_position,
            &sender_admission,
            &records,
        )?;
        let page_count = u32::try_from(records.len())
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        records
            .into_iter()
            .enumerate()
            .map(|(index, record)| {
                let (events, activation_receipts, decisions) = match record {
                    MembershipHistorySuffixRecordV3::Event(event) => {
                        (vec![event], Vec::new(), Vec::new())
                    }
                    MembershipHistorySuffixRecordV3::ActivationReceipt(receipt) => {
                        (Vec::new(), vec![receipt], Vec::new())
                    }
                    MembershipHistorySuffixRecordV3::Decision(decision) => {
                        (Vec::new(), Vec::new(), vec![decision])
                    }
                };
                let page = MembershipHistorySuffixPageV3 {
                    format_version: MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V3,
                    transfer_id,
                    page_index: u32::try_from(index)
                        .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?,
                    page_count,
                    lineage_id: self.lineage_id.clone(),
                    base_position: base_position.clone(),
                    target_position: target_position.clone(),
                    sender_admission: sender_admission.clone(),
                    events,
                    activation_receipts,
                    decisions,
                };
                page.validate_envelope()?;
                Ok(page)
            })
            .collect()
    }

    pub fn apply_suffix_pages_v3(
        &mut self,
        pages: &[MembershipHistorySuffixPageV3],
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<bool, MembershipHistoryV2Error> {
        let first = pages
            .first()
            .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
        if pages.len() != first.page_count as usize
            || pages.len() > MAX_MEMBERSHIP_HISTORY_SUFFIX_PAGES
            || self.lineage_id != first.lineage_id
            || self.current_position()? != first.base_position
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        let mut ordered = pages.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|page| page.page_index);
        let mut records = Vec::with_capacity(ordered.len());
        for (index, page) in ordered.iter().enumerate() {
            page.validate_envelope()?;
            if page.page_index as usize != index
                || page.page_count != first.page_count
                || page.transfer_id != first.transfer_id
                || page.lineage_id != first.lineage_id
                || page.base_position != first.base_position
                || page.target_position != first.target_position
                || page.sender_admission != first.sender_admission
            {
                return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
            }
            if let Some(event) = page.events.first() {
                records.push(MembershipHistorySuffixRecordV3::Event(event.clone()));
                self.verify_and_receive_event(event.clone(), verifier)?;
            } else if let Some(receipt) = page.activation_receipts.first() {
                records.push(MembershipHistorySuffixRecordV3::ActivationReceipt(
                    receipt.clone(),
                ));
                self.verify_and_record_activation_receipt(receipt.clone(), verifier)?;
            } else if let Some(decision) = page.decisions.first() {
                records.push(MembershipHistorySuffixRecordV3::Decision(decision.clone()));
                self.verify_and_record_peer_decision(decision.clone(), verifier)?;
            }
        }
        if suffix_transfer_id_v3(
            &first.lineage_id,
            &first.base_position,
            &first.target_position,
            &first.sender_admission,
            &records,
        )? != first.transfer_id
            || self.current_position()? != first.target_position
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        Ok(true)
    }

    fn export_pages_v2(
        &self,
        sender_admission: AdmissionChangeFacts,
    ) -> Result<Vec<MembershipHistoryPageV2>, MembershipHistoryV2Error> {
        let position = self.validate_exchange_sender(&sender_admission)?;
        let persisted_bytes = self.encode_persisted_v2()?;
        let transfer_id = history_transfer_id(&persisted_bytes);
        let persisted: PersistedMembershipHistoryV2 = postcard::from_bytes(&persisted_bytes)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        let metadata = MembershipHistoryPageMetadata {
            transfer_id,
            lineage_id: &persisted.lineage_id,
            position: &position,
            sender_admission: &sender_admission,
            activation_baseline: persisted.activation_baseline.as_ref(),
            known_head: persisted.known_head,
        };
        let mut pages = vec![empty_history_page(&metadata, 0)?];
        for record in persisted
            .events
            .iter()
            .map(MembershipHistoryPageRecord::Event)
            .chain(
                persisted
                    .activation_receipts
                    .iter()
                    .map(MembershipHistoryPageRecord::ActivationReceipt),
            )
            .chain(
                persisted
                    .peer_decisions
                    .iter()
                    .map(MembershipHistoryPageRecord::Decision),
            )
        {
            append_history_page_record(&mut pages, &metadata, record)?;
        }
        let page_count = u32::try_from(pages.len())
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        for page in &mut pages {
            page.page_count = page_count;
            page.validate_envelope()?;
        }
        Ok(pages)
    }

    pub fn import_exchange_pages_v2(
        pages: &[MembershipHistoryPageV2],
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<Self, MembershipHistoryV2Error> {
        let first = pages
            .first()
            .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
        if first.page_count == 0 || pages.len() != first.page_count as usize {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        let mut ordered = pages.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|page| page.page_index);
        for (expected_index, page) in ordered.iter().enumerate() {
            page.validate_envelope()?;
            if page.page_index as usize != expected_index
                || page.page_count != first.page_count
                || page.transfer_id != first.transfer_id
                || page.lineage_id != first.lineage_id
                || page.position != first.position
                || page.sender_admission != first.sender_admission
            {
                return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
            }
        }
        let mut persisted = PersistedMembershipHistoryV2 {
            format_version: PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2,
            lineage_id: first.lineage_id.clone(),
            events: Vec::new(),
            activation_receipts: Vec::new(),
            peer_decisions: Vec::new(),
            activation_baseline: first.activation_baseline.clone(),
            known_head: first.known_head,
        };
        for page in ordered {
            persisted.events.extend(page.events.iter().cloned());
            persisted
                .activation_receipts
                .extend(page.activation_receipts.iter().cloned());
            persisted
                .peer_decisions
                .extend(page.decisions.iter().cloned());
        }
        let encoded = postcard::to_stdvec(&persisted)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        if history_transfer_id(&encoded) != first.transfer_id {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        let history = Self::decode_persisted_v2(&encoded, verifier)?;
        let sender_member = first.sender_admission.member_instance;
        let sender_credential = history
            .credential_for(sender_member)
            .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
        if history.lineage_id != first.lineage_id
            || history.current_position()? != first.position
            || sender_credential.member_instance_id(&first.sender_admission.device_id)
                != sender_member
            || (!history.active_members().contains(&sender_member)
                && !history.has_removal_decision_by(sender_member))
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        verify_signature(
            verifier,
            sender_credential,
            &first.sender_admission.signing_payload(),
            &first.sender_admission.identity_signature,
        )?;
        Ok(history)
    }

    pub fn is_complete_extension_of(&self, previous: &Self) -> bool {
        if self.lineage_id != previous.lineage_id
            || self.activation_baseline != previous.activation_baseline
            || !previous
                .events
                .iter()
                .all(|(id, event)| self.events.get(id) == Some(event))
            || !previous
                .activation_receipts
                .iter()
                .all(|(id, receipt)| self.activation_receipts.get(id) == Some(receipt))
            || !previous
                .peer_decisions
                .iter()
                .all(|(id, decision)| self.peer_decisions.get(id) == Some(decision))
        {
            return false;
        }
        let Some(previous_head) = previous.known_head else {
            return true;
        };
        let mut cursor = self.known_head;
        while let Some(event_id) = cursor {
            if event_id == previous_head {
                return true;
            }
            cursor = self
                .events
                .get(&event_id)
                .and_then(|event| event.parent_event_id);
        }
        false
    }

    pub fn is_authorized_active_member_extension_of(
        &self,
        previous: &Self,
        source_member: MemberInstanceId,
    ) -> bool {
        if self.lineage_id != previous.lineage_id
            || self.activation_baseline != previous.activation_baseline
            || !previous
                .events
                .iter()
                .all(|(id, event)| self.events.get(id) == Some(event))
            || !previous
                .activation_receipts
                .iter()
                .all(|(id, receipt)| self.activation_receipts.get(id) == Some(receipt))
            || self.peer_decisions.iter().any(|(id, decision)| {
                previous.peer_decisions.get(id) != Some(decision)
                    && decision.decided_by_member_instance_id != source_member
            })
        {
            return false;
        }
        let Some(previous_head) = previous.known_head else {
            return true;
        };
        let mut cursor = self.known_head;
        while let Some(event_id) = cursor {
            if event_id == previous_head {
                return true;
            }
            cursor = self
                .events
                .get(&event_id)
                .and_then(|event| event.parent_event_id);
        }
        false
    }

    pub fn decode_persisted_v2(
        bytes: &[u8],
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<Self, MembershipHistoryV2Error> {
        let mut persisted: PersistedMembershipHistoryV2 = postcard::from_bytes(bytes)
            .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
        if persisted.format_version != PERSISTED_MEMBERSHIP_HISTORY_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        let mut history = match persisted.activation_baseline.take() {
            Some(baseline) => Self::from_activation_baseline(baseline.into())?,
            None => Self::new(persisted.lineage_id.clone()),
        };
        history.verify_activation_baseline_signatures(verifier)?;
        if history.lineage_id != persisted.lineage_id {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        persisted
            .events
            .sort_by_key(|event| (event.parent_depth, event.event_id()));
        let mut receipts_by_event = BTreeMap::<_, Vec<_>>::new();
        for receipt in persisted.activation_receipts {
            receipts_by_event
                .entry(receipt.event_id)
                .or_default()
                .push(receipt);
        }
        for event in persisted.events {
            let event_id = event.event_id();
            history.verify_and_receive_event(event, verifier)?;
            if let Some(receipts) = receipts_by_event.remove(&event_id) {
                for receipt in receipts {
                    history.verify_and_record_activation_receipt(receipt, verifier)?;
                }
            }
        }
        for receipts in receipts_by_event.into_values() {
            for receipt in receipts {
                history.verify_and_record_activation_receipt(receipt, verifier)?;
            }
        }
        persisted
            .peer_decisions
            .sort_by_key(|decision| decision.decision_id());
        for decision in persisted.peer_decisions {
            history.verify_and_record_peer_decision(decision, verifier)?;
        }
        if persisted.known_head.is_some()
            && !persisted.known_head.is_some_and(|head| {
                history.snapshots.contains_key(&head)
                    || history
                        .activation_baseline
                        .as_ref()
                        .is_some_and(|baseline| baseline.head_and_depth().0 == head)
            })
        {
            return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
        }
        history.known_head = persisted.known_head;
        history.rebuild_snapshots()?;
        Ok(history)
    }

    pub fn new(lineage_id: String) -> Self {
        Self {
            lineage_id,
            events: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            credentials: BTreeMap::new(),
            operation_ids: BTreeSet::new(),
            activation_receipts: BTreeMap::new(),
            peer_decisions: BTreeMap::new(),
            activation_baseline: None,
            known_head: None,
        }
    }

    pub fn new_single_member_root(
        lineage_id: String,
        facts: AdmissionChangeFacts,
        credential: MembershipCredential,
    ) -> Result<Self, MembershipHistoryV2Error> {
        credential.validate()?;
        if lineage_id.is_empty()
            || facts.member_instance != credential.member_instance_id(&facts.device_id)
        {
            return Err(MembershipHistoryV2Error::InvalidActivationBaseline);
        }
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/membership-history-root/v2\0");
        hasher.update((lineage_id.len() as u64).to_be_bytes());
        hasher.update(lineage_id.as_bytes());
        let facts_payload = facts.signing_payload();
        hasher.update((facts_payload.len() as u64).to_be_bytes());
        hasher.update(facts_payload);
        hasher.update(credential.credential_id.as_bytes());
        Self::from_activation_baseline(MembershipActivationBaselineV2::Established {
            lineage_id,
            head_event_id: MembershipEventId::from_bytes(hasher.finalize().into()),
            head_depth: 0,
            current_members: vec![(facts, credential)],
        })
    }

    pub fn from_activation_baseline(
        mut activation_baseline: MembershipActivationBaselineV2,
    ) -> Result<Self, MembershipHistoryV2Error> {
        let lineage_id = activation_baseline.lineage_id().to_owned();
        let mut credential_index = BTreeMap::new();
        match &mut activation_baseline {
            MembershipActivationBaselineV2::Established {
                current_members, ..
            } => {
                current_members
                    .sort_by(|left, right| left.0.member_instance.cmp(&right.0.member_instance));
                for (facts, credential) in current_members.iter() {
                    credential.validate()?;
                    if facts.member_instance != credential.member_instance_id(&facts.device_id)
                        || credential_index
                            .insert(facts.member_instance, credential.clone())
                            .is_some()
                    {
                        return Err(MembershipHistoryV2Error::CredentialConflict);
                    }
                }
            }
        }
        if lineage_id.is_empty() || credential_index.is_empty() {
            return Err(MembershipHistoryV2Error::InvalidActivationBaseline);
        }
        let (head, _) = activation_baseline.head_and_depth();
        let mut history = Self {
            lineage_id,
            events: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            credentials: credential_index,
            operation_ids: BTreeSet::new(),
            activation_receipts: BTreeMap::new(),
            peer_decisions: BTreeMap::new(),
            activation_baseline: Some(activation_baseline),
            known_head: Some(head),
        };
        history.rebuild_snapshots()?;
        Ok(history)
    }

    pub fn depth(&self, event_id: MembershipEventId) -> Option<u64> {
        self.events
            .get(&event_id)
            .map(|event| event.parent_depth)
            .or_else(|| {
                self.activation_baseline.as_ref().and_then(|baseline| {
                    let (head, depth) = baseline.head_and_depth();
                    (head == event_id).then_some(depth)
                })
            })
    }

    pub fn credential_for(&self, member: MemberInstanceId) -> Option<&MembershipCredential> {
        self.credentials.get(&member)
    }

    pub fn admission_facts_for(&self, member: MemberInstanceId) -> Option<&AdmissionChangeFacts> {
        self.events
            .values()
            .find_map(|event| match &event.operation {
                MembershipOperationV2::AddDevice { admission }
                    if admission.facts.member_instance == member =>
                {
                    Some(&admission.facts)
                }
                MembershipOperationV2::AddDevice { .. }
                | MembershipOperationV2::RemoveDevice { .. } => None,
            })
            .or_else(|| match self.activation_baseline.as_ref()? {
                MembershipActivationBaselineV2::Established {
                    current_members, ..
                } => current_members
                    .iter()
                    .find_map(|(facts, _)| (facts.member_instance == member).then_some(facts)),
            })
    }

    pub fn current_position(
        &self,
    ) -> Result<BaseMembershipHistoryPosition, MembershipHistoryV2Error> {
        let event_id = self.known_head;
        let depth = event_id
            .and_then(|head| self.depth(head))
            .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
        let encoded = self.encode_persisted_v2()?;
        let mut hasher = Sha256::new();
        hasher.update(b"uniclipboard/membership-history-position/v1\0");
        hasher.update((encoded.len() as u64).to_be_bytes());
        hasher.update(encoded);
        Ok(BaseMembershipHistoryPosition {
            event_id,
            depth,
            history_digest: hasher.finalize().into(),
        })
    }

    /// 判断远端位置是否是本机已验证分支中的严格祖先。
    /// 摘要规划只用它阻止“新节点向旧节点索取旧历史”的反向覆盖。
    pub fn contains_strict_ancestor_position(
        &self,
        position: &BaseMembershipHistoryPosition,
    ) -> bool {
        self.known_head
            .and_then(|head| self.depth(head))
            .is_some_and(|local_depth| position.depth < local_depth)
            && position
                .event_id
                .is_some_and(|event_id| self.events.contains_key(&event_id))
    }

    pub fn effective_members(&self) -> BTreeSet<MemberInstanceId> {
        self.known_head
            .and_then(|head| self.snapshots.get(&head))
            .map(|snapshot| snapshot.members.clone())
            .unwrap_or_default()
    }

    pub fn active_members(&self) -> BTreeSet<MemberInstanceId> {
        self.known_head
            .and_then(|head| self.snapshots.get(&head))
            .map(|snapshot| snapshot.active_members.clone())
            .unwrap_or_default()
    }

    /// Resolves an effective member only from signed admission facts retained
    /// by the current history. External roster projections never grant
    /// membership or supply missing identity facts.
    pub fn effective_member_for_device(&self, device_id: &DeviceId) -> Option<MemberInstanceId> {
        self.effective_members().into_iter().find(|member| {
            self.admission_facts_for(*member)
                .is_some_and(|facts| &facts.device_id == device_id)
        })
    }

    pub fn effective_members_at(&self, event_id: MembershipEventId) -> BTreeSet<MemberInstanceId> {
        self.snapshots
            .get(&event_id)
            .map(|snapshot| snapshot.members.clone())
            .unwrap_or_default()
    }

    pub fn active_members_at(&self, event_id: MembershipEventId) -> BTreeSet<MemberInstanceId> {
        self.snapshots
            .get(&event_id)
            .map(|snapshot| snapshot.active_members.clone())
            .unwrap_or_default()
    }

    pub fn contains_event_id(&self, event_id: &[u8; 32]) -> bool {
        self.events
            .keys()
            .any(|candidate| candidate.as_bytes() == event_id)
    }

    pub fn device_for_member(
        &self,
        member: &MemberInstanceId,
        candidate_devices: &[DeviceId],
    ) -> Option<DeviceId> {
        self.events
            .values()
            .find_map(|event| match &event.operation {
                MembershipOperationV2::AddDevice { admission }
                    if admission.facts.member_instance == *member =>
                {
                    Some(admission.facts.device_id.clone())
                }
                _ => None,
            })
            .or_else(|| {
                let credential = self.credentials.get(member)?;
                candidate_devices
                    .iter()
                    .find(|device| credential.member_instance_id(device) == *member)
                    .cloned()
            })
    }

    pub fn has_admitted_device(
        &self,
        device_id: &DeviceId,
        candidate_devices: &[DeviceId],
    ) -> bool {
        self.credentials.keys().any(|member| {
            self.device_for_member(member, candidate_devices).as_ref() == Some(device_id)
        })
    }

    pub fn member_for_device(
        &self,
        device_id: &DeviceId,
        candidate_devices: &[DeviceId],
    ) -> Option<MemberInstanceId> {
        self.credentials.keys().copied().find(|member| {
            self.device_for_member(member, candidate_devices).as_ref() == Some(device_id)
        })
    }

    pub fn is_restricted_removed_member_extension_of(
        &self,
        previous: &Self,
        source_member: MemberInstanceId,
    ) -> bool {
        !previous.active_members().contains(&source_member)
            && self.is_authorized_decision_delivery_of(previous, source_member)
    }

    pub fn is_authorized_decision_delivery_of(
        &self,
        previous: &Self,
        source_member: MemberInstanceId,
    ) -> bool {
        self.lineage_id == previous.lineage_id
            && self.activation_baseline == previous.activation_baseline
            && (previous.active_members().contains(&source_member)
                || self.has_removal_decision_by(source_member))
            && self
                .events
                .iter()
                .all(|(id, event)| previous.events.get(id) == Some(event))
            && self
                .activation_receipts
                .iter()
                .all(|(id, receipt)| previous.activation_receipts.get(id) == Some(receipt))
            && self.peer_decisions.iter().all(|(id, decision)| {
                previous.peer_decisions.get(id) == Some(decision)
                    || decision.decided_by_member_instance_id == source_member
            })
    }

    fn has_removal_decision_by(&self, member: MemberInstanceId) -> bool {
        self.peer_decisions.values().any(|decision| {
            decision.decided_by_member_instance_id == member
                && self
                    .events
                    .get(&decision.removal_event_id)
                    .is_some_and(|event| {
                        matches!(event.operation, MembershipOperationV2::RemoveDevice { .. })
                    })
        })
    }

    pub fn decision_for(
        &self,
        removal_event_id: MembershipEventId,
        decided_by: MemberInstanceId,
    ) -> Option<&MembershipDecisionV2> {
        self.peer_decisions.get(&(removal_event_id, decided_by))
    }

    pub fn latest_decision_on_removal_authored_by(
        &self,
        author: MemberInstanceId,
        decided_by: MemberInstanceId,
    ) -> Option<RemovalDecision> {
        self.peer_decisions
            .values()
            .filter_map(|decision| {
                let event = self.events.get(&decision.removal_event_id)?;
                (event.author_member_instance_id == author
                    && decision.decided_by_member_instance_id == decided_by)
                    .then_some((event.parent_depth, decision.decision))
            })
            .max_by_key(|(depth, _)| *depth)
            .map(|(_, decision)| decision)
    }

    pub fn removal_decision_recipients_for(
        &self,
        decided_by: MemberInstanceId,
    ) -> BTreeSet<MemberInstanceId> {
        let mut recipients = BTreeSet::new();
        for decision in self
            .peer_decisions
            .values()
            .filter(|decision| decision.decided_by_member_instance_id == decided_by)
        {
            let Some(parent) = self
                .events
                .get(&decision.removal_event_id)
                .and_then(|event| event.parent_event_id)
            else {
                continue;
            };
            if let Some(snapshot) = self.snapshots.get(&parent) {
                recipients.extend(snapshot.members.iter().copied());
            }
        }
        recipients.remove(&decided_by);
        recipients
    }

    pub fn removal_choices_diverge(&self, left: MemberInstanceId, right: MemberInstanceId) -> bool {
        self.events.values().any(|event| {
            if !matches!(event.operation, MembershipOperationV2::RemoveDevice { .. }) {
                return false;
            }
            let choice = |member| {
                if event.author_member_instance_id == member {
                    Some(RemovalDecision::Accept)
                } else {
                    self.decision_for(event.event_id(), member)
                        .map(|decision| decision.decision)
                }
            };
            matches!((choice(left), choice(right)), (Some(left), Some(right)) if left != right)
        })
    }

    pub fn current_head(&self) -> Option<MembershipEventId> {
        self.known_head
    }

    pub fn event(&self, event_id: MembershipEventId) -> Option<&MembershipEventV2> {
        self.events.get(&event_id)
    }

    pub fn members_digest_at(&self, event_id: MembershipEventId) -> Option<[u8; 32]> {
        self.snapshots
            .get(&event_id)
            .map(|snapshot| members_digest(&snapshot.members))
    }

    pub fn pending_removal_decision(
        &self,
        local_member: MemberInstanceId,
    ) -> Option<MembershipEventId> {
        let current_head = self.known_head?;
        let current_members = self.snapshots.get(&current_head)?;
        if !current_members.members.contains(&local_member) {
            return None;
        }
        self.events
            .iter()
            .filter(|(_, event)| {
                event.parent_event_id == Some(current_head)
                    && event.author_member_instance_id != local_member
                    && matches!(event.operation, MembershipOperationV2::RemoveDevice { .. })
            })
            .map(|(event_id, _)| *event_id)
            .find(|event_id| !self.peer_decisions.contains_key(&(*event_id, local_member)))
    }

    /// Builds a rule-complete removal event for the current local branch.
    /// The caller must sign its payload before applying it to the history.
    pub fn create_unsigned_local_removal_event(
        &self,
        author: MemberInstanceId,
        author_credential: &MembershipCredential,
        target: MemberInstanceId,
        operation_id: [u8; 16],
        security_state_digest: [u8; 32],
    ) -> Result<MembershipEventV2, MembershipHistoryV2Error> {
        if self.credentials.get(&author) != Some(author_credential) {
            return Err(MembershipHistoryV2Error::InvalidCredential);
        }
        if !self.active_members().contains(&author) {
            return Err(MembershipHistoryV2Error::UnauthorizedAuthor);
        }
        if author == target || !self.effective_members().contains(&target) {
            return Err(MembershipHistoryV2Error::InvalidOperation);
        }

        let position = self.current_position()?;
        let operation = MembershipOperationV2::RemoveDevice { member: target };
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
            security_state_digest,
            Vec::new(),
            None,
            Vec::new(),
        ))
    }

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

    /// Builds a rule-complete local decision for the current pending removal.
    /// The caller must sign its payload before applying it to the history.
    pub fn create_unsigned_local_removal_decision(
        &self,
        removal_event_id: MembershipEventId,
        local_member: MemberInstanceId,
        local_credential: &MembershipCredential,
        decision: RemovalDecision,
        decision_nonce: [u8; 16],
    ) -> Result<MembershipDecisionV2, MembershipHistoryV2Error> {
        if self.pending_removal_decision(local_member) != Some(removal_event_id) {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        if self.credentials.get(&local_member) != Some(local_credential) {
            return Err(MembershipHistoryV2Error::InvalidCredential);
        }
        let removal = self
            .events
            .get(&removal_event_id)
            .ok_or(MembershipHistoryV2Error::UnknownRemoval)?;
        let parent_id = removal
            .parent_event_id
            .ok_or(MembershipHistoryV2Error::InvalidDecision)?;
        let resulting_members_digest = match decision {
            RemovalDecision::Accept => removal.resulting_members_digest,
            RemovalDecision::Reject => self
                .events
                .get(&parent_id)
                .map(|event| event.resulting_members_digest)
                .ok_or(MembershipHistoryV2Error::UnknownParent)?,
        };

        Ok(MembershipDecisionV2::new(
            MEMBERSHIP_DECISION_FORMAT_V2,
            self.lineage_id.clone(),
            removal_event_id,
            local_member,
            local_credential.credential_id,
            local_credential.signature_algorithm_version,
            decision,
            Some(parent_id),
            resulting_members_digest,
            decision_nonce,
            Vec::new(),
        ))
    }

    /// Verifies and applies a signed local decision to this membership branch.
    pub fn apply_signed_local_removal_decision(
        &mut self,
        decision: MembershipDecisionV2,
        local_member: MemberInstanceId,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipDecisionStoreOutcome, MembershipHistoryV2Error> {
        if decision.decided_by_member_instance_id != local_member {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        let removal = self
            .events
            .get(&decision.removal_event_id)
            .ok_or(MembershipHistoryV2Error::UnknownRemoval)?;
        if self
            .peer_decisions
            .get(&(decision.removal_event_id, local_member))
            == Some(&decision)
        {
            return Ok(MembershipDecisionStoreOutcome::AlreadyKnown);
        }
        if removal.parent_event_id != self.known_head {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        let removal_event_id = decision.removal_event_id;
        let choice = decision.decision;
        let outcome = self.verify_and_record_peer_decision(decision, verifier)?;
        if choice == RemovalDecision::Accept {
            self.known_head = Some(removal_event_id);
        }
        self.rebuild_snapshots()?;
        Ok(outcome)
    }

    pub fn merge_remote_history(
        &mut self,
        incoming: &Self,
        local_member: MemberInstanceId,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<bool, MembershipHistoryV2Error> {
        if self.lineage_id != incoming.lineage_id
            || self.activation_baseline != incoming.activation_baseline
        {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        let mut changed = false;
        let mut events = incoming.events.values().cloned().collect::<Vec<_>>();
        events.sort_by_key(|event| (event.parent_depth, event.event_id()));
        for event in events {
            let event_id = event.event_id();
            if let Some(existing) = self.events.get(&event_id) {
                if existing != &event {
                    return Err(MembershipHistoryV2Error::InvalidSignature);
                }
            } else {
                let previous_head = self.known_head;
                let waits_for_local_decision = event.parent_event_id == previous_head
                    && event.author_member_instance_id != local_member
                    && matches!(event.operation, MembershipOperationV2::RemoveDevice { .. })
                    && previous_head
                        .and_then(|head| self.snapshots.get(&head))
                        .is_some_and(|snapshot| snapshot.members.contains(&local_member));
                self.verify_and_receive_event(event, verifier)?;
                if waits_for_local_decision {
                    self.known_head = previous_head;
                    self.rebuild_snapshots()?;
                }
                changed = true;
            }
            if let Some(record) = incoming.activation_receipts.get(&event_id) {
                match self.verify_and_record_activation_receipt(
                    record.activation_receipt.clone(),
                    verifier,
                )? {
                    MembershipActivationReceiptStoreOutcome::Stored => changed = true,
                    MembershipActivationReceiptStoreOutcome::AlreadyKnown => {}
                }
            }
        }
        let mut decisions = incoming
            .peer_decisions
            .values()
            .cloned()
            .collect::<Vec<_>>();
        decisions.sort_by_key(MembershipDecisionV2::decision_id);
        for decision in decisions {
            match self.verify_and_record_peer_decision(decision, verifier)? {
                MembershipDecisionStoreOutcome::Stored => changed = true,
                MembershipDecisionStoreOutcome::AlreadyKnown => {}
            }
        }
        Ok(changed)
    }

    pub fn expected_resulting_members_digest(
        &self,
        parent_event_id: Option<MembershipEventId>,
        operation: &MembershipOperationV2,
    ) -> Result<[u8; 32], MembershipHistoryV2Error> {
        let mut members = match parent_event_id {
            Some(parent) => self
                .snapshots
                .get(&parent)
                .ok_or(MembershipHistoryV2Error::UnknownParent)?
                .members
                .clone(),
            None if self.known_head.is_none() => BTreeSet::new(),
            None => return Err(MembershipHistoryV2Error::InvalidGenesis),
        };
        apply_membership_operation(&mut members, operation)?;
        Ok(members_digest(&members))
    }

    pub fn verify_and_receive_event(
        &mut self,
        event: MembershipEventV2,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipHistoryV2ReceiveOutcome, MembershipHistoryV2Error> {
        if event.event_format_version != MEMBERSHIP_EVENT_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if event.author_signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if event.lineage_id != self.lineage_id {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        let event_id = event.event_id();
        if let Some(existing) = self.events.get(&event_id) {
            return if existing == &event {
                Ok(MembershipHistoryV2ReceiveOutcome::AlreadyKnown)
            } else {
                Err(MembershipHistoryV2Error::InvalidSignature)
            };
        }
        if self.operation_ids.contains(&event.operation_id) {
            return Err(MembershipHistoryV2Error::OperationReplay);
        }

        let (parent_snapshot, author_credential) = self.validate_parent_and_author(&event)?;
        self.verify_signature(&event, &author_credential, verifier)?;
        self.validate_operation(&event, &parent_snapshot)?;

        let expected_digest =
            self.expected_resulting_members_digest(event.parent_event_id, &event.operation)?;
        if event.resulting_members_digest != expected_digest {
            return Err(MembershipHistoryV2Error::ResultingMembersDigestMismatch);
        }

        if let MembershipOperationV2::AddDevice { admission } = &event.operation {
            if let Some(existing) = self.credentials.get(&admission.facts.member_instance) {
                if existing != &admission.membership_credential {
                    return Err(MembershipHistoryV2Error::CredentialConflict);
                }
                return Err(MembershipHistoryV2Error::InvalidOperation);
            }
            self.credentials.insert(
                admission.facts.member_instance,
                admission.membership_credential.clone(),
            );
        }

        let extends_known_head = event.parent_event_id == self.known_head;
        self.operation_ids.insert(event.operation_id);
        self.events.insert(event_id, event);
        self.rebuild_snapshots()?;
        if self.known_head.is_none() || extends_known_head {
            self.known_head = Some(event_id);
            Ok(MembershipHistoryV2ReceiveOutcome::Applied)
        } else {
            Ok(MembershipHistoryV2ReceiveOutcome::Diverged)
        }
    }

    pub fn verify_and_record_activation_receipt(
        &mut self,
        receipt: AdmissionActivationReceipt,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipActivationReceiptStoreOutcome, MembershipHistoryV2Error> {
        if receipt.receipt_format_version != ACTIVATION_RECEIPT_FORMAT_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        let event = self.events.get(&receipt.event_id).ok_or(
            MembershipHistoryV2Error::MissingMembershipEvent(receipt.event_id),
        )?;
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(MembershipHistoryV2Error::InvalidActivationReceipt);
        };
        if receipt.joiner_member_instance_id != admission.facts.member_instance
            || receipt.applied_history_digest != event.resulting_members_digest
            || receipt.installed_security_commitment_id != admission.security_commitment_id
        {
            return Err(MembershipHistoryV2Error::InvalidActivationReceipt);
        }
        verify_signature(
            verifier,
            &admission.membership_credential,
            &receipt.signing_payload(),
            &receipt.signature,
        )?;
        let record = MembershipActivationReceiptRecord::new(receipt);
        if let Some(existing) = self.activation_receipts.get(&record.event_id) {
            return if existing == &record {
                Ok(MembershipActivationReceiptStoreOutcome::AlreadyKnown)
            } else {
                Err(MembershipHistoryV2Error::ActivationReceiptConflict)
            };
        }
        self.activation_receipts.insert(record.event_id, record);
        self.rebuild_snapshots()?;
        Ok(MembershipActivationReceiptStoreOutcome::Stored)
    }

    pub fn verify_and_record_peer_decision(
        &mut self,
        decision: MembershipDecisionV2,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<MembershipDecisionStoreOutcome, MembershipHistoryV2Error> {
        if decision.decision_format_version != MEMBERSHIP_DECISION_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if decision.signature_algorithm_version != ED25519_SIGNATURE_ALGORITHM_V1 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if decision.lineage_id != self.lineage_id {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        let removal = self
            .events
            .get(&decision.removal_event_id)
            .ok_or(MembershipHistoryV2Error::UnknownRemoval)?;
        let MembershipOperationV2::RemoveDevice { .. } = removal.operation else {
            return Err(MembershipHistoryV2Error::UnknownRemoval);
        };
        let parent_id = removal
            .parent_event_id
            .ok_or(MembershipHistoryV2Error::InvalidDecision)?;
        let parent_snapshot = self
            .snapshots
            .get(&parent_id)
            .ok_or(MembershipHistoryV2Error::UnknownParent)?;
        if !parent_snapshot
            .members
            .contains(&decision.decided_by_member_instance_id)
            || decision.decided_by_member_instance_id == removal.author_member_instance_id
            || decision.observed_applied_head != Some(parent_id)
        {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        let credential = self
            .credentials
            .get(&decision.decided_by_member_instance_id)
            .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
        if decision.decider_credential_id != credential.credential_id
            || decision.signature_algorithm_version != credential.signature_algorithm_version
        {
            return Err(MembershipHistoryV2Error::InvalidCredential);
        }
        let expected_digest = match decision.decision {
            RemovalDecision::Accept => removal.resulting_members_digest,
            RemovalDecision::Reject => self
                .events
                .get(&parent_id)
                .map(|event| event.resulting_members_digest)
                .ok_or(MembershipHistoryV2Error::UnknownParent)?,
        };
        if decision.resulting_members_digest != expected_digest {
            return Err(MembershipHistoryV2Error::InvalidDecision);
        }
        verify_signature(
            verifier,
            credential,
            &decision.signing_payload(),
            &decision.signature,
        )?;
        let key = (
            decision.removal_event_id,
            decision.decided_by_member_instance_id,
        );
        if let Some(existing) = self.peer_decisions.get(&key) {
            return if existing == &decision {
                Ok(MembershipDecisionStoreOutcome::AlreadyKnown)
            } else {
                Err(MembershipHistoryV2Error::DecisionConflict)
            };
        }
        self.peer_decisions.insert(key, decision);
        Ok(MembershipDecisionStoreOutcome::Stored)
    }

    fn validate_parent_and_author(
        &self,
        event: &MembershipEventV2,
    ) -> Result<(MembershipHistorySnapshot, MembershipCredential), MembershipHistoryV2Error> {
        match event.parent_event_id {
            None => {
                if self.known_head.is_some() || event.parent_depth != 0 {
                    return Err(MembershipHistoryV2Error::InvalidGenesis);
                }
                let MembershipOperationV2::AddDevice { admission } = &event.operation else {
                    return Err(MembershipHistoryV2Error::InvalidGenesis);
                };
                if event.author_member_instance_id != admission.facts.member_instance
                    || event.author_credential_id != admission.membership_credential.credential_id
                    || event.author_signature_algorithm_version
                        != admission.membership_credential.signature_algorithm_version
                {
                    return Err(MembershipHistoryV2Error::InvalidGenesis);
                }
                Ok((
                    MembershipHistorySnapshot {
                        members: BTreeSet::new(),
                        active_members: BTreeSet::new(),
                    },
                    admission.membership_credential.clone(),
                ))
            }
            Some(parent_id) => {
                let parent_depth = self
                    .depth(parent_id)
                    .ok_or(MembershipHistoryV2Error::UnknownParent)?;
                if event.parent_depth != parent_depth.saturating_add(1) {
                    return Err(MembershipHistoryV2Error::InvalidParentDepth);
                }
                let parent_snapshot = self
                    .snapshots
                    .get(&parent_id)
                    .ok_or(MembershipHistoryV2Error::UnknownParent)?;
                if !parent_snapshot
                    .members
                    .contains(&event.author_member_instance_id)
                {
                    return Err(MembershipHistoryV2Error::UnauthorizedAuthor);
                }
                if !parent_snapshot
                    .active_members
                    .contains(&event.author_member_instance_id)
                {
                    return Err(MembershipHistoryV2Error::AwaitingActivationReceipt);
                }
                let credential = self
                    .credentials
                    .get(&event.author_member_instance_id)
                    .ok_or(MembershipHistoryV2Error::InvalidCredential)?;
                if credential.credential_id != event.author_credential_id
                    || credential.signature_algorithm_version
                        != event.author_signature_algorithm_version
                {
                    return Err(MembershipHistoryV2Error::InvalidCredential);
                }
                Ok((parent_snapshot.clone(), credential.clone()))
            }
        }
    }

    fn verify_signature(
        &self,
        event: &MembershipEventV2,
        credential: &MembershipCredential,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<(), MembershipHistoryV2Error> {
        credential.validate()?;
        verify_signature(
            verifier,
            credential,
            &event.signing_payload(),
            &event.signature,
        )
    }

    fn validate_operation(
        &self,
        event: &MembershipEventV2,
        parent_snapshot: &MembershipHistorySnapshot,
    ) -> Result<(), MembershipHistoryV2Error> {
        match &event.operation {
            MembershipOperationV2::AddDevice { admission } => {
                admission.membership_credential.validate()?;
                if admission.facts.member_instance
                    != admission
                        .membership_credential
                        .member_instance_id(&admission.facts.device_id)
                    || parent_snapshot
                        .members
                        .contains(&admission.facts.member_instance)
                {
                    return Err(MembershipHistoryV2Error::InvalidCredential);
                }
                Ok(())
            }
            MembershipOperationV2::RemoveDevice { member } => {
                if *member == event.author_member_instance_id
                    || !parent_snapshot.members.contains(member)
                {
                    return Err(MembershipHistoryV2Error::InvalidOperation);
                }
                Ok(())
            }
        }
    }

    fn rebuild_snapshots(&mut self) -> Result<(), MembershipHistoryV2Error> {
        let mut ordered_events = self.events.values().cloned().collect::<Vec<_>>();
        ordered_events.sort_by_key(|event| (event.parent_depth, event.event_id()));
        let mut snapshots = BTreeMap::new();
        if let Some(baseline) = &self.activation_baseline {
            let (head, _) = baseline.head_and_depth();
            let members = baseline.current_member_ids();
            snapshots.insert(
                head,
                MembershipHistorySnapshot {
                    members: members.clone(),
                    active_members: members,
                },
            );
        }
        for event in ordered_events {
            let mut snapshot = match event.parent_event_id {
                Some(parent) => snapshots
                    .get(&parent)
                    .cloned()
                    .ok_or(MembershipHistoryV2Error::UnknownParent)?,
                None => MembershipHistorySnapshot {
                    members: BTreeSet::new(),
                    active_members: BTreeSet::new(),
                },
            };
            match &event.operation {
                MembershipOperationV2::AddDevice { admission } => {
                    snapshot.members.insert(admission.facts.member_instance);
                    if event.parent_event_id.is_none()
                        || self.activation_receipts.contains_key(&event.event_id())
                    {
                        snapshot
                            .active_members
                            .insert(admission.facts.member_instance);
                    }
                }
                MembershipOperationV2::RemoveDevice { member } => {
                    snapshot.members.remove(member);
                    snapshot.active_members.remove(member);
                }
            }
            snapshots.insert(event.event_id(), snapshot);
        }
        self.snapshots = snapshots;
        Ok(())
    }

    fn verify_activation_baseline_signatures(
        &self,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<(), MembershipHistoryV2Error> {
        let Some(MembershipActivationBaselineV2::Established {
            current_members, ..
        }) = &self.activation_baseline
        else {
            return Ok(());
        };
        for (facts, credential) in current_members {
            verify_signature(
                verifier,
                credential,
                &facts.signing_payload(),
                &facts.identity_signature,
            )?;
        }
        Ok(())
    }
}

fn verify_signature(
    verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    credential: &MembershipCredential,
    payload: &[u8],
    signature: &[u8],
) -> Result<(), MembershipHistoryV2Error> {
    match verifier.verify(
        credential.signature_algorithm_version,
        &credential.public_key,
        payload,
        signature,
    ) {
        Ok(true) => Ok(()),
        Ok(false) | Err(HistoricalMembershipSignatureError::VerificationFailed) => {
            Err(MembershipHistoryV2Error::InvalidSignature)
        }
        Err(HistoricalMembershipSignatureError::UnsupportedAlgorithm) => {
            Err(MembershipHistoryV2Error::UpgradeRequired)
        }
    }
}

struct MembershipHistoryPageMetadata<'a> {
    transfer_id: [u8; 32],
    lineage_id: &'a str,
    position: &'a BaseMembershipHistoryPosition,
    sender_admission: &'a AdmissionChangeFacts,
    activation_baseline: Option<&'a PersistedActivationBaselineV2>,
    known_head: Option<MembershipEventId>,
}

#[derive(Clone, Copy)]
enum MembershipHistoryPageRecord<'a> {
    Event(&'a MembershipEventV2),
    ActivationReceipt(&'a AdmissionActivationReceipt),
    Decision(&'a MembershipDecisionV2),
}

impl MembershipHistoryPageRecord<'_> {
    fn count_in(self, page: &MembershipHistoryPageV2) -> usize {
        match self {
            Self::Event(_) => page.events.len(),
            Self::ActivationReceipt(_) => page.activation_receipts.len(),
            Self::Decision(_) => page.decisions.len(),
        }
    }

    fn push_onto(self, page: &mut MembershipHistoryPageV2) {
        match self {
            Self::Event(event) => page.events.push(event.clone()),
            Self::ActivationReceipt(receipt) => page.activation_receipts.push(receipt.clone()),
            Self::Decision(decision) => page.decisions.push(decision.clone()),
        }
    }

    fn pop_from(self, page: &mut MembershipHistoryPageV2) {
        match self {
            Self::Event(_) => {
                page.events.pop();
            }
            Self::ActivationReceipt(_) => {
                page.activation_receipts.pop();
            }
            Self::Decision(_) => {
                page.decisions.pop();
            }
        }
    }
}

fn empty_history_page(
    metadata: &MembershipHistoryPageMetadata<'_>,
    page_index: usize,
) -> Result<MembershipHistoryPageV2, MembershipHistoryV2Error> {
    let page_index =
        u32::try_from(page_index).map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
    let page = MembershipHistoryPageV2 {
        exchange_format_version: MEMBERSHIP_HISTORY_EXCHANGE_FORMAT_V2,
        transfer_id: metadata.transfer_id,
        page_index,
        page_count: u32::MAX,
        lineage_id: metadata.lineage_id.to_owned(),
        position: metadata.position.clone(),
        sender_admission: metadata.sender_admission.clone(),
        events: Vec::new(),
        activation_receipts: Vec::new(),
        decisions: Vec::new(),
        activation_baseline: (page_index == 0)
            .then(|| metadata.activation_baseline.cloned())
            .flatten(),
        known_head: (page_index == 0).then_some(metadata.known_head).flatten(),
    };
    if page.encoded_frame_size()? > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
    }
    Ok(page)
}

fn append_history_page_record(
    pages: &mut Vec<MembershipHistoryPageV2>,
    metadata: &MembershipHistoryPageMetadata<'_>,
    record: MembershipHistoryPageRecord<'_>,
) -> Result<(), MembershipHistoryV2Error> {
    let current = pages
        .last_mut()
        .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
    if record.count_in(current) == MAX_MEMBERSHIP_HISTORY_RECORDS_PER_PAGE {
        let next_page_index = pages.len();
        pages.push(empty_history_page(metadata, next_page_index)?);
    }

    let current = pages
        .last_mut()
        .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
    record.push_onto(current);
    if current.encoded_frame_size()? <= MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        return Ok(());
    }
    record.pop_from(current);
    if current.record_counts().total() == 0 {
        return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
    }

    let next_page_index = pages.len();
    pages.push(empty_history_page(metadata, next_page_index)?);
    let current = pages
        .last_mut()
        .ok_or(MembershipHistoryV2Error::InvalidPersistedHistory)?;
    record.push_onto(current);
    if current.encoded_frame_size()? > MAX_MEMBERSHIP_HISTORY_FRAME_SIZE {
        record.pop_from(current);
        return Err(MembershipHistoryV2Error::InvalidPersistedHistory);
    }
    Ok(())
}

#[derive(Serialize)]
enum MembershipHistorySuffixRecordV3 {
    Event(MembershipEventV2),
    ActivationReceipt(AdmissionActivationReceipt),
    Decision(MembershipDecisionV2),
}

fn suffix_transfer_id_v3(
    lineage_id: &str,
    base_position: &BaseMembershipHistoryPosition,
    target_position: &BaseMembershipHistoryPosition,
    sender_admission: &AdmissionChangeFacts,
    records: &[MembershipHistorySuffixRecordV3],
) -> Result<[u8; 32], MembershipHistoryV2Error> {
    let encoded = postcard::to_stdvec(&(
        MEMBERSHIP_HISTORY_SUFFIX_FORMAT_V3,
        lineage_id,
        base_position,
        target_position,
        sender_admission,
        records,
    ))
    .map_err(|_| MembershipHistoryV2Error::InvalidPersistedHistory)?;
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-history-suffix/v3\0");
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(hasher.finalize().into())
}

fn history_transfer_id(encoded_history: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-history-transfer/v2\0");
    hasher.update((encoded_history.len() as u64).to_be_bytes());
    hasher.update(encoded_history);
    hasher.finalize().into()
}

fn apply_membership_operation(
    members: &mut BTreeSet<MemberInstanceId>,
    operation: &MembershipOperationV2,
) -> Result<(), MembershipHistoryV2Error> {
    match operation {
        MembershipOperationV2::AddDevice { admission } => {
            if !members.insert(admission.facts.member_instance) {
                return Err(MembershipHistoryV2Error::InvalidOperation);
            }
        }
        MembershipOperationV2::RemoveDevice { member } => {
            if !members.remove(member) {
                return Err(MembershipHistoryV2Error::InvalidOperation);
            }
        }
    }
    Ok(())
}

fn members_digest(members: &BTreeSet<MemberInstanceId>) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/membership-members/v2\0");
    hasher.update((members.len() as u64).to_be_bytes());
    for member in members {
        hasher.update(member.as_bytes());
    }
    hasher.finalize().into()
}

fn append_operation(bytes: &mut Vec<u8>, operation: &MembershipOperationV2) {
    match operation {
        MembershipOperationV2::AddDevice { admission } => {
            bytes.push(1);
            append_admission(bytes, admission);
        }
        MembershipOperationV2::RemoveDevice { member } => {
            bytes.push(2);
            bytes.extend_from_slice(member.as_bytes());
        }
    }
}

fn append_admission(bytes: &mut Vec<u8>, admission: &MembershipAdmissionV2) {
    bytes.extend_from_slice(admission.facts.member_instance.as_bytes());
    append_field(bytes, admission.facts.device_id.as_str().as_bytes());
    append_field(bytes, admission.facts.device_name.as_bytes());
    append_field(
        bytes,
        admission.facts.identity_fingerprint.as_display().as_bytes(),
    );
    append_field(bytes, &admission.facts.transport_public_key);
    append_field(bytes, &admission.facts.transport_address_blob);
    append_field(bytes, &admission.facts.identity_signature);
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
    append_field(bytes, &admission.membership_credential.public_key);
    bytes.extend_from_slice(admission.membership_credential.credential_id.as_bytes());
    bytes.extend_from_slice(&admission.resume_public_key_digest);
    bytes.extend_from_slice(&admission.security_commitment_id);
}

fn append_field(bytes: &mut Vec<u8>, value: &[u8]) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value);
}

fn append_optional_event_id(bytes: &mut Vec<u8>, event_id: Option<MembershipEventId>) {
    match event_id {
        Some(event_id) => {
            bytes.push(1);
            bytes.extend_from_slice(event_id.as_bytes());
        }
        None => bytes.push(0),
    }
}

fn append_optional_digest(bytes: &mut Vec<u8>, digest: Option<[u8; 32]>) {
    match digest {
        Some(digest) => {
            bytes.push(1);
            bytes.extend_from_slice(&digest);
        }
        None => bytes.push(0),
    }
}
