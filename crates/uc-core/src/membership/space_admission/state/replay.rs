use super::super::exchange::{
    AdmissionEvidenceRelation, AdmissionMessageEvidence, AdmissionReplayDecision,
    AdmissionReplayError,
};
use super::super::id::AdmissionMessageId;
use super::super::message::{AdmissionRole, SpaceAdmissionEnvelopeV1};
use super::*;

const JOINER_ONLY: &[AdmissionRole] = &[AdmissionRole::Joiner];
const SPONSOR_ONLY: &[AdmissionRole] = &[AdmissionRole::Sponsor];
const SPONSOR_OR_HELPER: &[AdmissionRole] =
    &[AdmissionRole::Sponsor, AdmissionRole::CompletionHelper];

struct ExpectedEvidence {
    sender_roles: &'static [AdmissionRole],
    sender_sequence: u64,
    predecessor_message_id: AdmissionMessageId,
}

impl ExpectedEvidence {
    fn accepts(&self, evidence: &AdmissionMessageEvidence) -> bool {
        self.sender_roles.contains(&evidence.sender_role())
            && evidence.sender_sequence() == self.sender_sequence
            && evidence.predecessor_message_id() == Some(self.predecessor_message_id)
    }
}

impl SpaceAdmissionAggregate {
    pub fn replay_or_reject<'a>(
        &'a self,
        incoming: &AdmissionMessageEvidence,
    ) -> Result<AdmissionReplayDecision<'a>, AdmissionReplayError> {
        let (known, expected) = self.replay_facts();

        if let Some((known_evidence, exact_reply)) = known {
            match known_evidence.relation_to(incoming) {
                AdmissionEvidenceRelation::ExactReplay => {
                    return Ok(match exact_reply {
                        Some(reply) => AdmissionReplayDecision::ExactReply(reply),
                        None => AdmissionReplayDecision::Duplicate,
                    });
                }
                AdmissionEvidenceRelation::Conflict => {
                    return Err(AdmissionReplayError::Conflict);
                }
                AdmissionEvidenceRelation::Distinct => {}
            }
        }

        if expected.is_some_and(|expected| expected.accepts(incoming)) {
            Ok(AdmissionReplayDecision::New)
        } else {
            Err(AdmissionReplayError::OutOfOrder)
        }
    }

    fn replay_facts(
        &self,
    ) -> (
        Option<(&AdmissionMessageEvidence, Option<&SpaceAdmissionEnvelopeV1>)>,
        Option<ExpectedEvidence>,
    ) {
        match &self.state {
            SpaceAdmissionRecordState::Joiner(
                SpaceAdmissionJoinerState::ResolvingInvitation(_)
                | SpaceAdmissionJoinerState::ResolvedInvitation(_),
            ) => (None, None),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => (
                None,
                Some(ExpectedEvidence {
                    sender_roles: SPONSOR_ONLY,
                    sender_sequence: 0,
                    predecessor_message_id: state
                        .pending_exchange
                        .request_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
                (Some((&state.candidate_evidence, None)), None)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => (
                Some((
                    &state.candidate_evidence,
                    Some(state.pending_exchange.request_envelope()),
                )),
                Some(ExpectedEvidence {
                    sender_roles: SPONSOR_ONLY,
                    sender_sequence: 1,
                    predecessor_message_id: state
                        .pending_exchange
                        .request_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(state)) => {
                (Some((&state.commit_evidence, None)), None)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) => (
                Some((
                    &state.commit_evidence,
                    Some(state.pending_exchange.request_envelope()),
                )),
                Some(ExpectedEvidence {
                    sender_roles: SPONSOR_ONLY,
                    sender_sequence: 2,
                    predecessor_message_id: state
                        .pending_exchange
                        .request_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(state)) => {
                (Some((&state.completion_evidence, None)), None)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(state)) => (
                Some((
                    &state.last_received,
                    Some(state.pending_exchange.request_envelope()),
                )),
                Some(ExpectedEvidence {
                    sender_roles: SPONSOR_ONLY,
                    sender_sequence: 1,
                    predecessor_message_id: state
                        .pending_exchange
                        .request_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(state)) => {
                (Some((&state.join_request_evidence, None)), None)
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => (
                Some((
                    state.saved_reply.inbound_evidence(),
                    Some(state.saved_reply.exact_reply_envelope()),
                )),
                Some(ExpectedEvidence {
                    sender_roles: JOINER_ONLY,
                    sender_sequence: 1,
                    predecessor_message_id: state
                        .saved_reply
                        .exact_reply_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) => (
                Some((
                    state.saved_reply.inbound_evidence(),
                    Some(state.saved_reply.exact_reply_envelope()),
                )),
                Some(ExpectedEvidence {
                    sender_roles: JOINER_ONLY,
                    sender_sequence: 2,
                    predecessor_message_id: state
                        .saved_reply
                        .exact_reply_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) => (
                Some((
                    state.saved_reply.inbound_evidence(),
                    Some(state.saved_reply.exact_reply_envelope()),
                )),
                Some(ExpectedEvidence {
                    sender_roles: JOINER_ONLY,
                    sender_sequence: 3,
                    predecessor_message_id: state
                        .saved_reply
                        .exact_reply_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Challenged(state),
            ) => (
                None,
                Some(ExpectedEvidence {
                    sender_roles: JOINER_ONLY,
                    sender_sequence: 3,
                    predecessor_message_id: state.last_sponsor_message_id,
                }),
            ),
            SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Applied(state),
            ) => (
                Some((
                    state.saved_reply.inbound_evidence(),
                    Some(state.saved_reply.exact_reply_envelope()),
                )),
                Some(ExpectedEvidence {
                    sender_roles: JOINER_ONLY,
                    sender_sequence: 3,
                    predecessor_message_id: state
                        .saved_reply
                        .exact_reply_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::PendingSettlement(state),
            )) => (
                Some((
                    &state.completion_evidence,
                    Some(state.pending_exchange.request_envelope()),
                )),
                Some(ExpectedEvidence {
                    sender_roles: SPONSOR_OR_HELPER,
                    sender_sequence: 3,
                    predecessor_message_id: state
                        .pending_exchange
                        .request_envelope()
                        .header()
                        .message_id(),
                }),
            ),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::Settled(state),
            )) => (Some((&state.last_received, None)), None),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(state)) => (
                Some((
                    state.saved_reply.inbound_evidence(),
                    Some(state.saved_reply.exact_reply_envelope()),
                )),
                None,
            ),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::LocalJoiner(_),
            )) => (None, None),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::Joiner(state),
            )) => (Some((&state.last_received, None)), None),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::Sponsor(state),
            )) => (
                Some((
                    state.saved_reply.inbound_evidence(),
                    Some(state.saved_reply.exact_reply_envelope()),
                )),
                None,
            ),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(
                SpaceAdmissionSupersededState::Candidate(state),
            )) => (Some((&state.last_received, None)), None),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(
                SpaceAdmissionSupersededState::Initiated { .. }
                | SpaceAdmissionSupersededState::Authenticated { .. },
            )) => (None, None),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
                _,
            )) => (None, None),
        }
    }
}
