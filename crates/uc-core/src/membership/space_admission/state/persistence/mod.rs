use serde::{Deserialize, Serialize};

use crate::ids::DeviceId;
use crate::membership::{
    AdmissionCompletionV1, AdmissionSecurityCommitmentV1, MembershipCredential, MembershipEventV2,
    PreparedAdmissionProofV1, ADMISSION_COMPLETION_FORMAT_V1, PREPARED_ADMISSION_PROOF_FORMAT_V1,
};

use super::super::artifact::{
    AdmissionContinuationRoute, AdmissionIdentitySignature, AdmissionKeyPackage,
    AdmissionMlsCommit, AdmissionMlsWelcome, AdmissionRecoveryPublicKey,
    AdmissionSealedRecoveryMaterial, SpaceAdmissionRoute,
};
use super::super::exchange::{AdmissionRetryState, SavedAdmissionReply};
use super::super::id::{AdmissionChannelPeerId, InvitationId};
use super::super::message::{
    AdmissionAppliedV1, AdmissionCandidateV1, AdmissionCommitV1, AdmissionCompleteAckV1,
    AdmissionCompleteV1, AdmissionJoinRequestV1, AdmissionPreparedV1, AdmissionRole,
    AdmissionSettledV1, SpaceAdmissionBodyV1, SpaceAdmissionEnvelopeHeaderV1,
    SpaceAdmissionMessageKind, SpaceAdmissionProtocolVersion, UnreadableHistoryPolicy,
};
use super::*;

mod aggregate;
mod initial;
mod joiner;
mod message;
mod sponsor;
mod terminal;
mod value;

use message::validate_envelope_evidence;
use value::*;

const ADMISSION_ACTIVATION_RECEIPT_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpaceAdmissionPersistenceError {
    #[error("the persisted admission encoding is invalid")]
    InvalidEncoding,
    #[error("the persisted admission version is not supported")]
    UnsupportedVersion,
    #[error("the persisted admission state violates protocol rules")]
    InvalidState,
}

#[derive(Serialize, Deserialize)]
struct PersistedSpaceAdmissionRecordV1 {
    format_version: u16,
    record_version: u64,
    admission_id: [u8; 32],
    state: PersistedSpaceAdmissionStateV1,
}

#[derive(Serialize, Deserialize)]
enum PersistedSpaceAdmissionStateV1 {
    JoinerInitiated(PersistedJoinerInitiatedV1),
    JoinerCandidate(PersistedJoinerCandidateV1),
    JoinerPrepared(PersistedJoinerPreparedV1),
    SponsorAccepted(PersistedSponsorAcceptedV1),
    SponsorCandidate(PersistedSponsorCandidateV1),
    JoinerCommitted(PersistedJoinerCommittedV1),
    JoinerApplied(PersistedJoinerAppliedV1),
    JoinerActivating(PersistedJoinerActivatingV1),
    JoinerCancelling(PersistedJoinerCancellingV1),
    SponsorCommitted(PersistedSponsorCommittedV1),
    SponsorApplied(PersistedSponsorAppliedV1),
    CompletionHelperChallenged(PersistedCompletionHelperChallengedV1),
    CompletionHelperApplied(PersistedCompletionHelperAppliedV1),
    ActivePendingSettlement(PersistedActivePendingSettlementV1),
    ActiveSettled(PersistedActiveSettledV1),
    Completed(PersistedCompletedV1),
    Superseded(PersistedSupersededV1),
    Rejected(PersistedRejectedV1),
    RecoveryRequired(u8),
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinerInitiatedV1 {
    join_id: [u8; 16],
    local_join_ordinal: u64,
    source_snapshot: Vec<u8>,
    private_state: Vec<u8>,
    channel_state: PersistedJoinerChannelStateV1,
    pending_exchange: PersistedPendingExchangeV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinerCandidateV1 {
    join_id: [u8; 16],
    local_join_ordinal: u64,
    source_snapshot: Vec<u8>,
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    candidate: PersistedCandidateEnvelopeV1,
    candidate_evidence: PersistedMessageEvidenceV1,
    staged_target_input: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinerPreparedV1 {
    join_id: [u8; 16],
    local_join_ordinal: u64,
    source_snapshot: Vec<u8>,
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    candidate_evidence: PersistedMessageEvidenceV1,
    verified_history: Vec<u8>,
    staged_target: Vec<u8>,
    pending_exchange: PersistedPreparedPendingExchangeV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedSponsorAcceptedV1 {
    invitation_claim: Vec<u8>,
    join_request: PersistedJoinRequestEnvelopeV1,
    join_request_evidence: PersistedMessageEvidenceV1,
    base_snapshot: Vec<u8>,
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PersistedSponsorCandidateV1 {
    invitation_claim: Vec<u8>,
    base_snapshot: Vec<u8>,
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    staged_security: Vec<u8>,
    saved_reply: PersistedSavedCandidateReplyV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedPeerBindingV1 {
    local_peer_id: [u8; 32],
    remote_peer_id: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct PersistedMessageEvidenceV1 {
    sender_role: u8,
    sender_sequence: u64,
    message_id: [u8; 32],
    predecessor_message_id: Option<[u8; 32]>,
    canonical_digest: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct PersistedSavedCandidateReplyV1 {
    inbound_evidence: PersistedMessageEvidenceV1,
    exact_reply: PersistedCandidateEnvelopeV1,
}

#[derive(Serialize, Deserialize)]
enum PersistedJoinerChannelStateV1 {
    AwaitingAuthentication {
        encrypted_password_equivalent: Vec<u8>,
    },
    Authenticated {
        local_peer_id: [u8; 32],
        remote_peer_id: [u8; 32],
        continuation_credential: Vec<u8>,
    },
}

#[derive(Serialize, Deserialize)]
struct PersistedPendingExchangeV1 {
    route: Vec<u8>,
    request: PersistedJoinRequestEnvelopeV1,
    expected_reply_kind: u8,
    retry_attempt_count: u32,
    retry_next_attempt_at_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct PersistedPreparedPendingExchangeV1 {
    route: Vec<u8>,
    request: PersistedPreparedEnvelopeV1,
    expected_reply_kind: u8,
    retry_attempt_count: u32,
    retry_next_attempt_at_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinRequestEnvelopeV1 {
    protocol_version: u16,
    admission_id: [u8; 32],
    sender_role: u8,
    sender_sequence: u64,
    message_id: [u8; 32],
    predecessor_message_id: Option<[u8; 32]>,
    body: PersistedJoinRequestV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedEnvelopeHeaderV1 {
    protocol_version: u16,
    admission_id: [u8; 32],
    sender_role: u8,
    sender_sequence: u64,
    message_id: [u8; 32],
    predecessor_message_id: Option<[u8; 32]>,
}

#[derive(Serialize, Deserialize)]
struct PersistedCandidateEnvelopeV1 {
    header: PersistedEnvelopeHeaderV1,
    body: PersistedCandidateV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedPreparedEnvelopeV1 {
    header: PersistedEnvelopeHeaderV1,
    proof: PreparedAdmissionProofV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinRequestV1 {
    invitation_id: [u8; 32],
    device_id: String,
    credential_format_version: u16,
    credential_signature_algorithm_version: u16,
    credential_public_key: Vec<u8>,
    credential_id: [u8; 32],
    key_package: Vec<u8>,
    recovery_public_key: [u8; 32],
    identity_signature: Vec<u8>,
    unreadable_history_policy: u8,
}

#[derive(Serialize, Deserialize)]
struct PersistedCandidateV1 {
    base_membership_history: Vec<u8>,
    candidate_event: MembershipEventV2,
    security_commitment: AdmissionSecurityCommitmentV1,
    mls_commit: Vec<u8>,
    mls_welcome: Vec<u8>,
    continuation_route: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinerContextV1 {
    join_id: [u8; 16],
    local_join_ordinal: u64,
    source_snapshot: Vec<u8>,
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinerCommittedV1 {
    context: PersistedJoinerContextV1,
    exact_commit: PersistedEnvelopeV1,
    commit_evidence: PersistedMessageEvidenceV1,
    staged_target: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinerAppliedV1 {
    context: PersistedJoinerContextV1,
    exact_commit: PersistedEnvelopeV1,
    commit_evidence: PersistedMessageEvidenceV1,
    staged_target: Vec<u8>,
    pending_exchange: PersistedAnyPendingExchangeV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinerActivatingV1 {
    context: PersistedJoinerContextV1,
    exact_commit: PersistedEnvelopeV1,
    staged_target: Vec<u8>,
    completion: PersistedEnvelopeV1,
    completion_evidence: PersistedMessageEvidenceV1,
    space_transition: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PersistedJoinerCancellingV1 {
    join_id: [u8; 16],
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    last_received: PersistedMessageEvidenceV1,
    pending_exchange: PersistedAnyPendingExchangeV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedSponsorCommittedV1 {
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    committed_history: Vec<u8>,
    sealed_security: Vec<u8>,
    saved_reply: PersistedSavedReplyV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedSponsorAppliedV1 {
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    committed_history: Vec<u8>,
    activation_receipt: AdmissionActivationReceipt,
    activated_security: Vec<u8>,
    saved_reply: PersistedSavedReplyV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedCompletionHelperChallengedV1 {
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    challenge_counter: u64,
    nonce: [u8; 32],
    last_joiner_message_id: [u8; 32],
    last_sponsor_message_id: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct PersistedCompletionHelperAppliedV1 {
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    verified_commit: PersistedEnvelopeV1,
    activation_receipt: AdmissionActivationReceipt,
    helper_security: Vec<u8>,
    saved_reply: PersistedSavedReplyV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedActivePendingSettlementV1 {
    join_id: [u8; 16],
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    completion_evidence: PersistedMessageEvidenceV1,
    transition_result: Vec<u8>,
    pending_exchange: PersistedAnyPendingExchangeV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedActiveSettledV1 {
    join_id: [u8; 16],
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    last_received: PersistedMessageEvidenceV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedCompletedV1 {
    peer_binding: PersistedPeerBindingV1,
    continuation_credential: Vec<u8>,
    saved_reply: PersistedSavedReplyV1,
}

#[derive(Serialize, Deserialize)]
enum PersistedSupersededV1 {
    Initiated {
        join_id: [u8; 16],
    },
    Authenticated {
        join_id: [u8; 16],
        peer_binding: PersistedPeerBindingV1,
        continuation_credential: Vec<u8>,
    },
    Candidate {
        join_id: [u8; 16],
        peer_binding: PersistedPeerBindingV1,
        continuation_credential: Vec<u8>,
        last_received: PersistedMessageEvidenceV1,
    },
}

#[derive(Serialize, Deserialize)]
enum PersistedRejectedV1 {
    LocalJoiner {
        join_id: [u8; 16],
        reason: u8,
    },
    Joiner {
        join_id: [u8; 16],
        peer_binding: PersistedPeerBindingV1,
        continuation_credential: Vec<u8>,
        reason: u8,
        last_received: PersistedMessageEvidenceV1,
    },
    Sponsor {
        peer_binding: PersistedPeerBindingV1,
        continuation_credential: Vec<u8>,
        reason: u8,
        saved_reply: PersistedSavedReplyV1,
    },
}

#[derive(Serialize, Deserialize)]
struct PersistedEnvelopeV1 {
    header: PersistedEnvelopeHeaderV1,
    body: PersistedBodyV1,
}

#[derive(Serialize, Deserialize)]
enum PersistedBodyV1 {
    JoinRequest(PersistedJoinRequestV1),
    Candidate(PersistedCandidateV1),
    Prepared(PreparedAdmissionProofV1),
    Commit {
        exact_candidate: PersistedCandidateV1,
        target_membership_history: Vec<u8>,
        sealed_recovery_material: Vec<u8>,
    },
    Applied(AdmissionActivationReceipt),
    Complete(AdmissionCompletionV1),
    CompleteAck([u8; 32]),
    Settled([u8; 32]),
    CancelRequested,
    Rejected(u8),
}

#[derive(Serialize, Deserialize)]
struct PersistedAnyPendingExchangeV1 {
    route: Vec<u8>,
    request: PersistedEnvelopeV1,
    expected_reply_kind: u8,
    retry_attempt_count: u32,
    retry_next_attempt_at_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct PersistedSavedReplyV1 {
    inbound_evidence: PersistedMessageEvidenceV1,
    exact_reply: PersistedEnvelopeV1,
}
