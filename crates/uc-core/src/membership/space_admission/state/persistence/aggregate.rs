use super::*;

impl SpaceAdmissionAggregate {
    /// Produces a sensitive plaintext payload that Infra must AEAD-seal before persistence.
    pub fn encode_persisted(&self) -> Result<Vec<u8>, SpaceAdmissionPersistenceError> {
        if self.format_version == SPACE_ADMISSION_RECORD_FORMAT_V2 {
            match &self.state {
                SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Terminated(
                    state,
                )) => {
                    return encode_record_v2(
                        self,
                        PersistedSpaceAdmissionStateV2::LocalJoinerTerminated {
                            join_id: *state.join_id.as_bytes(),
                            local_join_ordinal: state.local_join_ordinal,
                            reason: encode_local_termination_reason(state.reason)?,
                            cleanup: state
                                .cleanup
                                .as_ref()
                                .map(encode_cleanup_obligation)
                                .transpose()?,
                        },
                    );
                }
                SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                    SpaceAdmissionRejectedState::Sponsor(state),
                )) if state.abandonment_cleanup.is_some() => {
                    return encode_record_v2(
                        self,
                        PersistedSpaceAdmissionStateV2::SponsorAbandoned {
                            peer_binding: PersistedPeerBindingV1::from(state.peer_binding),
                            continuation_credential: state
                                .continuation_credential
                                .as_bytes()
                                .to_vec(),
                            saved_reply: PersistedSavedReplyV1::try_from(&state.saved_reply)?,
                            cleanup: encode_sponsor_abandonment_cleanup(
                                state
                                    .abandonment_cleanup
                                    .as_ref()
                                    .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
                            ),
                        },
                    );
                }
                SpaceAdmissionRecordState::Terminal(
                    SpaceAdmissionTerminalState::SponsorExpired(state),
                ) => {
                    return encode_record_v2(
                        self,
                        PersistedSpaceAdmissionStateV2::SponsorExpired {
                            cleanup: encode_sponsor_abandonment_cleanup(&state.abandonment_cleanup),
                        },
                    );
                }
                SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state))
                    if state.confirmation.is_some() =>
                {
                    return encode_record_v2(
                        self,
                        PersistedSpaceAdmissionStateV2::SponsorApplied {
                            applied: PersistedSponsorAppliedV1::try_from(state)?,
                            confirmation: encode_sponsor_confirmation(
                                state
                                    .confirmation
                                    .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
                            ),
                        },
                    );
                }
                SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(
                    state,
                )) if state.confirmation.is_some() => {
                    return encode_record_v2(
                        self,
                        PersistedSpaceAdmissionStateV2::SponsorCompleted {
                            completed: PersistedCompletedV1::try_from(state)?,
                            confirmation: encode_sponsor_confirmation(
                                state
                                    .confirmation
                                    .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
                            ),
                        },
                    );
                }
                _ => {}
            }
        } else if self.format_version != SPACE_ADMISSION_RECORD_FORMAT_V1 {
            return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
        }
        let state = match &self.state {
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
                state,
            )) => PersistedSpaceAdmissionStateV1::JoinerResolvingInvitation(
                PersistedJoinerResolvingInvitationV1::from(state),
            ),
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvedInvitation(
                state,
            )) => PersistedSpaceAdmissionStateV1::JoinerResolvedInvitation(
                PersistedJoinerResolvedInvitationV1::from(state),
            ),
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
            SpaceAdmissionRecordState::Terminal(
                SpaceAdmissionTerminalState::Terminated(_)
                | SpaceAdmissionTerminalState::SponsorExpired(_),
            ) => {
                return Err(SpaceAdmissionPersistenceError::InvalidState);
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
                state,
            )) => PersistedSpaceAdmissionStateV1::RecoveryRequired(encode_recovery_category(
                state.category,
            )),
        };
        if self.format_version == SPACE_ADMISSION_RECORD_FORMAT_V2 {
            // V2 复用已验证的 V1 状态编码，并只为新增状态提供专属变体。
            let encoded_state = postcard::to_stdvec(&state)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)?;
            encode_record_v2(
                self,
                PersistedSpaceAdmissionStateV2::Existing(encoded_state),
            )
        } else {
            postcard::to_stdvec(&PersistedSpaceAdmissionRecordV1 {
                format_version: self.format_version,
                record_version: self.record_version,
                admission_id: *self.admission_id.as_bytes(),
                state,
            })
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)
        }
    }

    /// Reconstructs a validated aggregate from a decrypted persisted payload.
    pub fn decode_persisted(bytes: &[u8]) -> Result<Self, SpaceAdmissionPersistenceError> {
        let (format_version, _) = postcard::take_from_bytes::<u16>(bytes)
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)?;
        if format_version == SPACE_ADMISSION_RECORD_FORMAT_V2 {
            return decode_record_v2(bytes);
        }
        let persisted = decode_record_with_legacy_pending_exchange(bytes)?;
        if persisted.format_version != SPACE_ADMISSION_RECORD_FORMAT_V1 {
            return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
        }
        let admission_id = SpaceAdmissionId::from_bytes(persisted.admission_id)
            .ok_or(SpaceAdmissionPersistenceError::InvalidState)?;
        let state = match persisted.state {
            PersistedSpaceAdmissionStateV1::JoinerResolvingInvitation(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvingInvitation(
                    state.into_domain()?,
                ))
            }
            PersistedSpaceAdmissionStateV1::JoinerResolvedInvitation(state) => {
                SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::ResolvedInvitation(
                    state.into_domain()?,
                ))
            }
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
            attempt_timeline: None,
            attempt_digest: None,
            state,
        })
    }
}

fn encode_record_v2(
    aggregate: &SpaceAdmissionAggregate,
    state: PersistedSpaceAdmissionStateV2,
) -> Result<Vec<u8>, SpaceAdmissionPersistenceError> {
    let timeline = aggregate
        .attempt_timeline
        .ok_or(SpaceAdmissionPersistenceError::InvalidState)?;
    postcard::to_stdvec(&PersistedSpaceAdmissionRecordV2 {
        format_version: SPACE_ADMISSION_RECORD_FORMAT_V2,
        record_version: aggregate.record_version,
        admission_id: *aggregate.admission_id.as_bytes(),
        started_at_ms: timeline.started_at_ms(),
        expires_at_ms: timeline.expires_at_ms(),
        attempt_digest: aggregate.attempt_digest,
        state,
    })
    .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)
}

fn decode_record_v2(
    bytes: &[u8],
) -> Result<SpaceAdmissionAggregate, SpaceAdmissionPersistenceError> {
    let (persisted, remaining): (PersistedSpaceAdmissionRecordV2, _) =
        postcard::take_from_bytes(bytes)
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)?;
    if !remaining.is_empty() {
        return Err(SpaceAdmissionPersistenceError::InvalidEncoding);
    }
    if persisted.format_version != SPACE_ADMISSION_RECORD_FORMAT_V2 {
        return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
    }
    let admission_id = SpaceAdmissionId::from_bytes(persisted.admission_id)
        .ok_or(SpaceAdmissionPersistenceError::InvalidState)?;
    let attempt_timeline =
        AdmissionAttemptTimeline::new(persisted.started_at_ms, persisted.expires_at_ms)
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?;
    if persisted.attempt_digest == Some([0; 32]) {
        return Err(SpaceAdmissionPersistenceError::InvalidState);
    }
    let state = match persisted.state {
        PersistedSpaceAdmissionStateV2::Existing(encoded_state) => {
            let (state, remaining): (PersistedSpaceAdmissionStateV1, _) =
                postcard::take_from_bytes(&encoded_state)
                    .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)?;
            if !remaining.is_empty() {
                return Err(SpaceAdmissionPersistenceError::InvalidEncoding);
            }
            let legacy = postcard::to_stdvec(&PersistedSpaceAdmissionRecordV1 {
                format_version: SPACE_ADMISSION_RECORD_FORMAT_V1,
                record_version: persisted.record_version,
                admission_id: persisted.admission_id,
                state,
            })
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)?;
            let aggregate = SpaceAdmissionAggregate::decode_persisted(&legacy)?;
            if !is_attempt_timeline_v2_state(&aggregate.state) {
                return Err(SpaceAdmissionPersistenceError::InvalidState);
            }
            aggregate.state
        }
        PersistedSpaceAdmissionStateV2::LocalJoinerTerminated {
            join_id,
            local_join_ordinal,
            reason,
            cleanup,
        } => {
            let cleanup = cleanup
                .map(|cleanup| decode_cleanup_obligation(cleanup, admission_id))
                .transpose()?;
            if let Some(cleanup) = &cleanup {
                validate_joiner_cleanup_attempt(
                    cleanup,
                    persisted
                        .attempt_digest
                        .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
                )?;
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Terminated(
                SpaceAdmissionLocalJoinerTerminated {
                    join_id: decode_join_id(join_id)?,
                    local_join_ordinal,
                    reason: decode_local_termination_reason(reason)?,
                    cleanup,
                },
            ))
        }
        PersistedSpaceAdmissionStateV2::SponsorApplied {
            applied,
            confirmation,
        } => {
            if persisted.attempt_digest.is_none() {
                return Err(SpaceAdmissionPersistenceError::InvalidState);
            }
            let mut state = applied.into_domain(admission_id)?;
            state.confirmation = Some(decode_sponsor_confirmation(confirmation, admission_id)?);
            SpaceAdmissionRecordState::Sponsor(SpaceAdmissionSponsorState::Applied(state))
        }
        PersistedSpaceAdmissionStateV2::SponsorCompleted {
            completed,
            confirmation,
        } => {
            if persisted.attempt_digest.is_none() {
                return Err(SpaceAdmissionPersistenceError::InvalidState);
            }
            let mut state = completed.into_domain(admission_id)?;
            state.confirmation = Some(decode_sponsor_confirmation(confirmation, admission_id)?);
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(state))
        }
        PersistedSpaceAdmissionStateV2::SponsorAbandoned {
            peer_binding,
            continuation_credential,
            saved_reply,
            cleanup,
        } => {
            let attempt_digest = persisted
                .attempt_digest
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?;
            let saved_reply = saved_reply.into_domain(admission_id)?;
            if saved_reply.exact_reply_envelope().kind() != SpaceAdmissionMessageKind::Abandoned {
                return Err(SpaceAdmissionPersistenceError::InvalidState);
            }
            let cleanup = decode_sponsor_abandonment_cleanup(cleanup)?;
            validate_sponsor_cleanup_attempt(&cleanup, attempt_digest)?;
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::Sponsor(SpaceAdmissionSponsorRejected {
                    peer_binding: peer_binding.into_domain()?,
                    continuation_credential: decode_continuation_credential(
                        continuation_credential,
                    )?,
                    reason: SpaceAdmissionRejectionReason::Cancelled,
                    saved_reply,
                    abandonment_cleanup: Some(cleanup),
                }),
            ))
        }
        PersistedSpaceAdmissionStateV2::SponsorExpired { cleanup } => {
            let attempt_digest = persisted
                .attempt_digest
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?;
            let cleanup = decode_sponsor_abandonment_cleanup(cleanup)?;
            validate_sponsor_cleanup_attempt(&cleanup, attempt_digest)?;
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::SponsorExpired(
                SpaceAdmissionSponsorExpired {
                    abandonment_cleanup: cleanup,
                },
            ))
        }
    };
    Ok(SpaceAdmissionAggregate {
        format_version: SPACE_ADMISSION_RECORD_FORMAT_V2,
        record_version: persisted.record_version,
        admission_id,
        attempt_timeline: Some(attempt_timeline),
        attempt_digest: persisted.attempt_digest,
        state,
    })
}

fn encode_sponsor_abandonment_cleanup(
    cleanup: &SponsorAbandonmentCleanup,
) -> PersistedSponsorAbandonmentCleanupV2 {
    match cleanup {
        SponsorAbandonmentCleanup::NotRequired => PersistedSponsorAbandonmentCleanupV2::NotRequired,
        SponsorAbandonmentCleanup::Known(binding) => {
            PersistedSponsorAbandonmentCleanupV2::Known(binding.canonical_bytes())
        }
        SponsorAbandonmentCleanup::Unknown {
            attempt_digest,
            member_instance_id,
            add_event_id,
        } => PersistedSponsorAbandonmentCleanupV2::Unknown {
            attempt_digest: *attempt_digest,
            member_instance_id: *member_instance_id.as_bytes(),
            add_event_id: *add_event_id.as_bytes(),
        },
    }
}

fn decode_sponsor_abandonment_cleanup(
    cleanup: PersistedSponsorAbandonmentCleanupV2,
) -> Result<SponsorAbandonmentCleanup, SpaceAdmissionPersistenceError> {
    match cleanup {
        PersistedSponsorAbandonmentCleanupV2::NotRequired => {
            Ok(SponsorAbandonmentCleanup::NotRequired)
        }
        PersistedSponsorAbandonmentCleanupV2::Known(binding) => {
            Ok(SponsorAbandonmentCleanup::Known(
                AdmissionMemberBindingV2::decode_canonical(&binding)
                    .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            ))
        }
        PersistedSponsorAbandonmentCleanupV2::Unknown {
            attempt_digest,
            member_instance_id,
            add_event_id,
        } if attempt_digest != [0; 32] => Ok(SponsorAbandonmentCleanup::Unknown {
            attempt_digest,
            member_instance_id: MemberInstanceId::from_bytes(member_instance_id),
            add_event_id: MembershipEventId::from_bytes(add_event_id),
        }),
        PersistedSponsorAbandonmentCleanupV2::Unknown { .. } => {
            Err(SpaceAdmissionPersistenceError::InvalidState)
        }
    }
}

fn validate_joiner_cleanup_attempt(
    cleanup: &AdmissionCleanupObligation,
    attempt_digest: [u8; 32],
) -> Result<(), SpaceAdmissionPersistenceError> {
    if cleanup
        .member_binding()
        .is_some_and(|binding| binding.attempt_digest() != &attempt_digest)
    {
        return Err(SpaceAdmissionPersistenceError::InvalidState);
    }
    let Some(pending) = cleanup.pending_exchange() else {
        return Ok(());
    };
    let SpaceAdmissionBodyV1::Abandonment(abandonment) = pending.request_envelope().body() else {
        return Err(SpaceAdmissionPersistenceError::InvalidState);
    };
    if abandonment.attempt_digest() != &attempt_digest
        || abandonment.member_binding() != cleanup.member_binding()
    {
        return Err(SpaceAdmissionPersistenceError::InvalidState);
    }
    Ok(())
}

fn validate_sponsor_cleanup_attempt(
    cleanup: &SponsorAbandonmentCleanup,
    attempt_digest: [u8; 32],
) -> Result<(), SpaceAdmissionPersistenceError> {
    match cleanup {
        SponsorAbandonmentCleanup::NotRequired => Ok(()),
        SponsorAbandonmentCleanup::Known(binding)
            if binding.attempt_digest() == &attempt_digest =>
        {
            Ok(())
        }
        SponsorAbandonmentCleanup::Unknown {
            attempt_digest: cleanup_digest,
            ..
        } if cleanup_digest == &attempt_digest => Ok(()),
        SponsorAbandonmentCleanup::Known(_) | SponsorAbandonmentCleanup::Unknown { .. } => {
            Err(SpaceAdmissionPersistenceError::InvalidState)
        }
    }
}

fn encode_sponsor_confirmation(
    summary: SponsorPairingConfirmationSummary,
) -> PersistedSponsorPairingConfirmationV2 {
    PersistedSponsorPairingConfirmationV2 {
        status: match summary.status {
            SponsorPairingConfirmationStatus::AwaitingPeerConfirmation => 0,
            SponsorPairingConfirmationStatus::Unconfirmed => 1,
            SponsorPairingConfirmationStatus::Confirmed => 2,
        },
        admission_id: *summary.admission_id.as_bytes(),
        member_instance_id: *summary.member_instance_id.as_bytes(),
        add_event_id: *summary.add_event_id.as_bytes(),
    }
}

fn decode_sponsor_confirmation(
    persisted: PersistedSponsorPairingConfirmationV2,
    admission_id: SpaceAdmissionId,
) -> Result<SponsorPairingConfirmationSummary, SpaceAdmissionPersistenceError> {
    if persisted.admission_id != *admission_id.as_bytes() {
        return Err(SpaceAdmissionPersistenceError::InvalidState);
    }
    let status = match persisted.status {
        0 => SponsorPairingConfirmationStatus::AwaitingPeerConfirmation,
        1 => SponsorPairingConfirmationStatus::Unconfirmed,
        2 => SponsorPairingConfirmationStatus::Confirmed,
        _ => return Err(SpaceAdmissionPersistenceError::InvalidState),
    };
    Ok(SponsorPairingConfirmationSummary {
        status,
        admission_id,
        member_instance_id: MemberInstanceId::from_bytes(persisted.member_instance_id),
        add_event_id: MembershipEventId::from_bytes(persisted.add_event_id),
    })
}

fn encode_cleanup_obligation(
    cleanup: &AdmissionCleanupObligation,
) -> Result<PersistedAdmissionCleanupObligationV2, SpaceAdmissionPersistenceError> {
    Ok(PersistedAdmissionCleanupObligationV2 {
        commit_knowledge: match cleanup.commit_knowledge {
            AdmissionCommitKnowledge::Unknown => 0,
            AdmissionCommitKnowledge::Known => 1,
        },
        member_binding: cleanup
            .member_binding
            .as_ref()
            .map(AdmissionMemberBindingV2::canonical_bytes),
        local_peer_id: *cleanup.peer_binding.local_peer_id().as_bytes(),
        remote_peer_id: *cleanup.peer_binding.remote_peer_id().as_bytes(),
        continuation_credential: cleanup.continuation_credential.as_bytes().to_vec(),
        pending_exchange: cleanup
            .pending_exchange
            .as_ref()
            .map(PersistedAnyPendingExchangeV1::try_from)
            .transpose()?,
        local_space_transition: cleanup
            .local_space_transition
            .as_ref()
            .map(|transition| transition.as_bytes().to_vec()),
    })
}

fn decode_cleanup_obligation(
    persisted: PersistedAdmissionCleanupObligationV2,
    admission_id: SpaceAdmissionId,
) -> Result<AdmissionCleanupObligation, SpaceAdmissionPersistenceError> {
    let pending_exchange = persisted
        .pending_exchange
        .map(|exchange| exchange.into_domain(admission_id))
        .transpose()?;
    let local_space_transition = persisted
        .local_space_transition
        .map(AdmissionSpaceTransition::from_bytes)
        .transpose()
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?;
    let commit_knowledge = match persisted.commit_knowledge {
        0 if persisted.member_binding.is_none() => AdmissionCommitKnowledge::Unknown,
        1 if persisted.member_binding.is_some() => AdmissionCommitKnowledge::Known,
        _ => return Err(SpaceAdmissionPersistenceError::InvalidState),
    };
    let member_binding = persisted
        .member_binding
        .map(|encoded| AdmissionMemberBindingV2::decode_canonical(&encoded))
        .transpose()
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?;
    let peer_binding = AdmissionPeerBinding::new(
        AdmissionChannelPeerId::from_bytes(persisted.local_peer_id)
            .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
        AdmissionChannelPeerId::from_bytes(persisted.remote_peer_id)
            .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
    )
    .ok_or(SpaceAdmissionPersistenceError::InvalidState)?;
    let continuation_credential =
        AdmissionContinuationCredential::from_bytes(persisted.continuation_credential)
            .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?;
    Ok(AdmissionCleanupObligation {
        commit_knowledge,
        member_binding,
        peer_binding,
        continuation_credential,
        pending_exchange,
        local_space_transition,
    })
}

const fn encode_local_termination_reason(
    reason: SpaceAdmissionTerminationReason,
) -> Result<u8, SpaceAdmissionPersistenceError> {
    match reason {
        SpaceAdmissionTerminationReason::Cancelled => Ok(0),
        SpaceAdmissionTerminationReason::Expired => Ok(1),
        SpaceAdmissionTerminationReason::Superseded => Ok(2),
    }
}

const fn decode_local_termination_reason(
    reason: u8,
) -> Result<SpaceAdmissionTerminationReason, SpaceAdmissionPersistenceError> {
    match reason {
        0 => Ok(SpaceAdmissionTerminationReason::Cancelled),
        1 => Ok(SpaceAdmissionTerminationReason::Expired),
        2 => Ok(SpaceAdmissionTerminationReason::Superseded),
        _ => Err(SpaceAdmissionPersistenceError::InvalidState),
    }
}

const fn is_attempt_timeline_v2_state(state: &SpaceAdmissionRecordState) -> bool {
    matches!(
        state,
        SpaceAdmissionRecordState::Joiner(_)
            | SpaceAdmissionRecordState::Sponsor(
                SpaceAdmissionSponsorState::Accepted(_)
                    | SpaceAdmissionSponsorState::Candidate(_)
                    | SpaceAdmissionSponsorState::Committed(_)
            )
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(_))
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(_))
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::LocalJoiner(_)
                    | SpaceAdmissionRejectedState::Joiner(_)
                    | SpaceAdmissionRejectedState::Sponsor(_)
            ))
            | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(_))
    )
}

fn decode_record_with_legacy_pending_exchange(
    bytes: &[u8],
) -> Result<PersistedSpaceAdmissionRecordV1, SpaceAdmissionPersistenceError> {
    if let Ok(persisted) = decode_exact_record(bytes) {
        return Ok(persisted);
    }

    let (format_version, _) = postcard::take_from_bytes::<u16>(bytes)
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)?;
    if format_version != SPACE_ADMISSION_RECORD_FORMAT_V1 {
        return Err(SpaceAdmissionPersistenceError::UnsupportedVersion);
    }

    // 未发布的旧 V1 布局在 pending_exchange.retry_state 后直接结束；补一个
    // Option::None 正好还原新增的尾部 block_reason 字段。
    let mut compatible = Vec::with_capacity(bytes.len().saturating_add(1));
    compatible.extend_from_slice(bytes);
    compatible.push(0);
    let persisted = decode_exact_record(&compatible)?;
    let is_legacy_pending = match &persisted.state {
        PersistedSpaceAdmissionStateV1::JoinerInitiated(state) => {
            state.pending_exchange.block_reason.is_none()
        }
        PersistedSpaceAdmissionStateV1::JoinerPrepared(state) => {
            state.pending_exchange.block_reason.is_none()
        }
        PersistedSpaceAdmissionStateV1::JoinerApplied(state) => {
            state.pending_exchange.block_reason.is_none()
        }
        PersistedSpaceAdmissionStateV1::JoinerCancelling(state) => {
            state.pending_exchange.block_reason.is_none()
        }
        PersistedSpaceAdmissionStateV1::ActivePendingSettlement(state) => {
            state.pending_exchange.block_reason.is_none()
        }
        _ => false,
    };
    if !is_legacy_pending {
        return Err(SpaceAdmissionPersistenceError::InvalidEncoding);
    }
    Ok(persisted)
}

fn decode_exact_record(
    bytes: &[u8],
) -> Result<PersistedSpaceAdmissionRecordV1, SpaceAdmissionPersistenceError> {
    let (persisted, remaining) = postcard::take_from_bytes(bytes)
        .map_err(|_| SpaceAdmissionPersistenceError::InvalidEncoding)?;
    if !remaining.is_empty() {
        return Err(SpaceAdmissionPersistenceError::InvalidEncoding);
    }
    Ok(persisted)
}
