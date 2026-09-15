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
        let confirmation = self
            .attempt_timeline
            .map(|_| SponsorPairingConfirmationSummary {
                status: SponsorPairingConfirmationStatus::AwaitingPeerConfirmation,
                admission_id: self.admission_id,
                member_instance_id: receipt.joiner_member_instance_id,
                add_event_id: receipt.event_id,
            });

        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(
            SpaceAdmissionSponsorApplied {
                peer_binding: state.peer_binding,
                continuation_credential: state.continuation_credential,
                committed_history: state.committed_history,
                activation_receipt: receipt.clone(),
                activated_security,
                saved_reply,
                confirmation,
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
            confirmation,
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
                state.confirmation.map(|summary| {
                    summary.with_status(SponsorPairingConfirmationStatus::Confirmed)
                }),
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
                None,
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
                confirmation,
            },
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn mark_sponsor_confirmation_unconfirmed(
        mut self,
        now_ms: i64,
    ) -> Result<Option<AdmissionTransition>, SpaceAdmissionAggregateError> {
        let Some(timeline) = self.attempt_timeline else {
            return Ok(None);
        };
        if !timeline.is_expired(now_ms) {
            return Ok(None);
        }
        let SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) =
            &mut self.state
        else {
            return Ok(None);
        };
        let Some(summary) = state.confirmation else {
            return Ok(None);
        };
        if summary.status != SponsorPairingConfirmationStatus::AwaitingPeerConfirmation {
            return Ok(None);
        }
        self.record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        state.confirmation =
            Some(summary.with_status(SponsorPairingConfirmationStatus::Unconfirmed));
        Ok(Some(AdmissionTransition::new(self, &[])))
    }

    pub(crate) fn terminate_sponsor_if_expired(
        mut self,
        now_ms: i64,
    ) -> Result<Option<AdmissionTransition>, SpaceAdmissionAggregateError> {
        let Some(timeline) = self.attempt_timeline else {
            return Ok(None);
        };
        if !timeline.is_expired(now_ms) {
            return Ok(None);
        }
        let cleanup = match &self.state {
            SpaceAdmissionRecordState::Sponsor(
                SpaceAdmissionSponsorState::Accepted(_) | SpaceAdmissionSponsorState::Candidate(_),
            ) => SponsorAbandonmentCleanup::NotRequired,
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) => {
                let attempt_digest = self
                    .attempt_digest
                    .ok_or(SpaceAdmissionAggregateError::UnsafeCancellation)?;
                SponsorAbandonmentCleanup::Known(sponsor_commit_member_binding(
                    attempt_digest,
                    &state.saved_reply,
                )?)
            }
            // 已写入完成关系的 Sponsor 到期后进入 Unconfirmed，仍允许迟到确认。
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(_)) => {
                return Ok(None);
            }
            _ => return Ok(None),
        };
        self.record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        self.format_version = SPACE_ADMISSION_RECORD_FORMAT_V2;
        self.state = SpaceAdmissionRecordState::Terminal(
            SpaceAdmissionTerminalState::SponsorExpired(SpaceAdmissionSponsorExpired {
                abandonment_cleanup: cleanup,
            }),
        );
        Ok(Some(AdmissionTransition::new(self, &[])))
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
                abandonment_cleanup: None,
            }),
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn accept_abandonment(
        mut self,
        abandonment: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        abandoned_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let attempt_digest = self
            .attempt_digest
            .ok_or(SpaceAdmissionAggregateError::InvalidAbandonmentRequest)?;
        if abandonment.header().admission_id() != self.admission_id
            || abandonment.header().protocol_version() != SpaceAdmissionProtocolVersion::V2
            || abandonment.kind() != SpaceAdmissionMessageKind::Abandonment
            || abandonment.header().sender_role() != AdmissionRole::Joiner
            || abandonment.header().sender_sequence() != 4
        {
            return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
        }
        let evidence = abandonment
            .evidence(canonical_digest)
            .ok_or(SpaceAdmissionAggregateError::InvalidAbandonmentRequest)?;
        let SpaceAdmissionBodyV1::Abandonment(body) = abandonment.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
        };
        if body.attempt_digest() != &attempt_digest {
            return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
        }
        let (peer_binding, continuation_credential, cleanup) = match self.state {
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => {
                if body.member_binding().is_some()
                    || abandonment.header().predecessor_message_id()
                        != Some(
                            state
                                .saved_reply
                                .exact_reply_envelope()
                                .header()
                                .message_id(),
                        )
                {
                    return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
                }
                (
                    state.peer_binding,
                    state.continuation_credential,
                    SponsorAbandonmentCleanup::NotRequired,
                )
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) => {
                if abandonment.header().predecessor_message_id()
                    != Some(
                        state
                            .saved_reply
                            .exact_reply_envelope()
                            .header()
                            .message_id(),
                    )
                {
                    return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
                }
                let binding = sponsor_commit_member_binding(attempt_digest, &state.saved_reply)?;
                if body
                    .member_binding()
                    .is_some_and(|claimed| claimed != &binding)
                {
                    return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
                }
                (
                    state.peer_binding,
                    state.continuation_credential,
                    SponsorAbandonmentCleanup::Known(binding),
                )
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) => {
                if abandonment.header().predecessor_message_id()
                    != Some(
                        state
                            .saved_reply
                            .exact_reply_envelope()
                            .header()
                            .message_id(),
                    )
                {
                    return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
                }
                let confirmation = state
                    .confirmation
                    .ok_or(SpaceAdmissionAggregateError::InvalidAbandonmentRequest)?;
                let cleanup = sponsor_applied_abandonment_cleanup(
                    attempt_digest,
                    confirmation,
                    body.member_binding(),
                )?;
                (state.peer_binding, state.continuation_credential, cleanup)
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(state)) => {
                if abandonment.header().predecessor_message_id()
                    != Some(
                        state
                            .saved_reply
                            .exact_reply_envelope()
                            .header()
                            .message_id(),
                    )
                {
                    return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
                }
                let confirmation = state
                    .confirmation
                    .ok_or(SpaceAdmissionAggregateError::InvalidAbandonmentRequest)?;
                let cleanup = sponsor_applied_abandonment_cleanup(
                    attempt_digest,
                    confirmation,
                    body.member_binding(),
                )?;
                (state.peer_binding, state.continuation_credential, cleanup)
            }
            _ => return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest),
        };
        let SpaceAdmissionBodyV1::Abandoned(reply) = abandoned_reply.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
        };
        if abandoned_reply.header().admission_id() != self.admission_id
            || abandoned_reply.header().protocol_version() != SpaceAdmissionProtocolVersion::V2
            || abandoned_reply.header().sender_role() != AdmissionRole::Sponsor
            || abandoned_reply.kind() != SpaceAdmissionMessageKind::Abandoned
            || abandoned_reply.header().sender_sequence() != 4
            || abandoned_reply.header().predecessor_message_id() != Some(evidence.message_id())
            || reply.abandonment_digest() != &canonical_digest
        {
            return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
        }
        let saved_reply = SavedAdmissionReply::new(self.admission_id, evidence, abandoned_reply)
            .map_err(|_| SpaceAdmissionAggregateError::InvalidAbandonmentRequest)?;
        self.format_version = SPACE_ADMISSION_RECORD_FORMAT_V2;
        self.record_version = record_version;
        self.state = SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
            SpaceAdmissionRejectedState::Sponsor(SpaceAdmissionSponsorRejected {
                peer_binding,
                continuation_credential,
                reason: SpaceAdmissionRejectionReason::Cancelled,
                saved_reply,
                abandonment_cleanup: Some(cleanup),
            }),
        ));
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn complete_abandonment_cleanup(
        mut self,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let cleanup = match &mut self.state {
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::Sponsor(state),
            )) => state
                .abandonment_cleanup
                .as_mut()
                .ok_or(SpaceAdmissionAggregateError::InvalidTransition)?,
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::SponsorExpired(
                state,
            )) => &mut state.abandonment_cleanup,
            _ => return Err(SpaceAdmissionAggregateError::InvalidTransition),
        };
        if matches!(cleanup, SponsorAbandonmentCleanup::NotRequired) {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        self.record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        *cleanup = SponsorAbandonmentCleanup::NotRequired;
        Ok(AdmissionTransition::new(self, &[]))
    }
}

fn sponsor_commit_member_binding(
    attempt_digest: [u8; 32],
    saved_reply: &SavedAdmissionReply,
) -> Result<AdmissionMemberBindingV2, SpaceAdmissionAggregateError> {
    let SpaceAdmissionBodyV1::Commit(commit) = saved_reply.exact_reply_envelope().body() else {
        return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
    };
    let event = commit.exact_candidate().candidate_event();
    let MembershipOperationV2::AddDevice { admission } = &event.operation else {
        return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
    };
    AdmissionMemberBindingV2::new(
        attempt_digest,
        SpaceId::from_str(&event.lineage_id),
        admission.facts.member_instance,
        event.event_id(),
    )
    .map_err(|_| SpaceAdmissionAggregateError::InvalidAbandonmentRequest)
}

fn sponsor_applied_abandonment_cleanup(
    attempt_digest: [u8; 32],
    confirmation: SponsorPairingConfirmationSummary,
    claimed: Option<&AdmissionMemberBindingV2>,
) -> Result<SponsorAbandonmentCleanup, SpaceAdmissionAggregateError> {
    if let Some(binding) = claimed {
        if binding.attempt_digest() != &attempt_digest
            || binding.member_instance_id() != confirmation.member_instance_id
            || binding.add_event_id() != confirmation.add_event_id
        {
            return Err(SpaceAdmissionAggregateError::InvalidAbandonmentRequest);
        }
    }
    Ok(SponsorAbandonmentCleanup::Unknown {
        attempt_digest,
        member_instance_id: confirmation.member_instance_id,
        add_event_id: confirmation.add_event_id,
    })
}
