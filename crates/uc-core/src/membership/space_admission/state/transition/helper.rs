use super::*;

impl SpaceAdmissionAggregate {
    #[cfg(test)]
    pub(crate) fn challenge_completion_helper(
        admission_id: SpaceAdmissionId,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: AdmissionContinuationCredential,
        challenge_counter: u64,
        nonce: AdmissionHelperNonce,
        last_joiner_message_id: AdmissionMessageId,
        last_sponsor_message_id: AdmissionMessageId,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        if challenge_counter == 0 || last_joiner_message_id == last_sponsor_message_id {
            return Err(SpaceAdmissionAggregateError::InvalidHelperChallenge);
        }
        let replacement = Self {
            format_version: SPACE_ADMISSION_RECORD_FORMAT_V1,
            record_version: 0,
            admission_id,
            state: SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Challenged(
                    SpaceAdmissionCompletionHelperChallenged {
                        peer_binding,
                        continuation_credential,
                        challenge_counter,
                        nonce,
                        last_joiner_message_id,
                        last_sponsor_message_id,
                    },
                ),
            ),
        };
        Ok(AdmissionTransition::new(replacement, &[]))
    }

    #[cfg(test)]
    pub(crate) fn complete_as_helper(
        mut self,
        inbound_evidence: AdmissionMessageEvidence,
        verified_commit: SpaceAdmissionEnvelopeV1,
        activation_receipt: AdmissionActivationReceipt,
        helper_security: AdmissionHelperSecurityState,
        complete_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::CompletionHelper(
            SpaceAdmissionCompletionHelperState::Challenged(state),
        ) = self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        if inbound_evidence.sender_role() != AdmissionRole::Joiner
            || inbound_evidence.sender_sequence() != 3
            || inbound_evidence.predecessor_message_id() != Some(state.last_sponsor_message_id)
            || verified_commit.header().admission_id() != self.admission_id
            || verified_commit.kind() != SpaceAdmissionMessageKind::Commit
        {
            return Err(SpaceAdmissionAggregateError::InvalidHelperCompletion);
        }
        let SpaceAdmissionBodyV1::Commit(commit) = verified_commit.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidHelperCompletion);
        };
        let candidate = commit.exact_candidate();
        if activation_receipt.attempt_id != *self.admission_id.as_bytes()
            || activation_receipt.event_id != candidate.candidate_event().event_id()
            || activation_receipt.installed_security_commitment_id
                != candidate.security_commitment().security_commitment_id
            || complete_reply.header().admission_id() != self.admission_id
            || complete_reply.kind() != SpaceAdmissionMessageKind::Complete
            || complete_reply.header().sender_role() != AdmissionRole::CompletionHelper
            || complete_reply.header().sender_sequence() != 0
            || complete_reply.header().predecessor_message_id()
                != Some(inbound_evidence.message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidHelperCompletion);
        }
        let SpaceAdmissionBodyV1::Complete(complete) = complete_reply.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidHelperCompletion);
        };
        let completion = complete.completion();
        if completion.attempt_id != activation_receipt.attempt_id
            || completion.event_id != activation_receipt.event_id
            || completion.security_commitment_id
                != activation_receipt.installed_security_commitment_id
        {
            return Err(SpaceAdmissionAggregateError::InvalidHelperCompletion);
        }
        let saved_reply =
            SavedAdmissionReply::new(self.admission_id, inbound_evidence, complete_reply)
                .map_err(|_| SpaceAdmissionAggregateError::InvalidHelperCompletion)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::CompletionHelper(
            SpaceAdmissionCompletionHelperState::Applied(SpaceAdmissionCompletionHelperApplied {
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                verified_commit,
                activation_receipt,
                helper_security,
                saved_reply,
            }),
        );
        Ok(AdmissionTransition::new(self, &[]))
    }

    #[cfg(test)]
    pub(crate) fn advance_helper_challenge(
        mut self,
        nonce: AdmissionHelperNonce,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::CompletionHelper(
            SpaceAdmissionCompletionHelperState::Challenged(state),
        ) = self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let challenge_counter = state
            .challenge_counter
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::CounterOverflow)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::CompletionHelper(
            SpaceAdmissionCompletionHelperState::Challenged(
                SpaceAdmissionCompletionHelperChallenged {
                    peer_binding: state.peer_binding,
                    continuation_credential: state.continuation_credential,
                    challenge_counter,
                    nonce,
                    last_joiner_message_id: state.last_joiner_message_id,
                    last_sponsor_message_id: state.last_sponsor_message_id,
                },
            ),
        );
        Ok(AdmissionTransition::new(self, &[]))
    }
}
