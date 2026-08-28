use super::super::message::SpaceAdmissionEnvelopeV1;
use super::*;

pub enum AdmissionPendingRecovery<'a> {
    Initial {
        encrypted_password_equivalent: &'a AdmissionEncryptedPasswordEquivalent,
        pending_exchange: &'a PendingAdmissionExchange,
    },
    Continuation {
        peer_binding: AdmissionPeerBinding,
        continuation_credential: &'a AdmissionContinuationCredential,
        pending_exchange: &'a PendingAdmissionExchange,
    },
}

pub struct SponsorCandidatePreparation<'a> {
    admission_id: SpaceAdmissionId,
    join_request: &'a SpaceAdmissionEnvelopeV1,
    base_snapshot: &'a AdmissionBaseSnapshot,
    peer_binding: AdmissionPeerBinding,
}

pub struct SponsorCommitPreparation<'a> {
    candidate_reply: &'a SpaceAdmissionEnvelopeV1,
    base_snapshot: &'a AdmissionBaseSnapshot,
    staged_security: &'a AdmissionStagedSecurityState,
}

pub struct JoinerAppliedPreparation<'a> {
    exact_commit: &'a SpaceAdmissionEnvelopeV1,
    staged_target: &'a AdmissionStagedTarget,
}

impl JoinerAppliedPreparation<'_> {
    pub const fn exact_commit(&self) -> &SpaceAdmissionEnvelopeV1 {
        self.exact_commit
    }

    pub const fn staged_target(&self) -> &AdmissionStagedTarget {
        self.staged_target
    }
}

impl SponsorCommitPreparation<'_> {
    pub const fn candidate_reply(&self) -> &SpaceAdmissionEnvelopeV1 {
        self.candidate_reply
    }

    pub const fn base_snapshot(&self) -> &AdmissionBaseSnapshot {
        self.base_snapshot
    }

    pub const fn staged_security(&self) -> &AdmissionStagedSecurityState {
        self.staged_security
    }
}

impl SponsorCandidatePreparation<'_> {
    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.admission_id
    }

    pub const fn join_request(&self) -> &SpaceAdmissionEnvelopeV1 {
        self.join_request
    }

    pub const fn base_snapshot(&self) -> &AdmissionBaseSnapshot {
        self.base_snapshot
    }

    pub const fn peer_binding(&self) -> AdmissionPeerBinding {
        self.peer_binding
    }
}

impl SpaceAdmissionAggregate {
    pub const fn is_terminal(&self) -> bool {
        matches!(self.state, SpaceAdmissionRecordState::Terminal(_))
    }

    pub fn pending_recovery(&self) -> Option<AdmissionPendingRecovery<'_>> {
        match &self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => {
                match &state.channel_state {
                    SpaceAdmissionJoinerChannelState::AwaitingAuthentication {
                        encrypted_password_equivalent,
                    } => Some(AdmissionPendingRecovery::Initial {
                        encrypted_password_equivalent,
                        pending_exchange: &state.pending_exchange,
                    }),
                    SpaceAdmissionJoinerChannelState::Authenticated {
                        peer_binding,
                        continuation_credential,
                    } => Some(AdmissionPendingRecovery::Continuation {
                        peer_binding: *peer_binding,
                        continuation_credential,
                        pending_exchange: &state.pending_exchange,
                    }),
                }
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => {
                Some(AdmissionPendingRecovery::Continuation {
                    peer_binding: state.peer_binding,
                    continuation_credential: &state.continuation_credential,
                    pending_exchange: &state.pending_exchange,
                })
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) => {
                Some(AdmissionPendingRecovery::Continuation {
                    peer_binding: state.peer_binding,
                    continuation_credential: &state.continuation_credential,
                    pending_exchange: &state.pending_exchange,
                })
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(state)) => {
                Some(AdmissionPendingRecovery::Continuation {
                    peer_binding: state.peer_binding,
                    continuation_credential: &state.continuation_credential,
                    pending_exchange: &state.pending_exchange,
                })
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::PendingSettlement(state),
            )) => Some(AdmissionPendingRecovery::Continuation {
                peer_binding: state.peer_binding,
                continuation_credential: &state.continuation_credential,
                pending_exchange: &state.pending_exchange,
            }),
            _ => None,
        }
    }

    pub fn sponsor_candidate_preparation(&self) -> Option<SponsorCandidatePreparation<'_>> {
        let SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(state)) =
            &self.state
        else {
            return None;
        };
        Some(SponsorCandidatePreparation {
            admission_id: self.admission_id,
            join_request: &state.join_request,
            base_snapshot: &state.base_snapshot,
            peer_binding: state.peer_binding,
        })
    }

    pub fn sponsor_peer_binding(&self) -> Option<AdmissionPeerBinding> {
        match &self.state {
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(state)) => {
                Some(state.peer_binding)
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => {
                Some(state.peer_binding)
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) => {
                Some(state.peer_binding)
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) => {
                Some(state.peer_binding)
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(state)) => {
                Some(state.peer_binding)
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::Sponsor(state),
            )) => Some(state.peer_binding),
            _ => None,
        }
    }

    pub fn sponsor_commit_preparation(&self) -> Option<SponsorCommitPreparation<'_>> {
        let SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) =
            &self.state
        else {
            return None;
        };
        Some(SponsorCommitPreparation {
            candidate_reply: state.saved_reply.exact_reply_envelope(),
            base_snapshot: &state.base_snapshot,
            staged_security: &state.staged_security,
        })
    }

    pub fn joiner_applied_preparation(&self) -> Option<JoinerAppliedPreparation<'_>> {
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(state)) =
            &self.state
        else {
            return None;
        };
        Some(JoinerAppliedPreparation {
            exact_commit: &state.exact_commit,
            staged_target: &state.staged_target,
        })
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
