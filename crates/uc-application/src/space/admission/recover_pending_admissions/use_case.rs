use std::sync::Arc;

use crate::deps::{
    AdmissionAttemptRepositoryPort, AdmissionOutboxDeliveryPort, AdmissionOutboxDeliveryResultV1,
    AdmissionOutboxDeliveryRouteV1, InvitationConsumeDeliveryResultV1,
};
use crate::space::admission::durable::transaction;
use crate::space::admission::durable::DurableAdmissionTransaction;
use crate::space::admission::SpaceAdmission;
use crate::space::workspace_membership::WorkspaceConvergenceError;
use uc_core::membership::{
    AdmissionAttemptV1, AdmissionOutboxMessageV1, AdmissionOutboxPurposeV1, MembershipEventV2,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct AdmissionRecoveryReportV1 {
    pub(crate) deliveries_attempted: usize,
    pub(crate) deliveries_confirmed: usize,
    pub(crate) attempts_compacted: usize,
}

/// Resumes every persisted Space admission after startup, resume, or retry.
pub(crate) struct RecoverPendingAdmissionsUseCase {
    admission: Arc<SpaceAdmission>,
}

impl RecoverPendingAdmissionsUseCase {
    pub(crate) fn new(admission: Arc<SpaceAdmission>) -> Self {
        Self { admission }
    }

    pub(crate) async fn execute(&self) -> Result<usize, WorkspaceConvergenceError> {
        self.admission
            .membership
            .deps
            .legacy_migration_recovery
            .recover()
            .await
            .map_err(|_| WorkspaceConvergenceError::RecoveryRequired)?;
        let recoverable = self.admission.admission.recoverable().await?;
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
                    .admission
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
                self.admission
                    .membership
                    .deps
                    .own_device
                    .as_str()
                    .as_bytes(),
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
            self.admission.complete_sponsor_applied(&frame).await?;
        }
        let report = recover_outbox_deliveries(
            &self.admission.admission,
            self.admission.membership.deps.admission_attempts.as_ref(),
            self.admission
                .membership
                .deps
                .admission_outbox_delivery
                .as_ref(),
        )
        .await?;
        Ok(report.deliveries_attempted + recovered_completions)
    }
}

pub(crate) async fn recover_outbox_deliveries(
    transaction: &DurableAdmissionTransaction,
    repository: &dyn AdmissionAttemptRepositoryPort,
    delivery: &(impl AdmissionOutboxDeliveryPort + ?Sized),
) -> Result<AdmissionRecoveryReportV1, WorkspaceConvergenceError> {
    let attempts = transaction.recoverable().await?;
    let mut report = AdmissionRecoveryReportV1::default();
    for attempt in attempts {
        for message in attempt
            .outboxes
            .iter()
            .filter(|message| !message.superseded)
        {
            report.deliveries_attempted += 1;
            let route = admission_outbox_delivery_route(&attempt, message)?;
            let Ok(outcome) = delivery
                .deliver(attempt.attempt_id, message, route.as_ref())
                .await
            else {
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
                            crate::space::admission::joiner::InvitationConsumeResultV1::Consumed
                        }
                        InvitationConsumeDeliveryResultV1::NotFound => {
                            crate::space::admission::joiner::InvitationConsumeResultV1::NotFound
                        }
                        InvitationConsumeDeliveryResultV1::Conflict => {
                            crate::space::admission::joiner::InvitationConsumeResultV1::Conflict
                        }
                    };
                    crate::space::admission::joiner::record_invitation_consume_result(
                        repository,
                        attempt.attempt_id,
                        result,
                    )
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
                            record_protocol_message_delivered(
                                repository,
                                attempt.attempt_id,
                                &acknowledgment,
                            )
                            .await?;
                        }
                        AdmissionOutboxPurposeV1::Rejected => {
                            crate::space::admission::sponsor::confirm_rejected_delivery(
                                repository,
                                attempt.attempt_id,
                                &acknowledgment,
                            )
                            .await?;
                        }
                        AdmissionOutboxPurposeV1::Complete => {
                            crate::space::admission::sponsor::confirm_complete_delivery(
                                repository,
                                attempt.attempt_id,
                                &acknowledgment,
                            )
                            .await?;
                        }
                        AdmissionOutboxPurposeV1::ExistingMemberSecurityUpdate
                        | AdmissionOutboxPurposeV1::HistoryOrReceiptBatch => {
                            transaction
                                .acknowledge_persisted_delivery(
                                    attempt.attempt_id,
                                    message.purpose,
                                    &acknowledgment,
                                )
                                .await?;
                        }
                        AdmissionOutboxPurposeV1::CancelRequested => {
                            crate::space::admission::cancel_space_join::confirm_superseded_join_cleanup_delivery(
                                repository,
                                attempt.attempt_id,
                                &acknowledgment,
                            )
                            .await?;
                        }
                        AdmissionOutboxPurposeV1::InvitationConsume => {
                            return Err(inconsistent(
                                "persisted acknowledgment cannot clear this admission outbox",
                            ));
                        }
                    }
                    true
                }
                AdmissionOutboxDeliveryResultV1::Rejected(rejected) => {
                    if message.purpose != AdmissionOutboxPurposeV1::CancelRequested {
                        return Err(inconsistent(
                            "rejection does not match admission outbox purpose",
                        ));
                    }
                    transaction
                        .joiner_record_rejected(attempt.attempt_id, &rejected)
                        .await?;
                    true
                }
            };
            if confirmed {
                report.deliveries_confirmed += 1;
            }
        }
        let Some(current) = transaction.load(attempt.attempt_id).await? else {
            continue;
        };
        if current.is_terminal()
            && current.outboxes.iter().all(|message| message.superseded)
            && current.write_ahead_recovery.is_none()
            && (current.space_transition.is_none() || current.space_transition_result.is_some())
            && !current.cleanup_pending
        {
            transaction.compact_if_settled(attempt.attempt_id).await?;
            report.attempts_compacted += 1;
        }
    }
    Ok(report)
}

pub(crate) async fn record_protocol_message_delivered(
    repository: &dyn AdmissionAttemptRepositoryPort,
    attempt_id: uc_core::membership::AdmissionAttemptId,
    acknowledgment: &uc_core::membership::AdmissionInboxRecordV1,
) -> Result<(), WorkspaceConvergenceError> {
    let mut attempt = repository
        .load(attempt_id)
        .await
        .map_err(crate::space::admission::durable::map_repository_error)?
        .ok_or_else(|| inconsistent("admission attempt was not found"))?;
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
                && crate::space::admission::durable::admission_acknowledgment(message)
                    == *acknowledgment
        })
        .ok_or_else(|| inconsistent("delivery acknowledgment does not match an outbox"))?;
    attempt.outboxes[index].superseded = true;
    if !attempt.inbox_dedup.contains(acknowledgment) {
        attempt.inbox_dedup.push(acknowledgment.clone());
    }
    let expected_version = attempt.record_version;
    attempt.record_version = expected_version
        .checked_add(1)
        .ok_or_else(|| inconsistent("admission record version overflow"))?;
    repository
        .compare_and_advance(attempt_id, expected_version, &attempt)
        .await
        .map_err(crate::space::admission::durable::map_repository_error)?;
    Ok(())
}

fn admission_outbox_delivery_route(
    attempt: &AdmissionAttemptV1,
    message: &AdmissionOutboxMessageV1,
) -> Result<Option<AdmissionOutboxDeliveryRouteV1>, WorkspaceConvergenceError> {
    if message.purpose != AdmissionOutboxPurposeV1::CancelRequested {
        return Ok(None);
    }
    if let (Some(event), Some(relationships)) = (
        attempt.candidate_event.as_deref(),
        attempt.target_relationships.as_deref(),
    ) {
        let event: MembershipEventV2 = postcard::from_bytes(event)
            .map_err(|error| WorkspaceConvergenceError::AdmissionStorage(error.to_string()))?;
        let mut sponsors = relationships
            .iter()
            .filter(|facts| facts.member_instance == event.author_member_instance_id);
        let sponsor = sponsors.next().ok_or_else(|| {
            inconsistent("superseded join has no sponsor continuation relationship")
        })?;
        if sponsors.next().is_some() {
            return Err(inconsistent(
                "superseded join has duplicate sponsor continuation relationships",
            ));
        }
        if !sponsor.transport_address_blob.is_empty() {
            return Ok(Some(AdmissionOutboxDeliveryRouteV1::Continuation(
                sponsor.transport_address_blob.clone(),
            )));
        }
    }
    if let Some(address) = attempt
        .sponsor_continuation_address
        .as_ref()
        .filter(|address| !address.is_empty())
    {
        return Ok(Some(AdmissionOutboxDeliveryRouteV1::Continuation(
            address.clone(),
        )));
    }
    if message.recipient.is_empty() {
        return Err(inconsistent(
            "superseded join cleanup has no delivery route",
        ));
    }
    Ok(Some(AdmissionOutboxDeliveryRouteV1::Invitation(
        message.recipient.clone(),
    )))
}

fn inconsistent(message: impl Into<String>) -> WorkspaceConvergenceError {
    WorkspaceConvergenceError::Inconsistent(message.into())
}
