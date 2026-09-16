use async_trait::async_trait;
use uc_application::deps::{
    JoinerActivationCommitToken, JoinerActivationMutation, JoinerActivationStateError,
    JoinerActivationStatePort, LoadedJoinerActivation, ValidateJoinerActivationIntentPort,
};
use uc_core::membership::{
    AdmissionEffect, AdmissionRecordPersistence, JoinerAdmission, SpaceAdmissionMessageKind,
};
use uc_observability_contract::diagnostics::connectivity::{observe_local_result, LocalWorkStep};

use crate::db::ports::DbExecutor;

use super::super::repository::codec::{into_anyhow, map_executor_error};
use super::super::repository::token::joiner_activation_token;
use super::super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

#[async_trait]
impl<E: DbExecutor + Send + Sync> JoinerActivationStatePort for SqliteSpaceAdmissionState<E> {
    #[tracing::instrument(name = "space_admission.joiner_activation_state.load", skip_all, err)]
    async fn load(&self) -> Result<Option<LoadedJoinerActivation>, JoinerActivationStateError> {
        self.executor
            .run(|conn| {
                let Some((profile_generation, admission_id, stored)) =
                    self.load_current_record_on(conn).map_err(into_anyhow)?
                else {
                    return Ok(None);
                };
                let record = self
                    .open_record(admission_id, &stored)
                    .map_err(into_anyhow)?;
                let admission = JoinerAdmission::try_from_record(record)
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                if admission.joiner_activation_preparation().is_none() {
                    return Ok(None);
                }
                let token = JoinerActivationCommitToken::from_bytes(joiner_activation_token(
                    profile_generation,
                    &admission,
                ))
                .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                Ok(Some(LoadedJoinerActivation::new(admission, token)))
            })
            .map_err(map_executor_error)
            .map_err(map_activation_error)
    }

    #[tracing::instrument(name = "space_admission.joiner_activation_state.commit", skip_all, err)]
    async fn commit(
        &self,
        token: JoinerActivationCommitToken,
        mutation: JoinerActivationMutation,
    ) -> Result<(), JoinerActivationStateError> {
        observe_local_result(LocalWorkStep::JoinerStateCommit, async {
            let transition = mutation.into_transition();
            if transition.effects() != [AdmissionEffect::PublishActive] {
                return Err(JoinerActivationStateError::recovery_required(
                    anyhow::anyhow!("joiner activation mutation has invalid effects"),
                ));
            }
            let replacement = transition.into_replacement();
            if !replacement.is_terminal()
                || replacement.current_exact_reply().map(|reply| reply.kind())
                    != Some(SpaceAdmissionMessageKind::CompleteAck)
            {
                return Err(JoinerActivationStateError::recovery_required(
                    anyhow::anyhow!("joiner activation mutation has invalid replacement state"),
                ));
            }

            self.executor
                .run(|conn| {
                    conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                        let mut state = self.load_state_on(conn).map_err(into_anyhow)?;
                        let admission_id = *replacement.admission_id().as_bytes();
                        if state.current_local_join_id != Some(admission_id) {
                            return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict));
                        }
                        let stored =
                            state.records.get(&admission_id).cloned().ok_or_else(|| {
                                into_anyhow(SpaceAdmissionStateStoreError::Conflict)
                            })?;
                        let current = self
                            .open_record(admission_id, &stored)
                            .map_err(into_anyhow)?;
                        let current = JoinerAdmission::try_from_record(current)
                            .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                        if current.joiner_activation_preparation().is_none() {
                            return Err(into_anyhow(SpaceAdmissionStateStoreError::Corrupt));
                        }
                        let expected_token =
                            joiner_activation_token(state.profile_generation, &current);
                        let expected_version = current
                            .record_version()
                            .checked_add(1)
                            .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                        if token.as_bytes() != &expected_token
                            || replacement.record_version() != expected_version
                        {
                            return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict));
                        }
                        let sealed = self
                            .seal_record(&replacement, stored.wrapped_data_key)
                            .map_err(into_anyhow)?;
                        state.records.insert(admission_id, sealed);
                        state.current_local_join_id = None;
                        self.save_state_on(conn, &state).map_err(into_anyhow)
                    })
                })
                .map_err(map_executor_error)
                .map_err(map_activation_error)
        })
        .await
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> ValidateJoinerActivationIntentPort
    for SqliteSpaceAdmissionState<E>
{
    async fn validate(
        &self,
        intent: uc_application::deps::JoinerActivationIntent,
    ) -> Result<bool, JoinerActivationStateError> {
        self.executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                if state.current_local_join_id != Some(*intent.admission_id().as_bytes()) {
                    return Ok(false);
                }
                let stored = state
                    .records
                    .get(intent.admission_id().as_bytes())
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                let record = self
                    .open_record(*intent.admission_id().as_bytes(), stored)
                    .map_err(into_anyhow)?;
                let admission = JoinerAdmission::try_from_record(record)
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                let Some(preparation) = admission.joiner_activation_preparation() else {
                    return Ok(false);
                };
                let current = uc_application::deps::JoinerActivationIntent::from_saved_plan(
                    intent.admission_id(),
                    preparation.space_transition().as_bytes(),
                )
                .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                Ok(current == intent)
            })
            .map_err(map_executor_error)
            .map_err(map_activation_error)
    }
}

fn map_activation_error(error: SpaceAdmissionStateStoreError) -> JoinerActivationStateError {
    match &error {
        SpaceAdmissionStateStoreError::Locked => JoinerActivationStateError::locked(error),
        SpaceAdmissionStateStoreError::Conflict => JoinerActivationStateError::state_changed(error),
        SpaceAdmissionStateStoreError::Corrupt => {
            JoinerActivationStateError::recovery_required(error)
        }
        SpaceAdmissionStateStoreError::Unavailable => {
            JoinerActivationStateError::unavailable(error)
        }
    }
}
