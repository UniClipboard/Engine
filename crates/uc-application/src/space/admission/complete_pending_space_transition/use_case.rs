use std::sync::Arc;

use uc_core::membership::{
    AdmissionAttemptId, AdmissionAttemptRoleStateV1, AdmissionAttemptV1,
    AdmissionSpaceTransitionV2, AdmissionTerminalResultV1, JoinerAdmissionStageV1,
    JoinerAdmissionStateV1,
};

use super::CompletePendingSpaceTransitionError;
use crate::deps::{
    AdmissionAttemptRepositoryPort, AdmissionSpaceTransitionPort, AdmissionSpaceTransitionStepV2,
};
use crate::space::admission::durable;
use crate::space::admission::query_space_join_status::QuerySpaceJoinStatusUseCase;
use crate::space::admission::CurrentJoinStatus;

pub(crate) struct CompletePendingSpaceTransitionUseCase {
    admission_attempts: Arc<dyn AdmissionAttemptRepositoryPort>,
    space_transition: Arc<dyn AdmissionSpaceTransitionPort>,
    query_join_status: QuerySpaceJoinStatusUseCase,
}

impl CompletePendingSpaceTransitionUseCase {
    pub(crate) fn new(
        admission_attempts: Arc<dyn AdmissionAttemptRepositoryPort>,
        space_transition: Arc<dyn AdmissionSpaceTransitionPort>,
    ) -> Self {
        Self {
            query_join_status: QuerySpaceJoinStatusUseCase::new(Arc::clone(&admission_attempts)),
            admission_attempts,
            space_transition,
        }
    }

    /// Completes a persisted cross-Space join after the current session has
    /// been drained. Repeating the call after completion returns the same
    /// active join.
    pub(crate) async fn execute(
        &self,
    ) -> Result<CurrentJoinStatus, CompletePendingSpaceTransitionError> {
        let attempts = self
            .admission_attempts
            .scan_recoverable()
            .await
            .map_err(durable::map_repository_error)
            .map_err(state)?;
        for attempt in attempts.into_iter().filter(is_pending_space_transition) {
            let attempt_id = attempt.attempt_id;
            self.complete_transition(attempt).await?;
            self.compact_if_settled(attempt_id).await?;
        }

        let status = self
            .query_join_status
            .execute()
            .await
            .map_err(state)?
            .ok_or(CompletePendingSpaceTransitionError::JoinNotActive)?;
        if !matches!(status, CurrentJoinStatus::Active { .. }) {
            return Err(CompletePendingSpaceTransitionError::JoinNotActive);
        }
        Ok(status)
    }

    async fn complete_transition(
        &self,
        mut attempt: AdmissionAttemptV1,
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        if !attempt.is_joiner() || attempt.completion.is_none() {
            return Err(state("Space transition started before join completion"));
        }

        loop {
            let transition = AdmissionSpaceTransitionV2::decode(
                attempt
                    .space_transition
                    .as_deref()
                    .ok_or_else(|| state("pending Space transition is missing"))?,
            )
            .ok_or_else(|| state("pending Space transition is invalid"))?;

            match self
                .space_transition
                .advance(&transition)
                .await
                .map_err(state)?
            {
                AdmissionSpaceTransitionStepV2::Advanced(next) => {
                    if !transition.can_advance_to(&next) {
                        return Err(state("Space transition skipped or replaced a phase"));
                    }
                    attempt.space_transition = Some(
                        next.encode()
                            .ok_or_else(|| state("advanced Space transition is invalid"))?,
                    );
                    self.persist_advance(attempt).await?;
                    attempt = self.load_required(transition.attempt_id()).await?;
                }
                AdmissionSpaceTransitionStepV2::Finished(result) => {
                    if !result.matches_cleanup_pending(&transition) {
                        return Err(state(
                            "Space transition result does not match cleanup state",
                        ));
                    }
                    let verified_history = attempt
                        .verified_membership_history
                        .clone()
                        .ok_or_else(|| state("Space transition verified history is missing"))?;
                    attempt.space_transition_result = Some(
                        result
                            .encode()
                            .ok_or_else(|| state("Space transition result cannot be encoded"))?,
                    );
                    attempt.terminal_result = Some(AdmissionTerminalResultV1::Active);
                    attempt.role_state =
                        AdmissionAttemptRoleStateV1::Joiner(JoinerAdmissionStateV1 {
                            stage: JoinerAdmissionStageV1::Completed,
                        });
                    self.persist_advance_with_history(
                        attempt,
                        Some(&verified_history),
                        &verified_history,
                    )
                    .await?;
                    return Ok(());
                }
            }
        }
    }

    async fn persist_advance(
        &self,
        mut attempt: AdmissionAttemptV1,
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        let expected_version = attempt.record_version;
        attempt.record_version = expected_version
            .checked_add(1)
            .ok_or_else(|| state("admission record version overflow"))?;
        self.admission_attempts
            .compare_and_advance(attempt.attempt_id, expected_version, &attempt)
            .await
            .map_err(durable::map_repository_error)
            .map_err(state)?;
        Ok(())
    }

    async fn persist_advance_with_history(
        &self,
        mut attempt: AdmissionAttemptV1,
        expected_history: Option<&[u8]>,
        history: &[u8],
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        let expected_version = attempt.record_version;
        attempt.record_version = expected_version
            .checked_add(1)
            .ok_or_else(|| state("admission record version overflow"))?;
        self.admission_attempts
            .compare_and_advance_with_membership_history_v2(
                attempt.attempt_id,
                expected_version,
                &attempt,
                expected_history,
                history,
            )
            .await
            .map_err(durable::map_repository_error)
            .map_err(state)?;
        Ok(())
    }

    async fn compact_if_settled(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<(), CompletePendingSpaceTransitionError> {
        if self
            .admission_attempts
            .load_terminal(attempt_id)
            .await
            .map_err(durable::map_repository_error)
            .map_err(state)?
            .is_some()
        {
            return Ok(());
        }
        let attempt = self.load_required(attempt_id).await?;
        if !attempt.is_terminal()
            || attempt.outboxes.iter().any(|message| !message.superseded)
            || attempt.write_ahead_recovery.is_some()
            || (attempt.space_transition.is_some() && attempt.space_transition_result.is_none())
            || attempt.cleanup_pending
        {
            return Err(state("completed Space transition is not settled"));
        }
        self.admission_attempts
            .compact_terminal(attempt_id, attempt.record_version)
            .await
            .map_err(durable::map_repository_error)
            .map_err(state)?;
        Ok(())
    }

    async fn load_required(
        &self,
        attempt_id: AdmissionAttemptId,
    ) -> Result<AdmissionAttemptV1, CompletePendingSpaceTransitionError> {
        self.admission_attempts
            .load(attempt_id)
            .await
            .map_err(durable::map_repository_error)
            .map_err(state)?
            .ok_or_else(|| state("admission attempt was not found"))
    }
}

fn is_pending_space_transition(attempt: &AdmissionAttemptV1) -> bool {
    attempt.is_joiner()
        && attempt.completion.is_some()
        && attempt.space_transition.is_some()
        && attempt.space_transition_result.is_none()
}

fn state(error: impl std::fmt::Display) -> CompletePendingSpaceTransitionError {
    CompletePendingSpaceTransitionError::State(error.to_string())
}
