use super::*;
use sha2::{Digest, Sha256};

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
                let attempt_digest = self
                    .attempt_digest
                    .ok_or(SpaceAdmissionAggregateError::InvalidAttemptTimeline)?;
                (
                    state.join_id,
                    state.local_join_ordinal,
                    Some(cleanup_obligation(
                        self.admission_id,
                        attempt_digest,
                        AdmissionCommitKnowledge::Unknown,
                        None,
                        state.peer_binding,
                        state.continuation_credential,
                        SpaceAdmissionRoute::from_bytes(
                            state.pending_exchange.route().as_bytes().to_vec(),
                        )
                        .map_err(|_| SpaceAdmissionAggregateError::InvalidTransition)?,
                        state.candidate_evidence.message_id(),
                        reason,
                    )?),
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
                        state.exact_commit.header().message_id(),
                        reason,
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
                        state.exact_commit.header().message_id(),
                        reason,
                    )?),
                )
            }
            SpaceAdmissionRecordState::Joiner(SpaceAdmissionJoinerState::Activating(state))
                if has_authenticated_attempt =>
            {
                let local_space_transition = AdmissionSpaceTransition::from_bytes(
                    state.space_transition.as_bytes().to_vec(),
                )
                .map_err(|_| SpaceAdmissionAggregateError::InvalidTransition)?;
                let mut cleanup = known_cleanup_obligation(
                    self.attempt_digest,
                    &state.exact_commit,
                    state.peer_binding,
                    state.continuation_credential,
                    state.completion.header().message_id(),
                    reason,
                )?;
                cleanup.local_space_transition = Some(local_space_transition);
                (state.join_id, state.local_join_ordinal, Some(cleanup))
            }
            _ => return Err(SpaceAdmissionAggregateError::UnsafeCancellation),
        };
        self.record_version = record_version;
        if cleanup.is_some() {
            self.format_version = SPACE_ADMISSION_RECORD_FORMAT_V2;
        }
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

    pub(crate) fn complete_local_space_termination(
        mut self,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Terminated(state)) =
            &mut self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let cleanup = state
            .cleanup
            .as_mut()
            .ok_or(SpaceAdmissionAggregateError::InvalidTransition)?;
        if cleanup.local_space_transition.take().is_none() {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        self.record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        self.format_version = SPACE_ADMISSION_RECORD_FORMAT_V2;
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

    /// 判断放弃通知是否已无法被邀请方接受。
    ///
    /// 期限到达后邀请方会按自己的期限收尾，不再依赖这条通知；没有期限的旧记录和本机旧协议版本的
    /// 通知同样无法再被接受。仍需本机切换空间的收尾不在此列。
    pub(crate) fn has_undeliverable_abandonment(&self, now_ms: i64) -> bool {
        let SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Terminated(state)) =
            &self.state
        else {
            return false;
        };
        let Some(cleanup) = state.cleanup.as_ref() else {
            return false;
        };
        let Some(pending) = cleanup.pending_exchange.as_ref() else {
            return false;
        };
        if cleanup.local_space_transition.is_some() {
            return false;
        }
        let within_deadline = self
            .attempt_timeline
            .is_some_and(|timeline| !timeline.is_expired(now_ms));
        !within_deadline
            || pending.request_envelope().header().protocol_version()
                != SpaceAdmissionProtocolVersion::CURRENT
    }

    /// 结束无法送达的放弃通知，保留终止围栏。
    pub(crate) fn end_undeliverable_abandonment(
        mut self,
        now_ms: i64,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        if !self.has_undeliverable_abandonment(now_ms) {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        if let SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Terminated(state)) =
            &mut self.state
        {
            if let Some(cleanup) = state.cleanup.as_mut() {
                cleanup.pending_exchange = None;
            }
        }
        self.record_version = record_version;
        Ok(AdmissionTransition::new(self, &[]))
    }

    pub(crate) fn accept_abandoned(
        mut self,
        abandoned: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        let SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Terminated(state)) =
            &mut self.state
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let cleanup = state
            .cleanup
            .as_mut()
            .ok_or(SpaceAdmissionAggregateError::InvalidTransition)?;
        let pending = cleanup
            .pending_exchange
            .as_ref()
            .ok_or(SpaceAdmissionAggregateError::InvalidTransition)?;
        let expected = AdmissionInboundExpectation::new(
            self.admission_id,
            abandoned.header().sender_role(),
            4,
            Some(pending.request_envelope().header().message_id()),
        );
        let AdmissionInboundDecision::New(_) = expected
            .classify(&abandoned, canonical_digest, None)
            .map_err(|_| SpaceAdmissionAggregateError::InvalidTransition)?
        else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        if abandoned.header().protocol_version() != SpaceAdmissionProtocolVersion::CURRENT
            || abandoned.kind() != SpaceAdmissionMessageKind::Abandoned
            || !matches!(
                abandoned.header().sender_role(),
                AdmissionRole::Sponsor | AdmissionRole::CompletionHelper
            )
        {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        let SpaceAdmissionBodyV1::Abandoned(body) = abandoned.body() else {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        };
        let request_digest: [u8; 32] = Sha256::digest(
            pending
                .request_envelope()
                .encode_canonical_v1()
                .map_err(|_| SpaceAdmissionAggregateError::InvalidTransition)?,
        )
        .into();
        if body.abandonment_digest() != &request_digest {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        cleanup.pending_exchange = None;
        self.record_version = record_version;
        Ok(AdmissionTransition::new(self, &[]))
    }
}

fn known_cleanup_obligation(
    attempt_digest: Option<[u8; 32]>,
    exact_commit: &SpaceAdmissionEnvelopeV1,
    peer_binding: AdmissionPeerBinding,
    continuation_credential: AdmissionContinuationCredential,
    predecessor_message_id: AdmissionMessageId,
    reason: SpaceAdmissionTerminationReason,
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
    let route = SpaceAdmissionRoute::from_bytes(
        commit
            .exact_candidate()
            .continuation_route()
            .as_bytes()
            .to_vec(),
    )
    .map_err(|_| SpaceAdmissionAggregateError::InvalidCommitReply)?;
    cleanup_obligation(
        exact_commit.header().admission_id(),
        *member_binding.attempt_digest(),
        AdmissionCommitKnowledge::Known,
        Some(member_binding),
        peer_binding,
        continuation_credential,
        route,
        predecessor_message_id,
        reason,
    )
}

#[allow(clippy::too_many_arguments)]
fn cleanup_obligation(
    admission_id: SpaceAdmissionId,
    attempt_digest: [u8; 32],
    commit_knowledge: AdmissionCommitKnowledge,
    member_binding: Option<AdmissionMemberBindingV2>,
    peer_binding: AdmissionPeerBinding,
    continuation_credential: AdmissionContinuationCredential,
    route: SpaceAdmissionRoute,
    predecessor_message_id: AdmissionMessageId,
    reason: SpaceAdmissionTerminationReason,
) -> Result<AdmissionCleanupObligation, SpaceAdmissionAggregateError> {
    let abandonment_reason = match reason {
        SpaceAdmissionTerminationReason::Cancelled => AdmissionAbandonmentReasonV2::Cancelled,
        SpaceAdmissionTerminationReason::Expired => AdmissionAbandonmentReasonV2::Expired,
        SpaceAdmissionTerminationReason::Superseded => AdmissionAbandonmentReasonV2::Superseded,
        SpaceAdmissionTerminationReason::ActivationRejected
        | SpaceAdmissionTerminationReason::CompletionRejected
        | SpaceAdmissionTerminationReason::MembershipHistoryRejected
        | SpaceAdmissionTerminationReason::SecurityMaterialRejected
        | SpaceAdmissionTerminationReason::RelationshipRejected
        | SpaceAdmissionTerminationReason::ActivationStateRejected => {
            AdmissionAbandonmentReasonV2::Rejected
        }
    };
    let body =
        AdmissionAbandonmentV2::new(attempt_digest, member_binding.clone(), abandonment_reason)
            .ok_or(SpaceAdmissionAggregateError::InvalidAttemptTimeline)?;
    let message_id = AdmissionMessageId::from_bytes(attempt_digest)
        .ok_or(SpaceAdmissionAggregateError::InvalidAttemptTimeline)?;
    let request = SpaceAdmissionEnvelopeV1::new_with_version(
        SpaceAdmissionProtocolVersion::CURRENT,
        admission_id,
        AdmissionRole::Joiner,
        4,
        message_id,
        Some(predecessor_message_id),
        SpaceAdmissionBodyV1::Abandonment(body),
    )
    .map_err(|_| SpaceAdmissionAggregateError::InvalidTransition)?;
    let pending_exchange = PendingAdmissionExchange::new(
        route,
        request,
        SpaceAdmissionMessageKind::Abandoned,
        AdmissionRetryState::new(0, 0)
            .map_err(|_| SpaceAdmissionAggregateError::InvalidTransition)?,
    )
    .map_err(|_| SpaceAdmissionAggregateError::InvalidTransition)?;
    Ok(AdmissionCleanupObligation {
        commit_knowledge,
        member_binding,
        peer_binding,
        continuation_credential,
        pending_exchange: Some(pending_exchange),
        local_space_transition: None,
    })
}
