use std::collections::{BTreeMap, BTreeSet};

use diesel::connection::Connection;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use uc_core::membership::{AdmissionRecordPersistence, SpaceAdmissionAggregate};
use uc_observability_contract::diagnostics::connectivity::{
    observe_local_sync_result, LocalWorkStep,
};

use super::persisted::{
    decode_repository, PersistedSpaceAdmissionMetadataV3, PersistedSpaceAdmissionRecordV3,
    PersistedSpaceAdmissionRepositoryV2, StoredSpaceAdmissionV1,
    SPACE_ADMISSION_REPOSITORY_FORMAT_V2, SPACE_ADMISSION_REPOSITORY_FORMAT_V3,
};
use super::{SpaceAdmissionStateStoreError, SqliteSpaceAdmissionState};
use crate::db::ports::DbExecutor;
use crate::security::{AdmissionKeyError, WrappedSpaceAdmissionDataKey};

const LEGACY_REPOSITORY_PAYLOAD_PURPOSE: &[u8] = b"space-admission-repository-v1";
const METADATA_PAYLOAD_PURPOSE: &[u8] = b"space-admission-repository-metadata-v3";
const RECORD_PAYLOAD_PURPOSE: &[u8] = b"space-admission-repository-record-v3";
const RECORD_LOOKUP_PURPOSE: &[u8] = b"space-admission-repository-record-lookup-v3";
const RECORD_CONTENT_PURPOSE: &[u8] = b"space-admission-repository-record-content-v3";
const MAX_READ_CACHE_BYTES: usize = 128 * 1024 * 1024;

pub(super) struct RepositoryReadCache {
    key_binding: [u8; 32],
    ciphertext: Vec<u8>,
    state: PersistedSpaceAdmissionRepositoryV2,
}

#[derive(QueryableByName)]
struct EncryptedRepositoryRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

#[derive(QueryableByName)]
pub(super) struct EncryptedRecordRow {
    #[diesel(sql_type = Binary)]
    lookup_token: Vec<u8>,
    #[diesel(sql_type = Binary)]
    content_token: Vec<u8>,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

#[derive(QueryableByName)]
struct RecordTokenRow {
    #[diesel(sql_type = Binary)]
    lookup_token: Vec<u8>,
    #[diesel(sql_type = Binary)]
    content_token: Vec<u8>,
}

impl<E: DbExecutor> SqliteSpaceAdmissionState<E> {
    pub(in crate::space::admission) fn load_state_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<PersistedSpaceAdmissionRepositoryV2, SpaceAdmissionStateStoreError> {
        observe_local_sync_result(LocalWorkStep::RepositoryLoad, || {
            let Some(row) = load_repository_row(conn)? else {
                self.clear_read_cache();
                return Ok(PersistedSpaceAdmissionRepositoryV2::fresh(
                    self.keys.profile_generation(),
                ));
            };
            if let Some(metadata) = self.try_open_metadata(&row.encrypted_payload)? {
                self.clear_read_cache();
                return self.load_v3_state_on(conn, metadata);
            }
            conn.immediate_transaction::<_, SpaceAdmissionStateStoreError, _>(|conn| {
                let current =
                    load_repository_row(conn)?.ok_or(SpaceAdmissionStateStoreError::Conflict)?;
                if let Some(metadata) = self.try_open_metadata(&current.encrypted_payload)? {
                    self.clear_read_cache();
                    return self.load_v3_state_on(conn, metadata);
                }
                let current_legacy = self.open_legacy_state(&current.encrypted_payload)?;
                self.persist_v3_state_on(conn, &current_legacy)?;
                self.clear_read_cache();
                Ok(current_legacy)
            })
        })
    }

    // rust-style: allow-qualified-path -- 可见性必须覆盖 repository 的相邻 joiner 模块
    pub(in crate::space::admission) fn load_current_record_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<Option<([u8; 16], [u8; 32], StoredSpaceAdmissionV1)>, SpaceAdmissionStateStoreError>
    {
        let Some(metadata) = self.load_metadata_on(conn)? else {
            return Ok(None);
        };
        let Some(admission_id) = metadata.current_local_join_id else {
            return Ok(None);
        };
        let stored = self.load_v3_record_on(conn, admission_id)?;
        Ok(Some((metadata.profile_generation, admission_id, stored)))
    }

    pub(super) fn load_metadata_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<Option<PersistedSpaceAdmissionMetadataV3>, SpaceAdmissionStateStoreError> {
        let Some(row) = load_repository_row(conn)? else {
            return Ok(None);
        };
        if let Some(metadata) = self.try_open_metadata(&row.encrypted_payload)? {
            return Ok(Some(metadata));
        }
        self.load_state_on(conn)?;
        let migrated = load_repository_row(conn)?.ok_or(SpaceAdmissionStateStoreError::Corrupt)?;
        self.try_open_metadata(&migrated.encrypted_payload)?
            .map(Some)
            .ok_or(SpaceAdmissionStateStoreError::Corrupt)
    }

    // rust-style: allow-qualified-path -- 可见性必须覆盖 repository 的相邻准入角色模块
    pub(in crate::space::admission) fn save_state_on(
        &self,
        conn: &mut SqliteConnection,
        state: &PersistedSpaceAdmissionRepositoryV2,
    ) -> Result<(), SpaceAdmissionStateStoreError> {
        observe_local_sync_result(LocalWorkStep::RepositorySave, || {
            conn.transaction::<_, SpaceAdmissionStateStoreError, _>(|conn| {
                self.persist_v3_state_on(conn, state)?;
                self.clear_read_cache();
                Ok(())
            })
        })
    }

    fn try_open_metadata(
        &self,
        encrypted: &[u8],
    ) -> Result<Option<PersistedSpaceAdmissionMetadataV3>, SpaceAdmissionStateStoreError> {
        let reader = self
            .keys
            .profile_payload_reader(METADATA_PAYLOAD_PURPOSE)
            .map_err(map_key_error)?;
        let plaintext = match reader.open_compact(encrypted) {
            Ok(plaintext) => plaintext,
            Err(AdmissionKeyError::SecureStorage) => {
                return Err(SpaceAdmissionStateStoreError::Locked)
            }
            Err(AdmissionKeyError::Corrupt | AdmissionKeyError::OpenFailed) => return Ok(None),
        };
        let metadata = postcard::from_bytes::<PersistedSpaceAdmissionMetadataV3>(&plaintext)
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        self.validate_metadata(&metadata)?;
        Ok(Some(metadata))
    }

    fn validate_metadata(
        &self,
        metadata: &PersistedSpaceAdmissionMetadataV3,
    ) -> Result<(), SpaceAdmissionStateStoreError> {
        if metadata.format_version != SPACE_ADMISSION_REPOSITORY_FORMAT_V3
            || metadata.profile_generation != self.keys.profile_generation()
        {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        Ok(())
    }

    fn open_legacy_state(
        &self,
        encrypted: &[u8],
    ) -> Result<PersistedSpaceAdmissionRepositoryV2, SpaceAdmissionStateStoreError> {
        let reader = match self
            .keys
            .profile_payload_reader(LEGACY_REPOSITORY_PAYLOAD_PURPOSE)
        {
            Ok(reader) => reader,
            Err(error) => {
                self.clear_read_cache();
                return Err(map_key_error(error));
            }
        };
        let key_binding = reader.cache_binding();
        let mut cache = self
            .read_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cache.as_ref() {
            if cached.key_binding == key_binding && cached.ciphertext == encrypted {
                return Ok(cached.state.clone());
            }
        }
        *cache = None;
        let plaintext = reader.open(encrypted).map_err(map_key_error)?;
        let state = decode_repository(&plaintext).ok_or(SpaceAdmissionStateStoreError::Corrupt)?;
        if state.format_version != SPACE_ADMISSION_REPOSITORY_FORMAT_V2
            || state.profile_generation != self.keys.profile_generation()
            || state
                .current_local_join_id
                .is_some_and(|id| !state.records.contains_key(&id))
            || state
                .latest_local_join_id
                .is_some_and(|id| !state.records.contains_key(&id))
        {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        let estimated_bytes = encrypted
            .len()
            .saturating_add(plaintext.len())
            .saturating_add(state.records.len().saturating_mul(256))
            .saturating_add(state.claimed_invitations.len().saturating_mul(128));
        if estimated_bytes <= MAX_READ_CACHE_BYTES {
            *cache = Some(RepositoryReadCache {
                key_binding,
                ciphertext: encrypted.to_vec(),
                state: state.clone(),
            });
        }
        Ok(state)
    }

    fn load_v3_state_on(
        &self,
        conn: &mut SqliteConnection,
        metadata: PersistedSpaceAdmissionMetadataV3,
    ) -> Result<PersistedSpaceAdmissionRepositoryV2, SpaceAdmissionStateStoreError> {
        self.validate_metadata(&metadata)?;
        let rows = sql_query(
            "SELECT lookup_token, content_token, encrypted_payload \
             FROM admission_repository_record ORDER BY lookup_token",
        )
        .load::<EncryptedRecordRow>(conn)
        .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)?;
        let mut records = BTreeMap::new();
        for row in rows {
            let record = self.open_v3_record_row(row)?;
            if records.insert(record.admission_id, record.stored).is_some() {
                return Err(SpaceAdmissionStateStoreError::Corrupt);
            }
        }
        if metadata
            .current_local_join_id
            .is_some_and(|id| !records.contains_key(&id))
            || metadata
                .latest_local_join_id
                .is_some_and(|id| !records.contains_key(&id))
        {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        Ok(PersistedSpaceAdmissionRepositoryV2 {
            format_version: SPACE_ADMISSION_REPOSITORY_FORMAT_V2,
            profile_generation: metadata.profile_generation,
            next_local_join_ordinal: metadata.next_local_join_ordinal,
            current_local_join_id: metadata.current_local_join_id,
            latest_local_join_id: metadata.latest_local_join_id,
            claimed_invitations: metadata.claimed_invitations,
            records,
        })
    }

    pub(super) fn load_v3_record_on(
        &self,
        conn: &mut SqliteConnection,
        admission_id: [u8; 32],
    ) -> Result<StoredSpaceAdmissionV1, SpaceAdmissionStateStoreError> {
        let lookup_token = self.record_lookup_token(admission_id)?;
        let row = sql_query(
            "SELECT lookup_token, content_token, encrypted_payload \
             FROM admission_repository_record WHERE lookup_token = ?",
        )
        .bind::<Binary, _>(lookup_token.to_vec())
        .get_result::<EncryptedRecordRow>(conn)
        .optional()
        .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)?
        .ok_or(SpaceAdmissionStateStoreError::Corrupt)?;
        let record = self.open_v3_record_row(row)?;
        if record.admission_id != admission_id {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        Ok(record.stored)
    }

    pub(super) fn open_v3_record_row(
        &self,
        row: EncryptedRecordRow,
    ) -> Result<PersistedSpaceAdmissionRecordV3, SpaceAdmissionStateStoreError> {
        #[cfg(test)]
        self.record_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let reader = self
            .keys
            .profile_payload_reader(RECORD_PAYLOAD_PURPOSE)
            .map_err(map_key_error)?;
        let plaintext = reader
            .open_compact(&row.encrypted_payload)
            .map_err(map_key_error)?;
        let record = postcard::from_bytes::<PersistedSpaceAdmissionRecordV3>(&plaintext)
            .map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        let lookup_token = self.record_lookup_token(record.admission_id)?;
        let content_token = self.record_content_token(&plaintext)?;
        if row.lookup_token != lookup_token || row.content_token != content_token {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        Ok(record)
    }

    fn persist_v3_state_on(
        &self,
        conn: &mut SqliteConnection,
        state: &PersistedSpaceAdmissionRepositoryV2,
    ) -> Result<(), SpaceAdmissionStateStoreError> {
        if state.format_version != SPACE_ADMISSION_REPOSITORY_FORMAT_V2
            || state.profile_generation != self.keys.profile_generation()
            || state
                .current_local_join_id
                .is_some_and(|id| !state.records.contains_key(&id))
            || state
                .latest_local_join_id
                .is_some_and(|id| !state.records.contains_key(&id))
        {
            return Err(SpaceAdmissionStateStoreError::Corrupt);
        }
        let existing = sql_query(
            "SELECT lookup_token, content_token FROM admission_repository_record ORDER BY lookup_token",
        )
        .load::<RecordTokenRow>(conn)
        .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)?
        .into_iter()
        .map(|row| (row.lookup_token, row.content_token))
        .collect::<BTreeMap<_, _>>();
        let mut retained = BTreeSet::new();
        for (&admission_id, stored) in &state.records {
            let record = PersistedSpaceAdmissionRecordV3 {
                admission_id,
                stored: stored.clone(),
            };
            let plaintext =
                postcard::to_stdvec(&record).map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
            let lookup_token = self.record_lookup_token(admission_id)?;
            let content_token = self.record_content_token(&plaintext)?;
            retained.insert(lookup_token.to_vec());
            if existing
                .get(lookup_token.as_slice())
                .is_some_and(|current| current.as_slice() == content_token)
            {
                continue;
            }
            let encrypted = self
                .keys
                .seal_profile_payload_compact(RECORD_PAYLOAD_PURPOSE, &plaintext)
                .map_err(map_key_error)?;
            sql_query(
                "INSERT INTO admission_repository_record \
                 (lookup_token, content_token, encrypted_payload) VALUES (?, ?, ?) \
                 ON CONFLICT(lookup_token) DO UPDATE SET \
                 content_token = excluded.content_token, encrypted_payload = excluded.encrypted_payload",
            )
            .bind::<Binary, _>(lookup_token.to_vec())
            .bind::<Binary, _>(content_token.to_vec())
            .bind::<Binary, _>(encrypted)
            .execute(conn)
            .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)?;
        }
        for lookup_token in existing.keys() {
            if !retained.contains(lookup_token) {
                sql_query("DELETE FROM admission_repository_record WHERE lookup_token = ?")
                    .bind::<Binary, _>(lookup_token)
                    .execute(conn)
                    .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)?;
            }
        }
        let metadata = PersistedSpaceAdmissionMetadataV3::from(state);
        let plaintext =
            postcard::to_stdvec(&metadata).map_err(|_| SpaceAdmissionStateStoreError::Corrupt)?;
        let encrypted = self
            .keys
            .seal_profile_payload_compact(METADATA_PAYLOAD_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        sql_query(
            "INSERT INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, ?) \
             ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<Binary, _>(encrypted)
        .execute(conn)
        .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)?;
        Ok(())
    }

    pub(super) fn record_lookup_token(
        &self,
        admission_id: [u8; 32],
    ) -> Result<[u8; 32], SpaceAdmissionStateStoreError> {
        self.keys
            .repository_token(RECORD_LOOKUP_PURPOSE, &admission_id)
            .map_err(map_key_error)
    }

    fn record_content_token(
        &self,
        plaintext: &[u8],
    ) -> Result<[u8; 32], SpaceAdmissionStateStoreError> {
        self.keys
            .repository_token(RECORD_CONTENT_PURPOSE, plaintext)
            .map_err(map_key_error)
    }

    fn clear_read_cache(&self) {
        let mut cache = self
            .read_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cache = None;
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

    pub(in crate::space::admission) fn seal_new_record<R: AdmissionRecordPersistence>(
        &self,
        aggregate: &R,
    ) -> Result<StoredSpaceAdmissionV1, SpaceAdmissionStateStoreError> {
        let admission_id = *aggregate.admission_id().as_bytes();
        let wrapped = self
            .keys
            .create_wrapped_attempt_key(admission_id)
            .map_err(map_key_error)?;
        self.seal_record(aggregate, wrapped)
    }

    pub(in crate::space::admission) fn seal_record<R: AdmissionRecordPersistence>(
        &self,
        aggregate: &R,
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
            encrypted_payload: encrypted_payload.into(),
        })
    }
}

fn load_repository_row(
    conn: &mut SqliteConnection,
) -> Result<Option<EncryptedRepositoryRow>, SpaceAdmissionStateStoreError> {
    sql_query("SELECT encrypted_payload FROM admission_repository_state WHERE singleton_id = 1")
        .get_result::<EncryptedRepositoryRow>(conn)
        .optional()
        .map_err(|_| SpaceAdmissionStateStoreError::Unavailable)
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

pub(super) fn map_key_error(error: AdmissionKeyError) -> SpaceAdmissionStateStoreError {
    match error {
        AdmissionKeyError::SecureStorage => SpaceAdmissionStateStoreError::Locked,
        AdmissionKeyError::Corrupt | AdmissionKeyError::OpenFailed => {
            SpaceAdmissionStateStoreError::Corrupt
        }
    }
}
