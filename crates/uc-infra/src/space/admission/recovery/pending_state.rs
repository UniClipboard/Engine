use async_trait::async_trait;
use uc_application::deps::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger, LoadedPendingAdmission,
    PendingAdmissionRecoveryStateError, PendingAdmissionRecoveryStatePort,
};
use uc_core::membership::{AdmissionRecordPersistence, JoinerAdmission, JoinerAdmissionTransition};

use crate::db::ports::DbExecutor;

use super::super::repository::codec::{into_anyhow, map_executor_error};
use super::super::repository::token::recovery_token;
use super::super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

#[async_trait]
impl<E: DbExecutor + Send + Sync> PendingAdmissionRecoveryStatePort
    for SqliteSpaceAdmissionState<E>
{
    #[tracing::instrument(name = "space_admission.recovery_state.load", skip_all, err)]
    async fn load(
        &self,
        _trigger: AdmissionRecoveryTrigger,
    ) -> Result<Vec<LoadedPendingAdmission>, PendingAdmissionRecoveryStateError> {
        self.executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                let mut loaded = Vec::new();
                for (admission_id, stored) in &state.records {
                    let aggregate = self
                        .open_record(*admission_id, stored)
                        .map_err(into_anyhow)?;
                    if aggregate.pending_recovery().is_none() {
                        continue;
                    }
                    let aggregate = JoinerAdmission::try_from_record(aggregate)
                        .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                    let token = AdmissionRecoveryCommitToken::from_bytes(recovery_token(
                        state.profile_generation,
                        &aggregate,
                    ))
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                    loaded.push(LoadedPendingAdmission::new(aggregate, token));
                }
                Ok(loaded)
            })
            .map_err(map_executor_error)
            .map_err(map_recovery_error)
    }

    #[tracing::instrument(name = "space_admission.recovery_state.commit", skip_all, err)]
    async fn commit(
        &self,
        token: AdmissionRecoveryCommitToken,
        transition: JoinerAdmissionTransition,
    ) -> Result<LoadedPendingAdmission, PendingAdmissionRecoveryStateError> {
        let replacement = transition.into_replacement();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self.load_state_on(conn).map_err(into_anyhow)?;
                    let admission_id = *replacement.admission_id().as_bytes();
                    let stored = state
                        .records
                        .get(&admission_id)
                        .cloned()
                        .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Conflict))?;
                    let current = self
                        .open_record(admission_id, &stored)
                        .map_err(into_anyhow)?;
                    let expected_token = recovery_token(state.profile_generation, &current);
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
                    if state.current_local_join_id == Some(admission_id)
                        && replacement.is_terminal()
                    {
                        state.current_local_join_id = None;
                    }
                    self.save_state_on(conn, &state).map_err(into_anyhow)?;
                    let next_token = AdmissionRecoveryCommitToken::from_bytes(recovery_token(
                        state.profile_generation,
                        &replacement,
                    ))
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                    Ok(LoadedPendingAdmission::new(replacement, next_token))
                })
            })
            .map_err(map_executor_error)
            .map_err(map_recovery_error)
    }
}

fn map_recovery_error(error: SpaceAdmissionStateStoreError) -> PendingAdmissionRecoveryStateError {
    match error {
        SpaceAdmissionStateStoreError::Locked => PendingAdmissionRecoveryStateError::Locked,
        SpaceAdmissionStateStoreError::Conflict => PendingAdmissionRecoveryStateError::StateChanged,
        SpaceAdmissionStateStoreError::Corrupt => {
            PendingAdmissionRecoveryStateError::RecoveryRequired
        }
        SpaceAdmissionStateStoreError::Unavailable => {
            PendingAdmissionRecoveryStateError::Unavailable
        }
    }
}
