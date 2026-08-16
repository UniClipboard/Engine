use std::sync::Arc;

use sha2::{Digest, Sha256};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionActivationReceipt, AdmissionAttemptId, AdmissionAttemptRepositoryError,
    AdmissionAttemptRepositoryPort, AdmissionAttemptRoleStateV1, AdmissionAttemptV1,
    AdmissionContentKeyCatalogV1, AdmissionInboxRecordV1, AdmissionOutboxDeliveryPort,
    AdmissionOutboxDeliveryResultV1, AdmissionOutboxMessageV1, AdmissionOutboxPurposeV1,
    AdmissionRejectionReasonV1, AdmissionSecurityCommitmentV1, AdmissionSecurityTransitionInput,
    AdmissionSecurityTransitionPort, AdmissionSpaceTransitionError, AdmissionSpaceTransitionPort,
    AdmissionSpaceTransitionPreparationV2, AdmissionSpaceTransitionResultV2,
    AdmissionSpaceTransitionStepV2, AdmissionSpaceTransitionV2, AdmissionTerminalResultV1,
    HistoricalMembershipSignatureVerifier, InvitationConsumeDeliveryResultV1,
    JoinerAdmissionStageV1, JoinerAdmissionStateV1, MembershipEventV2,
    MembershipHistoryV2ReceiveOutcome, MembershipOperationV2, SponsorAdmissionStageV1,
    SponsorAdmissionStateV1, VersionedMembershipHistory,
};

use super::WorkspaceConvergenceError;

/// Owns durable admission progression. Network and product callers never
/// construct or advance the stored state directly.
pub(crate) struct DurableAdmissionTransaction {
    repository: Arc<dyn AdmissionAttemptRepositoryPort>,
    history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    security_transition: Arc<dyn AdmissionSecurityTransitionPort>,
    space_transition: Arc<dyn AdmissionSpaceTransitionPort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct DurableAdmissionCandidateV1 {
    pub lineage_id: String,
    pub base_history_position: Vec<u8>,
    pub candidate_event: Vec<u8>,
    pub candidate_event_id: [u8; 32],
    pub candidate_key_package: Vec<u8>,
    pub target_members_digest: [u8; 32],
    pub security_commitment: Vec<u8>,
    pub security_commit: Vec<u8>,
    pub security_welcome: Vec<u8>,
    pub target_protection_group_id: String,
    pub target_key_catalog: Vec<u8>,
    pub target_relationships: Vec<uc_core::membership::AdmissionChangeFacts>,
    pub staged_security_state: Vec<u8>,
    pub identity_binding: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum InvitationConsumeResultV1 {
    Consumed,
    NotFound,
    Conflict,
    Retryable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingMemberRemovalOutcomeV1 {
    AdmissionRejected(AdmissionOutboxMessageV1),
    OrdinaryMemberRemovalRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingInboundMemberProjectionV1 {
    pub device_id: DeviceId,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AdmissionRecoveryReportV1 {
    pub deliveries_attempted: usize,
    pub deliveries_confirmed: usize,
    pub attempts_compacted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JoinerActivationOutcomeV1 {
    Active(AdmissionInboxRecordV1),
    SpaceTransitionRequired,
}

pub(crate) fn verify_candidate_preparation(
    mut history: VersionedMembershipHistory,
    candidate_event: &MembershipEventV2,
    sponsor_commitment: &AdmissionSecurityCommitmentV1,
    joiner_commitment: &AdmissionSecurityCommitmentV1,
    verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
) -> Result<VersionedMembershipHistory, WorkspaceConvergenceError> {
    sponsor_commitment
        .validate()
        .map_err(|error| inconsistent(error.to_string()))?;
    joiner_commitment
        .validate()
        .map_err(|error| inconsistent(error.to_string()))?;
    if sponsor_commitment != joiner_commitment {
        return Err(inconsistent(
            "joiner security result does not match sponsor candidate",
        ));
    }
    if candidate_event.lineage_id != sponsor_commitment.lineage_id
        || candidate_event.parent_event_id != sponsor_commitment.base_history_position.event_id
        || candidate_event.parent_depth
            != sponsor_commitment
                .base_history_position
                .depth
                .saturating_add(1)
        || candidate_event.admission_bundle_digest
            != Some(sponsor_commitment.admission_bundle_digest)
    {
        return Err(inconsistent(
            "candidate event does not match its base history and security result",
        ));
    }
    let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
        return Err(inconsistent("candidate event is not an AddDevice"));
    };
    if admission.security_commitment_id != sponsor_commitment.security_commitment_id {
        return Err(inconsistent(
            "candidate event does not bind the verified security result",
        ));
    }
    match history.verify_and_receive_event(candidate_event.clone(), verifier) {
        Ok(MembershipHistoryV2ReceiveOutcome::Applied) => Ok(history),
        Ok(MembershipHistoryV2ReceiveOutcome::AlreadyKnown) => Err(inconsistent(
            "candidate event already exists in the base history",
        )),
        Ok(MembershipHistoryV2ReceiveOutcome::Diverged) => Err(inconsistent(
            "candidate event does not extend the supplied base history",
        )),
        Err(error) => Err(inconsistent(error.to_string())),
    }
}

// The versioned channel calls these transitions in the protocol-integration
// stage. Until then production startup uses only `recoverable`.
#[cfg_attr(not(test), allow(dead_code))]
impl DurableAdmissionTransaction {
    pub(crate) fn new(
        repository: Arc<dyn AdmissionAttemptRepositoryPort>,
        history_verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        security_transition: Arc<dyn AdmissionSecurityTransitionPort>,
        space_transition: Arc<dyn AdmissionSpaceTransitionPort>,
    ) -> Self {
        Self {
            repository,
            history_verifier,
            security_transition,
            space_transition,
        }
    }

    pub(crate) async fn start_join(
        &self,
        attempt_id: AdmissionAttemptId,
        join_id: [u8; 16],
        sponsor: &[u8],
        request_payload: &[u8],
        pending_security_state: &[u8],
        candidate_key_package: &[u8],
        target_access_state: &[u8],
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        if let Some(existing) = self.load(attempt_id).await? {
            return self.match_existing_start(
                existing,
                join_id,
                sponsor,
                request_payload,
                pending_security_state,
                candidate_key_package,
                target_access_state,
            );
        }

        let metadata = self
            .repository
            .profile_metadata()
            .await
            .map_err(map_repository_error)?;
        let mut attempt =
            AdmissionAttemptV1::new_joiner(attempt_id, join_id, JoinerAdmissionStageV1::Initiated);
        attempt.local_join_ordinal = Some(metadata.next_local_join_ordinal);
        attempt.joiner_pending_security_state = Some(pending_security_state.to_vec());
        attempt.candidate_key_package = Some(candidate_key_package.to_vec());
        attempt.target_access_state = Some(target_access_state.to_vec());
        attempt.outboxes.push(outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::JoinRequest,
            sponsor,
            None,
            request_payload,
        ));

        match self.repository.create(&attempt, None, None).await {
            Ok(_) => Ok(attempt),
            Err(AdmissionAttemptRepositoryError::AlreadyExists) => {
                let existing = self.load(attempt_id).await?.ok_or_else(|| {
                    admission_storage("admission start disappeared after conflict")
                })?;
                self.match_existing_start(
                    existing,
                    join_id,
                    sponsor,
                    request_payload,
                    pending_security_state,
                    candidate_key_package,
                    target_access_state,
                )
            }
            Err(AdmissionAttemptRepositoryError::VersionConflict) => {
                Err(WorkspaceConvergenceError::AdmissionInProgress)
            }
            Err(error) => Err(map_repository_error(error)),
        }
    }

    pub(crate) async fn sponsor_accept_and_offer(
        &self,
        attempt_id: AdmissionAttemptId,
        invitation_digest: [u8; 32],
        request: &AdmissionOutboxMessageV1,
        candidate: DurableAdmissionCandidateV1,
        base_history: VersionedMembershipHistory,
        candidate_event: &MembershipEventV2,
        sponsor_commitment: &AdmissionSecurityCommitmentV1,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if sponsor_commitment.attempt_id != *attempt_id.as_bytes() {
            return Err(inconsistent(
                "candidate security result is bound to another attempt",
            ));
        }
        require_message(
            attempt_id,
            request,
            AdmissionOutboxPurposeV1::JoinRequest,
            None,
        )?;
        let encoded_base_history = base_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let verified_history = verify_candidate_preparation(
            base_history,
            candidate_event,
            sponsor_commitment,
            sponsor_commitment,
            self.history_verifier.as_ref(),
        )?;
        require_candidate_encoding(
            &candidate,
            candidate_event,
            sponsor_commitment,
            &verified_history,
        )?;
        let encoded_history = verified_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let candidate_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Candidate,
            recipient,
            Some(request.message_id),
            payload,
        );
        let invitation_consume = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::InvitationConsume,
            b"pairing-invitation-service",
            Some(request.message_id),
            &invitation_digest,
        );
        if let Some(existing) = self.load(attempt_id).await? {
            if candidate_matches(&existing, &candidate, true)
                && existing.invitation_claim.as_deref() == Some(invitation_digest.as_slice())
                && existing
                    .inbox_dedup
                    .iter()
                    .any(|record| *record == inbox_record(request))
                && existing.verified_membership_history.as_deref()
                    == Some(encoded_history.as_slice())
                && existing.base_membership_history.as_deref()
                    == Some(encoded_base_history.as_slice())
                && existing.outboxes.contains(&candidate_message)
                && existing.outboxes.contains(&invitation_consume)
            {
                return Ok(candidate_message);
            }
            return Err(inconsistent("sponsor admission replay does not match"));
        }
        let mut attempt = sponsor_candidate_attempt(
            attempt_id,
            invitation_digest,
            candidate,
            encoded_base_history.clone(),
            encoded_history,
        );
        attempt.inbox_dedup.push(inbox_record(request));
        attempt.outboxes.push(candidate_message.clone());
        attempt.outboxes.push(invitation_consume);
        self.repository
            .create(
                &attempt,
                Some(invitation_digest),
                Some(&encoded_base_history),
            )
            .await
            .map_err(map_repository_error)?;
        Ok(candidate_message)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn joiner_verify_and_prepare(
        &self,
        attempt_id: AdmissionAttemptId,
        candidate_message: &AdmissionOutboxMessageV1,
        candidate: DurableAdmissionCandidateV1,
        base_history: VersionedMembershipHistory,
        candidate_event: &MembershipEventV2,
        sponsor_commitment: &AdmissionSecurityCommitmentV1,
        prepared_proof: &[u8],
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if sponsor_commitment.attempt_id != *attempt_id.as_bytes() {
            return Err(inconsistent(
                "candidate security result is bound to another attempt",
            ));
        }
        let attempt = self.required_attempt(attempt_id).await?;
        let pending_state = attempt
            .joiner_pending_security_state
            .as_deref()
            .ok_or_else(|| inconsistent("joiner pending security state is missing"))?;
        let key_package = attempt
            .candidate_key_package
            .as_deref()
            .ok_or_else(|| inconsistent("joiner key package is missing"))?;
        if key_package != candidate.candidate_key_package {
            return Err(inconsistent(
                "candidate key package does not match the saved join material",
            ));
        }
        let transition_input = AdmissionSecurityTransitionInput {
            attempt_id: *attempt_id.as_bytes(),
            base_history_position: sponsor_commitment.base_history_position.clone(),
            candidate_core_digest: sponsor_commitment.candidate_core_digest,
            key_catalog_digest: sponsor_commitment.key_catalog_digest,
            admission_bundle_digest: sponsor_commitment.admission_bundle_digest,
        };
        let staged = self
            .security_transition
            .stage_joiner(
                pending_state,
                key_package,
                &sponsor_commitment.mls_group_id,
                &candidate.security_welcome,
                &candidate.security_commit,
                &transition_input,
            )
            .map_err(|error| inconsistent(error.to_string()))?;
        let encoded_base_history = base_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let current_history = self
            .repository
            .load_membership_history_v2()
            .await
            .map_err(map_repository_error)?;
        if current_history
            .as_deref()
            .is_some_and(|history| history != encoded_base_history.as_slice())
        {
            return Err(inconsistent(
                "joiner current history does not match the candidate base history",
            ));
        }
        let verified_history = verify_candidate_preparation(
            base_history,
            candidate_event,
            sponsor_commitment,
            &staged.public_commitment,
            self.history_verifier.as_ref(),
        )?;
        require_candidate_encoding(
            &candidate,
            candidate_event,
            sponsor_commitment,
            &verified_history,
        )?;
        let encoded_history = verified_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let mut attempt = attempt;
        let prepared_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Prepared,
            recipient,
            Some(candidate_message.message_id),
            payload,
        );
        if attempt.stage_rank().is_some_and(|rank| rank >= 3)
            && candidate_matches(&attempt, &candidate, false)
            && attempt.prepared_proof.as_deref() == Some(prepared_proof)
            && attempt.staged_security_state.as_deref() == Some(staged.staged_state.as_slice())
            && attempt.verified_membership_history.as_deref() == Some(encoded_history.as_slice())
            && attempt.base_membership_history.as_deref() == Some(encoded_base_history.as_slice())
            && attempt.outboxes.contains(&prepared_message)
        {
            return Ok(prepared_message);
        }
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Initiated)?;
        let request_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::JoinRequest)?;
        require_message(
            attempt_id,
            candidate_message,
            AdmissionOutboxPurposeV1::Candidate,
            Some(request_id),
        )?;
        let transition = self
            .space_transition
            .prepare_if_needed(&AdmissionSpaceTransitionPreparationV2 {
                attempt_id,
                target_space_id: candidate.lineage_id.clone(),
                target_security_commitment: sponsor_commitment.clone(),
                target_membership_history: encoded_history.clone(),
                target_security_state: staged.staged_state.clone(),
                target_protection_group_id: candidate.target_protection_group_id.clone(),
                target_key_catalog: candidate.target_key_catalog.clone(),
                local_device_id: match &candidate_event.operation {
                    MembershipOperationV2::AddDevice { admission } => {
                        admission.facts.device_id.clone()
                    }
                    MembershipOperationV2::RemoveDevice { .. } => {
                        return Err(inconsistent("admission candidate is not AddDevice"));
                    }
                },
                target_relationships: candidate.target_relationships.clone(),
                target_access_state: attempt
                    .target_access_state
                    .clone()
                    .ok_or_else(|| inconsistent("target access state is missing"))?,
            })
            .await
            .map_err(map_space_transition_error)?;
        if transition.attempt_id() != attempt_id
            || transition.target_space_id() != candidate.lineage_id
            || !transition.is_initial()
        {
            return Err(inconsistent(
                "prepared space transition does not match the admission candidate",
            ));
        }
        let encoded_transition = transition
            .encode()
            .ok_or_else(|| inconsistent("prepared space transition is invalid"))?;
        apply_candidate(&mut attempt, candidate);
        attempt.space_transition = Some(encoded_transition);
        attempt.staged_security_state = Some(staged.staged_state);
        attempt.base_membership_history = Some(encoded_base_history.clone());
        attempt.verified_membership_history = Some(encoded_history);
        attempt.prepared_proof = Some(prepared_proof.to_vec());
        attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Prepared,
        });
        accept_incoming(
            &mut attempt,
            candidate_message,
            &[AdmissionOutboxPurposeV1::JoinRequest],
        );
        attempt.outboxes.push(prepared_message.clone());
        self.persist_advance_with_history(
            attempt,
            current_history.as_deref(),
            &encoded_base_history,
        )
        .await?;
        Ok(prepared_message)
    }

    pub(crate) async fn sponsor_commit(
        &self,
        attempt_id: AdmissionAttemptId,
        prepared_message: &AdmissionOutboxMessageV1,
        prepared_proof: &[u8],
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let commit_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Commit,
            recipient,
            Some(prepared_message.message_id),
            payload,
        );
        if attempt.stage_rank().is_some_and(|rank| rank >= 4)
            && attempt.prepared_proof.as_deref() == Some(prepared_proof)
            && attempt.outboxes.contains(&commit_message)
        {
            return Ok(commit_message);
        }
        if matches!(
            attempt.role_state,
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected
            })
        ) && attempt.rejection_reason == Some(AdmissionRejectionReasonV1::BaseHistoryChanged)
        {
            return active_outbox(&attempt, AdmissionOutboxPurposeV1::Rejected);
        }
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Candidate)?;
        let candidate_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Candidate)?;
        require_message(
            attempt_id,
            prepared_message,
            AdmissionOutboxPurposeV1::Prepared,
            Some(candidate_id),
        )?;
        let base_history = attempt
            .base_membership_history
            .clone()
            .ok_or_else(|| inconsistent("candidate base membership history is missing"))?;
        let current_history = self
            .repository
            .load_membership_history_v2()
            .await
            .map_err(map_repository_error)?;
        if current_history.as_deref() != Some(base_history.as_slice()) {
            return self
                .reject_base_history_changed(attempt, prepared_message, prepared_proof, recipient)
                .await;
        }
        attempt.prepared_proof = Some(prepared_proof.to_vec());
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Committed,
        });
        accept_incoming(
            &mut attempt,
            prepared_message,
            &[AdmissionOutboxPurposeV1::Candidate],
        );
        attempt.outboxes.push(commit_message.clone());
        let history = attempt
            .verified_membership_history
            .clone()
            .ok_or_else(|| inconsistent("committed membership history is missing"))?;
        match self
            .persist_advance_with_history(attempt, Some(&base_history), &history)
            .await
        {
            Ok(()) => Ok(commit_message),
            Err(WorkspaceConvergenceError::AdmissionInProgress) => {
                let current_attempt = self.required_attempt(attempt_id).await?;
                if matches!(
                    current_attempt.role_state,
                    AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                        stage: SponsorAdmissionStageV1::Rejected
                    })
                ) {
                    return active_outbox(&current_attempt, AdmissionOutboxPurposeV1::Rejected);
                }
                if current_attempt.stage_rank().is_some_and(|rank| rank >= 4)
                    && current_attempt.prepared_proof.as_deref() == Some(prepared_proof)
                {
                    return active_outbox(&current_attempt, AdmissionOutboxPurposeV1::Commit);
                }
                require_sponsor_stage(&current_attempt, SponsorAdmissionStageV1::Candidate)?;
                let current_history = self
                    .repository
                    .load_membership_history_v2()
                    .await
                    .map_err(map_repository_error)?;
                if current_history.as_deref() != Some(base_history.as_slice()) {
                    self.reject_base_history_changed(
                        current_attempt,
                        prepared_message,
                        prepared_proof,
                        recipient,
                    )
                    .await
                } else {
                    Err(WorkspaceConvergenceError::AdmissionInProgress)
                }
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn request_cancel(
        &self,
        attempt_id: AdmissionAttemptId,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        if !attempt.is_joiner() || attempt.is_terminal() {
            return Err(inconsistent("only a pending local join can be cancelled"));
        }
        if let Some(existing) = attempt.outboxes.iter().find(|message| {
            message.purpose == AdmissionOutboxPurposeV1::CancelRequested && !message.superseded
        }) {
            return Ok(existing.clone());
        }
        let predecessor = attempt
            .outboxes
            .iter()
            .rev()
            .find(|message| !message.superseded)
            .map(|message| message.message_id);
        let cancel = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::CancelRequested,
            recipient,
            predecessor,
            payload,
        );
        attempt.cancel_request = Some(payload.to_vec());
        attempt.outboxes.push(cancel.clone());
        self.persist_advance(attempt).await?;
        Ok(cancel)
    }

    pub(crate) async fn sponsor_remove_pending_member(
        &self,
        attempt_id: AdmissionAttemptId,
        removal_event: &MembershipEventV2,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<PendingMemberRemovalOutcomeV1, WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let stage = match attempt.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 { stage }) => stage,
            _ => return Err(inconsistent("only a sponsor can remove a pending member")),
        };
        if matches!(
            stage,
            SponsorAdmissionStageV1::Applied | SponsorAdmissionStageV1::Completed
        ) {
            return Ok(PendingMemberRemovalOutcomeV1::OrdinaryMemberRemovalRequired);
        }
        if stage == SponsorAdmissionStageV1::Rejected
            && attempt.rejection_reason == Some(AdmissionRejectionReasonV1::RemovedBeforeActivation)
        {
            return active_outbox(&attempt, AdmissionOutboxPurposeV1::Rejected)
                .map(PendingMemberRemovalOutcomeV1::AdmissionRejected);
        }

        let candidate_event: MembershipEventV2 = postcard::from_bytes(
            attempt
                .candidate_event
                .as_deref()
                .ok_or_else(|| inconsistent("pending candidate event is missing"))?,
        )
        .map_err(admission_storage)?;
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(inconsistent("pending candidate is not an AddDevice"));
        };
        if removal_event.parent_event_id != Some(candidate_event.event_id())
            || !matches!(
                removal_event.operation,
                MembershipOperationV2::RemoveDevice { member }
                    if member == admission.facts.member_instance
            )
        {
            return Err(inconsistent(
                "pending-member removal does not match the admission candidate",
            ));
        }
        let encoded_candidate_history = attempt
            .verified_membership_history
            .clone()
            .ok_or_else(|| inconsistent("verified candidate history is missing"))?;
        let mut removed_history = VersionedMembershipHistory::decode_persisted_v2(
            &encoded_candidate_history,
            self.history_verifier.as_ref(),
        )
        .map_err(|error| inconsistent(error.to_string()))?;
        match removed_history
            .verify_and_receive_event(removal_event.clone(), self.history_verifier.as_ref())
            .map_err(|error| inconsistent(error.to_string()))?
        {
            MembershipHistoryV2ReceiveOutcome::Applied => {}
            MembershipHistoryV2ReceiveOutcome::AlreadyKnown
            | MembershipHistoryV2ReceiveOutcome::Diverged => {
                return Err(inconsistent(
                    "pending-member removal does not extend the candidate history",
                ));
            }
        }
        let encoded_removed_history = removed_history
            .encode_persisted_v2()
            .map_err(|error| inconsistent(error.to_string()))?;
        let predecessor = attempt
            .outboxes
            .iter()
            .rev()
            .find(|message| {
                !message.superseded
                    && matches!(
                        message.purpose,
                        AdmissionOutboxPurposeV1::Candidate | AdmissionOutboxPurposeV1::Commit
                    )
            })
            .map(|message| message.message_id)
            .ok_or_else(|| inconsistent("pending-member removal predecessor is missing"))?;
        for message in &mut attempt.outboxes {
            if matches!(
                message.purpose,
                AdmissionOutboxPurposeV1::Candidate
                    | AdmissionOutboxPurposeV1::Prepared
                    | AdmissionOutboxPurposeV1::Commit
                    | AdmissionOutboxPurposeV1::Applied
            ) {
                message.superseded = true;
            }
        }
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(AdmissionRejectionReasonV1::RemovedBeforeActivation);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Rejected,
        });
        let rejected = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Rejected,
            recipient,
            Some(predecessor),
            &encode_rejection_payload(
                AdmissionRejectionReasonV1::RemovedBeforeActivation,
                payload,
            )?,
        );
        attempt.outboxes.push(rejected.clone());

        if stage == SponsorAdmissionStageV1::Committed {
            self.persist_advance_with_history(
                attempt,
                Some(&encoded_candidate_history),
                &encoded_removed_history,
            )
            .await?;
        } else {
            self.persist_advance(attempt).await?;
        }
        Ok(PendingMemberRemovalOutcomeV1::AdmissionRejected(rejected))
    }

    pub(crate) async fn sponsor_reject_before_commit(
        &self,
        attempt_id: AdmissionAttemptId,
        reason: AdmissionRejectionReasonV1,
        recipient: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if !matches!(
            reason,
            AdmissionRejectionReasonV1::InvitationUnavailable
                | AdmissionRejectionReasonV1::AuthenticationRejected
                | AdmissionRejectionReasonV1::IdentityConflict
                | AdmissionRejectionReasonV1::JoinerHistoryAhead
                | AdmissionRejectionReasonV1::HistoryConflict
                | AdmissionRejectionReasonV1::PeerUpgradeRequired
        ) {
            return Err(inconsistent(
                "rejection reason requires its dedicated admission transition",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        if matches!(
            attempt.role_state,
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected
            })
        ) && attempt.rejection_reason == Some(reason)
        {
            return active_outbox(&attempt, AdmissionOutboxPurposeV1::Rejected);
        }
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Candidate)?;
        let predecessor = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Candidate)?;
        let payload = encode_rejection_payload(reason, &[])?;
        let rejected = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Rejected,
            recipient,
            Some(predecessor),
            &payload,
        );
        for message in &mut attempt.outboxes {
            if matches!(
                message.purpose,
                AdmissionOutboxPurposeV1::Candidate | AdmissionOutboxPurposeV1::Prepared
            ) {
                message.superseded = true;
            }
        }
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(reason);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Rejected,
        });
        attempt.outboxes.push(rejected.clone());
        self.persist_advance(attempt).await?;
        Ok(rejected)
    }

    pub(crate) async fn record_admission_unavailable(
        &self,
        attempt_id: AdmissionAttemptId,
        join_request: &AdmissionOutboxMessageV1,
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        let attempt = self.required_attempt(attempt_id).await?;
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Initiated)?;
        let request = attempt
            .outboxes
            .iter()
            .find(|message| {
                message.purpose == AdmissionOutboxPurposeV1::JoinRequest && !message.superseded
            })
            .ok_or_else(|| inconsistent("pending join request outbox is missing"))?;
        if request != join_request {
            return Err(inconsistent(
                "admission unavailable does not match the pending join request",
            ));
        }
        Ok(request.clone())
    }

    pub(crate) async fn record_invitation_consume_result(
        &self,
        attempt_id: AdmissionAttemptId,
        result: InvitationConsumeResultV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let Some(index) = attempt.outboxes.iter().position(|message| {
            message.purpose == AdmissionOutboxPurposeV1::InvitationConsume && !message.superseded
        }) else {
            return Ok(());
        };
        match result {
            InvitationConsumeResultV1::Retryable => Ok(()),
            InvitationConsumeResultV1::Consumed
            | InvitationConsumeResultV1::NotFound
            | InvitationConsumeResultV1::Conflict => {
                attempt.outboxes[index].superseded = true;
                self.persist_advance(attempt).await
            }
        }
    }

    pub(crate) async fn acknowledge_delivery(
        &self,
        attempt_id: AdmissionAttemptId,
        acknowledgment: &AdmissionInboxRecordV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let index = attempt
            .outboxes
            .iter()
            .position(|message| {
                !message.superseded
                    && matches!(
                        message.purpose,
                        AdmissionOutboxPurposeV1::JoinRequest
                            | AdmissionOutboxPurposeV1::Candidate
                            | AdmissionOutboxPurposeV1::Prepared
                            | AdmissionOutboxPurposeV1::Commit
                            | AdmissionOutboxPurposeV1::Applied
                    )
                    && admission_acknowledgment(message) == *acknowledgment
            })
            .ok_or_else(|| inconsistent("delivery acknowledgment does not match an outbox"))?;
        attempt.outboxes[index].superseded = true;
        if !attempt.inbox_dedup.contains(acknowledgment) {
            attempt.inbox_dedup.push(acknowledgment.clone());
        }
        self.persist_advance(attempt).await
    }

    pub(crate) async fn enqueue_post_commit_delivery(
        &self,
        attempt_id: AdmissionAttemptId,
        purpose: AdmissionOutboxPurposeV1,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if !matches!(
            purpose,
            AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate
                | AdmissionOutboxPurposeV1::HistoryOrReceiptBatch
        ) {
            return Err(inconsistent("outbox is not a post-commit delivery"));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Applied)?;
        let predecessor = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Complete)?;
        let message = outbound_message(attempt_id, purpose, recipient, Some(predecessor), payload);
        if attempt.outboxes.contains(&message) {
            return Ok(message);
        }
        attempt.outboxes.push(message.clone());
        self.persist_advance(attempt).await?;
        Ok(message)
    }

    pub(crate) async fn acknowledge_persisted_delivery(
        &self,
        attempt_id: AdmissionAttemptId,
        purpose: AdmissionOutboxPurposeV1,
        acknowledgment: &AdmissionInboxRecordV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        if !matches!(
            purpose,
            AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate
                | AdmissionOutboxPurposeV1::HistoryOrReceiptBatch
        ) {
            return Err(inconsistent(
                "acknowledgment is not persisted-delivery evidence",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        let index = attempt
            .outboxes
            .iter()
            .position(|message| {
                message.purpose == purpose
                    && !message.superseded
                    && admission_acknowledgment(message) == *acknowledgment
            })
            .ok_or_else(|| inconsistent("persisted-delivery evidence does not match an outbox"))?;
        attempt.outboxes[index].superseded = true;
        if !attempt.inbox_dedup.contains(acknowledgment) {
            attempt.inbox_dedup.push(acknowledgment.clone());
        }
        self.persist_advance(attempt).await
    }

    pub(crate) async fn sponsor_decide_cancel(
        &self,
        attempt_id: AdmissionAttemptId,
        cancel: &AdmissionOutboxMessageV1,
        recipient: &[u8],
        rejected_payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        require_message(
            attempt_id,
            cancel,
            AdmissionOutboxPurposeV1::CancelRequested,
            cancel.predecessor_message_id,
        )?;
        let mut attempt = self.required_attempt(attempt_id).await?;
        let stage = match attempt.role_state {
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 { stage }) => stage,
            _ => return Err(inconsistent("only a sponsor can decide cancellation")),
        };
        match stage {
            SponsorAdmissionStageV1::Accepted
            | SponsorAdmissionStageV1::Candidate
            | SponsorAdmissionStageV1::Prepared => {
                accept_incoming(
                    &mut attempt,
                    cancel,
                    &[
                        AdmissionOutboxPurposeV1::Candidate,
                        AdmissionOutboxPurposeV1::Prepared,
                    ],
                );
                attempt.cancel_request = Some(cancel.payload.clone());
                attempt.cancel_outcome = Some(b"cancelled".to_vec());
                attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
                attempt.rejection_reason = Some(AdmissionRejectionReasonV1::Cancelled);
                attempt.role_state =
                    AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                        stage: SponsorAdmissionStageV1::Rejected,
                    });
                let rejected = outbound_message(
                    attempt_id,
                    AdmissionOutboxPurposeV1::Rejected,
                    recipient,
                    Some(cancel.message_id),
                    &encode_rejection_payload(
                        AdmissionRejectionReasonV1::Cancelled,
                        rejected_payload,
                    )?,
                );
                attempt.outboxes.push(rejected.clone());
                self.persist_advance(attempt).await?;
                Ok(rejected)
            }
            SponsorAdmissionStageV1::Committed | SponsorAdmissionStageV1::Applied => {
                accept_incoming(&mut attempt, cancel, &[]);
                attempt.cancel_request = Some(cancel.payload.clone());
                attempt.cancel_outcome = Some(b"too_late_committed".to_vec());
                let committed = attempt
                    .outboxes
                    .iter()
                    .find(|message| {
                        matches!(
                            message.purpose,
                            AdmissionOutboxPurposeV1::Commit | AdmissionOutboxPurposeV1::Complete
                        ) && !message.superseded
                    })
                    .cloned()
                    .ok_or_else(|| inconsistent("committed admission outbox is missing"))?;
                self.persist_advance(attempt).await?;
                Ok(committed)
            }
            SponsorAdmissionStageV1::Completed | SponsorAdmissionStageV1::Rejected => {
                Err(inconsistent("admission is already terminal"))
            }
        }
    }

    pub(crate) async fn joiner_record_rejected(
        &self,
        attempt_id: AdmissionAttemptId,
        rejected: &AdmissionOutboxMessageV1,
    ) -> Result<AdmissionInboxRecordV1, WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        let acknowledgment = admission_acknowledgment(rejected);
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::Rejected)
            && attempt.inbox_dedup.contains(&acknowledgment)
        {
            return Ok(acknowledgment);
        }
        if !attempt.is_joiner() || attempt.is_terminal() {
            return Err(inconsistent("only a pending join can receive rejection"));
        }
        let predecessor = rejected
            .predecessor_message_id
            .ok_or_else(|| inconsistent("rejection predecessor is missing"))?;
        if !attempt
            .outboxes
            .iter()
            .any(|message| message.message_id == predecessor && !message.superseded)
        {
            return Err(inconsistent("rejection does not match pending join work"));
        }
        require_message(
            attempt_id,
            rejected,
            AdmissionOutboxPurposeV1::Rejected,
            Some(predecessor),
        )?;
        let rejection_reason = decode_rejection_reason(&rejected.payload)?;
        if let Some(encoded_transition) = attempt.space_transition.as_deref() {
            let transition = AdmissionSpaceTransitionV2::decode(encoded_transition)
                .ok_or_else(|| inconsistent("saved space transition is invalid"))?;
            if transition.phase_rank() >= transition.activation_started_rank() {
                return Err(inconsistent(
                    "committed space transition cannot be rejected or rolled back",
                ));
            }
            self.space_transition
                .discard_pre_activation(&transition)
                .await
                .map_err(map_space_transition_error)?;
            attempt.space_transition = None;
            attempt.target_access_state = None;
        }
        if let Some(staged) = attempt.staged_security_state.take() {
            self.security_transition.discard(staged);
        }
        if let Some(pending) = attempt.joiner_pending_security_state.take() {
            self.security_transition.discard(pending);
        }
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(rejection_reason);
        attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Rejected,
        });
        for message in &mut attempt.outboxes {
            message.superseded = true;
        }
        if !attempt.inbox_dedup.contains(&acknowledgment) {
            attempt.inbox_dedup.push(acknowledgment.clone());
        }
        self.persist_advance(attempt).await?;
        Ok(acknowledgment)
    }

    pub(crate) async fn sponsor_confirm_rejected(
        &self,
        attempt_id: AdmissionAttemptId,
        rejected_ack: &AdmissionInboxRecordV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        if !matches!(
            attempt.role_state,
            AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
                stage: SponsorAdmissionStageV1::Rejected
            })
        ) || attempt.terminal_result != Some(AdmissionTerminalResultV1::Rejected)
        {
            return Err(inconsistent("sponsor admission is not rejected"));
        }
        if attempt.inbox_dedup.contains(rejected_ack) {
            return Ok(());
        }
        let rejected_index = attempt
            .outboxes
            .iter()
            .position(|message| {
                message.purpose == AdmissionOutboxPurposeV1::Rejected
                    && !message.superseded
                    && admission_acknowledgment(message) == *rejected_ack
            })
            .ok_or_else(|| inconsistent("rejected acknowledgment does not match"))?;
        attempt.outboxes[rejected_index].superseded = true;
        attempt.inbox_dedup.push(rejected_ack.clone());
        self.persist_advance(attempt).await
    }

    pub(crate) async fn joiner_apply(
        &self,
        attempt_id: AdmissionAttemptId,
        commit_message: &AdmissionOutboxMessageV1,
        activation_receipt: &AdmissionActivationReceipt,
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if activation_receipt.attempt_id != *attempt_id.as_bytes() {
            return Err(inconsistent(
                "activation receipt is bound to another attempt",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        let encoded_receipt = postcard::to_stdvec(activation_receipt).map_err(admission_storage)?;
        let applied_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Applied,
            recipient,
            Some(commit_message.message_id),
            payload,
        );
        if attempt.stage_rank().is_some_and(|rank| rank >= 5)
            && attempt.activation_receipt.as_deref() == Some(encoded_receipt.as_slice())
            && attempt.outboxes.contains(&applied_message)
        {
            return Ok(applied_message);
        }
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Prepared)?;
        let prepared_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Prepared)?;
        require_message(
            attempt_id,
            commit_message,
            AdmissionOutboxPurposeV1::Commit,
            Some(prepared_id),
        )?;
        let staged_history = attempt
            .verified_membership_history
            .as_deref()
            .ok_or_else(|| inconsistent("verified joiner history is missing"))?;
        let encoded_history = record_activation_receipt(
            staged_history,
            activation_receipt,
            self.history_verifier.as_ref(),
        )?;
        attempt.activation_receipt = Some(encoded_receipt);
        attempt.verified_membership_history = Some(encoded_history.clone());
        attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
            stage: JoinerAdmissionStageV1::Applied,
        });
        accept_incoming(
            &mut attempt,
            commit_message,
            &[
                AdmissionOutboxPurposeV1::Prepared,
                AdmissionOutboxPurposeV1::CancelRequested,
            ],
        );
        if attempt.cancel_request.is_some() {
            attempt.cancel_outcome = Some(b"too_late_committed".to_vec());
        }
        attempt.outboxes.push(applied_message.clone());
        let base_history = attempt
            .base_membership_history
            .clone()
            .ok_or_else(|| inconsistent("joiner base membership history is missing"))?;
        self.persist_advance_with_history(attempt, Some(&base_history), &encoded_history)
            .await?;
        Ok(applied_message)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn sponsor_complete(
        &self,
        attempt_id: AdmissionAttemptId,
        applied_message: &AdmissionOutboxMessageV1,
        activation_receipt: &AdmissionActivationReceipt,
        completion: &[u8],
        recipient: &[u8],
        payload: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        if activation_receipt.attempt_id != *attempt_id.as_bytes() {
            return Err(inconsistent(
                "activation receipt is bound to another attempt",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        let encoded_receipt = postcard::to_stdvec(activation_receipt).map_err(admission_storage)?;
        let complete_message = outbound_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Complete,
            recipient,
            Some(applied_message.message_id),
            payload,
        );
        if attempt.stage_rank().is_some_and(|rank| rank >= 5)
            && attempt.activation_receipt.as_deref() == Some(encoded_receipt.as_slice())
            && attempt.completion.as_deref() == Some(completion)
            && attempt.outboxes.contains(&complete_message)
        {
            return Ok(complete_message);
        }
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Committed)?;
        let commit_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Commit)?;
        require_message(
            attempt_id,
            applied_message,
            AdmissionOutboxPurposeV1::Applied,
            Some(commit_id),
        )?;
        let committed_history = self
            .repository
            .load_membership_history_v2()
            .await
            .map_err(map_repository_error)?
            .ok_or_else(|| inconsistent("committed sponsor history is missing"))?;
        let encoded_history = record_activation_receipt(
            &committed_history,
            activation_receipt,
            self.history_verifier.as_ref(),
        )?;
        let expected_history = committed_history;
        attempt.activation_receipt = Some(encoded_receipt);
        attempt.verified_membership_history = Some(encoded_history.clone());
        attempt.completion = Some(completion.to_vec());
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Applied,
        });
        accept_incoming(
            &mut attempt,
            applied_message,
            &[AdmissionOutboxPurposeV1::Commit],
        );
        attempt.outboxes.push(complete_message.clone());
        self.persist_advance_with_history(attempt, Some(&expected_history), &encoded_history)
            .await?;
        Ok(complete_message)
    }

    pub(crate) async fn joiner_activate(
        &self,
        attempt_id: AdmissionAttemptId,
        complete_message: &AdmissionOutboxMessageV1,
        completion: &[u8],
    ) -> Result<JoinerActivationOutcomeV1, WorkspaceConvergenceError> {
        let acknowledgment = inbox_record(complete_message);
        if let Some(terminal) = self
            .repository
            .load_terminal(attempt_id)
            .await
            .map_err(map_repository_error)?
        {
            if terminal.terminal_result == AdmissionTerminalResultV1::Active
                && terminal.replay_result == completion
                && terminal.acknowledgment_rebuild.contains(&acknowledgment)
            {
                return Ok(JoinerActivationOutcomeV1::Active(acknowledgment));
            }
            return Err(inconsistent(
                "complete replay does not match compacted admission result",
            ));
        }
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::Active)
            && attempt.completion.as_deref() == Some(completion)
            && attempt.inbox_dedup.contains(&acknowledgment)
        {
            return Ok(JoinerActivationOutcomeV1::Active(acknowledgment));
        }
        require_joiner_stage(&attempt, JoinerAdmissionStageV1::Applied)?;
        if attempt.completion.is_none() {
            let applied_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Applied)?;
            require_message(
                attempt_id,
                complete_message,
                AdmissionOutboxPurposeV1::Complete,
                Some(applied_id),
            )?;
            attempt.completion = Some(completion.to_vec());
            accept_incoming(
                &mut attempt,
                complete_message,
                &[AdmissionOutboxPurposeV1::Applied],
            );
            self.persist_advance(attempt).await?;
            attempt = self.required_attempt(attempt_id).await?;
        } else if attempt.completion.as_deref() != Some(completion)
            || !attempt.inbox_dedup.contains(&acknowledgment)
        {
            return Err(inconsistent("complete replay does not match saved state"));
        }
        if let Some(encoded_transition) = attempt.space_transition.as_deref() {
            let transition = AdmissionSpaceTransitionV2::decode(encoded_transition)
                .ok_or_else(|| inconsistent("saved space transition is invalid"))?;
            if matches!(transition, AdmissionSpaceTransitionV2::CrossSpace(_)) {
                return Ok(JoinerActivationOutcomeV1::SpaceTransitionRequired);
            }
            self.resume_space_transition(attempt).await?;
        } else {
            attempt.terminal_result = Some(AdmissionTerminalResultV1::Active);
            attempt.role_state = AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
                stage: JoinerAdmissionStageV1::Completed,
            });
            self.persist_advance(attempt).await?;
        }
        Ok(JoinerActivationOutcomeV1::Active(acknowledgment))
    }

    async fn resume_space_transition(
        &self,
        mut attempt: AdmissionAttemptV1,
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        if !attempt.is_joiner() || attempt.completion.is_none() {
            return Err(inconsistent(
                "space transition cannot run before joiner Complete is saved",
            ));
        }
        loop {
            let transition = AdmissionSpaceTransitionV2::decode(
                attempt
                    .space_transition
                    .as_deref()
                    .ok_or_else(|| inconsistent("space transition disappeared"))?,
            )
            .ok_or_else(|| inconsistent("saved space transition is invalid"))?;
            match self
                .space_transition
                .advance(&transition)
                .await
                .map_err(map_space_transition_error)?
            {
                AdmissionSpaceTransitionStepV2::Advanced(next) => {
                    if !transition.can_advance_to(&next) {
                        return Err(inconsistent(
                            "space transition adapter skipped or replaced a phase",
                        ));
                    }
                    attempt.space_transition = Some(
                        next.encode()
                            .ok_or_else(|| inconsistent("advanced space transition is invalid"))?,
                    );
                    self.persist_advance(attempt).await?;
                    attempt = self.required_attempt(transition.attempt_id()).await?;
                }
                AdmissionSpaceTransitionStepV2::Finished(result) => {
                    if !result.matches_cleanup_pending(&transition) {
                        return Err(inconsistent(
                            "space transition result does not match cleanup state",
                        ));
                    }
                    attempt.space_transition_result = Some(encode_transition_result(&result)?);
                    attempt.terminal_result = Some(AdmissionTerminalResultV1::Active);
                    attempt.role_state =
                        AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
                            stage: JoinerAdmissionStageV1::Completed,
                        });
                    self.persist_advance(attempt).await?;
                    return self.required_attempt(transition.attempt_id()).await;
                }
            }
        }
    }

    pub(crate) async fn sponsor_confirm_active(
        &self,
        attempt_id: AdmissionAttemptId,
        complete_ack: &AdmissionInboxRecordV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let mut attempt = self.required_attempt(attempt_id).await?;
        if attempt.terminal_result == Some(AdmissionTerminalResultV1::Completed)
            && attempt.inbox_dedup.contains(complete_ack)
        {
            return Ok(());
        }
        require_sponsor_stage(&attempt, SponsorAdmissionStageV1::Applied)?;
        let complete_index = attempt
            .outboxes
            .iter()
            .position(|message| {
                message.purpose == AdmissionOutboxPurposeV1::Complete && !message.superseded
            })
            .ok_or_else(|| inconsistent("complete outbox is missing"))?;
        if inbox_record(&attempt.outboxes[complete_index]) != *complete_ack {
            return Err(inconsistent("complete acknowledgment does not match"));
        }
        attempt.outboxes[complete_index].superseded = true;
        attempt.inbox_dedup.push(complete_ack.clone());
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Completed);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Completed,
        });
        self.persist_advance(attempt).await
    }

    pub(crate) async fn recoverable(
        &self,
    ) -> Result<Vec<AdmissionAttemptV1>, WorkspaceConvergenceError> {
        self.repository
            .scan_recoverable()
            .await
            .map_err(map_repository_error)
    }

    pub(crate) async fn requires_session_transition(
        &self,
    ) -> Result<bool, WorkspaceConvergenceError> {
        Ok(self.recoverable().await?.into_iter().any(|attempt| {
            attempt.is_joiner()
                && attempt.completion.is_some()
                && attempt.space_transition.is_some()
                && attempt.space_transition_result.is_none()
        }))
    }

    pub(crate) async fn recover_space_transitions_after_session_drain(
        &self,
    ) -> Result<usize, WorkspaceConvergenceError> {
        let attempts = self.recoverable().await?;
        let mut finished = 0;
        for attempt in attempts {
            if attempt.is_joiner()
                && attempt.completion.is_some()
                && attempt.space_transition.is_some()
                && attempt.space_transition_result.is_none()
            {
                let attempt_id = attempt.attempt_id;
                self.resume_space_transition(attempt).await?;
                self.compact_if_settled(attempt_id).await?;
                finished += 1;
            }
        }
        Ok(finished)
    }

    pub(crate) async fn recover_with(
        &self,
        delivery: &(impl AdmissionOutboxDeliveryPort + ?Sized),
    ) -> Result<AdmissionRecoveryReportV1, WorkspaceConvergenceError> {
        let attempts = self.recoverable().await?;
        let mut report = AdmissionRecoveryReportV1::default();
        for attempt in attempts {
            for message in attempt
                .outboxes
                .iter()
                .filter(|message| !message.superseded)
            {
                report.deliveries_attempted += 1;
                let Ok(outcome) = delivery.deliver(attempt.attempt_id, message).await else {
                    continue;
                };
                let confirmed = match outcome {
                    AdmissionOutboxDeliveryResultV1::Deferred => false,
                    AdmissionOutboxDeliveryResultV1::InvitationConsume(result) => {
                        if message.purpose != AdmissionOutboxPurposeV1::InvitationConsume {
                            return Err(inconsistent(
                                "invitation result does not match admission outbox purpose",
                            ));
                        }
                        let result = match result {
                            InvitationConsumeDeliveryResultV1::Consumed => {
                                InvitationConsumeResultV1::Consumed
                            }
                            InvitationConsumeDeliveryResultV1::NotFound => {
                                InvitationConsumeResultV1::NotFound
                            }
                            InvitationConsumeDeliveryResultV1::Conflict => {
                                InvitationConsumeResultV1::Conflict
                            }
                        };
                        self.record_invitation_consume_result(attempt.attempt_id, result)
                            .await?;
                        true
                    }
                    AdmissionOutboxDeliveryResultV1::Persisted(acknowledgment) => {
                        match message.purpose {
                            AdmissionOutboxPurposeV1::JoinRequest
                            | AdmissionOutboxPurposeV1::Candidate
                            | AdmissionOutboxPurposeV1::Prepared
                            | AdmissionOutboxPurposeV1::Commit
                            | AdmissionOutboxPurposeV1::Applied => {
                                self.acknowledge_delivery(attempt.attempt_id, &acknowledgment)
                                    .await?;
                            }
                            AdmissionOutboxPurposeV1::Rejected => {
                                self.sponsor_confirm_rejected(attempt.attempt_id, &acknowledgment)
                                    .await?;
                            }
                            AdmissionOutboxPurposeV1::Complete => {
                                self.sponsor_confirm_active(attempt.attempt_id, &acknowledgment)
                                    .await?;
                            }
                            AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate
                            | AdmissionOutboxPurposeV1::HistoryOrReceiptBatch => {
                                self.acknowledge_persisted_delivery(
                                    attempt.attempt_id,
                                    message.purpose,
                                    &acknowledgment,
                                )
                                .await?;
                            }
                            AdmissionOutboxPurposeV1::CancelRequested
                            | AdmissionOutboxPurposeV1::InvitationConsume => {
                                return Err(inconsistent(
                                    "persisted acknowledgment cannot clear this admission outbox",
                                ));
                            }
                        }
                        true
                    }
                };
                if confirmed {
                    report.deliveries_confirmed += 1;
                }
            }
            let Some(current) = self.load(attempt.attempt_id).await? else {
                continue;
            };
            if current.is_terminal()
                && current.outboxes.iter().all(|message| message.superseded)
                && current.write_ahead_recovery.is_none()
                && (current.space_transition.is_none() || current.space_transition_result.is_some())
                && !current.cleanup_pending
            {
                self.compact_if_settled(attempt.attempt_id).await?;
                report.attempts_compacted += 1;
            }
        }
        Ok(report)
    }

    pub(crate) async fn pending_inbound_member(
        &self,
        active_lineage_id: &str,
    ) -> Result<Option<PendingInboundMemberProjectionV1>, WorkspaceConvergenceError> {
        let mut matching = self
            .repository
            .scan_recoverable()
            .await
            .map_err(map_repository_error)?
            .into_iter()
            .filter(|attempt| {
                !attempt.is_terminal()
                    && matches!(attempt.role_state, AdmissionAttemptRoleStateV1::Sponsor(_))
                    && attempt.lineage_id.as_deref() == Some(active_lineage_id)
            });
        let Some(attempt) = matching.next() else {
            return Ok(None);
        };
        if matching.next().is_some() {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let event: MembershipEventV2 = postcard::from_bytes(
            attempt
                .candidate_event
                .as_deref()
                .ok_or_else(|| inconsistent("pending inbound candidate event is missing"))?,
        )
        .map_err(admission_storage)?;
        let MembershipOperationV2::AddDevice { admission } = event.operation else {
            return Err(inconsistent(
                "pending inbound candidate is not an AddDevice",
            ));
        };
        Ok(Some(PendingInboundMemberProjectionV1 {
            device_id: admission.facts.device_id,
            display_name: admission.facts.device_name,
        }))
    }

    pub(crate) async fn reset_join_projection_if_quiet(
        &self,
    ) -> Result<uc_core::membership::AdmissionProfileMetadataV1, WorkspaceConvergenceError> {
        let metadata = self
            .repository
            .profile_metadata()
            .await
            .map_err(map_repository_error)?;
        self.repository
            .advance_projection_floor(metadata.device_trust_revision)
            .await
            .map_err(|error| match error {
                AdmissionAttemptRepositoryError::VersionConflict => {
                    WorkspaceConvergenceError::Unavailable
                }
                other => map_repository_error(other),
            })
    }

    pub(crate) async fn compact_if_settled(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<uc_core::membership::TerminalAdmissionAttemptV1, WorkspaceConvergenceError> {
        if let Some(terminal) = self
            .repository
            .load_terminal(attempt_id)
            .await
            .map_err(map_repository_error)?
        {
            return Ok(terminal);
        }
        let attempt = self.required_attempt(attempt_id).await?;
        if !attempt.is_terminal()
            || attempt.outboxes.iter().any(|message| !message.superseded)
            || attempt.write_ahead_recovery.is_some()
            || (attempt.space_transition.is_some() && attempt.space_transition_result.is_none())
            || attempt.cleanup_pending
        {
            return Err(WorkspaceConvergenceError::AdmissionInProgress);
        }
        self.repository
            .compact_terminal(attempt_id, attempt.record_version)
            .await
            .map_err(map_repository_error)
    }

    async fn load(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<Option<AdmissionAttemptV1>, WorkspaceConvergenceError> {
        self.repository
            .load(attempt_id)
            .await
            .map_err(map_repository_error)
    }

    async fn required_attempt(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        self.load(attempt_id)
            .await?
            .ok_or_else(|| inconsistent("admission attempt was not found"))
    }

    async fn reject_base_history_changed(
        &self,
        mut attempt: AdmissionAttemptV1,
        prepared_message: &AdmissionOutboxMessageV1,
        prepared_proof: &[u8],
        recipient: &[u8],
    ) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
        let candidate_id = active_outbox_id(&attempt, AdmissionOutboxPurposeV1::Candidate)?;
        require_message(
            attempt.attempt_id,
            prepared_message,
            AdmissionOutboxPurposeV1::Prepared,
            Some(candidate_id),
        )?;
        accept_incoming(
            &mut attempt,
            prepared_message,
            &[AdmissionOutboxPurposeV1::Candidate],
        );
        attempt.prepared_proof = Some(prepared_proof.to_vec());
        attempt.terminal_result = Some(AdmissionTerminalResultV1::Rejected);
        attempt.rejection_reason = Some(AdmissionRejectionReasonV1::BaseHistoryChanged);
        attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Rejected,
        });
        let rejected = outbound_message(
            attempt.attempt_id,
            AdmissionOutboxPurposeV1::Rejected,
            recipient,
            Some(prepared_message.message_id),
            &encode_rejection_payload(
                AdmissionRejectionReasonV1::BaseHistoryChanged,
                b"base_history_changed",
            )?,
        );
        attempt.outboxes.push(rejected.clone());
        self.persist_advance(attempt).await?;
        Ok(rejected)
    }

    async fn persist_advance(
        &self,
        mut attempt: AdmissionAttemptV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let expected = attempt.record_version;
        attempt.record_version = attempt
            .record_version
            .checked_add(1)
            .ok_or_else(|| inconsistent("admission record version overflow"))?;
        self.repository
            .compare_and_advance(attempt.attempt_id, expected, &attempt)
            .await
            .map_err(map_repository_error)?;
        Ok(())
    }

    async fn persist_advance_with_history(
        &self,
        mut attempt: AdmissionAttemptV1,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<(), WorkspaceConvergenceError> {
        let expected = attempt.record_version;
        attempt.record_version = attempt
            .record_version
            .checked_add(1)
            .ok_or_else(|| inconsistent("admission record version overflow"))?;
        self.repository
            .compare_and_advance_with_membership_history_v2(
                attempt.attempt_id,
                expected,
                &attempt,
                expected_membership_history_v2,
                membership_history_v2,
            )
            .await
            .map_err(map_repository_error)?;
        Ok(())
    }

    fn match_existing_start(
        &self,
        existing: AdmissionAttemptV1,
        join_id: [u8; 16],
        sponsor: &[u8],
        request_payload: &[u8],
        pending_security_state: &[u8],
        candidate_key_package: &[u8],
        target_access_state: &[u8],
    ) -> Result<AdmissionAttemptV1, WorkspaceConvergenceError> {
        let expected = outbound_message(
            existing.attempt_id,
            AdmissionOutboxPurposeV1::JoinRequest,
            sponsor,
            None,
            request_payload,
        );
        let is_same_start = existing.is_joiner()
            && existing.join_id == Some(join_id)
            && existing.stage_rank() == Some(0)
            && existing.joiner_pending_security_state.as_deref() == Some(pending_security_state)
            && existing.candidate_key_package.as_deref() == Some(candidate_key_package)
            && existing.target_access_state.as_deref() == Some(target_access_state)
            && existing.outboxes.as_slice() == [expected];
        if is_same_start {
            Ok(existing)
        } else {
            Err(admission_storage(
                "attempt identity was reused with different join input",
            ))
        }
    }
}

fn sponsor_candidate_attempt(
    attempt_id: AdmissionAttemptId,
    invitation_digest: [u8; 32],
    candidate: DurableAdmissionCandidateV1,
    base_membership_history: Vec<u8>,
    verified_membership_history: Vec<u8>,
) -> AdmissionAttemptV1 {
    let mut attempt =
        AdmissionAttemptV1::new_joiner(attempt_id, [0; 16], JoinerAdmissionStageV1::Initiated);
    attempt.join_id = None;
    attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
        stage: SponsorAdmissionStageV1::Candidate,
    });
    apply_candidate(&mut attempt, candidate);
    attempt.base_membership_history = Some(base_membership_history);
    attempt.verified_membership_history = Some(verified_membership_history);
    attempt.invitation_claim = Some(invitation_digest.to_vec());
    attempt
}

fn require_candidate_encoding(
    candidate: &DurableAdmissionCandidateV1,
    candidate_event: &MembershipEventV2,
    sponsor_commitment: &AdmissionSecurityCommitmentV1,
    verified_history: &VersionedMembershipHistory,
) -> Result<(), WorkspaceConvergenceError> {
    let encoded_base_position = postcard::to_stdvec(&sponsor_commitment.base_history_position)
        .map_err(admission_storage)?;
    let encoded_candidate_event =
        postcard::to_stdvec(candidate_event).map_err(admission_storage)?;
    let encoded_commitment = postcard::to_stdvec(sponsor_commitment).map_err(admission_storage)?;
    if candidate.lineage_id != candidate_event.lineage_id
        || candidate.base_history_position != encoded_base_position
        || candidate.candidate_event != encoded_candidate_event
        || candidate.candidate_event_id != *candidate_event.event_id().as_bytes()
        || candidate.target_members_digest != candidate_event.resulting_members_digest
        || candidate.security_commitment != encoded_commitment
    {
        return Err(inconsistent(
            "persisted candidate does not match the verified V2 candidate",
        ));
    }
    let catalog = AdmissionContentKeyCatalogV1::decode(&candidate.target_key_catalog)
        .map_err(|error| inconsistent(error.to_string()))?;
    if catalog.target_epoch != sponsor_commitment.target_epoch
        || catalog.digest() != sponsor_commitment.key_catalog_digest
        || candidate.target_protection_group_id.is_empty()
        || candidate.target_protection_group_id.len() > 128
        || !candidate.target_protection_group_id.is_ascii()
    {
        return Err(inconsistent(
            "candidate content-key catalog does not match the security commitment",
        ));
    }
    let mut member_instances = std::collections::BTreeSet::new();
    let mut device_ids = std::collections::BTreeSet::new();
    for facts in &candidate.target_relationships {
        let credential = verified_history
            .credential_for(facts.member_instance)
            .ok_or_else(|| inconsistent("candidate relationship has no history credential"))?;
        if credential.member_instance_id(&facts.device_id) != facts.member_instance
            || !member_instances.insert(facts.member_instance)
            || !device_ids.insert(facts.device_id.clone())
        {
            return Err(inconsistent(
                "candidate relationship projection does not match verified history",
            ));
        }
    }
    if member_instances != verified_history.effective_members()
        || !matches!(
            &candidate_event.operation,
            MembershipOperationV2::AddDevice { admission }
                if candidate.target_relationships.contains(&admission.facts)
        )
    {
        return Err(inconsistent(
            "candidate relationship projection is incomplete",
        ));
    }
    Ok(())
}

fn record_activation_receipt(
    encoded_history: &[u8],
    activation_receipt: &AdmissionActivationReceipt,
    verifier: &(impl HistoricalMembershipSignatureVerifier + ?Sized),
) -> Result<Vec<u8>, WorkspaceConvergenceError> {
    let mut history = VersionedMembershipHistory::decode_persisted_v2(encoded_history, verifier)
        .map_err(|error| inconsistent(error.to_string()))?;
    history
        .verify_and_record_activation_receipt(activation_receipt.clone(), verifier)
        .map_err(|error| inconsistent(error.to_string()))?;
    history
        .encode_persisted_v2()
        .map_err(|error| inconsistent(error.to_string()))
}

fn apply_candidate(attempt: &mut AdmissionAttemptV1, candidate: DurableAdmissionCandidateV1) {
    attempt.lineage_id = Some(candidate.lineage_id);
    attempt.base_history_position = Some(candidate.base_history_position);
    attempt.candidate_event = Some(candidate.candidate_event);
    attempt.candidate_event_id = Some(candidate.candidate_event_id);
    attempt.candidate_key_package = Some(candidate.candidate_key_package);
    attempt.target_members_digest = Some(candidate.target_members_digest);
    attempt.security_commitment = Some(candidate.security_commitment);
    attempt.security_commit = Some(candidate.security_commit);
    attempt.security_welcome = Some(candidate.security_welcome);
    attempt.target_protection_group_id = Some(candidate.target_protection_group_id);
    attempt.target_key_catalog = Some(candidate.target_key_catalog);
    attempt.target_relationships = Some(candidate.target_relationships);
    attempt.staged_security_state = Some(candidate.staged_security_state);
    attempt.identity_binding = Some(candidate.identity_binding);
}

fn candidate_matches(
    attempt: &AdmissionAttemptV1,
    candidate: &DurableAdmissionCandidateV1,
    compare_staged_state: bool,
) -> bool {
    attempt.lineage_id.as_deref() == Some(candidate.lineage_id.as_str())
        && attempt.base_history_position.as_deref()
            == Some(candidate.base_history_position.as_slice())
        && attempt.candidate_event.as_deref() == Some(candidate.candidate_event.as_slice())
        && attempt.candidate_event_id == Some(candidate.candidate_event_id)
        && attempt.candidate_key_package.as_deref()
            == Some(candidate.candidate_key_package.as_slice())
        && attempt.target_members_digest == Some(candidate.target_members_digest)
        && attempt.security_commitment.as_deref() == Some(candidate.security_commitment.as_slice())
        && attempt.security_commit.as_deref() == Some(candidate.security_commit.as_slice())
        && attempt.security_welcome.as_deref() == Some(candidate.security_welcome.as_slice())
        && attempt.target_protection_group_id.as_deref()
            == Some(candidate.target_protection_group_id.as_str())
        && attempt.target_key_catalog.as_deref() == Some(candidate.target_key_catalog.as_slice())
        && attempt.target_relationships.as_deref()
            == Some(candidate.target_relationships.as_slice())
        && (!compare_staged_state
            || attempt.staged_security_state.as_deref()
                == Some(candidate.staged_security_state.as_slice()))
        && attempt.identity_binding.as_deref() == Some(candidate.identity_binding.as_slice())
}

fn require_joiner_stage(
    attempt: &AdmissionAttemptV1,
    expected: JoinerAdmissionStageV1,
) -> Result<(), WorkspaceConvergenceError> {
    if matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 { stage }) if stage == expected
    ) {
        Ok(())
    } else {
        Err(inconsistent("joiner admission message is out of order"))
    }
}

fn require_sponsor_stage(
    attempt: &AdmissionAttemptV1,
    expected: SponsorAdmissionStageV1,
) -> Result<(), WorkspaceConvergenceError> {
    if matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 { stage }) if stage == expected
    ) {
        Ok(())
    } else {
        Err(inconsistent("sponsor admission message is out of order"))
    }
}

fn require_message(
    attempt_id: AdmissionAttemptId,
    message: &AdmissionOutboxMessageV1,
    purpose: AdmissionOutboxPurposeV1,
    predecessor: Option<[u8; 32]>,
) -> Result<(), WorkspaceConvergenceError> {
    let expected = outbound_message(
        attempt_id,
        purpose,
        &message.recipient,
        predecessor,
        &message.payload,
    );
    if *message == expected {
        Ok(())
    } else {
        Err(inconsistent("admission message is out of order"))
    }
}

fn active_outbox_id(
    attempt: &AdmissionAttemptV1,
    purpose: AdmissionOutboxPurposeV1,
) -> Result<[u8; 32], WorkspaceConvergenceError> {
    attempt
        .outboxes
        .iter()
        .find(|message| message.purpose == purpose && !message.superseded)
        .map(|message| message.message_id)
        .ok_or_else(|| inconsistent("required admission outbox is missing"))
}

fn active_outbox(
    attempt: &AdmissionAttemptV1,
    purpose: AdmissionOutboxPurposeV1,
) -> Result<AdmissionOutboxMessageV1, WorkspaceConvergenceError> {
    attempt
        .outboxes
        .iter()
        .find(|message| message.purpose == purpose && !message.superseded)
        .cloned()
        .ok_or_else(|| inconsistent("required admission outbox is missing"))
}

fn accept_incoming(
    attempt: &mut AdmissionAttemptV1,
    incoming: &AdmissionOutboxMessageV1,
    superseded: &[AdmissionOutboxPurposeV1],
) {
    let record = inbox_record(incoming);
    if !attempt
        .inbox_dedup
        .iter()
        .any(|existing| existing.message_id == record.message_id)
    {
        attempt.inbox_dedup.push(record);
    }
    for message in &mut attempt.outboxes {
        if superseded.contains(&message.purpose) {
            message.superseded = true;
        }
    }
}

fn inbox_record(message: &AdmissionOutboxMessageV1) -> AdmissionInboxRecordV1 {
    let payload_digest: [u8; 32] = Sha256::digest(&message.payload).into();
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-message-ack/v1\0");
    hasher.update(message.message_id);
    hasher.update(payload_digest);
    AdmissionInboxRecordV1 {
        message_id: message.message_id,
        payload_digest,
        acknowledgment_payload: hasher.finalize().to_vec(),
    }
}

pub(crate) fn admission_acknowledgment(
    message: &AdmissionOutboxMessageV1,
) -> AdmissionInboxRecordV1 {
    inbox_record(message)
}

fn outbound_message(
    attempt_id: AdmissionAttemptId,
    purpose: AdmissionOutboxPurposeV1,
    recipient: &[u8],
    predecessor_message_id: Option<[u8; 32]>,
    payload: &[u8],
) -> AdmissionOutboxMessageV1 {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-message/v1\0");
    hasher.update(attempt_id.as_bytes());
    hasher.update([purpose as u8]);
    hasher.update(predecessor_message_id.unwrap_or([0; 32]));
    hasher.update((recipient.len() as u64).to_be_bytes());
    hasher.update(recipient);
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(payload);
    AdmissionOutboxMessageV1 {
        purpose,
        recipient: recipient.to_vec(),
        message_id: hasher.finalize().into(),
        predecessor_message_id,
        payload: payload.to_vec(),
        superseded: false,
    }
}

fn encode_rejection_payload(
    reason: AdmissionRejectionReasonV1,
    detail: &[u8],
) -> Result<Vec<u8>, WorkspaceConvergenceError> {
    postcard::to_stdvec(&(reason, detail.to_vec())).map_err(admission_storage)
}

fn decode_rejection_reason(
    payload: &[u8],
) -> Result<AdmissionRejectionReasonV1, WorkspaceConvergenceError> {
    postcard::from_bytes::<(AdmissionRejectionReasonV1, Vec<u8>)>(payload)
        .map(|(reason, _)| reason)
        .map_err(|_| inconsistent("rejection payload is invalid"))
}

fn encode_transition_result(
    result: &AdmissionSpaceTransitionResultV2,
) -> Result<Vec<u8>, WorkspaceConvergenceError> {
    result
        .encode()
        .ok_or_else(|| inconsistent("space transition result cannot be encoded"))
}

fn map_space_transition_error(error: AdmissionSpaceTransitionError) -> WorkspaceConvergenceError {
    match error {
        AdmissionSpaceTransitionError::Locked | AdmissionSpaceTransitionError::Storage => {
            admission_storage(error)
        }
        AdmissionSpaceTransitionError::Unavailable => WorkspaceConvergenceError::Unavailable,
        AdmissionSpaceTransitionError::RecoveryRequired => {
            WorkspaceConvergenceError::RecoveryRequired
        }
        AdmissionSpaceTransitionError::Inconsistent => inconsistent(error.to_string()),
    }
}

fn map_repository_error(error: AdmissionAttemptRepositoryError) -> WorkspaceConvergenceError {
    match error {
        AdmissionAttemptRepositoryError::VersionConflict => {
            WorkspaceConvergenceError::AdmissionInProgress
        }
        other => admission_storage(other),
    }
}

fn admission_storage(error: impl std::fmt::Display) -> WorkspaceConvergenceError {
    WorkspaceConvergenceError::AdmissionStorage(error.to_string())
}

fn inconsistent(message: impl Into<String>) -> WorkspaceConvergenceError {
    WorkspaceConvergenceError::Inconsistent(message.into())
}
