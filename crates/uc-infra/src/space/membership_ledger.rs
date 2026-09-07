use std::sync::Arc;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use sha2::{Digest, Sha256};
use uc_application::deps::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation,
};
use zeroize::Zeroizing;

use crate::db::ports::DbExecutor;
use crate::security::{AdmissionKeyError, AdmissionKeyManager};

mod codec;
#[cfg(test)]
mod tests;
const MEMBERSHIP_LEDGER_PURPOSE: &[u8] = b"membership-ledger-v1";

#[derive(QueryableByName)]
struct EncryptedLedgerRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

pub struct SqliteMembershipLedger<E> {
    executor: E,
    keys: Arc<AdmissionKeyManager>,
}

impl<E> SqliteMembershipLedger<E> {
    pub fn new(executor: E, keys: Arc<AdmissionKeyManager>) -> Self {
        Self { executor, keys }
    }
}

impl<E: DbExecutor> SqliteMembershipLedger<E> {
    pub(crate) async fn apply_projection(
        &self,
        relationships: &crate::db::repositories::EncryptedRelationshipStore<E>,
        plan: &uc_application::deps::MembershipProjectionPlan,
    ) -> Result<(), uc_application::deps::ApplyMembershipProjectionError> {
        use uc_application::deps::ApplyMembershipProjectionError;
        relationships
            .reconcile_membership_projection(plan, |conn| {
                let current = self.load_on(conn).map_err(|source| {
                    ApplyMembershipProjectionError::Dependency {
                        source: anyhow::Error::new(source),
                    }
                })?;
                let digest = current
                    .membership_history
                    .as_deref()
                    .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
                if current.revision != plan.expected_revision
                    || digest != Some(plan.expected_history_digest)
                {
                    return Err(ApplyMembershipProjectionError::Conflict);
                }
                Ok(())
            })
            .await
    }

    fn load_on(
        &self,
        conn: &mut SqliteConnection,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        let row = sql_query(
            "SELECT encrypted_payload FROM membership_ledger_state WHERE singleton_id = 1",
        )
        .get_result::<EncryptedLedgerRow>(conn)
        .optional()
        .map_err(|_| MembershipLedgerError::Unavailable)?;
        let Some(row) = row else {
            return Ok(LoadedMembershipLedger::no_current_space());
        };
        let plaintext = Zeroizing::new(
            self.keys
                .open_profile_payload(MEMBERSHIP_LEDGER_PURPOSE, &row.encrypted_payload)
                .map_err(map_key_error)?,
        );
        codec::decode(&plaintext, self.keys.profile_generation())
    }

    fn save_on(
        &self,
        conn: &mut SqliteConnection,
        ledger: &LoadedMembershipLedger,
    ) -> Result<(), MembershipLedgerError> {
        let plaintext = Zeroizing::new(codec::encode(ledger, self.keys.profile_generation())?);
        let encrypted = self
            .keys
            .seal_profile_payload(MEMBERSHIP_LEDGER_PURPOSE, &plaintext)
            .map_err(map_key_error)?;
        sql_query(
            "INSERT INTO membership_ledger_state (singleton_id, encrypted_payload) VALUES (1, ?) \
             ON CONFLICT(singleton_id) DO UPDATE SET encrypted_payload = excluded.encrypted_payload",
        )
        .bind::<Binary, _>(encrypted)
        .execute(conn)
        .map_err(|_| MembershipLedgerError::Unavailable)?;
        if self.load_on(conn)? != *ledger {
            return Err(MembershipLedgerError::Corrupt);
        }
        Ok(())
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> LoadMembershipLedgerPort for SqliteMembershipLedger<E> {
    async fn load(&self) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        self.executor
            .run(|conn| self.load_on(conn).map_err(anyhow::Error::new))
            .map_err(map_executor_error)
    }
}

#[async_trait]
impl<E: DbExecutor + Send + Sync> CommitMembershipLedgerPort for SqliteMembershipLedger<E> {
    async fn compare_and_commit(
        &self,
        mutation: MembershipLedgerMutation,
    ) -> Result<LoadedMembershipLedger, MembershipLedgerError> {
        self.executor
            .run(|conn| {
                conn.immediate_transaction::<_, anyhow::Error, _>(|conn| {
                    let current = self.load_on(conn).map_err(anyhow::Error::new)?;
                    let current_digest = current
                        .membership_history
                        .as_deref()
                        .map(|bytes| <[u8; 32]>::from(Sha256::digest(bytes)));
                    let next_revision = mutation
                        .expected_revision
                        .checked_add(1)
                        .ok_or_else(|| anyhow::Error::new(MembershipLedgerError::Corrupt))?;
                    if current.revision != mutation.expected_revision
                        || current_digest != mutation.expected_history_digest
                        || mutation.replacement.revision != next_revision
                    {
                        return Err(anyhow::Error::new(MembershipLedgerError::Conflict));
                    }
                    self.save_on(conn, &mutation.replacement)
                        .map_err(anyhow::Error::new)?;
                    Ok(mutation.replacement)
                })
            })
            .map_err(map_executor_error)
    }
}

fn map_key_error(error: AdmissionKeyError) -> MembershipLedgerError {
    match error {
        AdmissionKeyError::SecureStorage => MembershipLedgerError::Locked,
        AdmissionKeyError::Corrupt | AdmissionKeyError::OpenFailed => {
            MembershipLedgerError::Corrupt
        }
    }
}

fn map_executor_error(error: anyhow::Error) -> MembershipLedgerError {
    error
        .downcast_ref::<MembershipLedgerError>()
        .copied()
        .unwrap_or(MembershipLedgerError::Unavailable)
}
