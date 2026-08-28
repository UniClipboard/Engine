use super::*;

impl From<&SpaceAdmissionCompletionHelperChallenged> for PersistedCompletionHelperChallengedV1 {
    fn from(state: &SpaceAdmissionCompletionHelperChallenged) -> Self {
        Self {
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            challenge_counter: state.challenge_counter,
            nonce: *state.nonce.as_bytes(),
            last_joiner_message_id: *state.last_joiner_message_id.as_bytes(),
            last_sponsor_message_id: *state.last_sponsor_message_id.as_bytes(),
        }
    }
}

impl PersistedCompletionHelperChallengedV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionCompletionHelperChallenged, SpaceAdmissionPersistenceError> {
        Ok(SpaceAdmissionCompletionHelperChallenged {
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: decode_continuation_credential(self.continuation_credential)?,
            challenge_counter: self.challenge_counter,
            nonce: AdmissionHelperNonce::from_bytes(self.nonce)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            last_joiner_message_id: decode_message_id(self.last_joiner_message_id)?,
            last_sponsor_message_id: decode_message_id(self.last_sponsor_message_id)?,
        })
    }
}

impl TryFrom<&SpaceAdmissionCompletionHelperApplied> for PersistedCompletionHelperAppliedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionCompletionHelperApplied) -> Result<Self, Self::Error> {
        Ok(Self {
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            verified_commit: PersistedEnvelopeV1::try_from(&state.verified_commit)?,
            activation_receipt: state.activation_receipt.clone(),
            helper_security: state.helper_security.as_bytes().to_vec(),
            saved_reply: PersistedSavedReplyV1::try_from(&state.saved_reply)?,
        })
    }
}

impl PersistedCompletionHelperAppliedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionCompletionHelperApplied, SpaceAdmissionPersistenceError> {
        let verified_commit = self.verified_commit.into_domain()?;
        validate_state_envelope(
            &verified_commit,
            admission_id,
            SpaceAdmissionMessageKind::Commit,
        )?;
        validate_activation_receipt(&self.activation_receipt, admission_id)?;
        let saved_reply = self.saved_reply.into_domain(admission_id)?;
        if saved_reply.exact_reply_envelope().kind() != SpaceAdmissionMessageKind::Complete {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionCompletionHelperApplied {
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: decode_continuation_credential(self.continuation_credential)?,
            verified_commit,
            activation_receipt: self.activation_receipt,
            helper_security: AdmissionHelperSecurityState::from_bytes(self.helper_security)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            saved_reply,
        })
    }
}

impl TryFrom<&SpaceAdmissionActivePendingSettlement> for PersistedActivePendingSettlementV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionActivePendingSettlement) -> Result<Self, Self::Error> {
        Ok(Self {
            join_id: *state.join_id.as_bytes(),
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            completion_evidence: PersistedMessageEvidenceV1::from(&state.completion_evidence),
            transition_result: state.transition_result.as_bytes().to_vec(),
            pending_exchange: PersistedAnyPendingExchangeV1::try_from(&state.pending_exchange)?,
        })
    }
}

impl PersistedActivePendingSettlementV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionActivePendingSettlement, SpaceAdmissionPersistenceError> {
        let completion_evidence = self.completion_evidence.into_domain()?;
        let pending_exchange = self.pending_exchange.into_domain(admission_id)?;
        if pending_exchange.request_envelope().kind() != SpaceAdmissionMessageKind::CompleteAck
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Settled
            || pending_exchange
                .exact_reply_for(&completion_evidence)
                .is_none()
        {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionActivePendingSettlement {
            join_id: decode_join_id(self.join_id)?,
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: decode_continuation_credential(self.continuation_credential)?,
            completion_evidence,
            transition_result: AdmissionSpaceTransitionResult::from_bytes(self.transition_result)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            pending_exchange,
        })
    }
}

impl From<&SpaceAdmissionActiveSettled> for PersistedActiveSettledV1 {
    fn from(state: &SpaceAdmissionActiveSettled) -> Self {
        Self {
            join_id: *state.join_id.as_bytes(),
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            last_received: PersistedMessageEvidenceV1::from(&state.last_received),
        }
    }
}

impl PersistedActiveSettledV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionActiveSettled, SpaceAdmissionPersistenceError> {
        Ok(SpaceAdmissionActiveSettled {
            join_id: decode_join_id(self.join_id)?,
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: decode_continuation_credential(self.continuation_credential)?,
            last_received: self.last_received.into_domain()?,
        })
    }
}

impl TryFrom<&SpaceAdmissionCompletedTerminal> for PersistedCompletedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionCompletedTerminal) -> Result<Self, Self::Error> {
        Ok(Self {
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            saved_reply: PersistedSavedReplyV1::try_from(&state.saved_reply)?,
        })
    }
}

impl PersistedCompletedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionCompletedTerminal, SpaceAdmissionPersistenceError> {
        let saved_reply = self.saved_reply.into_domain(admission_id)?;
        if saved_reply.exact_reply_envelope().kind() != SpaceAdmissionMessageKind::Settled {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionCompletedTerminal {
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: decode_continuation_credential(self.continuation_credential)?,
            saved_reply,
        })
    }
}

impl From<&SpaceAdmissionSupersededState> for PersistedSupersededV1 {
    fn from(state: &SpaceAdmissionSupersededState) -> Self {
        match state {
            SpaceAdmissionSupersededState::Initiated { join_id } => Self::Initiated {
                join_id: *join_id.as_bytes(),
            },
            SpaceAdmissionSupersededState::Authenticated {
                join_id,
                peer_binding,
                continuation_credential,
            } => Self::Authenticated {
                join_id: *join_id.as_bytes(),
                peer_binding: PersistedPeerBindingV1::from(*peer_binding),
                continuation_credential: continuation_credential.as_bytes().to_vec(),
            },
            SpaceAdmissionSupersededState::Candidate(state) => Self::Candidate {
                join_id: *state.join_id.as_bytes(),
                peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
                continuation_credential: state.continuation_credential.as_bytes().to_vec(),
                last_received: PersistedMessageEvidenceV1::from(&state.last_received),
            },
        }
    }
}

impl PersistedSupersededV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionSupersededState, SpaceAdmissionPersistenceError> {
        match self {
            Self::Initiated { join_id } => Ok(SpaceAdmissionSupersededState::Initiated {
                join_id: decode_join_id(join_id)?,
            }),
            Self::Authenticated {
                join_id,
                peer_binding,
                continuation_credential,
            } => Ok(SpaceAdmissionSupersededState::Authenticated {
                join_id: decode_join_id(join_id)?,
                peer_binding: peer_binding.into_domain()?,
                continuation_credential: decode_continuation_credential(continuation_credential)?,
            }),
            Self::Candidate {
                join_id,
                peer_binding,
                continuation_credential,
                last_received,
            } => Ok(SpaceAdmissionSupersededState::Candidate(
                SpaceAdmissionSupersededTerminal {
                    join_id: decode_join_id(join_id)?,
                    peer_binding: peer_binding.into_domain()?,
                    continuation_credential: decode_continuation_credential(
                        continuation_credential,
                    )?,
                    last_received: last_received.into_domain()?,
                },
            )),
        }
    }
}

impl TryFrom<&SpaceAdmissionRejectedState> for PersistedRejectedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionRejectedState) -> Result<Self, Self::Error> {
        Ok(match state {
            SpaceAdmissionRejectedState::LocalJoiner(state) => Self::LocalJoiner {
                join_id: *state.join_id.as_bytes(),
                reason: encode_rejection_reason(state.reason),
            },
            SpaceAdmissionRejectedState::Joiner(state) => Self::Joiner {
                join_id: *state.join_id.as_bytes(),
                peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
                continuation_credential: state.continuation_credential.as_bytes().to_vec(),
                reason: encode_rejection_reason(state.reason),
                last_received: PersistedMessageEvidenceV1::from(&state.last_received),
            },
            SpaceAdmissionRejectedState::Sponsor(state) => Self::Sponsor {
                peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
                continuation_credential: state.continuation_credential.as_bytes().to_vec(),
                reason: encode_rejection_reason(state.reason),
                saved_reply: PersistedSavedReplyV1::try_from(&state.saved_reply)?,
            },
        })
    }
}

impl PersistedRejectedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionRejectedState, SpaceAdmissionPersistenceError> {
        match self {
            Self::LocalJoiner { join_id, reason } => Ok(SpaceAdmissionRejectedState::LocalJoiner(
                SpaceAdmissionLocalJoinerRejected {
                    join_id: decode_join_id(join_id)?,
                    reason: decode_rejection_reason(reason)?,
                },
            )),
            Self::Joiner {
                join_id,
                peer_binding,
                continuation_credential,
                reason,
                last_received,
            } => Ok(SpaceAdmissionRejectedState::Joiner(
                SpaceAdmissionJoinerRejected {
                    join_id: decode_join_id(join_id)?,
                    peer_binding: peer_binding.into_domain()?,
                    continuation_credential: decode_continuation_credential(
                        continuation_credential,
                    )?,
                    reason: decode_rejection_reason(reason)?,
                    last_received: last_received.into_domain()?,
                },
            )),
            Self::Sponsor {
                peer_binding,
                continuation_credential,
                reason,
                saved_reply,
            } => {
                let saved_reply = saved_reply.into_domain(admission_id)?;
                if saved_reply.exact_reply_envelope().kind() != SpaceAdmissionMessageKind::Rejected
                {
                    return Err(SpaceAdmissionPersistenceError::InvalidState);
                }
                Ok(SpaceAdmissionRejectedState::Sponsor(
                    SpaceAdmissionSponsorRejected {
                        peer_binding: peer_binding.into_domain()?,
                        continuation_credential: decode_continuation_credential(
                            continuation_credential,
                        )?,
                        reason: decode_rejection_reason(reason)?,
                        saved_reply,
                    },
                ))
            }
        }
    }
}
