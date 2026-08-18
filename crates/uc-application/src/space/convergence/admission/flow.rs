use super::super::*;
use super::transaction;
use super::{
    admission_invitation_digest, admission_operation_id, admission_resume_public_key_digest,
    candidate_frame, common_existing_member_delivery_payload, complete_ack_frame,
    durable_frame_from_outbox, validate_candidate_request,
};

impl WorkspaceConvergence {
    pub(crate) async fn validate_join_request(
        &self,
        request: &uc_core::pairing::JoinerRequest,
    ) -> Result<(), WorkspaceConvergenceError> {
        request
            .validate_durable_identity()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_owned()))?;
        let verified = self
            .deps
            .historical_membership_signatures
            .verify(
                request.membership_credential.signature_algorithm_version,
                &request.membership_credential.public_key,
                &request.admission.signing_payload(),
                &request.admission.identity_signature,
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if !verified {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        Ok(())
    }

    pub(crate) async fn verified_admission_base_history(
        &self,
    ) -> Result<uc_core::membership::VersionedMembershipHistory, WorkspaceConvergenceError> {
        if let Some(encoded) = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(transaction::map_repository_error)?
        {
            let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                &encoded,
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            let state = self.load_state().await?;
            let own_instance = self
                .deps
                .member_signatures
                .current_member_instance(&self.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            if history.lineage_id() != state.space_lineage
                || !history.active_members().contains(&own_instance)
            {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            return Ok(history);
        }

        let state = self.load_state().await?;
        if state.removed {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        let own_instance = state
            .own_instance
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        let current_instance = self
            .deps
            .member_signatures
            .current_member_instance(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        if current_instance != own_instance {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let remote_members = self
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
            .into_iter()
            .any(|member| member.device_id != self.deps.own_device);
        let legacy = state
            .membership_reconciliation
            .as_ref()
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        let head = legacy
            .applied_head()
            .filter(|head| legacy.known_head() == Some(*head))
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        let event = legacy
            .event(head)
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        if remote_members
            || legacy.known_event_count() != 1
            || legacy.effective_members() != [own_instance].into()
            || event.author_member_instance_id != own_instance
            || !matches!(
                &event.operation,
                uc_core::membership::MembershipOperation::AddDevice { admission }
                    if admission.member_instance == own_instance
                        && admission.device_id == self.deps.own_device
            )
        {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        if credential.member_instance_id(&self.deps.own_device) != own_instance
            || !self
                .deps
                .member_signatures
                .verify_current_member_payload(
                    &self.deps.own_device,
                    &event.signing_payload(),
                    &event.signature,
                )
                .await
                .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?
        {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let own_facts = self.local_admission_facts(Some(own_instance)).await?;
        uc_core::membership::VersionedMembershipHistory::from_activation_baseline(
            uc_core::membership::MembershipActivationBaselineV2::FullyVerifiedMigration {
                lineage_id: state.space_lineage,
                head_event_id: head,
                head_depth: event.parent_depth,
                current_members: vec![(own_facts, credential)],
            },
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))
    }

    pub(crate) async fn prepare_sponsor_candidate(
        &self,
        request: &uc_core::pairing::JoinerRequest,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionIdentityBindingV1, AdmissionOutboxPurposeV1,
            MembershipAdmissionV2, MembershipEventV2, MembershipOperationV2,
            SponsorAdmissionSecurityRecipient, SponsorAdmissionSecurityRequest,
            MEMBERSHIP_EVENT_FORMAT_V2,
        };

        let _guard = self.state_lock.lock().await;
        self.validate_join_request(request).await?;
        let attempt_id = AdmissionAttemptId::from_bytes(request.attempt_id);
        let invitation_digest = admission_invitation_digest(request.invitation_code.as_str());
        let stable_request_binding = crate::space::admission::adapter::stable_join_request_binding(
            &request.device_id,
            &request.identity_fingerprint,
        );
        let request_message = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::JoinRequest,
            request.invitation_code.as_str().as_bytes(),
            None,
            &stable_request_binding,
        );
        if request_message.message_id != request.request_message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }

        if let Some(existing) = self.admission.load(attempt_id).await? {
            if existing.invitation_claim.as_deref() != Some(invitation_digest.as_slice())
                || existing.candidate_key_package.as_deref() != Some(request.key_package.as_slice())
                || existing
                    .resume_public_key
                    .as_deref()
                    .is_some_and(|key| key != request.resume_public_key)
            {
                return Err(WorkspaceConvergenceError::AdmissionInProgress);
            }
            let candidate_message = existing
                .outboxes
                .iter()
                .find(|message| message.purpose == AdmissionOutboxPurposeV1::Candidate)
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "persisted sponsor admission has no candidate".to_owned(),
                    )
                })?;
            let payload = transaction::DurableAdmissionCandidatePayloadV1::decode(
                &candidate_message.payload,
            )?;
            validate_candidate_request(&payload.candidate, request)?;
            return candidate_frame(attempt_id, candidate_message);
        }

        let base_history = self.verified_admission_base_history().await?;
        if base_history.active_members() != base_history.effective_members() {
            return Err(WorkspaceConvergenceError::AdmissionInProgress);
        }
        let base_position = base_history
            .current_position()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_instance = own_credential.member_instance_id(&self.deps.own_device);
        if !base_history.active_members().contains(&own_instance)
            || base_history.credential_for(own_instance) != Some(&own_credential)
        {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }

        let sponsor_facts = self.local_admission_facts(Some(own_instance)).await?;
        if sponsor_facts.device_id != self.deps.own_device {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let mut target_relationships = Vec::new();
        let mut existing_recipients = Vec::new();
        for member in base_history.active_members() {
            let credential = base_history
                .credential_for(member)
                .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
            let facts = if member == own_instance {
                sponsor_facts.clone()
            } else {
                base_history
                    .admission_facts_for(member)
                    .cloned()
                    .ok_or(WorkspaceConvergenceError::RecoveryRequired)?
            };
            if credential.member_instance_id(&facts.device_id) != member {
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            if member != own_instance {
                existing_recipients.push(SponsorAdmissionSecurityRecipient {
                    device_id: facts.device_id.clone(),
                    credential_id: credential.credential_id,
                });
            }
            target_relationships.push(facts);
        }
        if target_relationships.iter().any(|facts| {
            facts.device_id == request.device_id || facts.member_instance == request.member_instance
        }) {
            return Err(WorkspaceConvergenceError::AdmissionConflict);
        }
        target_relationships.push(request.admission.clone());
        target_relationships.sort_by_key(|facts| facts.member_instance);

        let resume_public_key_digest =
            admission_resume_public_key_digest(&request.resume_public_key);
        let operation_id = admission_operation_id(attempt_id);
        let provisional_operation = MembershipOperationV2::AddDevice {
            admission: MembershipAdmissionV2 {
                facts: request.admission.clone(),
                membership_credential: request.membership_credential.clone(),
                resume_public_key_digest,
                security_commitment_id: [0; 32],
            },
        };
        let resulting_members_digest = base_history
            .expected_resulting_members_digest(base_position.event_id, &provisional_operation)
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let provisional_event = MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            base_history.lineage_id().to_owned(),
            base_position.event_id,
            base_position.depth.saturating_add(1),
            operation_id,
            own_instance,
            own_credential.credential_id,
            own_credential.signature_algorithm_version,
            provisional_operation,
            resulting_members_digest,
            [0; 32],
            Vec::new(),
            None,
            Vec::new(),
        );
        let candidate_core_digest = provisional_event
            .admission_candidate_core_digest(request.attempt_id, &request.key_package)
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let prepared_security = self
            .deps
            .prepare_sponsor_admission_security
            .prepare_sponsor_admission_security(SponsorAdmissionSecurityRequest {
                space_id: uc_core::ids::SpaceId::from_string(base_history.lineage_id().to_owned()),
                attempt_id: request.attempt_id,
                base_history_position: base_position.clone(),
                candidate_core_digest,
                candidate_identity: request.device_id.as_str().as_bytes().to_vec(),
                candidate_key_package: request.key_package.clone(),
                existing_recipients,
            })
            .await
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        if prepared_security.public_commitment.attempt_id != request.attempt_id
            || prepared_security.public_commitment.lineage_id != base_history.lineage_id()
            || prepared_security.public_commitment.base_history_position != base_position
            || prepared_security.public_commitment.candidate_core_digest != candidate_core_digest
        {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "prepared sponsor security result does not match the candidate".to_owned(),
            ));
        }
        let security_update_payload =
            common_existing_member_delivery_payload(&prepared_security.existing_member_deliveries)?;
        let operation = MembershipOperationV2::AddDevice {
            admission: MembershipAdmissionV2 {
                facts: request.admission.clone(),
                membership_credential: request.membership_credential.clone(),
                resume_public_key_digest,
                security_commitment_id: prepared_security.public_commitment.security_commitment_id,
            },
        };
        let mut candidate_event = MembershipEventV2::new(
            MEMBERSHIP_EVENT_FORMAT_V2,
            base_history.lineage_id().to_owned(),
            base_position.event_id,
            base_position.depth.saturating_add(1),
            operation_id,
            own_instance,
            own_credential.credential_id,
            own_credential.signature_algorithm_version,
            operation,
            resulting_members_digest,
            prepared_security.public_commitment.group_context_digest,
            security_update_payload,
            Some(prepared_security.public_commitment.admission_bundle_digest),
            Vec::new(),
        );
        if candidate_event
            .admission_candidate_core_digest(request.attempt_id, &request.key_package)
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
            != candidate_core_digest
        {
            return Err(WorkspaceConvergenceError::Inconsistent(
                "candidate core changed after security preparation".to_owned(),
            ));
        }
        candidate_event.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&candidate_event.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let candidate_event_id = candidate_event.event_id();
        let identity_binding = AdmissionIdentityBindingV1::new(
            base_history.lineage_id().to_owned(),
            candidate_event_id,
            &sponsor_facts,
            &request.admission,
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        .encode()
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let encoded_base_history = base_history
            .encode_persisted_v2()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let candidate = transaction::DurableAdmissionCandidateV1 {
            lineage_id: base_history.lineage_id().to_owned(),
            base_history_position: postcard::to_stdvec(&base_position)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?,
            candidate_event: postcard::to_stdvec(&candidate_event)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?,
            candidate_event_id: *candidate_event_id.as_bytes(),
            candidate_key_package: request.key_package.clone(),
            resume_public_key: request.resume_public_key.clone(),
            target_members_digest: resulting_members_digest,
            security_commitment: postcard::to_stdvec(&prepared_security.public_commitment)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?,
            security_commit: prepared_security.commit,
            security_welcome: prepared_security.welcome,
            target_protection_group_id: prepared_security.target_protection_group_id,
            target_key_catalog: prepared_security
                .target_key_catalog
                .encode()
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?,
            target_relationships,
            existing_member_deliveries: prepared_security.existing_member_deliveries,
            staged_security_state: prepared_security.staged_state,
            identity_binding,
        };
        let payload = transaction::DurableAdmissionCandidatePayloadV1::new(
            encoded_base_history,
            candidate.clone(),
        )
        .encode()?;
        let candidate_message = self
            .admission
            .sponsor_accept_and_offer(
                attempt_id,
                invitation_digest,
                &request_message,
                candidate,
                base_history,
                &candidate_event,
                &prepared_security.public_commitment,
                request.device_id.as_str().as_bytes(),
                &payload,
            )
            .await?;
        candidate_frame(attempt_id, &candidate_message)
    }

    pub(crate) async fn prepare_joiner_candidate(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        proof_signer: &(dyn uc_core::ports::space::GroupAdmissionPort + Send + Sync),
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
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let candidate_message = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Candidate,
            self.deps.own_device.as_str().as_bytes(),
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
            self.deps.historical_membership_signatures.as_ref(),
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
        if admission.facts.device_id != self.deps.own_device {
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

    pub(crate) async fn commit_sponsor_prepared(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1,
            MembershipEventV2, MembershipOperationV2, PreparedAdmissionProofV1,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Prepared {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let attempt = self
            .admission
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let prepared = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Prepared,
            self.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if prepared.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let proof: PreparedAdmissionProofV1 = postcard::from_bytes(&frame.payload)
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
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        if proof.proof_format_version != uc_core::membership::PREPARED_ADMISSION_PROOF_FORMAT_V1
            || proof.attempt_id != frame.attempt_id
            || proof.lineage_id != candidate_event.lineage_id
            || proof.base_history_position != commitment.base_history_position
            || proof.candidate_event_id != candidate_event.event_id()
            || proof.target_members_digest != candidate_event.resulting_members_digest
            || proof.security_commitment_id != commitment.security_commitment_id
            || proof.joiner_member_instance_id != admission.facts.member_instance
            || proof.joiner_credential_id != admission.membership_credential.credential_id
            || !self
                .deps
                .historical_membership_signatures
                .verify(
                    admission.membership_credential.signature_algorithm_version,
                    &admission.membership_credential.public_key,
                    &proof.signing_payload(),
                    &proof.signature,
                )
                .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let commit_payload = transaction::DurableAdmissionCommitPayloadV1 {
            format_version: transaction::DurableAdmissionCommitPayloadV1::FORMAT_V1,
            candidate_event_id: *candidate_event.event_id().as_bytes(),
            security_commitment_id: commitment.security_commitment_id,
            prepared_proof: frame.payload.clone(),
            resume_public_key: attempt.resume_public_key.clone().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("resume public key is missing".to_owned())
            })?,
            existing_member_deliveries: attempt
                .existing_member_security_deliveries
                .clone()
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "existing-member security deliveries are missing".to_owned(),
                    )
                })?,
            completion_recovery_routes: transaction::completion_recovery_routes(
                attempt.target_relationships.as_deref().ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "completion recovery routes are missing".to_owned(),
                    )
                })?,
            ),
        }
        .encode()?;
        let commit = self
            .admission
            .sponsor_commit(
                attempt_id,
                &prepared,
                &frame.payload,
                admission.facts.device_id.as_str().as_bytes(),
                &commit_payload,
            )
            .await?;
        durable_frame_from_outbox(
            attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Commit,
            AdmissionOutboxPurposeV1::Commit,
            &commit,
        )
    }

    pub(crate) async fn apply_joiner_commit(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
        receipt_signer: &(dyn uc_core::ports::space::GroupAdmissionPort + Send + Sync),
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionActivationReceipt, AdmissionAttemptId, AdmissionOutboxPurposeV1,
            AdmissionSecurityCommitmentV1, AdmissionSecurityTransitionInput, MembershipEventV2,
            MembershipOperationV2,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Commit {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let commit = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Commit,
            self.deps.own_device.as_str().as_bytes(),
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

    pub(crate) async fn complete_sponsor_applied(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionActivationReceipt, AdmissionAttemptId, AdmissionCompletionV1,
            AdmissionOutboxPurposeV1, AdmissionSecurityCommitmentV1, MembershipEventV2,
            MembershipOperationV2, VersionedMembershipHistory,
        };
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Applied {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let attempt = self
            .admission
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let applied = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Applied,
            self.deps.own_device.as_str().as_bytes(),
            frame.predecessor_message_id,
            &frame.payload,
        );
        if applied.message_id != frame.message_id {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let receipt: AdmissionActivationReceipt = postcard::from_bytes(&frame.payload)
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
        let mut completed_history = VersionedMembershipHistory::decode_persisted_v2(
            &self
                .deps
                .admission_attempts
                .load_membership_history_v2()
                .await
                .map_err(transaction::map_repository_error)?
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "committed membership history is missing".to_owned(),
                    )
                })?,
            self.deps.historical_membership_signatures.as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        completed_history
            .verify_and_record_activation_receipt(
                receipt.clone(),
                self.deps.historical_membership_signatures.as_ref(),
            )
            .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
        self.admission
            .sponsor_prepare_security_activation(attempt_id, &receipt)
            .await?;
        self.deps
            .activate_sponsor_admission_security
            .activate_sponsor_admission_security(
                uc_core::membership::ActivateSponsorAdmissionSecurityRequest {
                    space_id: uc_core::ids::SpaceId::from_string(
                        candidate_event.lineage_id.clone(),
                    ),
                    staged_state: attempt.staged_security_state.clone().ok_or_else(|| {
                        WorkspaceConvergenceError::Inconsistent(
                            "sponsor staged security state is missing".to_owned(),
                        )
                    })?,
                    commit: attempt.security_commit.clone().ok_or_else(|| {
                        WorkspaceConvergenceError::Inconsistent(
                            "sponsor security commit is missing".to_owned(),
                        )
                    })?,
                    expected_commitment: commitment.clone(),
                },
            )
            .await
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let completed_position = completed_history
            .current_position()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_instance = own_credential.member_instance_id(&self.deps.own_device);
        if !completed_history.active_members().contains(&own_instance) {
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        let receipt_bytes = postcard::to_stdvec(&receipt)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let mut completion = AdmissionCompletionV1::new(
            frame.attempt_id,
            candidate_event.event_id(),
            sha2::Sha256::digest(&receipt_bytes).into(),
            commitment.security_commitment_id,
            own_instance,
            own_credential.credential_id,
            completed_position,
            Vec::new(),
        );
        completion.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&completion.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let completion_bytes = postcard::to_stdvec(&completion)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let MembershipOperationV2::AddDevice { admission } = &candidate_event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        // Completion is not externally visible until the sponsor has also
        // installed the admitted member's roster, trust and address facts.
        // This is idempotent, so recovery can repeat it after any later save
        // or send failure without creating a second relationship.
        self.save_member_facts(&admission.facts, self.deps.clock.now_ms())
            .await?;
        let complete = self
            .admission
            .sponsor_complete(
                attempt_id,
                &applied,
                &receipt,
                &completion_bytes,
                admission.facts.device_id.as_str().as_bytes(),
                &completion_bytes,
            )
            .await?;
        durable_frame_from_outbox(
            attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Complete,
            AdmissionOutboxPurposeV1::Complete,
            &complete,
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
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let complete = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Complete,
            self.deps.own_device.as_str().as_bytes(),
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
            self.deps.historical_membership_signatures.as_ref(),
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

    pub(crate) async fn confirm_sponsor_complete_ack(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::CompleteAck {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = uc_core::membership::AdmissionAttemptId::from_bytes(frame.attempt_id);
        if complete_ack_frame(
            attempt_id,
            frame.predecessor_message_id.unwrap_or([0; 32]),
            frame.payload.clone(),
        )
        .message_id
            != frame.message_id
            || frame.predecessor_message_id.is_none()
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let acknowledgment: uc_core::membership::AdmissionInboxRecordV1 =
            postcard::from_bytes(&frame.payload)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        self.admission
            .sponsor_confirm_active(attempt_id, &acknowledgment)
            .await
    }

    pub async fn cancel_join_space(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, WorkspaceConvergenceError> {
        self.admission.cancel_local_join(join_id).await
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
        preparation: &(dyn uc_core::ports::space::GroupAdmissionPort + Send + Sync),
        local_device_id: &DeviceId,
        sponsor: &[u8],
        sponsor_continuation_address: &[u8],
        stable_request_binding: &[u8],
        preserve_unreadable_history: bool,
    ) -> Result<
        crate::space::admission::adapter::DurableLocalJoinPreparation,
        WorkspaceConvergenceError,
    > {
        let _guard = self.state_lock.lock().await;
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
        let _guard = self.state_lock.lock().await;
        self.admission
            .joiner_reject_before_candidate(
                uc_core::membership::AdmissionAttemptId::from_bytes(attempt_id),
                reason,
            )
            .await
    }

    pub(crate) async fn reject_superseded_join_cleanup(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{AdmissionAttemptId, AdmissionOutboxPurposeV1};

        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::CancelRequested {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let cancel: uc_core::membership::AdmissionOutboxMessageV1 =
            postcard::from_bytes(&frame.payload)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        if cancel.purpose != AdmissionOutboxPurposeV1::CancelRequested
            || cancel.message_id != frame.message_id
            || cancel.predecessor_message_id != frame.predecessor_message_id
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let rejected = self
            .admission
            .sponsor_decide_cancel(
                attempt_id,
                &cancel,
                &cancel.recipient,
                b"superseded_by_new_join",
            )
            .await?;
        Ok(uc_core::pairing::DurableAdmissionFrame {
            attempt_id: frame.attempt_id,
            kind: uc_core::pairing::DurableAdmissionMessageKind::Rejected,
            message_id: rejected.message_id,
            predecessor_message_id: rejected.predecessor_message_id,
            payload: postcard::to_stdvec(&rejected)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?,
        })
    }

    pub(crate) async fn confirm_superseded_join_cleanup_sent(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        use uc_core::membership::{AdmissionAttemptId, AdmissionOutboxPurposeV1};

        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::Rejected {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let rejected: uc_core::membership::AdmissionOutboxMessageV1 =
            postcard::from_bytes(&frame.payload)
                .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        if rejected.purpose != AdmissionOutboxPurposeV1::Rejected
            || rejected.message_id != frame.message_id
            || rejected.predecessor_message_id != frame.predecessor_message_id
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.state_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let acknowledgment = transaction::admission_acknowledgment(&rejected);
        self.admission
            .sponsor_confirm_rejected(attempt_id, &acknowledgment)
            .await?;
        self.admission.compact_if_settled(attempt_id).await?;
        Ok(())
    }

    pub async fn requires_session_transition(&self) -> Result<bool, WorkspaceConvergenceError> {
        self.admission.requires_session_transition().await
    }

    pub async fn recover_space_transition_after_session_drain(
        &self,
    ) -> Result<usize, WorkspaceConvergenceError> {
        let finished = self
            .admission
            .recover_space_transitions_after_session_drain()
            .await?;
        if finished > 0 {
            self.notify();
        }
        Ok(finished)
    }
}
