use super::*;

impl SpaceAdmissionAggregate {
    pub(crate) fn fix_candidate(
        mut self,
        candidate_reply: SpaceAdmissionEnvelopeV1,
        staged_security: AdmissionStagedSecurityState,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        if candidate_reply.kind() != SpaceAdmissionMessageKind::Candidate {
            return Err(SpaceAdmissionAggregateError::InvalidCandidateReply);
        }
        let saved_reply = SavedAdmissionReply::new(
            self.admission_id,
            state.join_request_evidence,
            candidate_reply,
        )
        .map_err(|_| SpaceAdmissionAggregateError::InvalidCandidateReply)?;
        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(
            SpaceAdmissionSponsorCandidate {
                invitation_claim: state.invitation_claim,
                base_snapshot: state.base_snapshot,
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                staged_security,
                saved_reply,
            },
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn commit_prepared(
        mut self,
        prepared: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        committed_history: AdmissionSignedMembershipHistory,
        sealed_security: AdmissionSealedSecurityState,
        commit_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let candidate_reply = state.saved_reply.exact_reply_envelope();
        if prepared.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if prepared.kind() != SpaceAdmissionMessageKind::Prepared
            || prepared.header().sender_sequence() != 1
            || prepared.header().predecessor_message_id()
                != Some(candidate_reply.header().message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidPreparedMessage);
        }
        let prepared_evidence = prepared
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidPreparedMessage)?;
        if commit_reply.header().admission_id() != self.admission_id
            || commit_reply.kind() != SpaceAdmissionMessageKind::Commit
            || commit_reply.header().sender_sequence() != 1
            || commit_reply.header().predecessor_message_id()
                != Some(prepared_evidence.message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidCommitReply);
        }
        let SpaceAdmissionBodyV1::Candidate(fixed_candidate) = candidate_reply.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidCommitReply);
        };
        let SpaceAdmissionBodyV1::Commit(commit) = commit_reply.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidCommitReply);
        };
        if commit.exact_candidate() != fixed_candidate
            || commit.target_membership_history() != &committed_history
        {
            return Err(SpaceAdmissionAggregateError::InvalidCommitReply);
        }
        let saved_reply =
            SavedAdmissionReply::new(self.admission_id, prepared_evidence, commit_reply)
                .map_err(|_| SpaceAdmissionAggregateError::InvalidCommitReply)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(
            SpaceAdmissionSponsorCommitted {
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                committed_history,
                sealed_security,
                saved_reply,
            },
        ));
        Ok(AdmissionTransition::new(
            self,
            &[AdmissionEffect::CommitMembership],
        ))
    }

    pub(crate) fn complete_applied(
        mut self,
        applied: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        activated_security: AdmissionActivatedSecurityState,
        complete_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let commit_reply = state.saved_reply.exact_reply_envelope();
        if applied.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if applied.kind() != SpaceAdmissionMessageKind::Applied
            || applied.header().sender_sequence() != 2
            || applied.header().predecessor_message_id() != Some(commit_reply.header().message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidAppliedMessage);
        }
        let applied_evidence = applied
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidAppliedMessage)?;
        if complete_reply.header().admission_id() != self.admission_id
            || complete_reply.kind() != SpaceAdmissionMessageKind::Complete
            || complete_reply.header().sender_sequence() != 2
            || complete_reply.header().predecessor_message_id()
                != Some(applied_evidence.message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteReply);
        }
        let SpaceAdmissionBodyV1::Commit(commit) = commit_reply.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidAppliedMessage);
        };
        let SpaceAdmissionBodyV1::Applied(applied_body) = applied.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidAppliedMessage);
        };
        let SpaceAdmissionBodyV1::Complete(complete) = complete_reply.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteReply);
        };
        let receipt = applied_body.activation_receipt();
        let completion = complete.completion();
        let candidate = commit.exact_candidate();
        if receipt.attempt_id != *self.admission_id.as_bytes()
            || receipt.event_id != candidate.candidate_event().event_id()
            || receipt.installed_security_commitment_id
                != candidate.security_commitment().security_commitment_id
            || completion.attempt_id != receipt.attempt_id
            || completion.event_id != receipt.event_id
            || completion.security_commitment_id != receipt.installed_security_commitment_id
        {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteReply);
        }
        let saved_reply =
            SavedAdmissionReply::new(self.admission_id, applied_evidence, complete_reply)
                .map_err(|_| SpaceAdmissionAggregateError::InvalidCompleteReply)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(
            SpaceAdmissionSponsorApplied {
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                committed_history: state.committed_history,
                activation_receipt: receipt.clone(),
                activated_security,
                saved_reply,
            },
        ));
        Ok(AdmissionTransition::new(
            self,
            &[
                AdmissionEffect::ActivateSecurity,
                AdmissionEffect::PublishMembership,
            ],
        ))
    }

    pub(crate) fn settle_complete_ack(
        mut self,
        complete_ack: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        settled_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let (
            peer_binding,
            continuation_credential,
            complete_message_id,
            settled_sender_role,
            settled_sender_sequence,
        ) = match self.state {
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) => (
                state.peer_binding,
                state.continuation_credential,
                state
                    .saved_reply
                    .exact_reply_envelope()
                    .header()
                    .message_id(),
                AdmissionRole::Sponsor,
                3,
            ),
            SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Applied(state),
            ) => (
                state.peer_binding,
                state.continuation_credential,
                state
                    .saved_reply
                    .exact_reply_envelope()
                    .header()
                    .message_id(),
                AdmissionRole::CompletionHelper,
                1,
            ),
            _ => return Err(SpaceAdmissionAggregateError::InvalidTransition),
        };
        if complete_ack.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if complete_ack.kind() != SpaceAdmissionMessageKind::CompleteAck
            || complete_ack.header().sender_sequence() != 3
            || complete_ack.header().predecessor_message_id() != Some(complete_message_id)
        {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteAckMessage);
        }
        let ack_evidence = complete_ack
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidCompleteAckMessage)?;
        if settled_reply.header().admission_id() != self.admission_id
            || settled_reply.kind() != SpaceAdmissionMessageKind::Settled
            || settled_reply.header().sender_role() != settled_sender_role
            || settled_reply.header().sender_sequence() != settled_sender_sequence
            || settled_reply.header().predecessor_message_id() != Some(ack_evidence.message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidSettledReply);
        }
        let saved_reply = SavedAdmissionReply::new(self.admission_id, ack_evidence, settled_reply)
            .map_err(|_| SpaceAdmissionAggregateError::InvalidSettledReply)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(
            SpaceAdmissionCompletedTerminal {
                peer_binding,
                continuation_credential,
                saved_reply,
            },
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn reject_cancel(
        mut self,
        cancel_request: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        rejected_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::UnsafeCancellation);
        };
        let candidate_reply = state.saved_reply.exact_reply_envelope();
        if cancel_request.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if cancel_request.kind() != SpaceAdmissionMessageKind::CancelRequested
            || cancel_request.header().sender_sequence() != 1
            || cancel_request.header().predecessor_message_id()
                != Some(candidate_reply.header().message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidCancellationRequest);
        }
        let cancel_evidence = cancel_request
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidCancellationRequest)?;
        if rejected_reply.header().admission_id() != self.admission_id
            || rejected_reply.kind() != SpaceAdmissionMessageKind::Rejected
            || rejected_reply.header().sender_sequence() != 1
            || rejected_reply.header().predecessor_message_id()
                != Some(cancel_evidence.message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidRejectedReply);
        }
        let SpaceAdmissionBodyV1::Rejected { reason } = rejected_reply.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidRejectedReply);
        };
        if *reason != SpaceAdmissionRejectionReason::Cancelled {
            return Err(SpaceAdmissionAggregateError::InvalidRejectedReply);
        }
        let reason = *reason;
        let saved_reply =
            SavedAdmissionReply::new(self.admission_id, cancel_evidence, rejected_reply)
                .map_err(|_| SpaceAdmissionAggregateError::InvalidRejectedReply)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::Sponsor(SpaceAdmissionSponsorRejected {
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                reason,
                saved_reply,
            }),
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }
}
