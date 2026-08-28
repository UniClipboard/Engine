use super::*;

impl SpaceAdmissionAggregate {
    /// Produces a sensitive plaintext payload that Infra must AEAD-seal before persistence.
    pub fn encode_persisted(&self) -> Result<Vec<u8>, SpaceAdmissionPersistenceError> {
        if self.format_version != SPACE_ADMISSION_RECORD_FORMAT_V1 {
            return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
        }
        let state = match &self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(state)) => {
                PersistedSpaceAdmissionStateV1::JoinerInitiated(
                    PersistedJoinerInitiatedV1::try_from(state)?,
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(state)) => {
                PersistedSpaceAdmissionStateV1::JoinerCandidate(
                    PersistedJoinerCandidateV1::try_from(state)?,
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(state)) => {
                PersistedSpaceAdmissionStateV1::JoinerPrepared(PersistedJoinerPreparedV1::try_from(
                    state,
                )?)
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(state)) => {
                PersistedSpaceAdmissionStateV1::SponsorAccepted(
                    PersistedSponsorAcceptedV1::try_from(state)?,
                )
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(state)) => {
                PersistedSpaceAdmissionStateV1::SponsorCandidate(
                    PersistedSponsorCandidateV1::try_from(state)?,
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(state)) => {
                PersistedSpaceAdmissionStateV1::JoinerCommitted(
                    PersistedJoinerCommittedV1::try_from(state)?,
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(state)) => {
                PersistedSpaceAdmissionStateV1::JoinerApplied(PersistedJoinerAppliedV1::try_from(
                    state,
                )?)
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(state)) => {
                PersistedSpaceAdmissionStateV1::JoinerActivating(
                    PersistedJoinerActivatingV1::try_from(state)?,
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(state)) => {
                PersistedSpaceAdmissionStateV1::JoinerCancelling(
                    PersistedJoinerCancellingV1::try_from(state)?,
                )
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(state)) => {
                PersistedSpaceAdmissionStateV1::SponsorCommitted(
                    PersistedSponsorCommittedV1::try_from(state)?,
                )
            }
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state)) => {
                PersistedSpaceAdmissionStateV1::SponsorApplied(PersistedSponsorAppliedV1::try_from(
                    state,
                )?)
            }
            SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Challenged(state),
            ) => PersistedSpaceAdmissionStateV1::CompletionHelperChallenged(
                PersistedCompletionHelperChallengedV1::from(state),
            ),
            SpaceAdmissionRecordState::CompletionHelper(
                SpaceAdmissionCompletionHelperState::Applied(state),
            ) => PersistedSpaceAdmissionStateV1::CompletionHelperApplied(
                PersistedCompletionHelperAppliedV1::try_from(state)?,
            ),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::PendingSettlement(state),
            )) => PersistedSpaceAdmissionStateV1::ActivePendingSettlement(
                PersistedActivePendingSettlementV1::try_from(state)?,
            ),
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                SpaceAdmissionActiveState::Settled(state),
            )) => {
                PersistedSpaceAdmissionStateV1::ActiveSettled(PersistedActiveSettledV1::from(state))
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(state)) => {
                PersistedSpaceAdmissionStateV1::Completed(PersistedCompletedV1::try_from(state)?)
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(state)) => {
                PersistedSpaceAdmissionStateV1::Superseded(PersistedSupersededV1::from(state))
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(state)) => {
                PersistedSpaceAdmissionStateV1::Rejected(PersistedRejectedV1::try_from(state)?)
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
                state,
            )) => PersistedSpaceAdmissionStateV1::RecoveryRequired(encode_recovery_category(
                state.category,
            )),
        };
        postcard::to_stdvec(&PersistedSpaceAdmissionRecordV1 {
            format_version: self.format_version,
            record_version: self.record_version,
            admission_id: *self.admission_id.as_bytes(),
            state,
        })
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)
    }

    /// Reconstructs a validated aggregate from a decrypted persisted payload.
    pub fn decode_persisted(bytes: &[u8]) -> Result<Self, SpaceAdmissionPersistenceError> {
        let persisted: PersistedSpaceAdmissionRecordV1 = postcard::from_bytes(bytes)
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)?;
        if persisted.format_version != SPACE_ADMISSION_RECORD_FORMAT_V1 {
            return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
        }
        let admission_id = SpaceAdmissionId::from_bytes(persisted.admission_id)
            .ok_or(SpaceAdmissionPersistenceError::InvalidState)?;
        let state = match persisted.state {
            PersistedSpaceAdmissionStateV1::JoinerInitiated(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Initiated(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::JoinerCandidate(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Candidate(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::JoinerPrepared(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Prepared(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::SponsorAccepted(state) => {
                SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Accepted(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::SponsorCandidate(state) => {
                SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Candidate(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::JoinerCommitted(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Committed(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::JoinerApplied(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Applied(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::JoinerActivating(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::JoinerCancelling(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Cancelling(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::SponsorCommitted(state) => {
                SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Committed(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::SponsorApplied(state) => {
                SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::CompletionHelperChallenged(state) => {
                SpaceAdmissionRecordState::CompletionHelper(
                    SpaceAdmissionCompletionHelperState::Challenged(state.into_domain()?),
                )
            }
            PersistedSpaceAdmissionStateV1::CompletionHelperApplied(state) => {
                SpaceAdmissionRecordState::CompletionHelper(
                    SpaceAdmissionCompletionHelperState::Applied(state.into_domain(admission_id)?),
                )
            }
            PersistedSpaceAdmissionStateV1::ActivePendingSettlement(state) => {
                SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                    SpaceAdmissionActiveState::PendingSettlement(state.into_domain(admission_id)?),
                ))
            }
            PersistedSpaceAdmissionStateV1::ActiveSettled(state) => {
                SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(
                    SpaceAdmissionActiveState::Settled(state.into_domain()?),
                ))
            }
            PersistedSpaceAdmissionStateV1::Completed(state) => {
                SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(
                    state.into_domain(admission_id)?,
                ))
            }
            PersistedSpaceAdmissionStateV1::Superseded(state) => {
                SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(
                    state.into_domain()?,
                ))
            }
            PersistedSpaceAdmissionStateV1::Rejected(state) => SpaceAdmissionRecordState::Terminal(
                SpaceAdmissionTerminalState::Rejected(state.into_domain(admission_id)?),
            ),
            PersistedSpaceAdmissionStateV1::RecoveryRequired(category) => {
                SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
                    SpaceAdmissionRecoveryRequiredTerminal {
                        category: decode_recovery_category(category)?,
                    },
                ))
            }
        };
        Ok(Self {
            format_version: persisted.format_version,
            record_version: persisted.record_version,
            admission_id,
            state,
        })
    }
}
