use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Text};
use serde::{Deserialize, Serialize};
use uc_core::ids::DeviceId;
use uc_core::membership::{LegacyUpgradeDescriptor, LegacyUpgradeError, LegacyUpgradeRequest};
use uc_core::space_access::PreparedGroupJoin;

use crate::db::ports::DbExecutor;
use crate::security::legacy_upgrade::{LegacyUpgradeAttemptStore, PreparedLegacyUpgradeAttempt};
use crate::security::MasterKey;

use super::encrypted_payload::{device_lookup_token, open, seal};
use super::DieselSpaceSecurityStore;

#[derive(QueryableByName)]
struct PendingJoinRow {
    #[diesel(sql_type = Text)]
    peer_lookup_token: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct PersistedPendingJoin {
    version: u8,
    peer: DeviceId,
    source_device_id: DeviceId,
    target_device_id: DeviceId,
    descriptor: LegacyUpgradeDescriptor,
    key_package: Vec<u8>,
    proof: Vec<u8>,
    private_state: Vec<u8>,
    updated_at_ms: i64,
}

fn repository_error(error: impl std::fmt::Display) -> LegacyUpgradeError {
    LegacyUpgradeError::Internal(error.to_string())
}

fn pending_join_aad(peer_lookup_token: &str) -> Vec<u8> {
    format!("uc-legacy-upgrade-pending-join-v1|{peer_lookup_token}").into_bytes()
}

fn persisted_pending_join(
    peer: &DeviceId,
    pending: &PreparedLegacyUpgradeAttempt,
    updated_at_ms: i64,
) -> Result<PersistedPendingJoin, LegacyUpgradeError> {
    let request = pending.request();
    if request.target_device_id() != peer
        || request.source_device_id() == peer
        || request.key_package() != pending.pending_group_join().key_package
    {
        return Err(LegacyUpgradeError::InvalidRequest);
    }
    Ok(PersistedPendingJoin {
        version: 1,
        peer: *peer,
        source_device_id: *request.source_device_id(),
        target_device_id: *request.target_device_id(),
        descriptor: request.descriptor().clone(),
        key_package: request.key_package().to_vec(),
        proof: request.proof().to_vec(),
        private_state: pending.pending_group_join().private_state().to_vec(),
        updated_at_ms,
    })
}

fn restore_pending_join(
    master_key: &MasterKey,
    peer: &DeviceId,
    row: PendingJoinRow,
) -> Result<PreparedLegacyUpgradeAttempt, LegacyUpgradeError> {
    let expected_lookup_token = device_lookup_token(master_key, peer).map_err(repository_error)?;
    if row.peer_lookup_token != expected_lookup_token {
        return Err(repository_error("legacy upgrade lookup token mismatch"));
    }
    let persisted: PersistedPendingJoin = open(
        master_key,
        &row.encrypted_payload,
        &pending_join_aad(&row.peer_lookup_token),
    )
    .map_err(repository_error)?;
    if persisted.version != 1
        || &persisted.peer != peer
        || &persisted.target_device_id != peer
        || persisted.source_device_id == *peer
        || persisted.updated_at_ms != row.updated_at_ms
    {
        return Err(repository_error(
            "legacy upgrade pending join integrity mismatch",
        ));
    }
    let request = LegacyUpgradeRequest::unsigned(
        persisted.source_device_id,
        persisted.target_device_id,
        persisted.descriptor,
        persisted.key_package.clone(),
    )
    .with_proof(persisted.proof);
    Ok(PreparedLegacyUpgradeAttempt::new(
        request,
        PreparedGroupJoin::new(persisted.key_package, persisted.private_state),
    ))
}

#[async_trait]
impl<E: DbExecutor> LegacyUpgradeAttemptStore for DieselSpaceSecurityStore<E> {
    async fn save_pending_attempt(
        &self,
        peer: &DeviceId,
        pending: &PreparedLegacyUpgradeAttempt,
        now_ms: i64,
    ) -> Result<(), LegacyUpgradeError> {
        let master_key = self.session.get_master_key().map_err(repository_error)?;
        let lookup_token = device_lookup_token(&master_key, peer).map_err(repository_error)?;
        let persisted = persisted_pending_join(peer, pending, now_ms)?;
        let encrypted = seal(&master_key, &persisted, &pending_join_aad(&lookup_token))
            .map_err(repository_error)?;
        self.executor
            .run(move |conn| {
                diesel::sql_query(
                    "INSERT INTO legacy_upgrade_pending_join \
                     (peer_lookup_token, encrypted_payload, updated_at_ms) VALUES (?, ?, ?) \
                     ON CONFLICT(peer_lookup_token) DO UPDATE SET \
                     encrypted_payload = excluded.encrypted_payload, \
                     updated_at_ms = excluded.updated_at_ms",
                )
                .bind::<Text, _>(lookup_token)
                .bind::<Binary, _>(encrypted)
                .bind::<BigInt, _>(now_ms)
                .execute(conn)?;
                Ok(())
            })
            .map_err(repository_error)
    }

    async fn load_pending_attempt(
        &self,
        peer: &DeviceId,
    ) -> Result<Option<PreparedLegacyUpgradeAttempt>, LegacyUpgradeError> {
        let master_key = self.session.get_master_key().map_err(repository_error)?;
        let lookup_token = device_lookup_token(&master_key, peer).map_err(repository_error)?;
        let peer = *peer;
        self.executor
            .run(move |conn| {
                let row = diesel::sql_query(
                    "SELECT peer_lookup_token, encrypted_payload, updated_at_ms \
                     FROM legacy_upgrade_pending_join WHERE peer_lookup_token = ?",
                )
                .bind::<Text, _>(lookup_token)
                .get_result::<PendingJoinRow>(conn)
                .optional()?;
                row.map(|row| restore_pending_join(&master_key, &peer, row))
                    .transpose()
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })
            .map_err(repository_error)
    }

    async fn clear_pending_attempt(&self, peer: &DeviceId) -> Result<(), LegacyUpgradeError> {
        let master_key = self.session.get_master_key().map_err(repository_error)?;
        let lookup_token = device_lookup_token(&master_key, peer).map_err(repository_error)?;
        self.executor
            .run(move |conn| {
                diesel::sql_query(
                    "DELETE FROM legacy_upgrade_pending_join WHERE peer_lookup_token = ?",
                )
                .bind::<Text, _>(lookup_token)
                .execute(conn)?;
                Ok(())
            })
            .map_err(repository_error)
    }
}
