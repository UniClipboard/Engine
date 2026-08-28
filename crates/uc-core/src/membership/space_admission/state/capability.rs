use super::super::exchange::{AdmissionReplayDecision, AdmissionReplayError};
use super::super::message::AdmissionRole;
use super::*;

pub trait AdmissionRecordPersistence {
    fn decode_persisted(bytes: &[u8]) -> Result<Self, SpaceAdmissionPersistenceError>
    where
        Self: Sized;
    fn admission_id(&self) -> SpaceAdmissionId;
    fn record_version(&self) -> u64;
    fn is_terminal(&self) -> bool;
    fn encode_persisted(&self) -> Result<Vec<u8>, SpaceAdmissionPersistenceError>;
}

#[derive(PartialEq, Eq)]
pub struct JoinerAdmission {
    record: SpaceAdmissionAggregate,
}

#[derive(PartialEq, Eq)]
pub struct SponsorAdmission {
    record: SpaceAdmissionAggregate,
}

pub struct JoinerAdmissionTransition {
    replacement: JoinerAdmission,
    effects: &'static [AdmissionEffect],
}

pub struct SponsorAdmissionTransition {
    replacement: SponsorAdmission,
    effects: &'static [AdmissionEffect],
}

impl std::fmt::Debug for JoinerAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.record.fmt(formatter)
    }
}

impl std::fmt::Debug for SponsorAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.record.fmt(formatter)
    }
}

impl JoinerAdmissionTransition {
    fn from_transition(transition: AdmissionTransition) -> Self {
        let effects = transition.effects();
        Self {
            replacement: JoinerAdmission {
                record: transition.into_replacement(),
            },
            effects,
        }
    }

    pub const fn replacement(&self) -> &JoinerAdmission {
        &self.replacement
    }

    pub fn into_replacement(self) -> JoinerAdmission {
        self.replacement
    }

    pub const fn effects(&self) -> &'static [AdmissionEffect] {
        self.effects
    }
}

impl SponsorAdmissionTransition {
    fn from_transition(transition: AdmissionTransition) -> Self {
        let effects = transition.effects();
        Self {
            replacement: SponsorAdmission {
                record: transition.into_replacement(),
            },
            effects,
        }
    }

    pub const fn replacement(&self) -> &SponsorAdmission {
        &self.replacement
    }

    pub fn into_replacement(self) -> SponsorAdmission {
        self.replacement
    }

    pub const fn effects(&self) -> &'static [AdmissionEffect] {
        self.effects
    }
}

impl JoinerAdmission {
    pub fn try_from_record(record: SpaceAdmissionAggregate) -> Option<Self> {
        if matches!(
            record.state,
            SpaceAdmissionRecordState::Joiner(_)
                | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Active(_))
                | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Superseded(_))
                | SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                    SpaceAdmissionRejectedState::LocalJoiner(_)
                        | SpaceAdmissionRejectedState::Joiner(_)
                ))
        ) {
            Some(Self { record })
        } else {
            None
        }
    }

    pub fn start_join(
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        local_join_ordinal: u64,
        source_snapshot: AdmissionSourceSnapshot,
        private_state: AdmissionJoinerPrivateState,
        encrypted_password_equivalent: AdmissionEncryptedPasswordEquivalent,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        SpaceAdmissionAggregate::start_join(
            admission_id,
            join_id,
            local_join_ordinal,
            source_snapshot,
            private_state,
            encrypted_password_equivalent,
            pending_exchange,
        )
        .map(JoinerAdmissionTransition::from_transition)
    }

    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.record.admission_id()
    }

    pub const fn is_active_settled(&self) -> bool {
        self.record.is_active_settled()
    }

    pub fn pending_recovery(&self) -> Option<AdmissionPendingRecovery<'_>> {
        self.record.pending_recovery()
    }

    pub const fn pending_exchange(&self) -> Option<&PendingAdmissionExchange> {
        self.record.pending_exchange()
    }

    pub fn current_exact_reply(&self) -> Option<&SpaceAdmissionEnvelopeV1> {
        self.record.current_exact_reply()
    }

    pub fn joiner_candidate_preparation(&self) -> Option<JoinerCandidatePreparation<'_>> {
        self.record.joiner_candidate_preparation()
    }

    pub fn joiner_applied_preparation(&self) -> Option<JoinerAppliedPreparation<'_>> {
        self.record.joiner_applied_preparation()
    }

    pub fn joiner_complete_preparation(&self) -> Option<JoinerCompletePreparation<'_>> {
        self.record.joiner_complete_preparation()
    }

    pub fn joiner_activation_preparation(&self) -> Option<JoinerActivationPreparation<'_>> {
        self.record.joiner_activation_preparation()
    }

    pub fn reject_before_authentication(
        self,
        reason: SpaceAdmissionRejectionReason,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .reject_before_authentication(reason)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn cancel_before_authentication(
        self,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .cancel_before_authentication()
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn with_authenticated_channel(
        self,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: AdmissionContinuationCredential,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .with_authenticated_channel(peer_binding, continuation_credential)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn accept_candidate(
        self,
        candidate: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        staged_target_input: AdmissionStagedTargetInput,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .accept_candidate(candidate, canonical_digest, staged_target_input)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn prepare_candidate(
        self,
        verified_history: AdmissionSignedMembershipHistory,
        staged_target: AdmissionStagedTarget,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .prepare_candidate(verified_history, staged_target, pending_exchange)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn accept_commit(
        self,
        commit: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .accept_commit(commit, canonical_digest)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn apply_commit(
        self,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .apply_commit(pending_exchange)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn accept_complete(
        self,
        complete: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        space_transition: AdmissionSpaceTransition,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .accept_complete(complete, canonical_digest, space_transition)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn activate_complete(
        self,
        transition_result: AdmissionSpaceTransitionResult,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .activate_complete(transition_result, pending_exchange)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn accept_settled(
        self,
        settled: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .accept_settled(settled, canonical_digest)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn cancel(
        self,
        pending_exchange: PendingAdmissionExchange,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .cancel(pending_exchange)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn supersede(self) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .supersede()
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn accept_rejection(
        self,
        rejected: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .accept_rejection(rejected, canonical_digest)
            .map(JoinerAdmissionTransition::from_transition)
    }

    pub fn require_recovery(
        self,
        category: AdmissionRecoveryCategory,
    ) -> Result<JoinerAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .require_recovery(category)
            .map(JoinerAdmissionTransition::from_transition)
    }
}

impl SponsorAdmission {
    pub fn try_from_record(record: SpaceAdmissionAggregate) -> Option<Self> {
        let is_sponsor = match &record.state {
            SpaceAdmissionRecordState::Sponsor(_) => true,
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Completed(state)) => {
                state
                    .saved_reply
                    .exact_reply_envelope()
                    .header()
                    .sender_role()
                    == AdmissionRole::Sponsor
            }
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::Rejected(
                SpaceAdmissionRejectedState::Sponsor(_),
            )) => true,
            _ => false,
        };
        if is_sponsor {
            Some(Self { record })
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn accept_join_request(
        admission_id: SpaceAdmissionId,
        invitation_claim: AdmissionInvitationClaim,
        join_request: SpaceAdmissionEnvelopeV1,
        join_request_evidence: AdmissionMessageEvidence,
        base_snapshot: AdmissionBaseSnapshot,
        peer_binding: AdmissionPeerBinding,
        continuation_credential: AdmissionContinuationCredential,
    ) -> Result<SponsorAdmissionTransition, SpaceAdmissionAggregateError> {
        SpaceAdmissionAggregate::accept_join_request(
            admission_id,
            invitation_claim,
            join_request,
            join_request_evidence,
            base_snapshot,
            peer_binding,
            continuation_credential,
        )
        .map(SponsorAdmissionTransition::from_transition)
    }

    pub const fn admission_id(&self) -> SpaceAdmissionId {
        self.record.admission_id()
    }

    pub fn current_exact_reply(&self) -> Option<&SpaceAdmissionEnvelopeV1> {
        self.record.current_exact_reply()
    }

    pub fn sponsor_candidate_preparation(&self) -> Option<SponsorCandidatePreparation<'_>> {
        self.record.sponsor_candidate_preparation()
    }

    pub fn sponsor_peer_binding(&self) -> Option<AdmissionPeerBinding> {
        self.record.sponsor_peer_binding()
    }

    pub fn sponsor_commit_preparation(&self) -> Option<SponsorCommitPreparation<'_>> {
        self.record.sponsor_commit_preparation()
    }

    pub fn sponsor_complete_preparation(&self) -> Option<SponsorCompletePreparation<'_>> {
        self.record.sponsor_complete_preparation()
    }

    pub fn sponsor_settlement_preparation(&self) -> Option<SponsorSettlementPreparation<'_>> {
        self.record.sponsor_settlement_preparation()
    }

    pub fn replay_or_reject<'a>(
        &'a self,
        incoming: &AdmissionMessageEvidence,
    ) -> Result<AdmissionReplayDecision<'a>, AdmissionReplayError> {
        self.record.replay_or_reject(incoming)
    }

    pub fn fix_candidate(
        self,
        candidate_reply: SpaceAdmissionEnvelopeV1,
        staged_security: AdmissionStagedSecurityState,
    ) -> Result<SponsorAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .fix_candidate(candidate_reply, staged_security)
            .map(SponsorAdmissionTransition::from_transition)
    }

    pub fn commit_prepared(
        self,
        prepared: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        committed_history: AdmissionSignedMembershipHistory,
        sealed_security: AdmissionSealedSecurityState,
        commit_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<SponsorAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .commit_prepared(
                prepared,
                canonical_digest,
                committed_history,
                sealed_security,
                commit_reply,
            )
            .map(SponsorAdmissionTransition::from_transition)
    }

    pub fn complete_applied(
        self,
        applied: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        activated_security: AdmissionActivatedSecurityState,
        complete_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<SponsorAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .complete_applied(
                applied,
                canonical_digest,
                activated_security,
                complete_reply,
            )
            .map(SponsorAdmissionTransition::from_transition)
    }

    pub fn settle_complete_ack(
        self,
        complete_ack: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        settled_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<SponsorAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .settle_complete_ack(complete_ack, canonical_digest, settled_reply)
            .map(SponsorAdmissionTransition::from_transition)
    }

    pub fn reject_cancel(
        self,
        cancel_request: SpaceAdmissionEnvelopeV1,
        canonical_digest: [u8; 32],
        rejected_reply: SpaceAdmissionEnvelopeV1,
    ) -> Result<SponsorAdmissionTransition, SpaceAdmissionAggregateError> {
        self.record
            .reject_cancel(cancel_request, canonical_digest, rejected_reply)
            .map(SponsorAdmissionTransition::from_transition)
    }
}

impl AdmissionRecordPersistence for SpaceAdmissionAggregate {
    fn decode_persisted(bytes: &[u8]) -> Result<Self, SpaceAdmissionPersistenceError> {
        SpaceAdmissionAggregate::decode_persisted(bytes)
    }

    fn admission_id(&self) -> SpaceAdmissionId {
        SpaceAdmissionAggregate::admission_id(self)
    }

    fn record_version(&self) -> u64 {
        SpaceAdmissionAggregate::record_version(self)
    }

    fn is_terminal(&self) -> bool {
        SpaceAdmissionAggregate::is_terminal(self)
    }

    fn encode_persisted(&self) -> Result<Vec<u8>, SpaceAdmissionPersistenceError> {
        SpaceAdmissionAggregate::encode_persisted(self)
    }
}

impl AdmissionRecordPersistence for JoinerAdmission {
    fn decode_persisted(bytes: &[u8]) -> Result<Self, SpaceAdmissionPersistenceError> {
        let record = SpaceAdmissionAggregate::decode_persisted(bytes)?;
        Self::try_from_record(record).ok_or(SpaceAdmissionPersistenceError::InvalidState)
    }

    fn admission_id(&self) -> SpaceAdmissionId {
        self.record.admission_id()
    }

    fn record_version(&self) -> u64 {
        self.record.record_version()
    }

    fn is_terminal(&self) -> bool {
        self.record.is_terminal()
    }

    fn encode_persisted(&self) -> Result<Vec<u8>, SpaceAdmissionPersistenceError> {
        self.record.encode_persisted()
    }
}

impl AdmissionRecordPersistence for SponsorAdmission {
    fn decode_persisted(bytes: &[u8]) -> Result<Self, SpaceAdmissionPersistenceError> {
        let record = SpaceAdmissionAggregate::decode_persisted(bytes)?;
        Self::try_from_record(record).ok_or(SpaceAdmissionPersistenceError::InvalidState)
    }

    fn admission_id(&self) -> SpaceAdmissionId {
        self.record.admission_id()
    }

    fn record_version(&self) -> u64 {
        self.record.record_version()
    }

    fn is_terminal(&self) -> bool {
        self.record.is_terminal()
    }

    fn encode_persisted(&self) -> Result<Vec<u8>, SpaceAdmissionPersistenceError> {
        self.record.encode_persisted()
    }
}
