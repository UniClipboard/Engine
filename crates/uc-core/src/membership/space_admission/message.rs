use crate::ids::DeviceId;

use super::super::{
    AdmissionActivationReceipt, AdmissionChangeFacts, AdmissionCompletionV1,
    AdmissionSecurityCommitmentV1, MembershipCredential, MembershipEventV2, MembershipOperationV2,
    PreparedAdmissionProofV1,
};
use super::artifact::{
    AdmissionContinuationRoute, AdmissionIdentitySignature, AdmissionKeyPackage,
    AdmissionMlsCommit, AdmissionMlsWelcome, AdmissionRecoveryPublicKey,
    AdmissionSealedRecoveryMaterial, AdmissionSignedMembershipHistory,
};
use super::exchange::AdmissionMessageEvidence;
use super::id::{AdmissionMessageId, InvitationId, SpaceAdmissionId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceAdmissionProtocolVersion {
    V1,
}

impl SpaceAdmissionProtocolVersion {
    pub const fn from_u16(value: u16) -> Option<Self> {
        match value {
            1 => Some(Self::V1),
            _ => None,
        }
    }

    pub const fn as_u16(self) -> u16 {
        match self {
            Self::V1 => 1,
        }
    }
}

impl SpaceAdmissionEnvelopeV1 {
    /// Encodes the typed envelope for an authenticated transport. The wire and
    /// encrypted persistence paths deliberately share one canonical mirror so
    /// they cannot disagree about protocol fields or enum discriminants.
    pub fn encode_canonical_v1(
        &self,
    ) -> Result<Vec<u8>, super::state::SpaceAdmissionPersistenceError> {
        super::state::encode_envelope_v1(self)
    }

    pub fn decode_canonical_v1(
        encoded: &[u8],
    ) -> Result<Self, super::state::SpaceAdmissionPersistenceError> {
        super::state::decode_envelope_v1(encoded)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRole {
    Joiner,
    Sponsor,
    CompletionHelper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceAdmissionMessageKind {
    JoinRequest,
    Candidate,
    Prepared,
    Commit,
    Applied,
    Complete,
    CompleteAck,
    Settled,
    CancelRequested,
    Rejected,
}

impl SpaceAdmissionMessageKind {
    pub const fn accepts_sender(self, sender: AdmissionRole) -> bool {
        match self {
            Self::JoinRequest
            | Self::Prepared
            | Self::Applied
            | Self::CompleteAck
            | Self::CancelRequested => matches!(sender, AdmissionRole::Joiner),
            Self::Candidate | Self::Commit => matches!(sender, AdmissionRole::Sponsor),
            Self::Complete | Self::Settled | Self::Rejected => {
                matches!(
                    sender,
                    AdmissionRole::Sponsor | AdmissionRole::CompletionHelper
                )
            }
        }
    }

    pub const fn can_reply_to(self, request: Self) -> bool {
        if matches!(self, Self::Rejected) {
            return !matches!(request, Self::Rejected | Self::Settled);
        }
        matches!(
            (request, self),
            (Self::JoinRequest, Self::Candidate)
                | (Self::Candidate, Self::Prepared)
                | (Self::Prepared, Self::Commit)
                | (Self::Commit, Self::Applied)
                | (Self::Applied, Self::Complete)
                | (Self::Complete, Self::CompleteAck)
                | (Self::CompleteAck, Self::Settled)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionMessageHeaderError {
    #[error("the message sender is not allowed for this message kind")]
    SenderNotAllowed,
    #[error("the initial join request header is invalid")]
    InvalidInitialJoinRequest,
    #[error("a non-initial admission message requires a predecessor")]
    MissingPredecessor,
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionEnvelopeHeaderV1 {
    protocol_version: SpaceAdmissionProtocolVersion,
    admission_id: SpaceAdmissionId,
    kind: SpaceAdmissionMessageKind,
    sender_role: AdmissionRole,
    sender_sequence: u64,
    message_id: AdmissionMessageId,
    predecessor_message_id: Option<AdmissionMessageId>,
}

impl SpaceAdmissionEnvelopeHeaderV1 {
    pub fn new(
        admission_id: SpaceAdmissionId,
        kind: SpaceAdmissionMessageKind,
        sender_role: AdmissionRole,
        sender_sequence: u64,
        message_id: AdmissionMessageId,
        predecessor_message_id: Option<AdmissionMessageId>,
    ) -> Result<Self, AdmissionMessageHeaderError> {
        if !kind.accepts_sender(sender_role) {
            return Err(AdmissionMessageHeaderError::SenderNotAllowed);
        }
        if kind == SpaceAdmissionMessageKind::JoinRequest {
            if sender_sequence != 0 || predecessor_message_id.is_some() {
                return Err(AdmissionMessageHeaderError::InvalidInitialJoinRequest);
            }
        } else if predecessor_message_id.is_none() {
            return Err(AdmissionMessageHeaderError::MissingPredecessor);
        }

        Ok(Self {
            protocol_version: SpaceAdmissionProtocolVersion::V1,
            admission_id,
            kind,
            sender_role,
            sender_sequence,
            message_id,
            predecessor_message_id,
        })
    }

    pub const fn protocol_version(&self) -> SpaceAdmissionProtocolVersion {
        self.protocol_version
    }

    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.admission_id
    }

    pub const fn kind(&self) -> SpaceAdmissionMessageKind {
        self.kind
    }

    pub const fn sender_role(&self) -> AdmissionRole {
        self.sender_role
    }

    pub const fn sender_sequence(&self) -> u64 {
        self.sender_sequence
    }

    pub const fn message_id(&self) -> AdmissionMessageId {
        self.message_id
    }

    pub const fn predecessor_message_id(&self) -> Option<AdmissionMessageId> {
        self.predecessor_message_id
    }
}

impl std::fmt::Debug for SpaceAdmissionEnvelopeHeaderV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpaceAdmissionEnvelopeHeaderV1")
            .field("protocol_version", &self.protocol_version)
            .field("admission_id", &"[REDACTED]")
            .field("kind", &self.kind)
            .field("sender_role", &self.sender_role)
            .field("sender_sequence", &self.sender_sequence)
            .field("message_id", &"[REDACTED]")
            .field("has_predecessor", &self.predecessor_message_id.is_some())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceAdmissionRejectionReason {
    InvitationUnavailable,
    AuthenticationRejected,
    IdentityConflict,
    BaseHistoryChanged,
    JoinerHistoryAhead,
    HistoryConflict,
    PeerUpgradeRequired,
    Cancelled,
    RemovedBeforeActivation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreadableHistoryPolicy {
    Discard,
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionJoinRequestError {
    #[error("the membership credential is invalid")]
    InvalidMembershipCredential,
    #[error("the signed identity facts do not match the joining device")]
    IdentityMismatch,
}

#[derive(PartialEq, Eq)]
pub struct AdmissionJoinRequestV1 {
    invitation_id: InvitationId,
    device_id: DeviceId,
    identity_facts: AdmissionChangeFacts,
    membership_credential: MembershipCredential,
    key_package: AdmissionKeyPackage,
    recovery_public_key: AdmissionRecoveryPublicKey,
    identity_signature: AdmissionIdentitySignature,
    unreadable_history_policy: UnreadableHistoryPolicy,
}

impl AdmissionJoinRequestV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        invitation_id: InvitationId,
        device_id: DeviceId,
        identity_facts: AdmissionChangeFacts,
        membership_credential: MembershipCredential,
        key_package: AdmissionKeyPackage,
        recovery_public_key: AdmissionRecoveryPublicKey,
        identity_signature: AdmissionIdentitySignature,
        unreadable_history_policy: UnreadableHistoryPolicy,
    ) -> Result<Self, AdmissionJoinRequestError> {
        membership_credential
            .validate()
            .map_err(|_| AdmissionJoinRequestError::InvalidMembershipCredential)?;
        if identity_facts.device_id != device_id
            || identity_facts.member_instance
                != membership_credential.member_instance_id(&device_id)
            || identity_facts.device_name.trim().is_empty()
            || identity_facts.transport_public_key.is_empty()
            || identity_facts.transport_address_blob.is_empty()
            || identity_facts.identity_signature.as_slice() != identity_signature.as_bytes()
        {
            return Err(AdmissionJoinRequestError::IdentityMismatch);
        }
        Ok(Self {
            invitation_id,
            device_id,
            identity_facts,
            membership_credential,
            key_package,
            recovery_public_key,
            identity_signature,
            unreadable_history_policy,
        })
    }

    pub const fn invitation_id(&self) -> InvitationId {
        self.invitation_id
    }

    pub const fn device_id(&self) -> DeviceId {
        self.device_id
    }

    pub const fn membership_credential(&self) -> &MembershipCredential {
        &self.membership_credential
    }

    pub const fn identity_facts(&self) -> &AdmissionChangeFacts {
        &self.identity_facts
    }

    pub const fn key_package(&self) -> &AdmissionKeyPackage {
        &self.key_package
    }

    pub const fn recovery_public_key(&self) -> &AdmissionRecoveryPublicKey {
        &self.recovery_public_key
    }

    pub const fn identity_signature(&self) -> &AdmissionIdentitySignature {
        &self.identity_signature
    }

    pub const fn unreadable_history_policy(&self) -> UnreadableHistoryPolicy {
        self.unreadable_history_policy
    }
}

impl std::fmt::Debug for AdmissionJoinRequestV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionJoinRequestV1")
            .field("invitation_id", &"[REDACTED]")
            .field("device_id", &"[REDACTED]")
            .field("membership_credential", &"[REDACTED]")
            .field("key_package", &self.key_package)
            .field("recovery_public_key", &"[REDACTED]")
            .field("identity_signature", &self.identity_signature)
            .field("unreadable_history_policy", &self.unreadable_history_policy)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionCandidateError {
    #[error("the candidate event is not an AddDevice operation")]
    NotAddDevice,
    #[error("the candidate security commitment is invalid")]
    InvalidSecurityCommitment,
    #[error("the candidate event and security commitment have different lineages")]
    LineageMismatch,
}

#[derive(PartialEq, Eq)]
pub struct AdmissionCandidateV1 {
    base_membership_history: AdmissionSignedMembershipHistory,
    candidate_event: MembershipEventV2,
    security_commitment: AdmissionSecurityCommitmentV1,
    mls_commit: AdmissionMlsCommit,
    mls_welcome: AdmissionMlsWelcome,
    continuation_route: AdmissionContinuationRoute,
}

impl AdmissionCandidateV1 {
    pub fn new(
        base_membership_history: AdmissionSignedMembershipHistory,
        candidate_event: MembershipEventV2,
        security_commitment: AdmissionSecurityCommitmentV1,
        mls_commit: AdmissionMlsCommit,
        mls_welcome: AdmissionMlsWelcome,
        continuation_route: AdmissionContinuationRoute,
    ) -> Result<Self, AdmissionCandidateError> {
        if !matches!(
            candidate_event.operation,
            MembershipOperationV2::AddDevice { .. }
        ) {
            return Err(AdmissionCandidateError::NotAddDevice);
        }
        security_commitment
            .validate()
            .map_err(|_| AdmissionCandidateError::InvalidSecurityCommitment)?;
        if candidate_event.lineage_id != security_commitment.lineage_id {
            return Err(AdmissionCandidateError::LineageMismatch);
        }
        Ok(Self {
            base_membership_history,
            candidate_event,
            security_commitment,
            mls_commit,
            mls_welcome,
            continuation_route,
        })
    }

    pub const fn base_membership_history(&self) -> &AdmissionSignedMembershipHistory {
        &self.base_membership_history
    }

    pub const fn candidate_event(&self) -> &MembershipEventV2 {
        &self.candidate_event
    }

    pub const fn security_commitment(&self) -> &AdmissionSecurityCommitmentV1 {
        &self.security_commitment
    }

    pub const fn mls_commit(&self) -> &AdmissionMlsCommit {
        &self.mls_commit
    }

    pub const fn mls_welcome(&self) -> &AdmissionMlsWelcome {
        &self.mls_welcome
    }

    pub const fn continuation_route(&self) -> &AdmissionContinuationRoute {
        &self.continuation_route
    }
}

impl std::fmt::Debug for AdmissionCandidateV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionCandidateV1([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct AdmissionPreparedV1 {
    proof: PreparedAdmissionProofV1,
}

impl AdmissionPreparedV1 {
    pub const fn new(proof: PreparedAdmissionProofV1) -> Self {
        Self { proof }
    }

    pub const fn proof(&self) -> &PreparedAdmissionProofV1 {
        &self.proof
    }
}

impl std::fmt::Debug for AdmissionPreparedV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionPreparedV1([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct AdmissionCommitV1 {
    exact_candidate: AdmissionCandidateV1,
    target_membership_history: AdmissionSignedMembershipHistory,
    sealed_recovery_material: AdmissionSealedRecoveryMaterial,
}

impl AdmissionCommitV1 {
    pub const fn new(
        exact_candidate: AdmissionCandidateV1,
        target_membership_history: AdmissionSignedMembershipHistory,
        sealed_recovery_material: AdmissionSealedRecoveryMaterial,
    ) -> Self {
        Self {
            exact_candidate,
            target_membership_history,
            sealed_recovery_material,
        }
    }

    pub const fn exact_candidate(&self) -> &AdmissionCandidateV1 {
        &self.exact_candidate
    }

    pub const fn target_membership_history(&self) -> &AdmissionSignedMembershipHistory {
        &self.target_membership_history
    }

    pub const fn sealed_recovery_material(&self) -> &AdmissionSealedRecoveryMaterial {
        &self.sealed_recovery_material
    }
}

impl std::fmt::Debug for AdmissionCommitV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionCommitV1([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct AdmissionAppliedV1 {
    activation_receipt: AdmissionActivationReceipt,
}

impl AdmissionAppliedV1 {
    pub const fn new(activation_receipt: AdmissionActivationReceipt) -> Self {
        Self { activation_receipt }
    }

    pub const fn activation_receipt(&self) -> &AdmissionActivationReceipt {
        &self.activation_receipt
    }
}

impl std::fmt::Debug for AdmissionAppliedV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionAppliedV1([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct AdmissionCompleteV1 {
    completion: AdmissionCompletionV1,
}

impl AdmissionCompleteV1 {
    pub const fn new(completion: AdmissionCompletionV1) -> Self {
        Self { completion }
    }

    pub const fn completion(&self) -> &AdmissionCompletionV1 {
        &self.completion
    }
}

impl std::fmt::Debug for AdmissionCompleteV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionCompleteV1([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct AdmissionSettledV1 {
    completion_ack_digest: [u8; 32],
}

impl AdmissionSettledV1 {
    pub fn new(completion_ack_digest: [u8; 32]) -> Option<Self> {
        if completion_ack_digest == [0; 32] {
            None
        } else {
            Some(Self {
                completion_ack_digest,
            })
        }
    }

    pub const fn completion_ack_digest(&self) -> &[u8; 32] {
        &self.completion_ack_digest
    }
}

impl std::fmt::Debug for AdmissionSettledV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionSettledV1([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub struct AdmissionCompleteAckV1 {
    completion_digest: [u8; 32],
}

impl AdmissionCompleteAckV1 {
    pub fn new(completion_digest: [u8; 32]) -> Option<Self> {
        if completion_digest == [0; 32] {
            None
        } else {
            Some(Self { completion_digest })
        }
    }

    pub const fn completion_digest(&self) -> &[u8; 32] {
        &self.completion_digest
    }
}

impl std::fmt::Debug for AdmissionCompleteAckV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AdmissionCompleteAckV1([REDACTED])")
    }
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionBodyV1 {
    JoinRequest(AdmissionJoinRequestV1),
    Candidate(AdmissionCandidateV1),
    Prepared(AdmissionPreparedV1),
    Commit(AdmissionCommitV1),
    Applied(AdmissionAppliedV1),
    Complete(AdmissionCompleteV1),
    CompleteAck(AdmissionCompleteAckV1),
    Settled(AdmissionSettledV1),
    CancelRequested,
    Rejected {
        reason: SpaceAdmissionRejectionReason,
    },
}

impl SpaceAdmissionBodyV1 {
    pub const fn kind(&self) -> SpaceAdmissionMessageKind {
        match self {
            Self::JoinRequest(_) => SpaceAdmissionMessageKind::JoinRequest,
            Self::Candidate(_) => SpaceAdmissionMessageKind::Candidate,
            Self::Prepared(_) => SpaceAdmissionMessageKind::Prepared,
            Self::Commit(_) => SpaceAdmissionMessageKind::Commit,
            Self::Applied(_) => SpaceAdmissionMessageKind::Applied,
            Self::Complete(_) => SpaceAdmissionMessageKind::Complete,
            Self::CompleteAck(_) => SpaceAdmissionMessageKind::CompleteAck,
            Self::Settled(_) => SpaceAdmissionMessageKind::Settled,
            Self::CancelRequested => SpaceAdmissionMessageKind::CancelRequested,
            Self::Rejected { .. } => SpaceAdmissionMessageKind::Rejected,
        }
    }
}

impl std::fmt::Debug for SpaceAdmissionBodyV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JoinRequest(_) => formatter.write_str("JoinRequest([REDACTED])"),
            Self::Candidate(_) => formatter.write_str("Candidate([REDACTED])"),
            Self::Prepared(_) => formatter.write_str("Prepared([REDACTED])"),
            Self::Commit(_) => formatter.write_str("Commit([REDACTED])"),
            Self::Applied(_) => formatter.write_str("Applied([REDACTED])"),
            Self::Complete(_) => formatter.write_str("Complete([REDACTED])"),
            Self::CompleteAck(_) => formatter.write_str("CompleteAck([REDACTED])"),
            Self::Settled(_) => formatter.write_str("Settled([REDACTED])"),
            Self::CancelRequested => formatter.write_str("CancelRequested"),
            Self::Rejected { .. } => formatter.write_str("Rejected([REDACTED])"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionProtocolMessageError {
    #[error("the message sender is not allowed for this message kind")]
    SenderNotAllowed,
    #[error("the initial join request header is invalid")]
    InvalidInitialJoinRequest,
    #[error("a non-initial admission message requires a predecessor")]
    MissingPredecessor,
    #[error("the admission message belongs to another admission")]
    AdmissionMismatch,
    #[error("the admission message came from an unexpected sender role")]
    UnexpectedSender,
    #[error("the admission message is out of order")]
    OutOfOrder,
    #[error("the admission message conflicts with saved evidence")]
    Conflict,
    #[error("the admission message digest is invalid")]
    InvalidDigest,
}

impl From<AdmissionMessageHeaderError> for AdmissionProtocolMessageError {
    fn from(error: AdmissionMessageHeaderError) -> Self {
        match error {
            AdmissionMessageHeaderError::SenderNotAllowed => Self::SenderNotAllowed,
            AdmissionMessageHeaderError::InvalidInitialJoinRequest => {
                Self::InvalidInitialJoinRequest
            }
            AdmissionMessageHeaderError::MissingPredecessor => Self::MissingPredecessor,
        }
    }
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionEnvelopeV1 {
    header: SpaceAdmissionEnvelopeHeaderV1,
    body: SpaceAdmissionBodyV1,
}

impl SpaceAdmissionEnvelopeV1 {
    pub fn new(
        admission_id: SpaceAdmissionId,
        sender_role: AdmissionRole,
        sender_sequence: u64,
        message_id: AdmissionMessageId,
        predecessor_message_id: Option<AdmissionMessageId>,
        body: SpaceAdmissionBodyV1,
    ) -> Result<Self, AdmissionProtocolMessageError> {
        let header = SpaceAdmissionEnvelopeHeaderV1::new(
            admission_id,
            body.kind(),
            sender_role,
            sender_sequence,
            message_id,
            predecessor_message_id,
        )?;
        Ok(Self { header, body })
    }

    pub const fn header(&self) -> &SpaceAdmissionEnvelopeHeaderV1 {
        &self.header
    }

    pub const fn kind(&self) -> SpaceAdmissionMessageKind {
        self.body.kind()
    }

    pub const fn body(&self) -> &SpaceAdmissionBodyV1 {
        &self.body
    }

    pub fn into_body(self) -> SpaceAdmissionBodyV1 {
        self.body
    }

    pub fn evidence(&self, canonical_digest: [u8; 32]) -> Option<AdmissionMessageEvidence> {
        AdmissionMessageEvidence::new(
            self.header.sender_role(),
            self.header.sender_sequence(),
            self.header.message_id(),
            self.header.predecessor_message_id(),
            canonical_digest,
        )
    }
}

impl std::fmt::Debug for SpaceAdmissionEnvelopeV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpaceAdmissionEnvelopeV1")
            .field("header", &self.header)
            .field("body", &self.body)
            .finish()
    }
}
