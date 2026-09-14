use super::*;

impl SpaceAdmissionAggregate {
    pub(crate) fn terminate_locally(
        mut self,
        reason: SpaceAdmissionTerminationReason,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let has_authenticated_attempt = self.attempt_digest.is_some();
        let (join_id, local_join_ordinal, cleanup) = match self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
                state,
            )) => (state.join_id, state.local_join_ordinal, None),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvedInvitation(
                state,
            )) => (state.join_id, state.local_join_ordinal, None),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => {
                match state.channel_state {
                    SpaceAdmissionJoinerChannelState::AwaitingAuthentication { .. } => {
                        (state.join_id, state.local_join_ordinal, None)
                    }
                    SpaceAdmissionJoinerChannelState::Authenticated { .. }
                        if self.attempt_timeline.is_some() =>
                    {
                        (state.join_id, state.local_join_ordinal, None)
                    }
                    SpaceAdmissionJoinerChannelState::Authenticated { .. } => {
                        return Err(SpaceAdmissionAggregateError::UnsafeCancellation);
                    }
                }
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
                (state.join_id, state.local_join_ordinal, None)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state))
                if has_authenticated_attempt =>
            {
                (
                    state.join_id,
                    state.local_join_ordinal,
                    Some(AdmissionCleanupObligation {
                        commit_knowledge: AdmissionCommitKnowledge::Unknown,
                        member_binding: None,
                        peer_binding: state.peer_binding,
                        continuation_credential: state.continuation_credential,
                    }),
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(state))
                if has_authenticated_attempt =>
            {
                (
                    state.join_id,
                    state.local_join_ordinal,
                    Some(known_cleanup_obligation(
                        self.attempt_digest,
                        &state.exact_commit,
                        state.peer_binding,
                        state.continuation_credential,
                    )?),
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state))
                if has_authenticated_attempt =>
            {
                (
                    state.join_id,
                    state.local_join_ordinal,
                    Some(known_cleanup_obligation(
                        self.attempt_digest,
                        &state.exact_commit,
                        state.peer_binding,
                        state.continuation_credential,
                    )?),
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(state))
                if has_authenticated_attempt =>
            {
                (
                    state.join_id,
                    state.local_join_ordinal,
                    Some(known_cleanup_obligation(
                        self.attempt_digest,
                        &state.exact_commit,
                        state.peer_binding,
                        state.continuation_credential,
                    )?),
                )
            }
            _ => return Err(SpaceAdmissionAggregateError::UnsafeCancellation),
        };
        self.record_version = record_version;
        self.state = if self.attempt_timeline.is_none()
            && matches!(reason, SpaceAdmissionTerminationReason::Cancelled)
        {
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::LocalJoiner(SpaceAdmissionLocalJoinerRejected {
                    join_id,
                    reason: SpaceAdmissionRejectionReason::Cancelled,
                }),
            ))
        } else {
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Terminated(
                SpaceAdmissionLocalJoinerTerminated {
                    join_id,
                    local_join_ordinal,
                    reason,
                    cleanup,
                },
            ))
        };
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn require_recovery(
        mut self,
        category: AdmissionRecoveryCategory,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        if matches!(self.state, SpaceAdmissionRecordState::Terminal(_)) {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        self.record_version = record_version;
        self.state =
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
                SpaceAdmissionRecoveryRequiredTerminal { category },
            ));
        Ok(AdmissionTransition::new(self, &[]))
    }
}

fn known_cleanup_obligation(
    attempt_digest: Option<[u8; 32]>,
    exact_commit: &SpaceAdmissionEnvelopeV1,
    peer_binding: AdmissionPeerBinding,
    continuation_credential: AdmissionContinuationCredential,
) -> Result<AdmissionCleanupObligation, SpaceAdmissionAggregateError> {
    let SpaceAdmissionBodyV1::Commit(commit) = exact_commit.body() else {
        return Err(SpaceAdmissionAggregateError::InvalidCommitReply);
    };
    let event = commit.exact_candidate().candidate_event();
    let MembershipOperationV2::AddDevice { admission } = &event.operation else {
        return Err(SpaceAdmissionAggregateError::InvalidCommitReply);
    };
    let member_binding = AdmissionMemberBindingV2::new(
        attempt_digest.ok_or(SpaceAdmissionAggregateError::InvalidAttemptTimeline)?,
        SpaceId::from_str(&event.lineage_id),
        admission.facts.member_instance,
        event.event_id(),
    )
    .map_err(|_| SpaceAdmissionAggregateError::InvalidCommitReply)?;
    Ok(AdmissionCleanupObligation {
        commit_knowledge: AdmissionCommitKnowledge::Known,
        member_binding: Some(member_binding),
        peer_binding,
        continuation_credential,
    })
}
