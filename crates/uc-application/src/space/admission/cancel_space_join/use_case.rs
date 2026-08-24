use std::sync::Arc;

use tokio::sync::broadcast;
use uc_core::membership::AdmissionOutboxPurposeV1;

use super::CancelSpaceJoinError;
use crate::deps::{AdmissionAttemptRepositoryError, AdmissionAttemptRepositoryPort};
use crate::space::admission::durable;
use crate::space::admission::query_space_join_status::QuerySpaceJoinStatusUseCase;
use crate::space::admission::CurrentJoinStatus;
use crate::space::workspace_membership::WorkspaceConvergenceError;

pub(in crate::space) async fn confirm_superseded_join_cleanup_delivery(
    repository: &dyn AdmissionAttemptRepositoryPort,
    attempt_id: uc_core::membership::AdmissionAttemptId,
    acknowledgment: &uc_core::membership::AdmissionInboxRecordV1,
) -> Result<(), WorkspaceConvergenceError> {
    use uc_core::membership::AdmissionTerminalResultV1;

    let mut attempt = repository
        .load(attempt_id)
        .await
        .map_err(durable::map_repository_error)?
        .ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent("admission attempt was not found".into())
        })?;
    if attempt.terminal_result != Some(AdmissionTerminalResultV1::SupersededByNewJoin) {
        return Err(WorkspaceConvergenceError::Inconsistent(
            "admission is not a superseded local join".into(),
        ));
    }
    let index = attempt
        .outboxes
        .iter()
        .position(|message| durable::admission_acknowledgment(message) == *acknowledgment)
        .ok_or_else(|| {
            WorkspaceConvergenceError::Inconsistent(
                "superseded delivery acknowledgment does not match an outbox".into(),
            )
        })?;
    if attempt.outboxes[index].purpose == AdmissionOutboxPurposeV1::CancelRequested {
        attempt.outboxes[index].superseded = true;
    }
    if !attempt.inbox_dedup.contains(acknowledgment) {
        attempt.inbox_dedup.push(acknowledgment.clone());
    }
    let expected_version = attempt.record_version;
    attempt.record_version = expected_version.checked_add(1).ok_or_else(|| {
        WorkspaceConvergenceError::Inconsistent("admission record version overflow".into())
    })?;
    repository
        .compare_and_advance(attempt_id, expected_version, &attempt)
        .await
        .map_err(durable::map_repository_error)?;
    Ok(())
}

pub(crate) struct CancelSpaceJoinUseCase {
    repository: Arc<dyn AdmissionAttemptRepositoryPort>,
    query_status: QuerySpaceJoinStatusUseCase,
    events: broadcast::Sender<u64>,
}

impl CancelSpaceJoinUseCase {
    pub(crate) fn new(
        repository: Arc<dyn AdmissionAttemptRepositoryPort>,
        events: broadcast::Sender<u64>,
    ) -> Self {
        Self {
            query_status: QuerySpaceJoinStatusUseCase::new(Arc::clone(&repository)),
            repository,
            events,
        }
    }

    pub(crate) async fn execute(
        &self,
        join_id: [u8; 16],
    ) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        let projection = self
            .repository
            .project_current_local_join()
            .await
            .map_err(map_repository_error)?
            .filter(|projection| projection.join_id == join_id)
            .ok_or(CancelSpaceJoinError::NotFound)?;

        if projection.terminal_result.is_some() {
            return self.query_current_status().await;
        }

        let mut attempt = self
            .repository
            .load(projection.attempt_id)
            .await
            .map_err(map_repository_error)?
            .ok_or_else(|| {
                CancelSpaceJoinError::State("current local join attempt is missing".to_owned())
            })?;

        if attempt.cancel_request.is_none() {
            let recipient = attempt
                .outboxes
                .iter()
                .find(|message| message.purpose == AdmissionOutboxPurposeV1::JoinRequest)
                .map(|message| message.recipient.clone())
                .ok_or_else(|| {
                    CancelSpaceJoinError::State(
                        "local join request recipient is missing".to_owned(),
                    )
                })?;
            let predecessor = attempt
                .outboxes
                .iter()
                .rev()
                .find(|message| !message.superseded)
                .map(|message| message.message_id);

            let payload = b"cancel_requested";
            attempt.cancel_request = Some(payload.to_vec());
            attempt.outboxes.push(durable::durable_admission_message(
                projection.attempt_id,
                AdmissionOutboxPurposeV1::CancelRequested,
                &recipient,
                predecessor,
                payload,
            ));

            let expected_version = attempt.record_version;
            attempt.record_version = expected_version.checked_add(1).ok_or_else(|| {
                CancelSpaceJoinError::State("admission record version overflow".to_owned())
            })?;
            self.repository
                .compare_and_advance(projection.attempt_id, expected_version, &attempt)
                .await
                .map_err(map_repository_error)?;
        }

        let result = self.query_current_status().await?;
        let revision = self
            .repository
            .profile_metadata()
            .await
            .map_err(map_repository_error)?
            .device_trust_revision;
        let _ = self.events.send(revision);
        Ok(result)
    }

    async fn query_current_status(&self) -> Result<CurrentJoinStatus, CancelSpaceJoinError> {
        self.query_status
            .execute()
            .await
            .map_err(|error| CancelSpaceJoinError::State(error.to_string()))?
            .ok_or(CancelSpaceJoinError::NotFound)
    }
}

fn map_repository_error(error: AdmissionAttemptRepositoryError) -> CancelSpaceJoinError {
    CancelSpaceJoinError::State(error.to_string())
}
