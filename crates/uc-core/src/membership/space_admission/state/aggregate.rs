use super::*;

pub const SPACE_ADMISSION_RECORD_FORMAT_V1: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionEffect {
    ConsumeInvitation,
    CommitMembership,
    ApplyMembership,
    ActivateSecurity,
    PublishMembership,
    ActivateSpace,
    PublishActive,
}

#[derive(PartialEq, Eq)]
pub struct AdmissionTransition {
    replacement: SpaceAdmissionAggregate,
    effects: &'static [AdmissionEffect],
}

impl AdmissionTransition {
    pub(super) const fn new(
        replacement: SpaceAdmissionAggregate,
        effects: &'static [AdmissionEffect],
    ) -> Self {
        Self {
            replacement,
            effects,
        }
    }

    pub fn into_replacement(self) -> SpaceAdmissionAggregate {
        self.replacement
    }

    pub const fn effects(&self) -> &'static [AdmissionEffect] {
        self.effects
    }

    #[cfg(test)]
    pub(crate) fn exact_reply(&self) -> Option<&SpaceAdmissionEnvelopeV1> {
        self.replacement.current_exact_reply()
    }
}

impl std::ops::Deref for AdmissionTransition {
    type Target = SpaceAdmissionAggregate;

    fn deref(&self) -> &Self::Target {
        &self.replacement
    }
}

impl std::fmt::Debug for AdmissionTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionTransition")
            .field("replacement", &self.replacement)
            .field("effects", &self.effects)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRecoveryCategory {
    ProtocolConflict,
    CorruptState,
    MissingKey,
    CounterOverflow,
    SpaceActivation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SpaceAdmissionAggregateError {
    #[error("the initial exchange belongs to another admission")]
    AdmissionMismatch,
    #[error("the initial exchange is not a JoinRequest expecting Candidate")]
    InvalidInitialExchange,
    #[error("the admission record version cannot be incremented")]
    RecordVersionOverflow,
    #[error("the admission transition is not valid from the current state")]
    InvalidTransition,
    #[error("the inbound message evidence does not match the message")]
    InvalidInboundEvidence,
    #[error("the candidate reply is invalid")]
    InvalidCandidateReply,
    #[error("the prepared request is invalid")]
    InvalidPreparedRequest,
    #[error("the prepared message is invalid")]
    InvalidPreparedMessage,
    #[error("the commit reply is invalid")]
    InvalidCommitReply,
    #[error("the commit message is invalid")]
    InvalidCommitMessage,
    #[error("the applied request is invalid")]
    InvalidAppliedRequest,
    #[error("the applied message is invalid")]
    InvalidAppliedMessage,
    #[error("the complete reply is invalid")]
    InvalidCompleteReply,
    #[error("the complete message is invalid")]
    InvalidCompleteMessage,
    #[error("the complete acknowledgement is invalid")]
    InvalidCompleteAck,
    #[error("the complete acknowledgement message is invalid")]
    InvalidCompleteAckMessage,
    #[error("the settled reply is invalid")]
    InvalidSettledReply,
    #[error("the settled message is invalid")]
    InvalidSettledMessage,
    #[error("the cancellation request is invalid")]
    InvalidCancellationRequest,
    #[error("the current admission cannot be superseded")]
    UnsafeSupersession,
    #[error("the current admission cannot be cancelled")]
    UnsafeCancellation,
    #[error("the admission was already formally committed")]
    TooLateCommitted,
    #[error("the rejected reply is invalid")]
    InvalidRejectedReply,
    #[error("the completion helper challenge is invalid")]
    InvalidHelperChallenge,
    #[error("the completion helper result is invalid")]
    InvalidHelperCompletion,
    #[error("the completion helper challenge counter cannot be incremented")]
    CounterOverflow,
}

impl SpaceAdmissionAggregateError {
    pub const fn category(self) -> AdmissionErrorCategory {
        match self {
            Self::InvalidTransition => AdmissionErrorCategory::OutOfOrder,
            Self::AdmissionMismatch | Self::UnsafeSupersession => AdmissionErrorCategory::Conflict,
            Self::UnsafeCancellation | Self::TooLateCommitted => {
                AdmissionErrorCategory::UnsafeCancellation
            }
            Self::RecordVersionOverflow | Self::CounterOverflow => {
                AdmissionErrorCategory::RecoveryRequired
            }
            Self::InvalidInitialExchange
            | Self::InvalidInboundEvidence
            | Self::InvalidCandidateReply
            | Self::InvalidPreparedRequest
            | Self::InvalidPreparedMessage
            | Self::InvalidCommitReply
            | Self::InvalidCommitMessage
            | Self::InvalidAppliedRequest
            | Self::InvalidAppliedMessage
            | Self::InvalidCompleteReply
            | Self::InvalidCompleteMessage
            | Self::InvalidCompleteAck
            | Self::InvalidCompleteAckMessage
            | Self::InvalidSettledReply
            | Self::InvalidSettledMessage
            | Self::InvalidCancellationRequest
            | Self::InvalidRejectedReply
            | Self::InvalidHelperChallenge
            | Self::InvalidHelperCompletion => AdmissionErrorCategory::Invalid,
        }
    }
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionTerminalState {
    Active(SpaceAdmissionActiveState),
    Completed(SpaceAdmissionCompletedTerminal),
    Superseded(SpaceAdmissionSupersededState),
    Rejected(SpaceAdmissionRejectedState),
    RecoveryRequired(SpaceAdmissionRecoveryRequiredTerminal),
}

#[derive(PartialEq, Eq)]
pub enum SpaceAdmissionRecordState {
    Joiner(SpaceAdmissionJoinerState),
    Sponsor(SpaceAdmissionSponsorState),
    CompletionHelper(SpaceAdmissionCompletionHelperState),
    Terminal(SpaceAdmissionTerminalState),
}

#[derive(PartialEq, Eq)]
pub struct SpaceAdmissionAggregate {
    pub(super) format_version: u16,
    pub(super) record_version: u64,
    pub(super) admission_id: SpaceAdmissionId,
    pub(super) state: SpaceAdmissionRecordState,
}

impl std::fmt::Debug for SpaceAdmissionAggregate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
                _,
            )) => "Joiner::ResolvingInvitation",
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvedInvitation(_)) => {
                "Joiner::ResolvedInvitation"
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(_)) => {
                "Joiner::Initiated"
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(_)) => {
                "Joiner::Prepared"
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(_)) => {
                "Joiner::Committed"
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(_)) => {
                "Joiner::Applied"
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(_)) => {
                "Joiner::Activating"
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(_)) => {
                "Joiner::Cancelling"
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(_)) => {
                "Joiner::Candidate"
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(_)) => {
                "Sponsor::Accepted"
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(_)) => {
                "Sponsor::Candidate"
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(_)) => {
                "Sponsor::Committed"
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(_)) => {
                "Sponsor::Applied"
            }
            SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Challenged(_),
            ) => "CompletionHelper::Challenged",
            SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Applied(_),
            ) => "CompletionHelper::Applied",
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::PendingSettlement(_),
            )) => "Terminal::ActivePendingSettlement",
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::Settled(_),
            )) => "Terminal::ActiveSettled",
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(_)) => {
                "Terminal::Completed"
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(_)) => {
                "Terminal::Superseded"
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(_)) => {
                "Terminal::Rejected"
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
                _,
            )) => "Terminal::RecoveryRequired",
        };
        formatter
            .debug_struct("SpaceAdmissionAggregate")
            .field("format_version", &self.format_version)
            .field("record_version", &self.record_version)
            .field("admission_id", &"[REDACTED]")
            .field("state", &state)
            .finish()
    }
}
