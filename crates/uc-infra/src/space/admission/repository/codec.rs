use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use uc_core::membership::SpaceAdmissionAggregate;

use super::persisted::{
    PersistedSpaceAdmissionRepositoryV1, StoredSpaceAdmissionV1,
    SPACE_ADMISSION_REPOSITORY_FORMAT_V1,
};
use super::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};
use crate::db::ports::DbExecutor;
use crate::security::{AdmissionKeyError, WrappedSpaceAdmissionDataKey};

const REPOSITORY_PAYLOAD_PURPOSE: &[u8] = b"space-admission-repository-v1";

#[derive(QueryableByName)]
struct EncryptedRepositoryRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    pub(in crate::space::admission) fn load_state_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<PersistedSpaceAdmissionRepositoryV1, SpaceAdmissionStateStoreError> {
        let row = sql_query(
            "SELECT encrypted_payload FROM admission_repository_state WHERE singleton_id = 1",
        )
        .get_result::<EncryptedRepositoryRow>(conn)
        .optional()
        .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)?;
        let Some(row) = row else {
            return Ok(PersistedSpaceAdmissionRepositoryV1::fresh(
                self.keys.profile_generation(),
            ));
        };
        let plaintext = self
            .keys
            .open_profile_payload(REPOSITORY_PAYLOAD_PURPOSE, &row.encrypted_payload)
            .map_err(map_key_error)?;
        let state: PersistedSpaceAdmissionRepositoryV1 =
            postcard::from_bytes(&plaintext).map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        if state.format_version != SPACE_ADMISSION_REPOSITORY_FORMAT_V1
            || state.profile_generation != self.keys.profile_generation()
            || state
                .current_local_join_id
                .is_some_and(|id| !state.records.contains_key(&id))
        {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        Ok(state)
    }

    pub(in crate::space::admission) fn save_state_on(
        &self,
        conn: &mut SqliteConnection,
        state: &PersistedSpaceAdmissionRepositoryV1,
    ) -> Result<(), SpaceAdmissionStateStoreError> {
        let plaintext =
            postcard::to_stdvec(state).map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        let encrypted = self
            .keys
            .seal_profile_payload(REPOSITORY_PAYLOAD_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?) \
             ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<Binary, _>(encrypted)
        .execute(conn)
        .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)?;
        let reopened = self.load_state_on(conn)?;
        if reopened != *state {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        Ok(())
    }

    pub(in crate::space::admission) fn open_record(
        &self,
        admission_id: [u8; 32],
        stored: &StoredSpaceAdmissionV1,
    ) -> Result<SpaceAdmissionAggregate, SpaceAdmissionStateStoreError> {
        let plaintext = self
            .keys
            .open_attempt_payload(
                admission_id,
                &stored.wrapped_data_key,
                &stored.encrypted_payload,
            )
            .map_err(map_key_error)?;
        let aggregate = SpaceAdmissionAggregate::decode_persisted(&plaintext)
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        if aggregate.admission_id().as_bytes() != &admission_id {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        Ok(aggregate)
    }

    pub(in crate::space::admission) fn seal_new_record(
        &self,
        aggregate: &SpaceAdmissionAggregate,
    ) -> Result<StoredSpaceAdmissionV1, SpaceAdmissionStateStoreError> {
        let admission_id = *aggregate.admission_id().as_bytes();
        let wrapped = self
            .keys
            .create_wrapped_attempt_key(admission_id)
            .map_err(map_key_error)?;
        self.seal_record(aggregate, wrapped)
    }

    pub(in crate::space::admission) fn seal_record(
        &self,
        aggregate: &SpaceAdmissionAggregate,
        wrapped_data_key: WrappedSpaceAdmissionDataKey,
    ) -> Result<StoredSpaceAdmissionV1, SpaceAdmissionStateStoreError> {
        let admission_id = *aggregate.admission_id().as_bytes();
        let plaintext = aggregate
            .encode_persisted()
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        let encrypted_payload = self
            .keys
            .seal_attempt_payload(admission_id, &wrapped_data_key, &plaintext)
            .map_err(map_key_error)?;
        Ok(StoredSpaceAdmissionV1 {
            wrapped_data_key,
            encrypted_payload,
        })
    }
}

pub(in crate::space::admission) fn map_executor_error(
    error: anyhow::Error,
) -> SpaceAdmissionStateStoreError {
    error
        .downcast_ref::<SpaceAdmissionStateStoreError>()
        .copied()
        .unwrap_or(SpaceAdmissionStateStoreError::Unavailable)
}

pub(in crate::space::admission) fn into_anyhow(
    error: SpaceAdmissionStateStoreError,
) -> anyhow::Error {
    anyhow::anyhow!(error)
}

fn map_key_error(error: AdmissionKeyError) -> SpaceAdmissionStateStoreError {
    match error {
        AdmissionKeyError::SecureStorage => SpaceAdmissionStateStoreError::Locked,
        AdmissionKeyError::Corrupt | AdmissionKeyError::OpenFailed => {
            SpaceAdmissionStateStoreError::Corrupt
        }
    }
}
