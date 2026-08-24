use crate::space::admission::durable::{
    admission_invitation_digest, admission_operation_id, admission_resume_public_key_digest,
    candidate_frame, common_existing_member_delivery_payload, complete_ack_frame,
    durable_frame_from_outbox, transaction, validate_candidate_request,
};
use crate::space::admission::*;
use crate::space::workspace_membership::*;

impl crate::space::admission::SpaceAdmission {
    pub(crate) async fn validate_join_request(
        &self,
        request: &uc_core::pairing::JoinerRequest,
    ) -> Result<(), WorkspaceConvergenceError> {
        request
            .validate_durable_identity()
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_owned()))?;
        let verified = self
            .membership
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
            .membership
            .deps
            .membership_history_repo
            .load_membership_history()
            .await
            .map_err(WorkspaceConvergenceError::from)?
        {
            let history = uc_core::membership::VersionedMembershipHistory::decode_persisted_v2(
                &encoded,
                self.membership
                    .deps
                    .historical_membership_signatures
                    .as_ref(),
            )
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
            let state = self.membership.load_state().await?;
            let own_instance = self
                .membership
                .deps
                .member_signatures
                .current_member_instance(&self.membership.deps.own_device)
                .await
                .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
            if history.lineage_id() != state.space_lineage
                || !history.active_members().contains(&own_instance)
            {
                tracing::warn!(
                    error_kind = "persisted_history_identity_mismatch",
                    "workspace admission base rejected"
                );
                return Err(WorkspaceConvergenceError::RecoveryRequired);
            }
            return Ok(history);
        }

        self.verified_legacy_admission_base_history().await
    }

    pub(crate) async fn verified_legacy_admission_base_history(
        &self,
    ) -> Result<uc_core::membership::VersionedMembershipHistory, WorkspaceConvergenceError> {
        let state = self.membership.load_state().await?;
        if state.removed {
            tracing::warn!(
                error_kind = "local_member_removed",
                "workspace admission base rejected"
            );
            return Err(WorkspaceConvergenceError::OwnInstanceRemoved);
        }
        let own_instance = state.own_instance.ok_or_else(|| {
            tracing::warn!(
                error_kind = "local_member_instance_unavailable",
                "workspace admission base rejected"
            );
            WorkspaceConvergenceError::RecoveryRequired
        })?;
        let current_instance = self
            .membership
            .deps
            .member_signatures
            .current_member_instance(&self.membership.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        if current_instance != own_instance {
            tracing::warn!(
                error_kind = "local_member_instance_mismatch",
                "workspace admission base rejected"
            );
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let remote_members = self
            .membership
            .deps
            .member_repo
            .list()
            .await
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?
            .into_iter()
            .any(|member| member.device_id != self.membership.deps.own_device);
        let legacy = state.membership_reconciliation.as_ref().ok_or_else(|| {
            tracing::warn!(
                error_kind = "membership_history_unavailable",
                "workspace admission base rejected"
            );
            WorkspaceConvergenceError::RecoveryRequired
        })?;
        let head = legacy
            .applied_head()
            .filter(|head| legacy.known_head() == Some(*head))
            .ok_or_else(|| {
                tracing::warn!(
                    error_kind = "membership_history_head_unavailable",
                    history_event_count = legacy.known_event_count(),
                    known_head_present = legacy.known_head().is_some(),
                    applied_head_present = legacy.applied_head().is_some(),
                    "workspace admission base rejected"
                );
                WorkspaceConvergenceError::RecoveryRequired
            })?;
        let event = legacy.event(head).ok_or_else(|| {
            tracing::warn!(
                error_kind = "membership_history_event_unavailable",
                "workspace admission base rejected"
            );
            WorkspaceConvergenceError::RecoveryRequired
        })?;
        if remote_members
            || legacy.known_event_count() != 1
            || legacy.effective_members() != [own_instance].into()
            || event.author_member_instance_id != own_instance
            || !matches!(
                &event.operation,
                uc_core::membership::MembershipOperation::AddDevice { admission }
                    if admission.member_instance == own_instance
                        && admission.device_id == self.membership.deps.own_device
            )
        {
            tracing::warn!(
                error_kind = "membership_baseline_mismatch",
                remote_members,
                history_event_count = legacy.known_event_count(),
                "workspace admission base rejected"
            );
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let credential = self
            .membership
            .deps
            .member_signatures
            .current_membership_credential(&self.membership.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        if credential.member_instance_id(&self.membership.deps.own_device) != own_instance
            || !self
                .membership
                .deps
                .member_signatures
                .verify_current_member_payload(
                    &self.membership.deps.own_device,
                    &event.signing_payload(),
                    &event.signature,
                )
                .await
                .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?
        {
            tracing::warn!(
                error_kind = "membership_credential_mismatch",
                "workspace admission base rejected"
            );
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }
        let own_facts = self
            .membership
            .local_admission_facts(Some(own_instance))
            .await?;
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
        use crate::deps::{SponsorAdmissionSecurityRecipient, SponsorAdmissionSecurityRequest};
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionIdentityBindingV1, AdmissionOutboxPurposeV1,
            MembershipAdmissionV2, MembershipEventV2, MembershipOperationV2,
            MEMBERSHIP_EVENT_FORMAT_V2,
        };

        let _guard = self.membership.state_write_lock.lock().await;
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
            .membership
            .deps
            .member_signatures
            .current_membership_credential(&self.membership.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_instance = own_credential.member_instance_id(&self.membership.deps.own_device);
        if !base_history.active_members().contains(&own_instance)
            || base_history.credential_for(own_instance) != Some(&own_credential)
        {
            return Err(WorkspaceConvergenceError::RecoveryRequired);
        }

        let sponsor_facts = self
            .membership
            .local_admission_facts(Some(own_instance))
            .await?;
        if sponsor_facts.device_id != self.membership.deps.own_device {
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
            .membership
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
            .membership
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
        let _guard = self.membership.state_write_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let attempt = self
            .admission
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let prepared = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Prepared,
            self.membership.deps.own_device.as_str().as_bytes(),
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
                .membership
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
        let _guard = self.membership.state_write_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let attempt = self
            .admission
            .load(attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let applied = transaction::durable_admission_message(
            attempt_id,
            AdmissionOutboxPurposeV1::Applied,
            self.membership.deps.own_device.as_str().as_bytes(),
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
                .membership
                .deps
                .membership_history_repo
                .load_membership_history()
                .await
                .map_err(WorkspaceConvergenceError::from)?
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "committed membership history is missing".to_owned(),
                    )
                })?,
            self.membership
                .deps
                .historical_membership_signatures
                .as_ref(),
        )
        .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        completed_history
            .verify_and_record_activation_receipt(
                receipt.clone(),
                self.membership
                    .deps
                    .historical_membership_signatures
                    .as_ref(),
            )
            .map_err(|_| WorkspaceConvergenceError::InvalidHandoff)?;
        self.admission
            .sponsor_prepare_security_activation(attempt_id, &receipt)
            .await?;
        self.membership
            .deps
            .activate_sponsor_admission_security
            .activate_sponsor_admission_security(
                crate::deps::ActivateSponsorAdmissionSecurityRequest {
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
            .membership
            .deps
            .member_signatures
            .current_membership_credential(&self.membership.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_instance = own_credential.member_instance_id(&self.membership.deps.own_device);
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
            .membership
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
        self.membership
            .save_member_facts(&admission.facts, self.membership.deps.clock.now_ms())
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

    pub(crate) async fn confirm_sponsor_complete_ack(
        &self,
        frame: &uc_core::pairing::DurableAdmissionFrame,
    ) -> Result<(), WorkspaceConvergenceError> {
        if frame.kind != uc_core::pairing::DurableAdmissionMessageKind::CompleteAck {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let _guard = self.membership.state_write_lock.lock().await;
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
        confirm_complete_delivery(
            self.membership.deps.admission_attempts.as_ref(),
            attempt_id,
            &acknowledgment,
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
        let _guard = self.membership.state_write_lock.lock().await;
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
        let _guard = self.membership.state_write_lock.lock().await;
        let attempt_id = AdmissionAttemptId::from_bytes(frame.attempt_id);
        let acknowledgment = transaction::admission_acknowledgment(&rejected);
        confirm_rejected_delivery(
            self.membership.deps.admission_attempts.as_ref(),
            attempt_id,
            &acknowledgment,
        )
        .await?;
        self.admission.compact_if_settled(attempt_id).await?;
        Ok(())
    }
}

pub(in crate::space) async fn confirm_rejected_delivery(
    repository: &dyn crate::deps::AdmissionAttemptRepositoryPort,
    attempt_id: uc_core::membership::AdmissionAttemptId,
    acknowledgment: &uc_core::membership::AdmissionInboxRecordV1,
) -> Result<(), WorkspaceConvergenceError> {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, AdmissionOutboxPurposeV1, AdmissionRejectionReasonV1,
        AdmissionTerminalResultV1, SponsorAdmissionStageV1, SponsorAdmissionStateV1,
    };

    if let Some(terminal) = repository
        .load_terminal(attempt_id)
        .await
        .map_err(crate::space::admission::durable::map_repository_error)?
    {
        if terminal.terminal_result == AdmissionTerminalResultV1::Rejected
            && terminal.rejection_reason == Some(AdmissionRejectionReasonV1::Cancelled)
            && terminal.acknowledgment_rebuild.contains(acknowledgment)
        {
            return Ok(());
        }
        return Err(inconsistent("rejected acknowledgment does not match"));
    }

    let mut attempt = load_required_attempt(repository, attempt_id).await?;
    if !matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Rejected,
        })
    ) || attempt.terminal_result != Some(AdmissionTerminalResultV1::Rejected)
    {
        return Err(inconsistent("sponsor admission is not rejected"));
    }
    if attempt.inbox_dedup.contains(acknowledgment) {
        return Ok(());
    }
    let rejected_index = attempt
        .outboxes
        .iter()
        .position(|message| {
            message.purpose == AdmissionOutboxPurposeV1::Rejected
                && !message.superseded
                && transaction::admission_acknowledgment(message) == *acknowledgment
        })
        .ok_or_else(|| inconsistent("rejected acknowledgment does not match"))?;
    attempt.outboxes[rejected_index].superseded = true;
    attempt.inbox_dedup.push(acknowledgment.clone());
    persist_attempt(repository, attempt).await
}

pub(in crate::space) async fn confirm_complete_delivery(
    repository: &dyn crate::deps::AdmissionAttemptRepositoryPort,
    attempt_id: uc_core::membership::AdmissionAttemptId,
    acknowledgment: &uc_core::membership::AdmissionInboxRecordV1,
) -> Result<(), WorkspaceConvergenceError> {
    use uc_core::membership::{
        AdmissionAttemptRoleStateV1, AdmissionOutboxPurposeV1, AdmissionTerminalResultV1,
        SponsorAdmissionStageV1, SponsorAdmissionStateV1,
    };

    if let Some(terminal) = repository
        .load_terminal(attempt_id)
        .await
        .map_err(crate::space::admission::durable::map_repository_error)?
    {
        if terminal.terminal_result == AdmissionTerminalResultV1::Completed
            && terminal.acknowledgment_rebuild.contains(acknowledgment)
        {
            return Ok(());
        }
        return Err(inconsistent(
            "complete acknowledgment does not match compacted admission result",
        ));
    }

    let mut attempt = load_required_attempt(repository, attempt_id).await?;
    if attempt.terminal_result == Some(AdmissionTerminalResultV1::Completed)
        && attempt.inbox_dedup.contains(acknowledgment)
    {
        return Ok(());
    }
    if !matches!(
        attempt.role_state,
        AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
            stage: SponsorAdmissionStageV1::Completed,
        })
    ) {
        return Err(inconsistent("sponsor admission message is out of order"));
    }
    let complete_index = attempt
        .outboxes
        .iter()
        .position(|message| {
            message.purpose == AdmissionOutboxPurposeV1::Complete && !message.superseded
        })
        .ok_or_else(|| inconsistent("complete outbox is missing"))?;
    if transaction::admission_acknowledgment(&attempt.outboxes[complete_index]) != *acknowledgment {
        return Err(inconsistent("complete acknowledgment does not match"));
    }
    attempt.outboxes[complete_index].superseded = true;
    attempt.inbox_dedup.push(acknowledgment.clone());
    attempt.terminal_result = Some(AdmissionTerminalResultV1::Completed);
    attempt.role_state = AdmissionAttemptRoleStateV1::Sponsor(SponsorAdmissionStateV1 {
        stage: SponsorAdmissionStageV1::Completed,
    });
    persist_attempt(repository, attempt).await
}

async fn load_required_attempt(
    repository: &dyn crate::deps::AdmissionAttemptRepositoryPort,
    attempt_id: uc_core::membership::AdmissionAttemptId,
) -> Result<uc_core::membership::AdmissionAttemptV1, WorkspaceConvergenceError> {
    repository
        .load(attempt_id)
        .await
        .map_err(crate::space::admission::durable::map_repository_error)?
        .ok_or_else(|| inconsistent("admission attempt was not found"))
}

async fn persist_attempt(
    repository: &dyn crate::deps::AdmissionAttemptRepositoryPort,
    mut attempt: uc_core::membership::AdmissionAttemptV1,
) -> Result<(), WorkspaceConvergenceError> {
    let expected_version = attempt.record_version;
    attempt.record_version = expected_version
        .checked_add(1)
        .ok_or_else(|| inconsistent("admission record version overflow"))?;
    repository
        .compare_and_advance(attempt.attempt_id, expected_version, &attempt)
        .await
        .map_err(crate::space::admission::durable::map_repository_error)?;
    Ok(())
}

fn inconsistent(message: impl Into<String>) -> WorkspaceConvergenceError {
    WorkspaceConvergenceError::Inconsistent(message.into())
}
