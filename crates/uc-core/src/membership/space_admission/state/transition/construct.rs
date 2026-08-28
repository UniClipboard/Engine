use super::*;

impl SpaceAdmissionAggregate {
    pub(crate) fn start_join(
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        local_join_ordinal: u64,
        source_snapshot: AdmissionSourceSnapshot,
        private_state: AdmissionJoinerPrivateState,
        encrypted_password_equivalent: AdmissionEncryptedPasswordEquivalent,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        if pending_exchange.request_envelope().header().admission_id() != admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if pending_exchange.request_envelope().kind() != SpaceAdmissionMessageKind::JoinRequest
            || pending_exchange.exact_expected_reply_kind() != SpaceAdmissionMessageKind::Candidate
        {
            return Err(SpaceAdmissionAggregateError::InvalidInitialExchange);
        }
        let replacement = Self {
            format_version: SPACE_ADMISSION_RECORD_FORMAT_V1,
            record_version: 0,
            admission_id,
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
        if join_request.header().admission_id() != admission_id {
            return Err(SpaceAdmissionAggregateError::AdmissionMismatch);
        }
        if join_request.kind() != SpaceAdmissionMessageKind::JoinRequest
            || !message_matches_evidence(&join_request, &join_request_evidence)
        {
            return Err(SpaceAdmissionAggregateError::InvalidInboundEvidence);
        }
        let replacement = Self {
            format_version: SPACE_ADMISSION_RECORD_FORMAT_V1,
            record_version: 0,
            admission_id,
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
