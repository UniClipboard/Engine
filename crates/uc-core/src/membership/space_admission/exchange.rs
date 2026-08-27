use super::artifact::SpaceAdmissionRoute;
use super::id::{AdmissionMessageId, SpaceAdmissionId};
use super::message::{
    AdmissionProtocolMessageError, AdmissionRole, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionMessageKind,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 准入规则对失败原因的稳定分类。
///
/// 具体状态变化错误和消息错误统一归入这些类别，调用方不需要理解内部阶段。
pub enum AdmissionErrorCategory {
    /// 消息或材料本身不满足准入规则。
    Invalid,
    /// 消息没有出现在当前状态期望的位置。
    OutOfOrder,
    /// 同一身份或位置出现了互相矛盾的事实。
    Conflict,
    /// 当前准入已经越过允许取消的边界。
    UnsafeCancellation,
    /// 记录无法继续安全推进，只能进入恢复处理。
    RecoveryRequired,
}

#[derive(PartialEq, Eq)]
/// 一条入站消息相对于当前期望的判断结果。
pub enum AdmissionInboundDecision {
    /// 消息是下一条合法新消息，并产生可保存的证据。
    New(AdmissionMessageEvidence),
    /// 消息与已经知道的同一条消息完全一致。
    ExactReplay,
}

/// 聚合根据已保存交换事实对一条消息作出的重放判断。
pub enum AdmissionReplayDecision<'a> {
    /// 消息是当前状态允许接收的下一条新消息。
    New,
    /// 消息已经处理过，但该阶段没有需要再次返回的回复。
    Duplicate,
    /// 消息已经处理过，必须返回当时保存的同一份回复。
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
/// 已保存证据无法将入站消息解释为合法新消息或精确重放。
pub enum AdmissionReplayError {
    #[error("the admission message conflicts with saved evidence")]
    Conflict,
    #[error("the admission message is out of order")]
    OutOfOrder,
}

impl AdmissionReplayError {
    /// 将重放错误收敛为稳定失败类别。
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
/// 构造待完成交换或固定回复时发现的规则错误。
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
/// 一项待完成交换的重试进度。
///
/// 重试次数和下次允许尝试的时间只能向前推进，不改变准入阶段本身。
pub struct AdmissionRetryState {
    attempt_count: u32,
    next_attempt_at_ms: i64,
}

impl AdmissionRetryState {
    /// 建立一份合法的重试进度；时间不能早于准入计时起点。
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

    /// 记录一次失败并推进下次尝试时间。
    ///
    /// 新时间不能早于当前时间，次数达到上限时拒绝继续累加。
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
/// 已经保存、等待取得精确回复的一次准入交换。
///
/// 它同时固定请求、预期回复种类、再次到达对端所需的信息和重试进度。重试必须继续使用同一请求，
/// 不能重新生成业务消息。
pub struct PendingAdmissionExchange {
    route: SpaceAdmissionRoute,
    request_envelope: SpaceAdmissionEnvelopeV1,
    exact_expected_reply_kind: SpaceAdmissionMessageKind,
    retry_state: AdmissionRetryState,
}

impl PendingAdmissionExchange {
    /// 建立待完成交换，并确认预期回复确实能够回答该请求。
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

    /// 判断当前保存的请求能否作为某条入站消息的精确后继回复。
    ///
    /// 只有前序消息身份一致且回复来自另一角色时才返回该请求。
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
/// 已处理入站消息及其当时生成的固定回复。
///
/// 后续收到相同消息时必须使用这份回复，不能再次执行状态变化或生成新内容。
pub struct SavedAdmissionReply {
    inbound_evidence: AdmissionMessageEvidence,
    exact_reply_envelope: SpaceAdmissionEnvelopeV1,
}

impl SavedAdmissionReply {
    /// 保存一份回复，并确认它属于同一 admission、直接跟随入站消息且由另一角色产生。
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
/// 当前状态对下一条入站消息的完整期望。
///
/// 期望由 admission、发送角色、发送序号和前序消息共同确定。
pub struct AdmissionInboundExpectation {
    admission_id: SpaceAdmissionId,
    sender_role: AdmissionRole,
    sender_sequence: u64,
    predecessor_message_id: Option<AdmissionMessageId>,
}

impl AdmissionInboundExpectation {
    /// 固定下一条入站消息必须满足的位置。
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

    /// 将消息判断为合法新消息、精确重放或协议错误。
    ///
    /// 已知消息先按重放规则判断；未知消息才继续检查角色、序号和前序关系。
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
/// 一条已认证消息的稳定业务证据。
///
/// 证据只保留判断顺序、重放和冲突所需的事实，不在调试输出中暴露消息身份或内容摘要。
pub struct AdmissionMessageEvidence {
    sender_role: AdmissionRole,
    sender_sequence: u64,
    message_id: AdmissionMessageId,
    predecessor_message_id: Option<AdmissionMessageId>,
    canonical_digest: [u8; 32],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// 两份消息证据之间的关系。
pub enum AdmissionEvidenceRelation {
    /// 消息身份和全部业务证据完全相同。
    ExactReplay,
    /// 消息身份不同，表示另一条消息。
    Distinct,
    /// 消息身份相同，但其他业务证据不同。
    Conflict,
}

impl AdmissionMessageEvidence {
    /// 从一条消息的稳定事实建立证据；全零内容摘要不是有效证据。
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

    /// 比较另一份证据是否是同一消息的精确重放、不同消息或冲突内容。
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
