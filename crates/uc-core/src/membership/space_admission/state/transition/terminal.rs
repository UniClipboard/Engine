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
        let (join_id, local_join_ordinal) = match self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
                state,
            )) => (state.join_id, state.local_join_ordinal),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvedInvitation(
                state,
            )) => (state.join_id, state.local_join_ordinal),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state))
                if matches!(
                    state.channel_state,
                    SpaceAdmissionJoinerChannelState::AwaitingAuthentication { .. }
                ) =>
            {
                (state.join_id, state.local_join_ordinal)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
                (state.join_id, state.local_join_ordinal)
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
