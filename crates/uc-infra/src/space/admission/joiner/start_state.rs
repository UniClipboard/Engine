use async_trait::async_trait;
use uc_application::deps::{
    JoinerStartMutation, JoinerStartStateError, JoinerStartStatePort, LoadedJoinerStartState,
    SpaceAdmissionCommitToken,
};
use uc_core::membership::{AdmissionRecordPersistence, JoinerAdmission};

use crate::db::ports::DbExecutor;

use super::super::repository::codec::{into_anyhow, map_executor_error};
use super::super::repository::token::joiner_start_token;
use super::super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

#[async_trait]
impl<E: DbExecutor + Send + Sync> JoinerStartStatePort for SqliteSpaceAdmissionState<E> {
    #[tracing::instrument(name = "space_admission.joiner_state.load", skip_all, err)]
    async fn load(&self) -> Result<LoadedJoinerStartState, JoinerStartStateError> {
        let (source_snapshot, requires_session_transition) = self
            .load_source_snapshot()
            .await
            .map_err(map_joiner_error)?;
        let source_bytes = source_snapshot.as_bytes().to_vec();
        self.executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                let current_join = state
                    .current_local_join_id
                    .map(|id| {
                        let stored = state
                            .records
                            .get(&id)
                            .ok_or(SpaceAdmissionStateStoreError::Corrupt)?;
                        let record = self.open_record(id, stored)?;
                        JoinerAdmission::try_from_record(record)
                            .ok_or(SpaceAdmissionStateStoreError::Corrupt)
                    })
                    .transpose()?;
                let token = SpaceAdmissionCommitToken::from_bytes(joiner_start_token(
                    &state,
                    current_join.as_ref(),
                    &source_bytes,
                ))
                .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                Ok(LoadedJoinerStartState::new(
                    state.next_local_join_ordinal,
                    source_snapshot,
                    current_join,
                    requires_session_transition,
                    token,
                ))
            })
            .map_err(map_executor_error)
            .map_err(map_joiner_error)
    }

    #[tracing::instrument(name = "space_admission.joiner_state.commit", skip_all, err)]
    async fn commit(
        &self,
        token: SpaceAdmissionCommitToken,
        mutation: JoinerStartMutation,
    ) -> Result<(), JoinerStartStateError> {
        let (source_snapshot, _) = self
            .load_source_snapshot()
            .await
            .map_err(map_joiner_error)?;
        let source_bytes = source_snapshot.as_bytes().to_vec();
        let (created, superseded) = mutation.into_parts();
        let created = created.into_replacement();
        let superseded = superseded.map(|transition| transition.into_replacement());

        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self.load_state_on(conn).map_err(into_anyhow)?;
                    let current_id = state.current_local_join_id;
                    let current = current_id
                        .map(|id| {
                            let stored = state
                                .records
                                .get(&id)
                                .ok_or(SpaceAdmissionStateStoreError::Corrupt)?;
                            let record = self.open_record(id, stored)?;
                            JoinerAdmission::try_from_record(record)
                                .ok_or(SpaceAdmissionStateStoreError::Corrupt)
                        })
                        .transpose()?;
                    let expected = joiner_start_token(&state, current.as_ref(), &source_bytes);
                    if token.as_bytes() != &expected {
                        return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict));
                    }
                    if created.record_version() != 0
                        || state
                            .records
                            .contains_key(created.admission_id().as_bytes())
                    {
                        return Err(into_anyhow(SpaceAdmissionStateStoreError::Corrupt));
                    }

                    match (current_id, current.as_ref(), superseded.as_ref()) {
                        (None, None, None) => {}
                        (Some(id), Some(current), Some(next)) => {
                            let expected_version =
                                current.record_version().checked_add(1).ok_or_else(|| {
                                    into_anyhow(SpaceAdmissionStateStoreError::Corrupt)
                                })?;
                            if next.admission_id().as_bytes() != &id
                                || next.record_version() != expected_version
                                || !next.is_terminal()
                            {
                                return Err(into_anyhow(SpaceAdmissionStateStoreError::Corrupt));
                            }
                            let wrapped = state
                                .records
                                .get(&id)
                                .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?
                                .wrapped_data_key
                                .clone();
                            let sealed = self.seal_record(next, wrapped).map_err(into_anyhow)?;
                            state.records.insert(id, sealed);
                        }
                        _ => return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict)),
                    }

                    let created_id = *created.admission_id().as_bytes();
                    let sealed = self.seal_new_record(&created).map_err(into_anyhow)?;
                    state.records.insert(created_id, sealed);
                    state.current_local_join_id = Some(created_id);
                    state.next_local_join_ordinal = state
                        .next_local_join_ordinal
                        .checked_add(1)
                        .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                    self.save_state_on(conn, &state).map_err(into_anyhow)
                })
            })
            .map_err(map_executor_error)
            .map_err(map_joiner_error)
    }
}

fn map_joiner_error(error: SpaceAdmissionStateStoreError) -> JoinerStartStateError {
    match error {
        SpaceAdmissionStateStoreError::Locked => JoinerStartStateError::Locked,
        SpaceAdmissionStateStoreError::Conflict => JoinerStartStateError::StateChanged,
        SpaceAdmissionStateStoreError::Corrupt => JoinerStartStateError::RecoveryRequired,
        SpaceAdmissionStateStoreError::Unavailable => JoinerStartStateError::Unavailable,
    }
}
