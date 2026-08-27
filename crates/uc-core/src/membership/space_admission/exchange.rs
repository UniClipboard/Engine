use super::artifact::SpaceAdmissionRoute;
use super::id::{AdmissionMessageId, SpaceAdmissionId};
use super::message::{
    AdmissionProtocolMessageError, AdmissionRole, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionMessageKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionErrorCategory {
    Invalid,
    OutOfOrder,
    Conflict,
    UnsafeCancellation,
    RecoveryRequired,
}

#[derive(PartialEq, Eq)]
pub enum AdmissionInboundDecision {
    New(AdmissionMessageEvidence),
    ExactReplay,
}

pub enum AdmissionReplayDecision<'a> {
    New,
    Duplicate,
    ExactReply(&'a SpaceAdmissionEnvelopeV1),
}

impl std::fmt::Debug for AdmissionReplayDecision<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::New => formatter.write_str("AdmissionReplayDecision::New"),
            Self::Duplicate => formatter.write_str("AdmissionReplayDecision::Duplicate"),
            Self::ExactReply(reply) => formatter
                .debug_tuple("AdmissionReplayDecision::ExactReply")
                .field(&reply.kind())
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionReplayError {
    #[error("the admission message conflicts with saved evidence")]
    Conflict,
    #[error("the admission message is out of order")]
    OutOfOrder,
}

impl AdmissionReplayError {
    pub const fn category(self) -> AdmissionErrorCategory {
        match self {
            Self::Conflict => AdmissionErrorCategory::Conflict,
            Self::OutOfOrder => AdmissionErrorCategory::OutOfOrder,
        }
    }
}

impl std::fmt::Debug for AdmissionInboundDecision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::New(_) => formatter.write_str("AdmissionInboundDecision::New([REDACTED])"),
            Self::ExactReplay => formatter.write_str("AdmissionInboundDecision::ExactReplay"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AdmissionPendingExchangeError {
    #[error("the expected reply cannot answer this admission message")]
    InvalidExpectedReply,
    #[error("the retry timestamp is invalid")]
    InvalidRetryTime,
    #[error("the retry count cannot be incremented")]
    RetryCountOverflow,
    #[error("the saved reply belongs to another admission")]
    ReplyAdmissionMismatch,
    #[error("the saved reply does not follow the inbound message")]
    ReplyPredecessorMismatch,
    #[error("the saved reply has the same role as the inbound message")]
    ReplyRoleMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionRetryState {
    attempt_count: u32,
    next_attempt_at_ms: i64,
}

impl AdmissionRetryState {
    pub fn new(
        attempt_count: u32,
        next_attempt_at_ms: i64,
    ) -> Result<Self, AdmissionPendingExchangeError> {
        if next_attempt_at_ms < 0 {
            return Err(AdmissionPendingExchangeError::InvalidRetryTime);
        }
        Ok(Self {
            attempt_count,
            next_attempt_at_ms,
        })
    }

    pub const fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    pub const fn next_attempt_at_ms(&self) -> i64 {
        self.next_attempt_at_ms
    }

    pub fn after_failure(
        &self,
        next_attempt_at_ms: i64,
    ) -> Result<Self, AdmissionPendingExchangeError> {
        if next_attempt_at_ms < self.next_attempt_at_ms {
            return Err(AdmissionPendingExchangeError::InvalidRetryTime);
        }
        let attempt_count = self
            .attempt_count
            .checked_add(1)
            .ok_or(AdmissionPendingExchangeError::RetryCountOverflow)?;
        Ok(Self {
            attempt_count,
            next_attempt_at_ms,
        })
    }
}

#[derive(PartialEq, Eq)]
pub struct PendingAdmissionExchange {
    route: SpaceAdmissionRoute,
    request_envelope: SpaceAdmissionEnvelopeV1,
    exact_expected_reply_kind: SpaceAdmissionMessageKind,
    retry_state: AdmissionRetryState,
}

impl PendingAdmissionExchange {
    pub fn new(
        route: SpaceAdmissionRoute,
        request_envelope: SpaceAdmissionEnvelopeV1,
        exact_expected_reply_kind: SpaceAdmissionMessageKind,
        retry_state: AdmissionRetryState,
    ) -> Result<Self, AdmissionPendingExchangeError> {
        if !exact_expected_reply_kind.can_reply_to(request_envelope.kind()) {
            return Err(AdmissionPendingExchangeError::InvalidExpectedReply);
        }
        Ok(Self {
            route,
            request_envelope,
            exact_expected_reply_kind,
            retry_state,
        })
    }

    pub const fn route(&self) -> &SpaceAdmissionRoute {
        &self.route
    }

    pub const fn request_envelope(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.request_envelope
    }

    pub const fn exact_expected_reply_kind(&self) -> SpaceAdmissionMessageKind {
        self.exact_expected_reply_kind
    }

    pub const fn retry_state(&self) -> &AdmissionRetryState {
        &self.retry_state
    }

    pub fn exact_reply_for(
        &self,
        inbound_evidence: &AdmissionMessageEvidence,
    ) -> Option<&SpaceAdmissionEnvelopeV1> {
        let header = self.request_envelope.header();
        (header.predecessor_message_id() == Some(inbound_evidence.message_id())
            && header.sender_role() != inbound_evidence.sender_role())
        .then_some(&self.request_envelope)
    }
}

impl std::fmt::Debug for PendingAdmissionExchange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingAdmissionExchange")
            .field("route", &self.route)
            .field("request_kind", &self.request_envelope.kind())
            .field("exact_expected_reply_kind", &self.exact_expected_reply_kind)
            .field("retry_state", &self.retry_state)
            .finish()
    }
}

#[derive(PartialEq, Eq)]
pub struct SavedAdmissionReply {
    inbound_evidence: AdmissionMessageEvidence,
    exact_reply_envelope: SpaceAdmissionEnvelopeV1,
}

impl SavedAdmissionReply {
    pub fn new(
        admission_id: SpaceAdmissionId,
        inbound_evidence: AdmissionMessageEvidence,
        exact_reply_envelope: SpaceAdmissionEnvelopeV1,
    ) -> Result<Self, AdmissionPendingExchangeError> {
        if exact_reply_envelope.header().admission_id() != admission_id {
            return Err(AdmissionPendingExchangeError::ReplyAdmissionMismatch);
        }
        if exact_reply_envelope.header().predecessor_message_id()
            != Some(inbound_evidence.message_id())
        {
            return Err(AdmissionPendingExchangeError::ReplyPredecessorMismatch);
        }
        if exact_reply_envelope.header().sender_role() == inbound_evidence.sender_role() {
            return Err(AdmissionPendingExchangeError::ReplyRoleMismatch);
        }
        Ok(Self {
            inbound_evidence,
            exact_reply_envelope,
        })
    }

    pub const fn inbound_evidence(&self) -> &AdmissionMessageEvidence {
        &self.inbound_evidence
    }

    pub const fn exact_reply_envelope(&self) -> &SpaceAdmissionEnvelopeV1 {
        &self.exact_reply_envelope
    }
}

impl std::fmt::Debug for SavedAdmissionReply {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SavedAdmissionReply")
            .field("inbound_evidence", &self.inbound_evidence)
            .field("reply_kind", &self.exact_reply_envelope.kind())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmissionInboundExpectation {
    admission_id: SpaceAdmissionId,
    sender_role: AdmissionRole,
    sender_sequence: u64,
    predecessor_message_id: Option<AdmissionMessageId>,
}

impl AdmissionInboundExpectation {
    pub const fn new(
        admission_id: SpaceAdmissionId,
        sender_role: AdmissionRole,
        sender_sequence: u64,
        predecessor_message_id: Option<AdmissionMessageId>,
    ) -> Self {
        Self {
            admission_id,
            sender_role,
            sender_sequence,
            predecessor_message_id,
        }
    }

    pub fn classify(
        &self,
        envelope: &SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        known_message: Option<&AdmissionMessageEvidence>,
    ) -> Result<AdmissionInboundDecision, AdmissionProtocolMessageError> {
        if envelope.header().admission_id() != self.admission_id {
            return Err(AdmissionProtocolMessageError::AdmissionMismatch);
        }
        let evidence = envelope
            .evidence(canonical_digest)
            .ok_or(AdmissionProtocolMessageError::InvalidDigest)?;

        if let Some(known) = known_message {
            return match known.relation_to(&evidence) {
                AdmissionEvidenceRelation::ExactReplay => Ok(AdmissionInboundDecision::ExactReplay),
                AdmissionEvidenceRelation::Conflict | AdmissionEvidenceRelation::Distinct => {
                    Err(AdmissionProtocolMessageError::Conflict)
                }
            };
        }

        if evidence.sender_role() != self.sender_role {
            return Err(AdmissionProtocolMessageError::UnexpectedSender);
        }
        if evidence.sender_sequence() != self.sender_sequence
            || evidence.predecessor_message_id() != self.predecessor_message_id
        {
            return Err(AdmissionProtocolMessageError::OutOfOrder);
        }
        Ok(AdmissionInboundDecision::New(evidence))
    }
}

#[derive(PartialEq, Eq)]
pub struct AdmissionMessageEvidence {
    sender_role: AdmissionRole,
    sender_sequence: u64,
    message_id: AdmissionMessageId,
    predecessor_message_id: Option<AdmissionMessageId>,
    canonical_digest: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionEvidenceRelation {
    ExactReplay,
    Distinct,
    Conflict,
}

impl AdmissionMessageEvidence {
    pub fn new(
        sender_role: AdmissionRole,
        sender_sequence: u64,
        message_id: AdmissionMessageId,
        predecessor_message_id: Option<AdmissionMessageId>,
        canonical_digest: [u8; 32],
    ) -> Option<Self> {
        if canonical_digest == [0; 32] {
            return None;
        }
        Some(Self {
            sender_role,
            sender_sequence,
            message_id,
            predecessor_message_id,
            canonical_digest,
        })
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

    pub const fn canonical_digest(&self) -> &[u8; 32] {
        &self.canonical_digest
    }

    pub fn relation_to(&self, incoming: &Self) -> AdmissionEvidenceRelation {
        if self.message_id != incoming.message_id {
            return AdmissionEvidenceRelation::Distinct;
        }

        if self.sender_role == incoming.sender_role
            && self.sender_sequence == incoming.sender_sequence
            && self.predecessor_message_id == incoming.predecessor_message_id
            && self.canonical_digest == incoming.canonical_digest
        {
            AdmissionEvidenceRelation::ExactReplay
        } else {
            AdmissionEvidenceRelation::Conflict
        }
    }
}

impl std::fmt::Debug for AdmissionMessageEvidence {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionMessageEvidence")
            .field("sender_role", &self.sender_role)
            .field("sender_sequence", &self.sender_sequence)
            .field("message_id", &"[REDACTED]")
            .field("has_predecessor", &self.predecessor_message_id.is_some())
            .field("canonical_digest", &"[REDACTED]")
            .finish()
    }
}
