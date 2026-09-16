use super::*;

impl TryFrom<&SpaceAdmissionJoinerInitiated> for PersistedJoinerInitiatedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionJoinerInitiated) -> Result<Self, Self::Error> {
        let channel_state = match &state.channel_state {
            SpaceAdmissionJoinerChannelState::AwaitingAuthentication {
                encrypted_password_equivalent,
            } => PersistedJoinerChannelStateV1::AwaitingAuthentication {
                encrypted_password_equivalent: encrypted_password_equivalent.as_bytes().to_vec(),
            },
            SpaceAdmissionJoinerChannelState::Authenticated {
                peer_binding,
                continuation_credential,
            } => PersistedJoinerChannelStateV1::Authenticated {
                local_peer_id: *peer_binding.local_peer_id().as_bytes(),
                remote_peer_id: *peer_binding.remote_peer_id().as_bytes(),
                continuation_credential: continuation_credential.as_bytes().to_vec(),
            },
        };
        Ok(Self {
            join_id: *state.join_id.as_bytes(),
            local_join_ordinal: state.local_join_ordinal,
            source_snapshot: state.source_snapshot.as_bytes().to_vec(),
            private_state: state.private_state.as_bytes().to_vec(),
            channel_state,
            pending_exchange: PersistedPendingExchangeV1::try_from(&state.pending_exchange)?,
        })
    }
}

impl PersistedJoinerInitiatedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionJoinerInitiated, SpaceAdmissionPersistenceError> {
        let channel_state = match self.channel_state {
            PersistedJoinerChannelStateV1::AwaitingAuthentication {
                encrypted_password_equivalent,
            } => SpaceAdmissionJoinerChannelState::AwaitingAuthentication {
                encrypted_password_equivalent: AdmissionEncryptedPasswordEquivalent::from_bytes(
                    encrypted_password_equivalent,
                )
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            },
            PersistedJoinerChannelStateV1::Authenticated {
                local_peer_id,
                remote_peer_id,
                continuation_credential,
            } => SpaceAdmissionJoinerChannelState::Authenticated {
                peer_binding: AdmissionPeerBinding::new(
                    AdmissionChannelPeerId::from_bytes(local_peer_id)
                        .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
                    AdmissionChannelPeerId::from_bytes(remote_peer_id)
                        .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
                )
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
                continuation_credential: AdmissionContinuationCredential::from_bytes(
                    continuation_credential,
                )
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            },
        };
        Ok(SpaceAdmissionJoinerInitiated {
            join_id: JoinId::from_bytes(self.join_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            local_join_ordinal: self.local_join_ordinal,
            source_snapshot: AdmissionSourceSnapshot::from_bytes(self.source_snapshot)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            private_state: AdmissionJoinerPrivateState::from_bytes(self.private_state)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            channel_state,
            pending_exchange: self.pending_exchange.into_domain(admission_id)?,
        })
    }
}

impl TryFrom<&SpaceAdmissionJoinerCandidate> for PersistedJoinerCandidateV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionJoinerCandidate) -> Result<Self, Self::Error> {
        Ok(Self {
            join_id: *state.join_id.as_bytes(),
            local_join_ordinal: state.local_join_ordinal,
            source_snapshot: state.source_snapshot.as_bytes().to_vec(),
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            candidate: PersistedCandidateEnvelopeV1::try_from(&state.candidate)?,
            candidate_evidence: PersistedMessageEvidenceV1::from(&state.candidate_evidence),
            staged_target_input: state.staged_target_input.as_bytes().to_vec(),
        })
    }
}

impl PersistedJoinerCandidateV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionJoinerCandidate, SpaceAdmissionPersistenceError> {
        let candidate = self.candidate.into_domain()?;
        let candidate_evidence = self.candidate_evidence.into_domain()?;
        validate_envelope_evidence(&candidate, &candidate_evidence)?;
        if candidate.header().admission_id() != admission_id
            || candidate.kind() != SpaceAdmissionMessageKind::Candidate
            || candidate.header().sender_sequence() != 0
        {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionJoinerCandidate {
            join_id: JoinId::from_bytes(self.join_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            local_join_ordinal: self.local_join_ordinal,
            source_snapshot: AdmissionSourceSnapshot::from_bytes(self.source_snapshot)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: AdmissionContinuationCredential::from_bytes(
                self.continuation_credential,
            )
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            candidate,
            candidate_evidence,
            staged_target_input: AdmissionStagedTargetInput::from_bytes(self.staged_target_input)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
        })
    }
}

impl TryFrom<&SpaceAdmissionJoinerPrepared> for PersistedJoinerPreparedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionJoinerPrepared) -> Result<Self, Self::Error> {
        Ok(Self {
            join_id: *state.join_id.as_bytes(),
            local_join_ordinal: state.local_join_ordinal,
            source_snapshot: state.source_snapshot.as_bytes().to_vec(),
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            candidate_evidence: PersistedMessageEvidenceV1::from(&state.candidate_evidence),
            verified_history: state.verified_history.as_bytes().to_vec(),
            staged_target: state.staged_target.as_bytes().to_vec(),
            pending_exchange: PersistedPreparedPendingExchangeV1::try_from(
                &state.pending_exchange,
            )?,
        })
    }
}

impl PersistedJoinerPreparedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionJoinerPrepared, SpaceAdmissionPersistenceError> {
        let candidate_evidence = self.candidate_evidence.into_domain()?;
        let pending_exchange = self
            .pending_exchange
            .into_domain(admission_id, &candidate_evidence)?;
        Ok(SpaceAdmissionJoinerPrepared {
            join_id: JoinId::from_bytes(self.join_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            local_join_ordinal: self.local_join_ordinal,
            source_snapshot: AdmissionSourceSnapshot::from_bytes(self.source_snapshot)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: AdmissionContinuationCredential::from_bytes(
                self.continuation_credential,
            )
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            candidate_evidence,
            verified_history: AdmissionSignedMembershipHistory::from_bytes(self.verified_history)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            staged_target: AdmissionStagedTarget::from_bytes(self.staged_target)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            pending_exchange,
        })
    }
}

impl TryFrom<&SpaceAdmissionSponsorAccepted> for PersistedSponsorAcceptedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionSponsorAccepted) -> Result<Self, Self::Error> {
        Ok(Self {
            invitation_claim: state.invitation_claim.as_bytes().to_vec(),
            join_request: PersistedJoinRequestEnvelopeV1::try_from(&state.join_request)?,
            join_request_evidence: PersistedMessageEvidenceV1::from(&state.join_request_evidence),
            base_snapshot: state.base_snapshot.as_bytes().to_vec(),
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
        })
    }
}

impl PersistedSponsorAcceptedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionSponsorAccepted, SpaceAdmissionPersistenceError> {
        let join_request = self.join_request.into_domain()?;
        let join_request_evidence = self.join_request_evidence.into_domain()?;
        validate_envelope_evidence(&join_request, &join_request_evidence)?;
        if join_request.header().admission_id() != admission_id
            || join_request.kind() != SpaceAdmissionMessageKind::JoinRequest
        {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionSponsorAccepted {
            invitation_claim: AdmissionInvitationClaim::from_bytes(self.invitation_claim)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            join_request,
            join_request_evidence,
            base_snapshot: AdmissionBaseSnapshot::from_bytes(self.base_snapshot)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: AdmissionContinuationCredential::from_bytes(
                self.continuation_credential,
            )
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
        })
    }
}

impl TryFrom<&SpaceAdmissionSponsorCandidate> for PersistedSponsorCandidateV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionSponsorCandidate) -> Result<Self, Self::Error> {
        Ok(Self {
            invitation_claim: state.invitation_claim.as_bytes().to_vec(),
            base_snapshot: state.base_snapshot.as_bytes().to_vec(),
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            staged_security: state.staged_security.as_bytes().to_vec(),
            saved_reply: PersistedSavedCandidateReplyV1::try_from(&state.saved_reply)?,
        })
    }
}

impl PersistedSponsorCandidateV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionSponsorCandidate, SpaceAdmissionPersistenceError> {
        Ok(SpaceAdmissionSponsorCandidate {
            invitation_claim: AdmissionInvitationClaim::from_bytes(self.invitation_claim)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            base_snapshot: AdmissionBaseSnapshot::from_bytes(self.base_snapshot)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: AdmissionContinuationCredential::from_bytes(
                self.continuation_credential,
            )
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            staged_security: AdmissionStagedSecurityState::from_bytes(self.staged_security)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            saved_reply: self.saved_reply.into_domain(admission_id)?,
        })
    }
}
