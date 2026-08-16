use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ids::DeviceId;

use super::{
    AdmissionChangeFacts, MemberInstanceId, MembershipDecision, MembershipDecisionId,
    MembershipEvent, MembershipEventId, RemovalDecision,
};

pub const MEMBERSHIP_CREDENTIAL_FORMAT_V1: u16 = 1;
pub const ED25519_SIGNATURE_ALGORITHM_V1: u16 = 1;
pub const MEMBERSHIP_EVENT_FORMAT_V2: u16 = 2;
pub const MEMBERSHIP_DECISION_FORMAT_V2: u16 = 2;
pub const LEGACY_PREFIX_CHECKPOINT_FORMAT_V2: u16 = 2;
pub const LEGACY_CHECKPOINT_ATTESTATION_FORMAT_V2: u16 = 2;
pub const ADMISSION_SECURITY_COMMITMENT_FORMAT_V1: u16 = 1;
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
        MemberInstanceId::derive(device_id.as_str(), self.credential_id.as_bytes())
    }

    fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.credential_format_version != MEMBERSHIP_CREDENTIAL_FORMAT_V1 {
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
pub struct MembershipEventV1Evidence {
    pub semantic_event: MembershipEvent,
    pub canonical_signing_payload: Vec<u8>,
    pub signature: Vec<u8>,
    pub original_event_id: MembershipEventId,
}

impl MembershipEventV1Evidence {
    pub fn new(
        semantic_event: MembershipEvent,
        canonical_signing_payload: Vec<u8>,
        signature: Vec<u8>,
        original_event_id: MembershipEventId,
    ) -> Result<Self, MembershipHistoryV2Error> {
        let evidence = Self {
            semantic_event,
            canonical_signing_payload,
            signature,
            original_event_id,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    pub fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.semantic_event.signing_payload() != self.canonical_signing_payload
            || self.semantic_event.signature != self.signature
            || self.semantic_event.event_id() != self.original_event_id
        {
            return Err(MembershipHistoryV2Error::InvalidLegacyEvidence);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event_version")]
pub enum VersionedMembershipEvent {
    V1Evidence(MembershipEventV1Evidence),
    V2(MembershipEventV2),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipDecisionV1Evidence {
    pub semantic_decision: MembershipDecision,
    pub canonical_signing_payload: Vec<u8>,
    pub signature: Vec<u8>,
    pub original_decision_id: MembershipDecisionId,
}

impl MembershipDecisionV1Evidence {
    pub fn new(
        semantic_decision: MembershipDecision,
        canonical_signing_payload: Vec<u8>,
        signature: Vec<u8>,
        original_decision_id: MembershipDecisionId,
    ) -> Result<Self, MembershipHistoryV2Error> {
        let evidence = Self {
            semantic_decision,
            canonical_signing_payload,
            signature,
            original_decision_id,
        };
        evidence.validate()?;
        Ok(evidence)
    }

    pub fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.semantic_decision.signing_payload() != self.canonical_signing_payload
            || self.semantic_decision.signature != self.signature
            || self.semantic_decision.decision_id() != self.original_decision_id
        {
            return Err(MembershipHistoryV2Error::InvalidLegacyEvidence);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision_version")]
pub enum VersionedMembershipDecision {
    V1Evidence(MembershipDecisionV1Evidence),
    V2(MembershipDecisionV2),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyPrefixCheckpointV2 {
    pub checkpoint_format_version: u16,
    pub lineage_id: String,
    pub v1_prefix_head: MembershipEventId,
    pub v1_prefix_depth: u64,
    pub v1_evidence_digest: [u8; 32],
    pub resulting_members_digest: [u8; 32],
    pub security_state_digest: [u8; 32],
    pub continuing_member_credentials: Vec<(MemberInstanceId, MembershipCredential)>,
    pub checkpoint_id: [u8; 32],
}

impl LegacyPrefixCheckpointV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        checkpoint_format_version: u16,
        lineage_id: String,
        v1_prefix_head: MembershipEventId,
        v1_prefix_depth: u64,
        v1_evidence_digest: [u8; 32],
        resulting_members_digest: [u8; 32],
        security_state_digest: [u8; 32],
        mut continuing_member_credentials: Vec<(MemberInstanceId, MembershipCredential)>,
    ) -> Result<Self, MembershipHistoryV2Error> {
        if checkpoint_format_version != LEGACY_PREFIX_CHECKPOINT_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        continuing_member_credentials.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.credential_id.cmp(&right.1.credential_id))
        });
        let mut previous_member = None;
        for (member, credential) in &continuing_member_credentials {
            credential.validate()?;
            if previous_member == Some(*member) {
                return Err(MembershipHistoryV2Error::CredentialConflict);
            }
            previous_member = Some(*member);
        }

        let mut checkpoint = Self {
            checkpoint_format_version,
            lineage_id,
            v1_prefix_head,
            v1_prefix_depth,
            v1_evidence_digest,
            resulting_members_digest,
            security_state_digest,
            continuing_member_credentials,
            checkpoint_id: [0; 32],
        };
        checkpoint.checkpoint_id = Sha256::digest(checkpoint.canonical_content()).into();
        checkpoint.validate()?;
        Ok(checkpoint)
    }

    pub fn validate(&self) -> Result<(), MembershipHistoryV2Error> {
        if self.checkpoint_format_version != LEGACY_PREFIX_CHECKPOINT_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.lineage_id.is_empty() || self.continuing_member_credentials.is_empty() {
            return Err(MembershipHistoryV2Error::InvalidLegacyEvidence);
        }
        let mut previous = None;
        for (member, credential) in &self.continuing_member_credentials {
            credential.validate()?;
            let key = (*member, credential.credential_id);
            if previous.is_some_and(|prior| prior >= key) {
                return Err(MembershipHistoryV2Error::InvalidLegacyEvidence);
            }
            previous = Some(key);
        }
        let expected_id: [u8; 32] = Sha256::digest(self.canonical_content()).into();
        if self.checkpoint_id != expected_id {
            return Err(MembershipHistoryV2Error::InvalidLegacyEvidence);
        }
        Ok(())
    }

    fn canonical_content(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard/legacy-prefix-checkpoint/v2\0");
        bytes.extend_from_slice(&self.checkpoint_format_version.to_be_bytes());
        append_field(&mut bytes, self.lineage_id.as_bytes());
        bytes.extend_from_slice(self.v1_prefix_head.as_bytes());
        bytes.extend_from_slice(&self.v1_prefix_depth.to_be_bytes());
        bytes.extend_from_slice(&self.v1_evidence_digest);
        bytes.extend_from_slice(&self.resulting_members_digest);
        bytes.extend_from_slice(&self.security_state_digest);
        bytes.extend_from_slice(&(self.continuing_member_credentials.len() as u64).to_be_bytes());
        for (member, credential) in &self.continuing_member_credentials {
            bytes.extend_from_slice(member.as_bytes());
            bytes.extend_from_slice(credential.credential_id.as_bytes());
            bytes.extend_from_slice(&credential.credential_format_version.to_be_bytes());
            bytes.extend_from_slice(&credential.signature_algorithm_version.to_be_bytes());
            append_field(&mut bytes, &credential.public_key);
        }
        bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyCheckpointAttestationV2 {
    pub attestation_format_version: u16,
    pub checkpoint_id: [u8; 32],
    pub attester_member_instance_id: MemberInstanceId,
    pub attester_credential_id: MembershipCredentialId,
    pub signature: Vec<u8>,
}

impl LegacyCheckpointAttestationV2 {
    pub fn new(
        attestation_format_version: u16,
        checkpoint_id: [u8; 32],
        attester_member_instance_id: MemberInstanceId,
        attester_credential_id: MembershipCredentialId,
        signature: Vec<u8>,
    ) -> Self {
        Self {
            attestation_format_version,
            checkpoint_id,
            attester_member_instance_id,
            attester_credential_id,
            signature,
        }
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"uniclipboard/legacy-checkpoint-attestation/v2\0");
        bytes.extend_from_slice(&self.attestation_format_version.to_be_bytes());
        bytes.extend_from_slice(&self.checkpoint_id);
        bytes.extend_from_slice(self.attester_member_instance_id.as_bytes());
        bytes.extend_from_slice(self.attester_credential_id.as_bytes());
        bytes
    }

    pub fn verify(
        &self,
        checkpoint: &LegacyPrefixCheckpointV2,
        verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
    ) -> Result<(), MembershipHistoryV2Error> {
        checkpoint.validate()?;
        if self.attestation_format_version != LEGACY_CHECKPOINT_ATTESTATION_FORMAT_V2 {
            return Err(MembershipHistoryV2Error::UpgradeRequired);
        }
        if self.checkpoint_id != checkpoint.checkpoint_id {
            return Err(MembershipHistoryV2Error::InvalidCheckpointAttestation);
        }
        let credential = checkpoint
            .continuing_member_credentials
            .iter()
            .find(|(member, credential)| {
                *member == self.attester_member_instance_id
                    && credential.credential_id == self.attester_credential_id
            })
            .map(|(_, credential)| credential)
            .ok_or(MembershipHistoryV2Error::InvalidCheckpointAttestation)?;
        verify_signature(
            verifier,
            credential,
            &self.signing_payload(),
            &self.signature,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseMembershipHistoryPositionV1 {
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
    pub base_history_position: BaseMembershipHistoryPositionV1,
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
    FullyVerifiedMigration {
        lineage_id: String,
        head_event_id: MembershipEventId,
        head_depth: u64,
        current_member_credentials: Vec<(MemberInstanceId, MembershipCredential)>,
    },
    LegacyAccepted {
        checkpoint: LegacyPrefixCheckpointV2,
    },
}

impl MembershipActivationBaselineV2 {
    fn lineage_id(&self) -> &str {
        match self {
            Self::FullyVerifiedMigration { lineage_id, .. } => lineage_id,
            Self::LegacyAccepted { checkpoint } => &checkpoint.lineage_id,
        }
    }

    fn head_and_depth(&self) -> (MembershipEventId, u64) {
        match self {
            Self::FullyVerifiedMigration {
                head_event_id,
                head_depth,
                ..
            } => (*head_event_id, *head_depth),
            Self::LegacyAccepted { checkpoint } => {
                (checkpoint.v1_prefix_head, checkpoint.v1_prefix_depth)
            }
        }
    }

    fn current_member_credentials(&self) -> &[(MemberInstanceId, MembershipCredential)] {
        match self {
            Self::FullyVerifiedMigration {
                current_member_credentials,
                ..
            } => current_member_credentials,
            Self::LegacyAccepted { checkpoint } => &checkpoint.continuing_member_credentials,
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
        base_history_position: BaseMembershipHistoryPositionV1,
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
    InvalidLegacyEvidence,
    InvalidCheckpointAttestation,
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
            Self::InvalidLegacyEvidence => "legacy membership evidence is invalid",
            Self::InvalidCheckpointAttestation => "legacy checkpoint attestation is invalid",
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
    FullyVerifiedMigration {
        lineage_id: String,
        head_event_id: MembershipEventId,
        head_depth: u64,
        current_member_credentials: Vec<(MemberInstanceId, MembershipCredential)>,
    },
    LegacyAccepted(LegacyPrefixCheckpointV2),
}

impl From<MembershipActivationBaselineV2> for PersistedActivationBaselineV2 {
    fn from(value: MembershipActivationBaselineV2) -> Self {
        match value {
            MembershipActivationBaselineV2::FullyVerifiedMigration {
                lineage_id,
                head_event_id,
                head_depth,
                current_member_credentials,
            } => Self::FullyVerifiedMigration {
                lineage_id,
                head_event_id,
                head_depth,
                current_member_credentials,
            },
            MembershipActivationBaselineV2::LegacyAccepted { checkpoint } => {
                Self::LegacyAccepted(checkpoint)
            }
        }
    }
}

impl From<PersistedActivationBaselineV2> for MembershipActivationBaselineV2 {
    fn from(value: PersistedActivationBaselineV2) -> Self {
        match value {
            PersistedActivationBaselineV2::FullyVerifiedMigration {
                lineage_id,
                head_event_id,
                head_depth,
                current_member_credentials,
            } => Self::FullyVerifiedMigration {
                lineage_id,
                head_event_id,
                head_depth,
                current_member_credentials,
            },
            PersistedActivationBaselineV2::LegacyAccepted(checkpoint) => {
                Self::LegacyAccepted { checkpoint }
            }
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
        if history.lineage_id != persisted.lineage_id {
            return Err(MembershipHistoryV2Error::InvalidLineage);
        }
        persisted
            .events
            .sort_by_key(|event| (event.parent_depth, event.event_id()));
        for event in persisted.events {
            history.verify_and_receive_event(event, verifier)?;
        }
        persisted
            .activation_receipts
            .sort_by_key(|receipt| receipt.event_id);
        for receipt in persisted.activation_receipts {
            history.verify_and_record_activation_receipt(receipt, verifier)?;
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

    pub fn from_activation_baseline(
        mut activation_baseline: MembershipActivationBaselineV2,
    ) -> Result<Self, MembershipHistoryV2Error> {
        let lineage_id = activation_baseline.lineage_id().to_owned();
        let credentials = match &mut activation_baseline {
            MembershipActivationBaselineV2::FullyVerifiedMigration {
                current_member_credentials,
                ..
            } => {
                current_member_credentials.sort_by(|left, right| left.0.cmp(&right.0));
                current_member_credentials
            }
            MembershipActivationBaselineV2::LegacyAccepted { checkpoint } => {
                checkpoint.validate()?;
                &mut checkpoint.continuing_member_credentials
            }
        };
        if lineage_id.is_empty() || credentials.is_empty() {
            return Err(MembershipHistoryV2Error::InvalidActivationBaseline);
        }
        let mut credential_index = BTreeMap::new();
        for (member, credential) in credentials.iter() {
            credential.validate()?;
            if credential_index
                .insert(*member, credential.clone())
                .is_some()
            {
                return Err(MembershipHistoryV2Error::CredentialConflict);
            }
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

    pub fn decision_for(
        &self,
        removal_event_id: MembershipEventId,
        decided_by: MemberInstanceId,
    ) -> Option<&MembershipDecisionV2> {
        self.peer_decisions.get(&(removal_event_id, decided_by))
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
            let members = baseline
                .current_member_credentials()
                .iter()
                .map(|(member, _)| *member)
                .collect::<BTreeSet<_>>();
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
            Err(MembershipHistoryV2Error::UnsupportedSignatureAlgorithm)
        }
    }
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
