use super::*;

impl PersistedJoinerContextV1 {
    fn from_parts(
        join_id: JoinId,
        local_join_ordinal: u64,
        source_snapshot: &AdmissionSourceSnapshot,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: &AdmissionContinuationCredential,
    ) -> Self {
        Self {
            join_id: *join_id.as_bytes(),
            local_join_ordinal,
            source_snapshot: source_snapshot.as_bytes().to_vec(),
            peer_binding: PersistedPeerBindingV1::from(peer_binding),
            continuation_credential: continuation_credential.as_bytes().to_vec(),
        }
    }

    pub(super) fn into_domain(
        self,
    ) -> Result<
        (
            JoinId,
            u64,
            AdmissionSourceSnapshot,
            AdmissionPeerBinding,
            AdmissionContinuationCredential,
        ),
        SpaceAdmissionPersistenceError,
    > {
        Ok((
            JoinId::from_bytes(self.join_id).ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            self.local_join_ordinal,
            AdmissionSourceSnapshot::from_bytes(self.source_snapshot)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            self.peer_binding.into_domain()?,
            decode_continuation_credential(self.continuation_credential)?,
        ))
    }
}

impl TryFrom<&SpaceAdmissionJoinerCommitted> for PersistedJoinerCommittedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionJoinerCommitted) -> Result<Self, Self::Error> {
        Ok(Self {
            context: PersistedJoinerContextV1::from_parts(
                state.join_id,
                state.local_join_ordinal,
                &state.source_snapshot,
                state.peer_binding,
                &state.continuation_credential,
            ),
            exact_commit: PersistedEnvelopeV1::try_from(&state.exact_commit)?,
            commit_evidence: PersistedMessageEvidenceV1::from(&state.commit_evidence),
            staged_target: state.staged_target.as_bytes().to_vec(),
        })
    }
}

impl PersistedJoinerCommittedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionJoinerCommitted, SpaceAdmissionPersistenceError> {
        let (join_id, local_join_ordinal, source_snapshot, peer_binding, continuation_credential) =
            self.context.into_domain()?;
        let exact_commit = self.exact_commit.into_domain()?;
        let commit_evidence = self.commit_evidence.into_domain()?;
        validate_state_envelope(
            &exact_commit,
            admission_id,
            SpaceAdmissionMessageKind::Commit,
        )?;
        validate_envelope_evidence(&exact_commit, &commit_evidence)?;
        Ok(SpaceAdmissionJoinerCommitted {
            join_id,
            local_join_ordinal,
            source_snapshot,
            peer_binding,
            continuation_credential,
            exact_commit,
            commit_evidence,
            staged_target: decode_staged_target(self.staged_target)?,
        })
    }
}

impl TryFrom<&SpaceAdmissionJoinerApplied> for PersistedJoinerAppliedV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionJoinerApplied) -> Result<Self, Self::Error> {
        Ok(Self {
            context: PersistedJoinerContextV1::from_parts(
                state.join_id,
                state.local_join_ordinal,
                &state.source_snapshot,
                state.peer_binding,
                &state.continuation_credential,
            ),
            exact_commit: PersistedEnvelopeV1::try_from(&state.exact_commit)?,
            commit_evidence: PersistedMessageEvidenceV1::from(&state.commit_evidence),
            staged_target: state.staged_target.as_bytes().to_vec(),
            pending_exchange: PersistedAnyPendingExchangeV1::try_from(&state.pending_exchange)?,
        })
    }
}

impl PersistedJoinerAppliedV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionJoinerApplied, SpaceAdmissionPersistenceError> {
        let (join_id, local_join_ordinal, source_snapshot, peer_binding, continuation_credential) =
            self.context.into_domain()?;
        let exact_commit = self.exact_commit.into_domain()?;
        let commit_evidence = self.commit_evidence.into_domain()?;
        validate_state_envelope(
            &exact_commit,
            admission_id,
            SpaceAdmissionMessageKind::Commit,
        )?;
        validate_envelope_evidence(&exact_commit, &commit_evidence)?;
        let pending_exchange = self.pending_exchange.into_domain(admission_id)?;
        if pending_exchange.request_envelope().kind() != SpaceAdmissionMessageKind::Applied
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Complete
            || pending_exchange.exact_reply_for(&commit_evidence).is_none()
        {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionJoinerApplied {
            join_id,
            local_join_ordinal,
            source_snapshot,
            peer_binding,
            continuation_credential,
            exact_commit,
            commit_evidence,
            staged_target: decode_staged_target(self.staged_target)?,
            pending_exchange,
        })
    }
}

impl TryFrom<&SpaceAdmissionJoinerActivating> for PersistedJoinerActivatingV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionJoinerActivating) -> Result<Self, Self::Error> {
        Ok(Self {
            context: PersistedJoinerContextV1::from_parts(
                state.join_id,
                state.local_join_ordinal,
                &state.source_snapshot,
                state.peer_binding,
                &state.continuation_credential,
            ),
            exact_commit: PersistedEnvelopeV1::try_from(&state.exact_commit)?,
            staged_target: state.staged_target.as_bytes().to_vec(),
            completion: PersistedEnvelopeV1::try_from(&state.completion)?,
            completion_evidence: PersistedMessageEvidenceV1::from(&state.completion_evidence),
            space_transition: state.space_transition.as_bytes().to_vec(),
        })
    }
}

impl PersistedJoinerActivatingV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionJoinerActivating, SpaceAdmissionPersistenceError> {
        let (join_id, local_join_ordinal, source_snapshot, peer_binding, continuation_credential) =
            self.context.into_domain()?;
        let exact_commit = self.exact_commit.into_domain()?;
        let completion = self.completion.into_domain()?;
        let completion_evidence = self.completion_evidence.into_domain()?;
        validate_state_envelope(
            &exact_commit,
            admission_id,
            SpaceAdmissionMessageKind::Commit,
        )?;
        validate_state_envelope(
            &completion,
            admission_id,
            SpaceAdmissionMessageKind::Complete,
        )?;
        validate_envelope_evidence(&completion, &completion_evidence)?;
        Ok(SpaceAdmissionJoinerActivating {
            join_id,
            local_join_ordinal,
            source_snapshot,
            peer_binding,
            continuation_credential,
            exact_commit,
            staged_target: decode_staged_target(self.staged_target)?,
            completion,
            completion_evidence,
            space_transition: AdmissionSpaceTransition::from_bytes(self.space_transition)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
        })
    }
}

impl TryFrom<&SpaceAdmissionJoinerCancelling> for PersistedJoinerCancellingV1 {
    type Error = SpaceAdmissionPersistenceError;

    fn try_from(state: &SpaceAdmissionJoinerCancelling) -> Result<Self, Self::Error> {
        Ok(Self {
            join_id: *state.join_id.as_bytes(),
            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
            continuation_credential: state.continuation_credential.as_bytes().to_vec(),
            last_received: PersistedMessageEvidenceV1::from(&state.last_received),
            pending_exchange: PersistedAnyPendingExchangeV1::try_from(&state.pending_exchange)?,
        })
    }
}

impl PersistedJoinerCancellingV1 {
    pub(super) fn into_domain(
        self,
        admission_id: SpaceAdmissionId,
    ) -> Result<SpaceAdmissionJoinerCancelling, SpaceAdmissionPersistenceError> {
        let last_received = self.last_received.into_domain()?;
        let pending_exchange = self.pending_exchange.into_domain(admission_id)?;
        if pending_exchange.request_envelope().kind() != SpaceAdmissionMessageKind::CancelRequested
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Rejected
            || pending_exchange.exact_reply_for(&last_received).is_none()
        {
            return Err(SpaceAdmissionPersistenceError::InvalidState);
        }
        Ok(SpaceAdmissionJoinerCancelling {
            join_id: decode_join_id(self.join_id)?,
            peer_binding: self.peer_binding.into_domain()?,
            continuation_credential: decode_continuation_credential(self.continuation_credential)?,
            last_received,
            pending_exchange,
        })
    }
}
