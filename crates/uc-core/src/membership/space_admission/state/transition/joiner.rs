use super::*;

impl SpaceAdmissionAggregate {
    pub(crate) fn mark_invitation_resolution_started(
        mut self,
    ) -> Result<(AdmissionTransition, AdmissionShortInvitationCode), SpaceAdmissionAggregateError>
    {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
            state,
        )) = self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let SpaceAdmissionInvitationResolutionState::Ready { short_code } = state.resolution else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        self.record_version = record_version;
        self.state =
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
                SpaceAdmissionJoinerResolvingInvitation {
                    join_id: state.join_id,
                    local_join_ordinal: state.local_join_ordinal,
                    source_snapshot: state.source_snapshot,
                    start_context: state.start_context,
                    resolution: SpaceAdmissionInvitationResolutionState::Started,
                },
            ));
        Ok((AdmissionTransition::new(self, &[]), short_code))
    }

    pub(crate) fn save_resolved_invitation(
        mut self,
        full_invitation: FullInvitation,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
            state,
        )) = self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        if !matches!(
            state.resolution,
            SpaceAdmissionInvitationResolutionState::Started
        ) {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Joiner(
            SpaceAdmissionJoinerState::ResolvedInvitation(SpaceAdmissionJoinerResolvedInvitation {
                join_id: state.join_id,
                local_join_ordinal: state.local_join_ordinal,
                source_snapshot: state.source_snapshot,
                start_context: state.start_context,
                full_invitation,
            }),
        );
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn reject_started_invitation_resolution(
        mut self,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
            state,
        )) = self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        if !matches!(
            state.resolution,
            SpaceAdmissionInvitationResolutionState::Started
        ) {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::LocalJoiner(SpaceAdmissionLocalJoinerRejected {
                join_id: state.join_id,
                reason: SpaceAdmissionRejectionReason::InvitationUnavailable,
            }),
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn reject_before_authentication(
        mut self,
        reason: SpaceAdmissionRejectionReason,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        if !matches!(
            reason,
            SpaceAdmissionRejectionReason::InvitationUnavailable
                | SpaceAdmissionRejectionReason::AuthenticationRejected
                | SpaceAdmissionRejectionReason::PeerUpgradeRequired
        ) {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        if !matches!(
            state.channel_state,
            SpaceAdmissionJoinerChannelState::AwaitingAuthentication { .. }
        ) {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::LocalJoiner(SpaceAdmissionLocalJoinerRejected {
                join_id: state.join_id,
                reason,
            }),
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn cancel_before_authentication(
        mut self,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let join_id = match self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
                state,
            )) => state.join_id,
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvedInvitation(
                state,
            )) => state.join_id,
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state))
                if matches!(
                    state.channel_state,
                    SpaceAdmissionJoinerChannelState::AwaitingAuthentication { .. }
                ) =>
            {
                state.join_id
            }
            _ => return Err(SpaceAdmissionAggregateError::UnsafeCancellation),
        };
        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::LocalJoiner(SpaceAdmissionLocalJoinerRejected {
                join_id,
                reason: SpaceAdmissionRejectionReason::Cancelled,
            }),
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn with_authenticated_channel(
        mut self,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: AdmissionContinuationCredential,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let SpaceAdmissionJoinerChannelState::AwaitingAuthentication { .. } = state.channel_state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(
            SpaceAdmissionJoinerInitiated {
                join_id: state.join_id,
                local_join_ordinal: state.local_join_ordinal,
                source_snapshot: state.source_snapshot,
                private_state: state.private_state,
                channel_state: SpaceAdmissionJoinerChannelState::Authenticated {
                    peer_binding,
                    continuation_credential,
                },
                pending_exchange: state.pending_exchange,
            },
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn accept_candidate(
        mut self,
        candidate: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        staged_target_input: AdmissionStagedTargetInput,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let SpaceAdmissionJoinerChannelState::Authenticated {
            peer_binding,
            continuation_credential,
        } = state.channel_state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let request_id = state
            .pending_exchange
            .request_envelope()
            .header()
            .message_id();
        if candidate.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if candidate.kind() != SpaceAdmissionMessageKind::Candidate
            || candidate.header().sender_sequence() != 0
            || candidate.header().predecessor_message_id() != Some(request_id)
        {
            return Err(SpaceAdmissionAggregateError::InvalidCandidateReply);
        }
        let candidate_evidence = candidate
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidCandidateReply)?;
        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(
            SpaceAdmissionJoinerCandidate {
                join_id: state.join_id,
                local_join_ordinal: state.local_join_ordinal,
                source_snapshot: state.source_snapshot,
                peer_binding,
                continuation_credential,
                candidate,
                candidate_evidence,
                staged_target_input,
            },
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn prepare_candidate(
        mut self,
        verified_history: AdmissionSignedMembershipHistory,
        staged_target: AdmissionStagedTarget,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let prepared_request = pending_exchange.request_envelope();
        if prepared_request.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if prepared_request.kind() != SpaceAdmissionMessageKind::Prepared
            || prepared_request.header().sender_sequence() != 1
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Commit
            || pending_exchange
                .exact_reply_for(&state.candidate_evidence)
                .is_none()
        {
            return Err(SpaceAdmissionAggregateError::InvalidPreparedRequest);
        }
        let SpaceAdmissionBodyV1::Prepared(prepared) = prepared_request.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidPreparedRequest);
        };
        if prepared.proof().attempt_id != *self.admission_id.as_bytes() {
            return Err(SpaceAdmissionAggregateError::InvalidPreparedRequest);
        }

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(
            SpaceAdmissionJoinerPrepared {
                join_id: state.join_id,
                local_join_ordinal: state.local_join_ordinal,
                source_snapshot: state.source_snapshot,
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                candidate_evidence: state.candidate_evidence,
                verified_history,
                staged_target,
                pending_exchange,
            },
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn accept_commit(
        mut self,
        commit: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let prepared_request = state.pending_exchange.request_envelope();
        if commit.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if commit.kind() != SpaceAdmissionMessageKind::Commit
            || commit.header().sender_sequence() != 1
            || commit.header().predecessor_message_id()
                != Some(prepared_request.header().message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidCommitMessage);
        }
        let SpaceAdmissionBodyV1::Prepared(prepared) = prepared_request.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidCommitMessage);
        };
        let SpaceAdmissionBodyV1::Commit(commit_body) = commit.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidCommitMessage);
        };
        let proof = prepared.proof();
        let candidate = commit_body.exact_candidate();
        if proof.attempt_id != *self.admission_id.as_bytes()
            || candidate.candidate_event().event_id() != proof.candidate_event_id
            || candidate.candidate_event().resulting_members_digest != proof.target_members_digest
            || candidate.security_commitment().security_commitment_id
                != proof.security_commitment_id
            || commit_body.target_membership_history() != &state.verified_history
        {
            return Err(SpaceAdmissionAggregateError::InvalidCommitMessage);
        }
        let commit_evidence = commit
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidCommitMessage)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(
            SpaceAdmissionJoinerCommitted {
                join_id: state.join_id,
                local_join_ordinal: state.local_join_ordinal,
                source_snapshot: state.source_snapshot,
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                exact_commit: commit,
                commit_evidence,
                staged_target: state.staged_target,
            },
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn apply_commit(
        mut self,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let applied_request = pending_exchange.request_envelope();
        if applied_request.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if applied_request.kind() != SpaceAdmissionMessageKind::Applied
            || applied_request.header().sender_sequence() != 2
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Complete
            || pending_exchange
                .exact_reply_for(&state.commit_evidence)
                .is_none()
        {
            return Err(SpaceAdmissionAggregateError::InvalidAppliedRequest);
        }
        let SpaceAdmissionBodyV1::Commit(commit) = state.exact_commit.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidAppliedRequest);
        };
        let SpaceAdmissionBodyV1::Applied(applied) = applied_request.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidAppliedRequest);
        };
        let receipt = applied.activation_receipt();
        let candidate = commit.exact_candidate();
        if receipt.attempt_id != *self.admission_id.as_bytes()
            || receipt.event_id != candidate.candidate_event().event_id()
            || receipt.installed_security_commitment_id
                != candidate.security_commitment().security_commitment_id
        {
            return Err(SpaceAdmissionAggregateError::InvalidAppliedRequest);
        }

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(
            SpaceAdmissionJoinerApplied {
                join_id: state.join_id,
                local_join_ordinal: state.local_join_ordinal,
                source_snapshot: state.source_snapshot,
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                exact_commit: state.exact_commit,
                commit_evidence: state.commit_evidence,
                staged_target: state.staged_target,
                pending_exchange,
            },
        ));
        Ok(AdmissionTransition::new(
            self,
            &[AdmissionEffect::ApplyMembership],
        ))
    }

    pub(crate) fn accept_complete(
        mut self,
        complete: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        space_transition: AdmissionSpaceTransition,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let applied_request = state.pending_exchange.request_envelope();
        if complete.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if complete.kind() != SpaceAdmissionMessageKind::Complete
            || complete.header().sender_sequence() != 2
            || complete.header().predecessor_message_id()
                != Some(applied_request.header().message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteMessage);
        }
        let SpaceAdmissionBodyV1::Applied(applied) = applied_request.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteMessage);
        };
        let SpaceAdmissionBodyV1::Complete(complete_body) = complete.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteMessage);
        };
        let receipt = applied.activation_receipt();
        let completion = complete_body.completion();
        if completion.attempt_id != receipt.attempt_id
            || completion.event_id != receipt.event_id
            || completion.security_commitment_id != receipt.installed_security_commitment_id
        {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteMessage);
        }
        let completion_evidence = complete
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidCompleteMessage)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(
            SpaceAdmissionJoinerActivating {
                join_id: state.join_id,
                local_join_ordinal: state.local_join_ordinal,
                source_snapshot: state.source_snapshot,
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                exact_commit: state.exact_commit,
                staged_target: state.staged_target,
                completion: complete,
                completion_evidence,
                space_transition,
            },
        ));
        Ok(AdmissionTransition::new(
            self,
            &[AdmissionEffect::ActivateSpace],
        ))
    }

    pub(crate) fn activate_complete(
        mut self,
        transition_result: AdmissionSpaceTransitionResult,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(state)) =
            self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let complete_ack = pending_exchange.request_envelope();
        if complete_ack.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if complete_ack.kind() != SpaceAdmissionMessageKind::CompleteAck
            || complete_ack.header().sender_sequence() != 3
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Settled
            || pending_exchange
                .exact_reply_for(&state.completion_evidence)
                .is_none()
        {
            return Err(SpaceAdmissionAggregateError::InvalidCompleteAck);
        }

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
            SpaceAdmissionActiveState::PendingSettlement(SpaceAdmissionActivePendingSettlement {
                join_id: state.join_id,
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                completion_evidence: state.completion_evidence,
                transition_result,
                pending_exchange,
            }),
        ));
        Ok(AdmissionTransition::new(
            self,
            &[AdmissionEffect::PublishActive],
        ))
    }

    pub(crate) fn accept_settled(
        mut self,
        settled: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
            SpaceAdmissionActiveState::PendingSettlement(state),
        )) = self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let complete_ack = state.pending_exchange.request_envelope();
        if settled.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if settled.kind() != SpaceAdmissionMessageKind::Settled
            || settled.header().sender_sequence() != 3
            || settled.header().predecessor_message_id() != Some(complete_ack.header().message_id())
        {
            return Err(SpaceAdmissionAggregateError::InvalidSettledMessage);
        }
        let last_received = settled
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidSettledMessage)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
            SpaceAdmissionActiveState::Settled(SpaceAdmissionActiveSettled {
                join_id: state.join_id,
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                last_received,
            }),
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn request_cancel(
        self,
        message_id: AdmissionMessageId,
        retry_state: AdmissionRetryState,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let (route, predecessor, sender_sequence) = match &self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
                let SpaceAdmissionBodyV1::Candidate(candidate) = state.candidate.body() else {
                    return Err(SpaceAdmissionAggregateError::InvalidCandidateReply);
                };
                (
                    SpaceAdmissionRoute::from_bytes(
                        candidate.continuation_route().as_bytes().to_vec(),
                    )
                    .map_err(|_| SpaceAdmissionAggregateError::InvalidCancellationRequest)?,
                    state.candidate_evidence.message_id(),
                    1,
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => (
                SpaceAdmissionRoute::from_bytes(state.pending_exchange.route().as_bytes().to_vec())
                    .map_err(|_| SpaceAdmissionAggregateError::InvalidCancellationRequest)?,
                state.candidate_evidence.message_id(),
                2,
            ),
            SpaceAdmissionRecordState::Joiner(
                SpaceAdmissionJoinerState::Committed(_)
                | SpaceAdmissionJoinerState::Applied(_)
                | SpaceAdmissionJoinerState::Activating(_),
            )
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(_)) => {
                return Err(SpaceAdmissionAggregateError::TooLateCommitted);
            }
            _ => return Err(SpaceAdmissionAggregateError::UnsafeCancellation),
        };
        let request = SpaceAdmissionEnvelopeV1::new(
            self.admission_id,
            AdmissionRole::Joiner,
            sender_sequence,
            message_id,
            Some(predecessor),
            SpaceAdmissionBodyV1::CancelRequested,
        )
        .map_err(|_| SpaceAdmissionAggregateError::InvalidCancellationRequest)?;
        let pending_exchange = PendingAdmissionExchange::new(
            route,
            request,
            SpaceAdmissionMessageKind::Rejected,
            retry_state,
        )
        .map_err(|_| SpaceAdmissionAggregateError::InvalidCancellationRequest)?;
        self.cancel(pending_exchange)
    }

    pub(crate) fn cancel(
        mut self,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let (join_id, peer_binding, continuation_credential, last_received, sender_sequence) =
            match self.state {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => (
                    state.join_id,
                    state.peer_binding,
                    state.continuation_credential,
                    state.candidate_evidence,
                    1,
                ),
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => (
                    state.join_id,
                    state.peer_binding,
                    state.continuation_credential,
                    state.candidate_evidence,
                    2,
                ),
                SpaceAdmissionRecordState::Joiner(
                    SpaceAdmissionJoinerState::Committed(_)
                    | SpaceAdmissionJoinerState::Applied(_)
                    | SpaceAdmissionJoinerState::Activating(_),
                )
                | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(_)) => {
                    return Err(SpaceAdmissionAggregateError::TooLateCommitted);
                }
                _ => return Err(SpaceAdmissionAggregateError::UnsafeCancellation),
            };
        let cancel_request = pending_exchange.request_envelope();
        if cancel_request.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if cancel_request.kind() != SpaceAdmissionMessageKind::CancelRequested
            || cancel_request.header().sender_sequence() != sender_sequence
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Rejected
            || pending_exchange.exact_reply_for(&last_received).is_none()
        {
            return Err(SpaceAdmissionAggregateError::InvalidCancellationRequest);
        }

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(
            SpaceAdmissionJoinerCancelling {
                join_id,
                peer_binding,
                continuation_credential,
                last_received,
                pending_exchange,
            },
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn supersede(mut self) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let superseded = match self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
                state,
            )) => SpaceAdmissionSupersededState::Initiated {
                join_id: state.join_id,
            },
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvedInvitation(
                state,
            )) => SpaceAdmissionSupersededState::Initiated {
                join_id: state.join_id,
            },
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => {
                match state.channel_state {
                    SpaceAdmissionJoinerChannelState::AwaitingAuthentication { .. } => {
                        SpaceAdmissionSupersededState::Initiated {
                            join_id: state.join_id,
                        }
                    }
                    SpaceAdmissionJoinerChannelState::Authenticated {
                        peer_binding,
                        continuation_credential,
                    } => SpaceAdmissionSupersededState::Authenticated {
                        join_id: state.join_id,
                        peer_binding,
                        continuation_credential,
                    },
                }
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
                SpaceAdmissionSupersededState::Candidate(SpaceAdmissionSupersededTerminal {
                    join_id: state.join_id,
                    peer_binding: state.peer_binding,
                    continuation_credential: state.continuation_credential,
                    last_received: state.candidate_evidence,
                })
            }
            _ => return Err(SpaceAdmissionAggregateError::UnsafeSupersession),
        };

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(
            superseded,
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn accept_rejection(
        mut self,
        rejected: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let (
            join_id,
            peer_binding,
            continuation_credential,
            expected_sender_sequence,
            expected_predecessor,
            required_reason,
        ) = match self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => {
                let SpaceAdmissionJoinerChannelState::Authenticated {
                    peer_binding,
                    continuation_credential,
                } = state.channel_state
                else {
                    return Err(SpaceAdmissionAggregateError::InvalidTransition);
                };
                (
                    state.join_id,
                    peer_binding,
                    continuation_credential,
                    0,
                    state
                        .pending_exchange
                        .request_envelope()
                        .header()
                        .message_id(),
                    None,
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(state)) => (
                state.join_id,
                state.peer_binding,
                state.continuation_credential,
                1,
                state
                    .pending_exchange
                    .request_envelope()
                    .header()
                    .message_id(),
                Some(SpaceAdmissionRejectionReason::Cancelled),
            ),
            _ => return Err(SpaceAdmissionAggregateError::InvalidTransition),
        };
        if rejected.header().admission_id() != self.admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if rejected.kind() != SpaceAdmissionMessageKind::Rejected
            || rejected.header().sender_sequence() != expected_sender_sequence
            || rejected.header().predecessor_message_id() != Some(expected_predecessor)
        {
            return Err(SpaceAdmissionAggregateError::InvalidRejectedReply);
        }
        let SpaceAdmissionBodyV1::Rejected { reason } = rejected.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidRejectedReply);
        };
        if required_reason.is_some_and(|required| *reason != required) {
            return Err(SpaceAdmissionAggregateError::InvalidRejectedReply);
        }
        let reason = *reason;
        let last_received = rejected
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidRejectedReply)?;

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::Joiner(SpaceAdmissionJoinerRejected {
                join_id,
                peer_binding,
                continuation_credential,
                reason,
                last_received,
            }),
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }
}
