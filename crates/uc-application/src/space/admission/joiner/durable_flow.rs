use crate::space::admission::durable::{
    complete_ack_frame, durable_frame_from_outbox, transaction,
};
use crate::space::admission::*;
use crate::space::workspace_membership::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(in crate::space) enum InvitationConsumeResultV1 {
    Consumed,
    NotFound,
    Conflict,
    Retryable,
}

pub(in crate::space) async fn record_invitation_consume_result(
    repository: &dyn crate::deps::AdmissionAttemptRepositoryPort,
    attempt_id: uc_core::membership::AdmissionAttemptId,
    result: InvitationConsumeResultV1,
) -> Result<(), WorkspaceConvergenceError> {
    use uc_core::membership::AdmissionOutboxPurposeV1;

    let mut attempt = repository
        .load(attempt_id)
        .await
        .map_err(crate::space::admission::durable::map_repository_error)?
        .ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent("admission attempt was not found".into())
        })?;
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
            let expected_version = attempt.record_version;
            attempt.record_version = expected_version.checked_add(1).ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("admission record version overflow".into())
            })?;
            repository
                .compare_and_advance(attempt_id, expected_version, &attempt)
                .await
                .map_err(crate::space::admission::durable::map_repository_error)
        }
    }
}

impl crate::space::admission::SpaceAdmission {
    pub(crate) async fn prepare_joiner_candidate(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        proof_signer: &(dyn crate::deps::GroupAdmissionPort + Send + Sync),
        target_access: &(dyn uc_core::ports::space::PrepareAdmissionTargetAccessPort + Send + Sync),
        passphrase: &uc_core::crypto::domain::Passphrase,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1,
            MembershipEventV2, MembershipOperationV2, VersionedMembershipHistory,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Candidate {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.membership.state_write_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let candidate_message = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Candidate,
            self.membership.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if candidate_message.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let Some(attempt) = self.admission.load(attempt_id).await? else {
            if self.admission.is_compacted_superseded(attempt_id).await? {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            return Err(WorkspaceConvergenceError::JoinNotFound);
        };
        if attempt.terminal_result
            == Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
        {
            self.admission
                .record_superseded_protocol_contradiction(attempt_id, &candidate_message)
                .await?;
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let payload = transaction::DurableAdmissionCandidatePayloadV1::decode(&frame.payload)?;
        let base_history = VersionedMembershipHistory::decode_persisted_v2(
            &payload.base_membership_history,
            self.membership
                .deps
                .historical_membership_signatures
                .as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let candidate_event: MembershipEventV2 =
            postcard::from_bytes(&payload.candidate.candidate_event)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let sponsor_commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(&payload.candidate.security_commitment)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let computed_core = candidate_event
            .admission_candidate_core_digest(
                frame.attempt_id,
                &payload.candidate.candidate_key_package,
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if computed_core != sponsor_commitment.candidate_core_digest {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let target_access_state = target_access
            .prepare_target_access(
                &uc_core::ids::SpaceId::from_string(payload.candidate.lineage_id.clone()),
                passphrase,
            )
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
            .into_bytes();
        let sponsor_device_id = payload
            .candidate
            .target_relationships
            .iter()
            .find(|facts| facts.member_instance == candidate_event.author_member_instance_id)
            .map(|facts| facts.device_id.clone())
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        if admission.facts.device_id != self.membership.deps.own_device {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let prepared = self
            .admission
            .joiner_verify_and_prepare(
                attempt_id,
                &candidate_message,
                payload.candidate,
                base_history,
                &candidate_event,
                &sponsor_commitment,
                &target_access_state,
                &[],
                Some(proof_signer),
                sponsor_device_id.as_str().as_bytes(),
                &[],
            )
            .await?;
        durable_frame_from_outbox(
            attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Prepared,
            AdmissionOutboxPurposeV1::Prepared,
            &prepared,
        )
    }

    pub(crate) async fn apply_joiner_commit(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        receipt_signer: &(dyn crate::deps::GroupAdmissionPort + Send + Sync),
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use crate::deps::AdmissionSecurityTransitionInput;
        use uc_core::membership::{
            AdmissionActivationReceipt, AdmissionAttemptId, AdmissionOutboxPurposeV1,
            AdmissionSecurityCommitmentV1, MembershipEventV2, MembershipOperationV2,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Commit {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.membership.state_write_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let commit = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Commit,
            self.membership.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if commit.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let Some(attempt) = self.admission.load(attempt_id).await? else {
            if self.admission.is_compacted_superseded(attempt_id).await? {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            return Err(WorkspaceConvergenceError::JoinNotFound);
        };
        if attempt.terminal_result
            == Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
        {
            self.admission
                .record_superseded_protocol_contradiction(attempt_id, &commit)
                .await?;
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let commit_payload = transaction::DurableAdmissionCommitPayloadV1::decode(&frame.payload)?;
        let candidate_event: MembershipEventV2 =
            postcard::from_bytes(attempt.candidate_event.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("candidate event is missing".to_owned())
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(attempt.security_commitment.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "candidate security commitment is missing".to_owned(),
                )
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        if commit_payload.candidate_event_id != *candidate_event.event_id().as_bytes()
            || commit_payload.security_commitment_id != commitment.security_commitment_id
            || attempt.prepared_proof.as_deref() != Some(commit_payload.prepared_proof.as_slice())
            || attempt.resume_public_key.as_deref()
                != Some(commit_payload.resume_public_key.as_slice())
            || attempt.existing_member_security_deliveries.as_deref()
                != Some(commit_payload.existing_member_deliveries.as_slice())
            || transaction::completion_recovery_routes(
                attempt.target_relationships.as_deref().ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "completion recovery routes are missing".to_owned(),
                    )
                })?,
            ) != commit_payload.completion_recovery_routes
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let transition_input = AdmissionSecurityTransitionInput {
            attempt_id: frame.attempt_id,
            base_history_position: commitment.base_history_position.clone(),
            candidate_core_digest: commitment.candidate_core_digest,
            key_catalog_digest: commitment.key_catalog_digest,
            admission_bundle_digest: commitment.admission_bundle_digest,
        };
        let rederived = self
            .membership
            .deps
            .admission_security_transition
            .derive_public_commitment(
                attempt.staged_security_state.as_deref().ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "joiner staged security state is missing".to_owned(),
                    )
                })?,
                attempt.security_commit.as_deref().ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent("security commit is missing".to_owned())
                })?,
                &transition_input,
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if rederived != commitment {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        let mut receipt = AdmissionActivationReceipt::new(
            1,
            frame.attempt_id,
            candidate_event.event_id(),
            candidate_event.resulting_members_digest,
            commitment.security_commitment_id,
            admission.facts.member_instance,
            Vec::new(),
        );
        let prepared_join = uc_core::space_access::PreparedGroupJoin::new(
            attempt.candidate_key_package.clone().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "candidate key package is missing".to_owned(),
                )
            })?,
            attempt
                .joiner_pending_security_state
                .clone()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "joiner signing state is missing".to_owned(),
                    )
                })?,
        )
        .with_member_instance(admission.facts.member_instance);
        receipt.signature = receipt_signer
            .sign_prepared_join_payload(&prepared_join, &receipt.signing_payload())
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let applied_payload = postcard::to_stdvec(&receipt)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let sponsor_device_id = attempt
            .target_relationships
            .as_deref()
            .and_then(|relationships| {
                relationships.iter().find(|facts| {
                    facts.member_instance == candidate_event.author_member_instance_id
                })
            })
            .map(|facts| facts.device_id.clone())
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let applied = self
            .admission
            .joiner_apply(
                attempt_id,
                &commit,
                &receipt,
                sponsor_device_id.as_str().as_bytes(),
                &applied_payload,
            )
            .await?;
        durable_frame_from_outbox(
            attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Applied,
            AdmissionOutboxPurposeV1::Applied,
            &applied,
        )
    }

    pub(crate) async fn activate_joiner_complete(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<crate::space::admission::adapter::DurableJoinerCompletion, WorkspaceConvergenceError>
    {
        use uc_core::membership::{
            AdmissionActivationReceipt, AdmissionAttemptId, AdmissionCompletionV1,
            AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1, MembershipEventV2,
            VersionedMembershipHistory,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Complete {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.membership.state_write_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let complete = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Complete,
            self.membership.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if complete.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let Some(attempt) = self.admission.load(attempt_id).await? else {
            if self.admission.is_compacted_superseded(attempt_id).await? {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            return Err(WorkspaceConvergenceError::JoinNotFound);
        };
        if attempt.terminal_result
            == Some(uc_core::membership::AdmissionTerminalResultV1::SupersededByNewJoin)
        {
            self.admission
                .record_superseded_protocol_contradiction(attempt_id, &complete)
                .await?;
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let completion: AdmissionCompletionV1 = postcard::from_bytes(&frame.payload)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let candidate_event: MembershipEventV2 =
            postcard::from_bytes(attempt.candidate_event.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("candidate event is missing".to_owned())
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(attempt.security_commitment.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent(
                    "candidate security commitment is missing".to_owned(),
                )
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let receipt_bytes = attempt.activation_receipt.as_deref().ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent("activation receipt is missing".to_owned())
        })?;
        let _: AdmissionActivationReceipt = postcard::from_bytes(receipt_bytes)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let history = VersionedMembershipHistory::decode_persisted_v2(
            attempt
                .verified_membership_history
                .as_deref()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "verified membership history is missing".to_owned(),
                    )
                })?,
            self.membership
                .deps
                .historical_membership_signatures
                .as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let completer_credential = history
            .credential_for(completion.completed_by_member_instance_id)
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let receipt_digest: [u8; 32] = sha2::Sha256::digest(receipt_bytes).into();
        if completion.completion_format_version
            != uc_core::membership::ADMISSION_COMPLETION_FORMAT_V1
            || completion.attempt_id != frame.attempt_id
            || completion.event_id != candidate_event.event_id()
            || completion.activation_receipt_digest != receipt_digest
            || completion.security_commitment_id != commitment.security_commitment_id
            || completion.completed_by_credential_id != completer_credential.credential_id
            || completion.completed_history_position
                != history
                    .current_position()
                    .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
            || !history
                .active_members()
                .contains(&completion.completed_by_member_instance_id)
            || !self
                .membership
                .deps
                .historical_membership_signatures
                .verify(
                    completer_credential.signature_algorithm_version,
                    &completer_credential.public_key,
                    &completion.signing_payload(),
                    &completion.signature,
                )
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let acknowledgment = match self
            .admission
            .joiner_activate(attempt_id, &complete, &frame.payload)
            .await?
        {
            transaction::JoinerActivationOutcomeV1::Active(acknowledgment) => {
                self.admission.compact_if_settled(attempt_id).await?;
                acknowledgment
            }
            transaction::JoinerActivationOutcomeV1::SpaceTransitionRequired => {
                return Ok(crate::space::admission::adapter::DurableJoinerCompletion::SpaceTransitionRequired);
            }
        };
        let payload = postcard::to_stdvec(&acknowledgment)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        Ok(
            crate::space::admission::adapter::DurableJoinerCompletion::Active(complete_ack_frame(
                attempt_id,
                frame.message_id,
                payload,
            )),
        )
    }

    pub(crate) async fn preflight_local_join_source(
        &self,
        preserve_unreadable_history: bool,
    ) -> Result<(), WorkspaceConvergenceError> {
        self.admission
            .preflight_join_source(preserve_unreadable_history)
            .await
    }

    pub(crate) async fn prepare_local_join_before_network(
        &self,
        preparation: &(dyn crate::deps::GroupAdmissionPort + Send + Sync),
        local_device_id: &DeviceId,
        sponsor: &[u8],
        sponsor_continuation_address: &[u8],
        stable_request_binding: &[u8],
        preserve_unreadable_history: bool,
    ) -> Result<
        crate::space::admission::adapter::DurableLocalJoinPreparation,
        WorkspaceConvergenceError,
    > {
        let _guard = self.membership.state_write_lock.lock().await;
        let start = self
            .admission
            .prepare_join_before_network(
                preparation,
                local_device_id,
                sponsor,
                sponsor_continuation_address,
                stable_request_binding,
                preserve_unreadable_history,
            )
            .await?;
        let join_id = start.attempt.join_id.ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent("local join id is missing".into())
        })?;
        Ok(
            crate::space::admission::adapter::DurableLocalJoinPreparation {
                attempt_id: *start.attempt.attempt_id.as_bytes(),
                join_id,
                request_message_id: start.request_message_id()?,
                resume_public_key: self
                    .admission
                    .load_join_recovery_material(start.attempt.attempt_id)
                    .await?
                    .resume_public_key,
                prepared_group_join: start.prepared_group_join,
            },
        )
    }

    pub(crate) async fn reject_local_join_before_candidate(
        &self,
        attempt_id: [u8; 32],
        reason: uc_core::membership::AdmissionRejectionReasonV1,
    ) -> Result<(), WorkspaceConvergenceError> {
        let _guard = self.membership.state_write_lock.lock().await;
        self.admission
            .joiner_reject_before_candidate(
                uc_core::membership::AdmissionAttemptId::from_bytes(attempt_id),
                reason,
            )
            .await
    }
}
