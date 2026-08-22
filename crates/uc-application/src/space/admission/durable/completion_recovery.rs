use super::transaction;
use super::*;
use super::{admission_resume_public_key_digest, durable_frame_from_outbox};

impl crate::space::admission::SpaceAdmission {
    pub(crate) async fn prepare_completion_recovery_hello(
        &self,
        attempt_id: [u8; 32],
        helper_member_instance: uc_core::membership::MemberInstanceId,
    ) -> Result<uc_core::membership::AdmissionCompletionRecoveryHelloV1, WorkspaceConvergenceError>
    {
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionCompletionRecoveryHelloV1, MembershipEventV2,
            MembershipOperationV2,
        };
        let _guard = self.membership.state_lock.lock().await;
        let attempt = self
            .admission
            .load(AdmissionAttemptId::from_bytes(attempt_id))
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        if !attempt.is_joiner()
            || attempt.stage_rank() != Some(5)
            || attempt.activation_receipt.is_none()
            || attempt.completion.is_some()
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let event: MembershipEventV2 =
            postcard::from_bytes(attempt.candidate_event.as_deref().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("candidate event is missing".to_owned())
            })?)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        AdmissionCompletionRecoveryHelloV1::new(
            AdmissionAttemptId::from_bytes(attempt_id),
            event.lineage_id.clone(),
            event.event_id(),
            event.author_member_instance_id,
            admission.facts.member_instance,
            helper_member_instance,
            attempt.resume_public_key.clone().ok_or_else(|| {
                WorkspaceConvergenceError::Inconsistent("resume public key is missing".to_owned())
            })?,
        )
        .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)
    }

    pub(crate) async fn challenge_completion_recovery(
        &self,
        hello: &uc_core::membership::AdmissionCompletionRecoveryHelloV1,
        transport_binding: uc_core::membership::AdmissionCompletionRecoveryTransportBindingV1,
        joiner_last_message_id: [u8; 32],
        helper_last_message_id: [u8; 32],
    ) -> Result<
        uc_core::membership::AdmissionCompletionRecoveryChallengeV1,
        WorkspaceConvergenceError,
    > {
        use uc_core::membership::{
            AdmissionCompletionRecoveryChallengeV1, MembershipOperationV2,
            VersionedMembershipHistory,
        };
        let _guard = self.membership.state_lock.lock().await;
        hello
            .validate()
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        if let Some(existing) = self
            .admission
            .load_completion_recovery_challenge(hello.attempt_id)
            .await?
        {
            if existing.hello_digest == hello.digest()
                && existing.transport_binding == transport_binding
                && existing.joiner_last_message_id == joiner_last_message_id
                && existing.helper_last_message_id == helper_last_message_id
            {
                return Ok(existing);
            }
        }
        let history_bytes = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(transaction::map_repository_error)?
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &history_bytes,
            self.membership
                .deps
                .historical_membership_signatures
                .as_ref(),
        )
        .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        let event = history
            .event(hello.event_id)
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.membership.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_instance = own_credential.member_instance_id(&self.membership.deps.own_device);
        if history.lineage_id() != hello.lineage_id
            || event.author_member_instance_id != hello.sponsor_member_instance
            || admission.facts.member_instance != hello.joiner_member_instance
            || admission.resume_public_key_digest
                != admission_resume_public_key_digest(&hello.resume_public_key)
            || transport_binding.joiner_transport_identity_digest
                != <[u8; 32]>::from(sha2::Sha256::digest(&admission.facts.transport_public_key))
            || own_instance != hello.helper_member_instance
            || own_credential.credential_id
                != history
                    .credential_for(own_instance)
                    .ok_or(WorkspaceConvergenceError::OwnInstanceRemoved)?
                    .credential_id
            || !history.active_members().contains(&own_instance)
            || !history
                .active_members()
                .contains(&hello.joiner_member_instance)
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let own_announcement = self
            .deps
            .announcement_material
            .current_announcement_material()
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        if transport_binding.helper_transport_identity_digest
            != <[u8; 32]>::from(sha2::Sha256::digest(&own_announcement.transport_public_key))
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let position = history
            .current_position()
            .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        let previous_counter = self
            .admission
            .load_completion_recovery_challenge(hello.attempt_id)
            .await?
            .map(|challenge| challenge.challenge_counter)
            .unwrap_or(0);
        let mut nonce = [0u8; 32];
        while nonce == [0; 32] {
            rand::rng().fill_bytes(&mut nonce);
        }
        let mut challenge = AdmissionCompletionRecoveryChallengeV1::new(
            hello,
            transport_binding,
            previous_counter
                .checked_add(1)
                .ok_or(WorkspaceConvergenceError::RecoveryRequired)?,
            nonce,
            joiner_last_message_id,
            helper_last_message_id,
            own_credential.credential_id,
            position,
        )
        .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        challenge.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&challenge.signing_payload())
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        self.admission
            .save_completion_recovery_challenge(hello.attempt_id, &challenge)
            .await?;
        Ok(challenge)
    }

    pub(crate) async fn respond_to_completion_recovery(
        &self,
        hello: &uc_core::membership::AdmissionCompletionRecoveryHelloV1,
        challenge: &uc_core::membership::AdmissionCompletionRecoveryChallengeV1,
    ) -> Result<uc_core::membership::AdmissionCompletionRecoveryResponseV1, WorkspaceConvergenceError>
    {
        use ed25519_dalek::Signer;
        use uc_core::membership::{
            AdmissionAttemptId, AdmissionCompletionRecoveryBundleV1,
            AdmissionCompletionRecoveryResponseV1, MembershipOperationV2,
            VersionedMembershipHistory,
        };
        let _guard = self.membership.state_lock.lock().await;
        challenge
            .validate()
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        if challenge.hello_digest != hello.digest() || challenge.signature.is_empty() {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let attempt = self
            .admission
            .load(AdmissionAttemptId::from_bytes(*hello.attempt_id.as_bytes()))
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        if !attempt.is_joiner() || attempt.stage_rank() != Some(5) || attempt.completion.is_some() {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let history = VersionedMembershipHistory::decode_persisted_v2(
            attempt
                .verified_membership_history
                .as_deref()
                .ok_or(WorkspaceConvergenceError::RecoveryRequired)?,
            self.membership
                .deps
                .historical_membership_signatures
                .as_ref(),
        )
        .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        let helper_credential = history
            .credential_for(hello.helper_member_instance)
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        if helper_credential.credential_id != challenge.helper_credential_id
            || challenge.helper_history_position
                != history
                    .current_position()
                    .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?
            || !history
                .active_members()
                .contains(&hello.helper_member_instance)
            || !self
                .deps
                .historical_membership_signatures
                .verify(
                    helper_credential.signature_algorithm_version,
                    &helper_credential.public_key,
                    &challenge.signing_payload(),
                    &challenge.signature,
                )
                .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let event: MembershipEventV2 = postcard::from_bytes(
            attempt
                .candidate_event
                .as_deref()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
        )
        .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        if event.event_id() != hello.event_id
            || event.author_member_instance_id != hello.sponsor_member_instance
            || admission.facts.member_instance != hello.joiner_member_instance
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let bundle = AdmissionCompletionRecoveryBundleV1 {
            format_version: uc_core::membership::ADMISSION_COMPLETION_RECOVERY_FORMAT_V1,
            candidate_event: attempt
                .candidate_event
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            candidate_key_package: attempt
                .candidate_key_package
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            security_commitment: attempt
                .security_commitment
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            security_commit: attempt
                .security_commit
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            security_welcome: attempt
                .security_welcome
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            target_protection_group_id: attempt
                .target_protection_group_id
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            target_key_catalog: attempt
                .target_key_catalog
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            existing_member_deliveries: attempt
                .existing_member_security_deliveries
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            activation_receipt: attempt
                .activation_receipt
                .clone()
                .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?,
            resume_public_key: hello.resume_public_key.clone(),
        };
        let mut response =
            AdmissionCompletionRecoveryResponseV1::new(hello.digest(), challenge.digest(), bundle)
                .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        let private_key: [u8; 32] = attempt
            .resume_private_key
            .as_deref()
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?
            .try_into()
            .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&private_key);
        if signing_key.verifying_key().to_bytes().as_slice() != hello.resume_public_key.as_slice() {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        response.resume_signature = signing_key
            .sign(&response.signing_payload())
            .to_bytes()
            .to_vec();
        Ok(response)
    }

    pub(crate) async fn complete_recovered_admission(
        &self,
        hello: &uc_core::membership::AdmissionCompletionRecoveryHelloV1,
        response: &uc_core::membership::AdmissionCompletionRecoveryResponseV1,
    ) -> Result<uc_core::pairing::DurableAdmissionFrame, WorkspaceConvergenceError> {
        use uc_core::membership::{
            AdmissionActivationReceipt, AdmissionCompletionV1, AdmissionSecurityCommitmentV1,
            MembershipActivationReceiptStoreOutcome, MembershipEventV2, MembershipOperationV2,
            VersionedMembershipHistory,
        };
        let _guard = self.membership.state_lock.lock().await;
        response
            .validate()
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        let saved_challenge = self
            .admission
            .load_completion_recovery_challenge(hello.attempt_id)
            .await?
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        if response.hello_digest != hello.digest()
            || response.challenge_digest != saved_challenge.digest()
            || response.bundle.resume_public_key != hello.resume_public_key
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let public_key: [u8; 32] = hello
            .resume_public_key
            .as_slice()
            .try_into()
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        let signature = ed25519_dalek::Signature::from_slice(&response.resume_signature)
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        ed25519_dalek::VerifyingKey::from_bytes(&public_key)
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?
            .verify_strict(&response.signing_payload(), &signature)
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;

        let event: MembershipEventV2 = postcard::from_bytes(&response.bundle.candidate_event)
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        let commitment: AdmissionSecurityCommitmentV1 =
            postcard::from_bytes(&response.bundle.security_commitment)
                .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        let receipt: AdmissionActivationReceipt =
            postcard::from_bytes(&response.bundle.activation_receipt)
                .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        let MembershipOperationV2::AddDevice { admission } = &event.operation else {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        };
        let history_bytes = self
            .deps
            .admission_attempts
            .load_membership_history_v2()
            .await
            .map_err(transaction::map_repository_error)?
            .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
        let history = VersionedMembershipHistory::decode_persisted_v2(
            &history_bytes,
            self.membership
                .deps
                .historical_membership_signatures
                .as_ref(),
        )
        .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        let local_event = history
            .event(hello.event_id)
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let mut receipt_probe = history.clone();
        let receipt_outcome = receipt_probe
            .verify_and_record_activation_receipt(
                receipt.clone(),
                self.membership
                    .deps
                    .historical_membership_signatures
                    .as_ref(),
            )
            .map_err(|_| WorkspaceConvergenceError::InvalidConfirmation)?;
        let parent_has_helper = event.parent_event_id.is_some_and(|parent| {
            history
                .effective_members_at(parent)
                .contains(&hello.helper_member_instance)
                && history
                    .active_members_at(parent)
                    .contains(&hello.helper_member_instance)
        });
        if local_event != &event
            || receipt_outcome != MembershipActivationReceiptStoreOutcome::AlreadyKnown
            || history
                .current_position()
                .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?
                != saved_challenge.helper_history_position
            || !parent_has_helper
            || !history
                .active_members()
                .contains(&hello.helper_member_instance)
            || event.lineage_id != hello.lineage_id
            || event.event_id() != hello.event_id
            || event.author_member_instance_id != hello.sponsor_member_instance
            || admission.facts.member_instance != hello.joiner_member_instance
            || admission.resume_public_key_digest
                != admission_resume_public_key_digest(&hello.resume_public_key)
            || commitment.attempt_id != *hello.attempt_id.as_bytes()
            || commitment.security_commitment_id != admission.security_commitment_id
            || event.admission_bundle_digest != Some(commitment.admission_bundle_digest)
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let own_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.membership.deps.own_device)
            .await
            .map_err(|_| WorkspaceConvergenceError::Unavailable)?;
        let own_instance = own_credential.member_instance_id(&self.membership.deps.own_device);
        if own_instance != hello.helper_member_instance
            || own_credential.credential_id != saved_challenge.helper_credential_id
        {
            return Err(WorkspaceConvergenceError::InvalidConfirmation);
        }
        let encoded_challenge = postcard::to_stdvec(&saved_challenge)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let encoded_response = postcard::to_stdvec(response)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let attempt = match self.admission.load(hello.attempt_id).await? {
            Some(existing)
                if matches!(
                    existing.role_state,
                    uc_core::membership::AdmissionAttemptRoleStateV1::CompletionHelper(_)
                ) && existing.resume_peers.as_slice()
                    == std::slice::from_ref(&encoded_challenge)
                    && existing.completion_recovery_deliveries.as_slice()
                        == std::slice::from_ref(&encoded_response) =>
            {
                if existing.terminal_result
                    == Some(uc_core::membership::AdmissionTerminalResultV1::Completed)
                {
                    let complete = existing
                        .outboxes
                        .iter()
                        .find(|message| {
                            message.purpose
                                == uc_core::membership::AdmissionOutboxPurposeV1::Complete
                                && !message.superseded
                        })
                        .ok_or(WorkspaceConvergenceError::RecoveryRequired)?;
                    return durable_frame_from_outbox(
                        hello.attempt_id,
                        uc_core::pairing::DurableAdmissionMessageKind::Complete,
                        uc_core::membership::AdmissionOutboxPurposeV1::Complete,
                        complete,
                    );
                }
                existing
            }
            Some(_) => return Err(WorkspaceConvergenceError::InvalidConfirmation),
            None => {
                self.admission
                    .create_completion_helper(
                        hello.attempt_id,
                        &saved_challenge,
                        response,
                        &hello.lineage_id,
                        *hello.event_id.as_bytes(),
                        event.resulting_members_digest,
                    )
                    .await?
            }
        };
        self.membership
            .deps
            .activate_completion_helper_admission_security
            .activate_completion_helper_admission_security(
                uc_core::membership::ActivateCompletionHelperAdmissionSecurityRequest {
                    space_id: uc_core::ids::SpaceId::from_string(hello.lineage_id.clone()),
                    attempt_id: *hello.attempt_id.as_bytes(),
                    helper_device_id: self.membership.deps.own_device.clone(),
                    helper_credential_id: own_credential.credential_id,
                    candidate_core_digest: commitment.candidate_core_digest,
                    security_commit: response.bundle.security_commit.clone(),
                    security_welcome: response.bundle.security_welcome.clone(),
                    target_key_catalog: response.bundle.target_key_catalog.clone(),
                    existing_member_deliveries: response.bundle.existing_member_deliveries.clone(),
                    expected_commitment: commitment.clone(),
                },
            )
            .await
            .map_err(|error| WorkspaceConvergenceError::Inconsistent(error.to_string()))?;
        let receipt_digest: [u8; 32] =
            sha2::Sha256::digest(&response.bundle.activation_receipt).into();
        let mut completion = AdmissionCompletionV1::new(
            *hello.attempt_id.as_bytes(),
            hello.event_id,
            receipt_digest,
            commitment.security_commitment_id,
            own_instance,
            own_credential.credential_id,
            saved_challenge.helper_history_position,
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
        self.membership
            .save_member_facts(&admission.facts, self.membership.deps.clock.now_ms())
            .await?;
        let complete = self
            .admission
            .complete_as_helper(
                attempt,
                &completion_bytes,
                admission.facts.device_id.as_str().as_bytes(),
                saved_challenge.joiner_last_message_id,
            )
            .await?;
        durable_frame_from_outbox(
            hello.attempt_id,
            uc_core::pairing::DurableAdmissionMessageKind::Complete,
            uc_core::membership::AdmissionOutboxPurposeV1::Complete,
            &complete,
        )
    }

    pub(crate) async fn recover_completion_with_helper(
        &self,
        attempt_id: [u8; 32],
        helper_device: &DeviceId,
        helper_member_instance: uc_core::membership::MemberInstanceId,
        helper_route: &[u8],
    ) -> Result<crate::space::admission::adapter::DurableJoinerCompletion, WorkspaceConvergenceError>
    {
        let hello = self
            .prepare_completion_recovery_hello(attempt_id, helper_member_instance)
            .await?;
        let attempt = self
            .admission
            .load(uc_core::membership::AdmissionAttemptId::from_bytes(
                attempt_id,
            ))
            .await?
            .ok_or(WorkspaceConvergenceError::JoinNotFound)?;
        let joiner_last_message_id = attempt
            .outboxes
            .iter()
            .find(|message| {
                message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::Applied
                    && !message.superseded
            })
            .map(|message| message.message_id)
            .ok_or(WorkspaceConvergenceError::InvalidConfirmation)?;
        let challenge = self
            .deps
            .admission_completion_recovery
            .request_completion_recovery_challenge(
                helper_device,
                helper_route,
                hello.clone(),
                joiner_last_message_id,
            )
            .await
            .map_err(map_completion_recovery_transport_error)?;
        let response = self
            .respond_to_completion_recovery(&hello, &challenge)
            .await?;
        let complete = self
            .deps
            .admission_completion_recovery
            .submit_completion_recovery_response(helper_device, helper_route, hello, response)
            .await
            .map_err(map_completion_recovery_transport_error)?;
        self.activate_joiner_complete(&complete).await
    }

    pub(in crate::space) async fn recover_pending_admissions(
        &self,
    ) -> Result<usize, WorkspaceConvergenceError> {
        self.membership
            .deps
            .legacy_migration_recovery
            .recover()
            .await
            .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        let recoverable = self.admission.recoverable().await?;
        let mut recovered_completions = 0usize;
        for attempt in &recoverable {
            if !attempt.is_joiner()
                || attempt.stage_rank() != Some(5)
                || attempt.completion.is_some()
            {
                continue;
            }
            let Some(event_bytes) = attempt.candidate_event.as_deref() else {
                continue;
            };
            let Ok(event) =
                postcard::from_bytes::<uc_core::membership::MembershipEventV2>(event_bytes)
            else {
                continue;
            };
            let uc_core::membership::MembershipOperationV2::AddDevice { admission } =
                &event.operation
            else {
                continue;
            };
            let Some(relationships) = attempt.target_relationships.as_deref() else {
                continue;
            };
            for helper in relationships.iter().filter(|facts| {
                facts.member_instance != event.author_member_instance_id
                    && facts.member_instance != admission.facts.member_instance
            }) {
                if self
                    .recover_completion_with_helper(
                        *attempt.attempt_id.as_bytes(),
                        &helper.device_id,
                        helper.member_instance,
                        &helper.transport_address_blob,
                    )
                    .await
                    .is_ok()
                {
                    recovered_completions += 1;
                    break;
                }
            }
        }
        for attempt in recoverable {
            if !matches!(
                attempt.role_state,
                uc_core::membership::AdmissionAttemptRoleStateV1::Sponsor(
                    uc_core::membership::SponsorAdmissionStateV1 {
                        stage: uc_core::membership::SponsorAdmissionStageV1::Committed,
                    },
                )
            ) {
                continue;
            }
            let Some(receipt_payload) = attempt.write_ahead_recovery.clone() else {
                continue;
            };
            let commit_id = attempt
                .outboxes
                .iter()
                .find(|message| {
                    message.purpose == uc_core::membership::AdmissionOutboxPurposeV1::Commit
                })
                .map(|message| message.message_id)
                .ok_or_else(|| {
                    WorkspaceConvergenceError::Inconsistent(
                        "recoverable sponsor activation has no Commit".to_owned(),
                    )
                })?;
            let applied = transaction::durable_admission_message(
                attempt.attempt_id,
                uc_core::membership::AdmissionOutboxPurposeV1::Applied,
                self.membership.deps.own_device.as_str().as_bytes(),
                Some(commit_id),
                &receipt_payload,
            );
            let frame = uc_core::pairing::DurableAdmissionFrame {
                attempt_id: *attempt.attempt_id.as_bytes(),
                kind: uc_core::pairing::DurableAdmissionMessageKind::Applied,
                message_id: applied.message_id,
                predecessor_message_id: applied.predecessor_message_id,
                payload: receipt_payload,
            };
            self.complete_sponsor_applied(&frame).await?;
        }
        let report = self
            .admission
            .recover_with(self.membership.deps.admission_outbox_delivery.as_ref())
            .await?;
        if report.deliveries_confirmed > 0 || report.attempts_compacted > 0 {
            self.membership.notify();
        }
        Ok(report.deliveries_attempted + recovered_completions)
    }
}

#[async_trait]
impl uc_core::membership::AdmissionCompletionRecoveryEndpointPort
    for crate::space::admission::SpaceAdmission
{
    async fn handle_completion_recovery_hello(
        &self,
        hello: uc_core::membership::AdmissionCompletionRecoveryHelloV1,
        transport_binding: uc_core::membership::AdmissionCompletionRecoveryTransportBindingV1,
        joiner_last_message_id: [u8; 32],
        helper_last_message_id: [u8; 32],
    ) -> Result<
        uc_core::membership::AdmissionCompletionRecoveryChallengeV1,
        uc_core::membership::AdmissionCompletionRecoveryTransportError,
    > {
        self.challenge_completion_recovery(
            &hello,
            transport_binding,
            joiner_last_message_id,
            helper_last_message_id,
        )
        .await
        .map_err(|_| uc_core::membership::AdmissionCompletionRecoveryTransportError::Rejected)
    }

    async fn handle_completion_recovery_response(
        &self,
        hello: uc_core::membership::AdmissionCompletionRecoveryHelloV1,
        response: uc_core::membership::AdmissionCompletionRecoveryResponseV1,
        transport_binding: uc_core::membership::AdmissionCompletionRecoveryTransportBindingV1,
    ) -> Result<
        uc_core::pairing::DurableAdmissionFrame,
        uc_core::membership::AdmissionCompletionRecoveryTransportError,
    > {
        let saved = self
            .admission
            .load_completion_recovery_challenge(hello.attempt_id)
            .await
            .map_err(|_| uc_core::membership::AdmissionCompletionRecoveryTransportError::Rejected)?
            .ok_or(uc_core::membership::AdmissionCompletionRecoveryTransportError::Rejected)?;
        if saved.transport_binding != transport_binding {
            return Err(uc_core::membership::AdmissionCompletionRecoveryTransportError::Rejected);
        }
        self.complete_recovered_admission(&hello, &response)
            .await
            .map_err(|_| uc_core::membership::AdmissionCompletionRecoveryTransportError::Rejected)
    }
}

fn map_completion_recovery_transport_error(
    error: uc_core::membership::AdmissionCompletionRecoveryTransportError,
) -> WorkspaceConvergenceError {
    match error {
        uc_core::membership::AdmissionCompletionRecoveryTransportError::Offline
        | uc_core::membership::AdmissionCompletionRecoveryTransportError::Transport => {
            WorkspaceConvergenceError::Unavailable
        }
        uc_core::membership::AdmissionCompletionRecoveryTransportError::Rejected => {
            WorkspaceConvergenceError::InvalidConfirmation
        }
    }
}
