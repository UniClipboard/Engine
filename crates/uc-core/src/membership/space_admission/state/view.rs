use super::super::message::SpaceAdmissionEnvelopeV1;
use super::*;

impl SpaceAdmissionAggregate {
    pub const fn is_terminal(&self) -> bool {
        matches!(self.state, SpaceAdmissionRecordState::Terminal(_))
    }

    pub fn current_exact_reply(&self) -> Option<&SpaceAdmissionEnvelopeV1> {
        match &self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => {
                Some(state.pending_exchange.request_envelope())
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) => {
                Some(state.pending_exchange.request_envelope())
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(state)) => {
                Some(state.pending_exchange.request_envelope())
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => {
                Some(state.saved_reply.exact_reply_envelope())
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) => {
                Some(state.saved_reply.exact_reply_envelope())
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) => {
                Some(state.saved_reply.exact_reply_envelope())
            }
            SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Applied(state),
            ) => Some(state.saved_reply.exact_reply_envelope()),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::PendingSettlement(state),
            )) => Some(state.pending_exchange.request_envelope()),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(state)) => {
                Some(state.saved_reply.exact_reply_envelope())
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::Sponsor(state),
            )) => Some(state.saved_reply.exact_reply_envelope()),
            SpaceAdmissionRecordState::Joiner(
                SpaceAdmissionJoinerState::Initiated(_)
                | SpaceAdmissionJoinerState::Candidate(_)
                | SpaceAdmissionJoinerState::Committed(_)
                | SpaceAdmissionJoinerState::Activating(_),
            )
            | SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(_))
            | SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Challenged(_),
            )
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::Settled(_),
            ))
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::LocalJoiner(_),
            ))
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::Joiner(_),
            ))
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(_))
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
                _,
            )) => None,
        }
    }

    pub const fn pending_exchange(&self) -> Option<&PendingAdmissionExchange> {
        match &self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => {
                Some(&state.pending_exchange)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => {
                Some(&state.pending_exchange)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) => {
                Some(&state.pending_exchange)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(state)) => {
                Some(&state.pending_exchange)
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::PendingSettlement(state),
            )) => Some(&state.pending_exchange),
            _ => None,
        }
    }
}
