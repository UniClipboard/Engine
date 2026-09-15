use super::*;

impl SpaceAdmissionAggregate {
    pub(crate) fn start_resolving_invitation(
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        local_join_ordinal: u64,
        source_snapshot: AdmissionSourceSnapshot,
        start_context: AdmissionJoinerStartContext,
        short_code: AdmissionShortInvitationCode,
        started_at_ms: i64,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let attempt_timeline = AdmissionAttemptTimeline::start(started_at_ms)
            .map_err(|_| SpaceAdmissionAggregateError::InvalidAttemptTimeline)?;
        Ok(AdmissionTransition::new(
            Self {
                format_version: SPACE_ADMISSION_RECORD_FORMAT_V2,
                record_version: 0,
                admission_id,
                attempt_timeline: Some(attempt_timeline),
                attempt_digest: None,
                state: SpaceAdmissionRecordState::Joiner(
                    SpaceAdmissionJoinerState::ResolvingInvitation(
                        SpaceAdmissionJoinerResolvingInvitation {
                            join_id,
                            local_join_ordinal,
                            source_snapshot,
                            start_context,
                            resolution: SpaceAdmissionInvitationResolutionState::Ready {
                                short_code,
                            },
                        },
                    ),
                ),
            },
            &[],
        ))
    }

    pub(crate) fn start_join(
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        local_join_ordinal: u64,
        source_snapshot: AdmissionSourceSnapshot,
        private_state: AdmissionJoinerPrivateState,
        encrypted_password_equivalent: AdmissionEncryptedPasswordEquivalent,
        pending_exchange: PendingAdmissionExchange,
        started_at_ms: i64,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let attempt_timeline = AdmissionAttemptTimeline::start(started_at_ms)
            .map_err(|_| SpaceAdmissionAggregateError::InvalidAttemptTimeline)?;
        if pending_exchange.request_envelope().header().admission_id() != admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if pending_exchange.request_envelope().kind() != SpaceAdmissionMessageKind::JoinRequest
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Candidate
        {
            return Err(SpaceAdmissionAggregateError::InvalidInitialExchange);
        }
        let replacement = Self {
            format_version: SPACE_ADMISSION_RECORD_FORMAT_V2,
            record_version: 0,
            admission_id,
            attempt_timeline: Some(attempt_timeline),
            attempt_digest: None,
            state: SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(
                SpaceAdmissionJoinerInitiated {
                    join_id,
                    local_join_ordinal,
                    source_snapshot,
                    private_state,
                    channel_state: SpaceAdmissionJoinerChannelState::AwaitingAuthentication {
                        encrypted_password_equivalent,
                    },
                    pending_exchange,
                },
            )),
        };
        Ok(AdmissionTransition::new(replacement, &[]))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn accept_join_request(
        admission_id: SpaceAdmissionId,
        invitation_claim: AdmissionInvitationClaim,
        join_request: SpaceAdmissionEnvelopeV1,
        join_request_evidence: AdmissionMessageEvidence,
        base_snapshot: AdmissionBaseSnapshot,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: AdmissionContinuationCredential,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        Self::accept_join_request_with_timeline(
            admission_id,
            invitation_claim,
            join_request,
            join_request_evidence,
            base_snapshot,
            peer_binding,
            continuation_credential,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn accept_join_request_with_timeline(
        admission_id: SpaceAdmissionId,
        invitation_claim: AdmissionInvitationClaim,
        join_request: SpaceAdmissionEnvelopeV1,
        join_request_evidence: AdmissionMessageEvidence,
        base_snapshot: AdmissionBaseSnapshot,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: AdmissionContinuationCredential,
        attempt_timeline: Option<AdmissionAttemptTimeline>,
        attempt_digest: Option<[u8; 32]>,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        if join_request.header().admission_id() != admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if join_request.kind() != SpaceAdmissionMessageKind::JoinRequest
            || !message_matches_evidence(&join_request, &join_request_evidence)
        {
            return Err(SpaceAdmissionAggregateError::InvalidInboundEvidence);
        }
        let replacement = Self {
            format_version: if attempt_timeline.is_some() {
                SPACE_ADMISSION_RECORD_FORMAT_V2
            } else {
                SPACE_ADMISSION_RECORD_FORMAT_V1
            },
            record_version: 0,
            admission_id,
            attempt_timeline,
            attempt_digest,
            state: SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(
                SpaceAdmissionSponsorAccepted {
                    invitation_claim,
                    join_request,
                    join_request_evidence,
                    base_snapshot,
                    peer_binding,
                    continuation_credential,
                },
            )),
        };
        Ok(AdmissionTransition::new(
            replacement,
            &[AdmissionEffect::ConsumeInvitation],
        ))
    }

    pub const fn format_version(&self) -> u16 {
        self.format_version
    }

    pub const fn record_version(&self) -> u64 {
        self.record_version
    }

    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.admission_id
    }

    #[cfg(test)]
    pub(crate) const fn state(&self) -> &SpaceAdmissionRecordState {
        &self.state
    }
}
