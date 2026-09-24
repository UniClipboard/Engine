use std::sync::Arc;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use uc_application::deps::{
    MembershipLedgerError, MembershipProjectionPlan, MembershipRecord, MembershipRecordCommit,
    MembershipRecordStorePort,
};
use uc_core::membership::HistoricalMembershipSignatureVerifier;
use uc_core::ports::ClockPort;
use zeroize::Zeroizing;

use super::codec::{self, Decoded};
use crate::db::ports::DbExecutor;
use crate::db::repositories::{EncryptedRelationshipStore, MembershipProjectionWriter};
use crate::security::{AdmissionKeyError, AdmissionKeyManager};

pub(super) const MEMBERSHIP_RECORD_PURPOSE: &[u8] = b"membership-ledger-v1";

#[derive(QueryableByName)]
pub(super) struct EncryptedRecordRow {
    #[diesel(sql_type = Binary)]
    pub(super) encrypted_payload: Vec<u8>,
}

pub struct SqliteMembershipRecordStore<E> {
    executor: E,
    keys: Arc<AdmissionKeyManager>,
    verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
    clock: Arc<dyn ClockPort>,
    /// 与记录同库的成员读模型。只构建暂存代际数据库的调用方不维护读模型，传入 `None`。
    relationships: Option<Arc<EncryptedRelationshipStore<E>>>,
}

impl<E> SqliteMembershipRecordStore<E> {
    pub fn new(
        executor: E,
        keys: Arc<AdmissionKeyManager>,
        verifier: Arc<dyn HistoricalMembershipSignatureVerifier>,
        clock: Arc<dyn ClockPort>,
    ) -> Self {
        Self {
            executor,
            keys,
            verifier,
            clock,
            relationships: None,
        }
    }

    /// 提交记录时在同一事务中维护成员读模型。读模型必须与记录位于同一数据库。
    pub fn with_projection(mut self, relationships: Arc<EncryptedRelationshipStore<E>>) -> Self {
        self.relationships = Some(relationships);
        self
    }
}

impl<E: DbExecutor> SqliteMembershipRecordStore<E> {
    /// 读取成员记录。旧格式在同一事务内迁移并写回 V5；迁移失败时原行保持不变。
    pub fn load(&self) -> Result<MembershipRecord, MembershipLedgerError> {
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    self.load_on(conn).map_err(anyhow::Error::new)
                })
            })
            .map_err(map_executor_error)
    }

    /// 仅当当前修订号等于 `expected_revision` 且替换记录的修订号更大时写入；读模型计划与记录在同一
    /// 事务中落实，任一失败时两者都不改变。
    pub(crate) fn commit_record(
        &self,
        expected_revision: u64,
        replacement: &MembershipRecord,
        projection: Option<(&MembershipProjectionWriter, &MembershipProjectionPlan)>,
    ) -> Result<(), MembershipLedgerError> {
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let current = self.load_on(conn).map_err(anyhow::Error::new)?;
                    if current.revision() != expected_revision
                        || replacement.revision() <= expected_revision
                    {
                        return Err(anyhow::Error::new(MembershipLedgerError::Conflict));
                    }
                    self.save_on(conn, replacement)
                        .map_err(anyhow::Error::new)?;
                    if let Some((writer, plan)) = projection {
                        writer
                            .apply(conn, plan)
                            .map_err(|_| anyhow::Error::new(MembershipLedgerError::Unavailable))?;
                    }
                    Ok(())
                })
            })
            .map_err(map_executor_error)
    }

    fn load_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<MembershipRecord, MembershipLedgerError> {
        let row = sql_query(
            "SELECT encrypted_payload FROM membership_ledger_state WHERE singleton_id = 1",
        )
        .get_result::<EncryptedRecordRow>(conn)
        .optional()
        .map_err(|_| MembershipLedgerError::Unavailable)?;
        let Some(row) = row else {
            return Ok(MembershipRecord::NoSpace { revision: 0 });
        };
        let plaintext = Zeroizing::new(
            self.keys
                .open_profile_payload(MEMBERSHIP_RECORD_PURPOSE, &row.encrypted_payload)
                .map_err(map_key_error)?,
        );
        match codec::decode(
            &plaintext,
            self.keys.profile_generation(),
            self.verifier.as_ref(),
            self.clock.now_ms(),
        )? {
            Decoded::Current(record) => Ok(record),
            Decoded::Migrated(record) => {
                self.save_on(conn, &record)?;
                Ok(record)
            }
        }
    }

    fn save_on(
        &self,
        conn: &mut SqliteConnection,
        record: &MembershipRecord,
    ) -> Result<(), MembershipLedgerError> {
        let plaintext = Zeroizing::new(codec::encode(record, self.keys.profile_generation())?);
        let encrypted = self
            .keys
            .seal_profile_payload(MEMBERSHIP_RECORD_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        sql_query(
            "INSERT INTO membership_ledger_state (singleton_id, encrypted_payload) VALUES (1, ?) \
             ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<Binary, _>(encrypted)
        .execute(conn)
        .map_err(|_| MembershipLedgerError::Unavailable)?;
        Ok(())
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> MembershipRecordStorePort for SqliteMembershipRecordStore<E> {
    async fn load(&self) -> Result<MembershipRecord, MembershipLedgerError> {
        SqliteMembershipRecordStore::load(self)
    }

    async fn commit(&self, commit: MembershipRecordCommit) -> Result<(), MembershipLedgerError> {
        let writer = match (&commit.projection, &self.relationships) {
            (None, _) => None,
            (Some(_), Some(relationships)) => Some(
                relationships
                    .membership_projection_writer()
                    .await
                    .map_err(|_| MembershipLedgerError::Unavailable)?,
            ),
            // 读模型计划无处落实时不写记录，避免两者分离。
            (Some(_), None) => return Err(MembershipLedgerError::Unavailable),
        };
        self.commit_record(
            commit.expected_revision,
            &commit.replacement,
            writer.as_ref().zip(commit.projection.as_ref()),
        )
    }
}

fn map_key_error(error: AdmissionKeyError) -> MembershipLedgerError {
    match error {
        AdmissionKeyError::SecureStorage { .. } | AdmissionKeyError::StorageNotPersisted => {
            MembershipLedgerError::Locked
        }
        AdmissionKeyError::Corrupt { .. }
        | AdmissionKeyError::InvalidLayout
        | AdmissionKeyError::OpenFailed { .. } => MembershipLedgerError::Corrupt,
    }
}

fn map_executor_error(error: anyhow::Error) -> MembershipLedgerError {
    error
        .downcast_ref::<MembershipLedgerError>()
        .copied()
        .unwrap_or(MembershipLedgerError::Unavailable)
}
