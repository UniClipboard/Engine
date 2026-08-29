use async_trait::async_trait;
use rand::RngCore;
use uc_application::deps::{
    CurrentJoinAdmissionStatePort, JoinerCancellationCommitToken, JoinerCancellationMaterial,
    JoinerCancellationMaterialError, JoinerCancellationMutation, JoinerCancellationStateError,
    LoadCurrentJoinStatusPort, LoadedCurrentJoin, PrepareJoinerCancellationPort,
    QueryDeviceTrustError,
};
use uc_application::facade::CurrentJoinStatus;
use uc_core::membership::{
    AdmissionMessageId, AdmissionRecordPersistence, AdmissionRetryState, JoinId, JoinerAdmission,
};

use crate::db::ports::DbExecutor;

use super::super::repository::codec::{into_anyhow, map_executor_error};
use super::super::repository::token::recovery_token;
use super::super::repository::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};

pub struct DefaultJoinerCancellationPreparation;

#[async_trait]
impl PrepareJoinerCancellationPort for DefaultJoinerCancellationPreparation {
    async fn prepare(&self) -> Result<JoinerCancellationMaterial, JoinerCancellationMaterialError> {
        let retry_state = AdmissionRetryState::new(0, 0).map_err(|error| {
            JoinerCancellationMaterialError::unavailable(anyhow::Error::new(error))
        })?;
        Ok(JoinerCancellationMaterial::new(
            mint_message_id(),
            retry_state,
        ))
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> CurrentJoinAdmissionStatePort for SqliteSpaceAdmissionState<E> {
    #[tracing::instrument(name = "space_admission.current_join_state.load", skip_all, err)]
    async fn load(
        &self,
        join_id: JoinId,
    ) -> Result<Option<LoadedCurrentJoin>, JoinerCancellationStateError> {
        self.executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                let Some(admission_id) = state.current_local_join_id else {
                    return Ok(None);
                };
                let stored = state
                    .records
                    .get(&admission_id)
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                let record = self
                    .open_record(admission_id, stored)
                    .map_err(into_anyhow)?;
                let admission = JoinerAdmission::try_from_record(record)
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                if admission.join_id() != join_id {
                    return Ok(None);
                }
                let token = JoinerCancellationCommitToken::from_bytes(recovery_token(
                    state.profile_generation,
                    &admission,
                ))
                .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                Ok(Some(LoadedCurrentJoin::new(admission, token)))
            })
            .map_err(map_executor_error)
            .map_err(map_state_error)
    }

    #[tracing::instrument(name = "space_admission.current_join_state.commit", skip_all, err)]
    async fn commit(
        &self,
        token: JoinerCancellationCommitToken,
        mutation: JoinerCancellationMutation,
    ) -> Result<(), JoinerCancellationStateError> {
        let transition = mutation.into_transition();
        if !transition.effects().is_empty() {
            return Err(JoinerCancellationStateError::recovery_required(
                anyhow::anyhow!("joiner cancellation mutation has unexpected effects"),
            ));
        }
        let replacement = transition.into_replacement();
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let mut state = self.load_state_on(conn).map_err(into_anyhow)?;
                    let admission_id = *replacement.admission_id().as_bytes();
                    if state.current_local_join_id != Some(admission_id) {
                        return Err(into_anyhow(SpaceAdmissionStateStoreError::Conflict));
                    }
                    let stored = state
                        .records
                        .get(&admission_id)
                        .cloned()
                        .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Conflict))?;
                    let current = self
                        .open_record(admission_id, &stored)
                        .map_err(into_anyhow)?;
                    let current = JoinerAdmission::try_from_record(current)
                        .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
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
                    if replacement.is_terminal() {
                        state.current_local_join_id = None;
                    }
                    self.save_state_on(conn, &state).map_err(into_anyhow)
                })
            })
            .map_err(map_executor_error)
            .map_err(map_state_error)
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> LoadCurrentJoinStatusPort for SqliteSpaceAdmissionState<E> {
    #[tracing::instrument(name = "space_admission.current_join_status.load", skip_all, err)]
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        self.executor
            .run(|conn| {
                let state = self.load_state_on(conn).map_err(into_anyhow)?;
                let Some(admission_id) = state.current_local_join_id else {
                    return Ok(None);
                };
                let stored = state
                    .records
                    .get(&admission_id)
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                let record = self
                    .open_record(admission_id, stored)
                    .map_err(into_anyhow)?;
                let admission = JoinerAdmission::try_from_record(record)
                    .ok_or_else(|| into_anyhow(SpaceAdmissionStateStoreError::Corrupt))?;
                Ok(Some(CurrentJoinStatus::Pending {
                    join_id: *admission.join_id().as_bytes(),
                    target_space_id: None,
                    sponsor_device_id: None,
                    sponsor_identity_fingerprint: None,
                    cancel_requested: admission.is_cancelling(),
                }))
            })
            .map_err(map_executor_error)
            .map_err(|error| QueryDeviceTrustError::Dependency {
                source: anyhow::Error::new(error),
            })
    }
}

fn mint_message_id() -> AdmissionMessageId {
    loop {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        if let Some(id) = AdmissionMessageId::from_bytes(bytes) {
            return id;
        }
    }
}

fn map_state_error(error: SpaceAdmissionStateStoreError) -> JoinerCancellationStateError {
    match &error {
        SpaceAdmissionStateStoreError::Locked => JoinerCancellationStateError::locked(error),
        SpaceAdmissionStateStoreError::Conflict => {
            JoinerCancellationStateError::state_changed(error)
        }
        SpaceAdmissionStateStoreError::Corrupt => {
            JoinerCancellationStateError::recovery_required(error)
        }
        SpaceAdmissionStateStoreError::Unavailable => {
            JoinerCancellationStateError::unavailable(error)
        }
    }
}
