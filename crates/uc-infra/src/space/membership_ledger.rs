use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::Binary;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uc_application::deps::{
    CommitMembershipLedgerPort, LoadMembershipLedgerPort, LoadedMembershipLedger,
    MembershipLedgerError, MembershipLedgerMutation, PeerHistorySyncState,
    PeerReconciliationRecord,
};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    BaseMembershipHistoryPosition, MemberInstanceId, MembershipHistoryRelationship,
};
use zeroize::Zeroizing;

use crate::db::ports::DbExecutor;
use crate::security::{AdmissionKeyError, AdmissionKeyManager};

const MEMBERSHIP_LEDGER_FORMAT_V1: u16 = 1;
const MEMBERSHIP_LEDGER_FORMAT_V2: u16 = 2;
const MEMBERSHIP_LEDGER_PURPOSE: &[u8] = b"membership-ledger-v1";

#[derive(Serialize, Deserialize)]
struct PersistedMembershipLedgerV1 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LegacyLoadedMembershipLedgerV1,
}

#[derive(Serialize, Deserialize)]
struct PersistedMembershipLedgerV2 {
    format_version: u16,
    profile_generation: [u8; 16],
    ledger: LoadedMembershipLedger,
}

#[derive(Serialize, Deserialize)]
struct LegacyPeerReconciliationRecordV1 {
    peer_device_id: DeviceId,
    relationship: MembershipHistoryRelationship,
    confirmed_position: Option<BaseMembershipHistoryPosition>,
    restricted_delivery: Vec<uc_application::deps::RestrictedMembershipDelivery>,
    updated_at_ms: i64,
}

#[derive(Serialize, Deserialize)]
struct LegacyLoadedMembershipLedgerV1 {
    revision: u64,
    lineage_id: Option<String>,
    membership_history: Option<Vec<u8>>,
    local_device_id: Option<DeviceId>,
    local_member_instance: Option<MemberInstanceId>,
    local_join_active: bool,
    peer_reconciliation: BTreeMap<DeviceId, LegacyPeerReconciliationRecordV1>,
    inbound_transfers: BTreeMap<DeviceId, uc_application::deps::InboundMembershipTransfer>,
    completed_inbound_transfers:
        BTreeMap<(DeviceId, [u8; 32]), uc_core::membership::MembershipHistoryV2Ack>,
    pending_effects: BTreeMap<[u8; 32], uc_application::deps::PendingMembershipEffect>,
}

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
        if let Ok(persisted) = postcard::from_bytes::<PersistedMembershipLedgerV2>(&plaintext) {
            if persisted.format_version == MEMBERSHIP_LEDGER_FORMAT_V2
                && persisted.profile_generation == self.keys.profile_generation()
            {
                return Ok(persisted.ledger);
            }
        }
        let persisted: PersistedMembershipLedgerV1 =
            postcard::from_bytes(&plaintext).map_err(|_| MembershipLedgerError::Corrupt)?;
        if persisted.format_version != MEMBERSHIP_LEDGER_FORMAT_V1
            || persisted.profile_generation != self.keys.profile_generation()
        {
            return Err(MembershipLedgerError::Corrupt);
        }
        Ok(migrate_v1_ledger(persisted.ledger))
    }

    fn save_on(
        &self,
        conn: &mut SqliteConnection,
        ledger: &LoadedMembershipLedger,
    ) -> Result<(), MembershipLedgerError> {
        let plaintext = Zeroizing::new(
            postcard::to_stdvec(&PersistedMembershipLedgerV2 {
                format_version: MEMBERSHIP_LEDGER_FORMAT_V2,
                profile_generation: self.keys.profile_generation(),
                ledger: ledger.clone(),
            })
            .map_err(|_| MembershipLedgerError::Corrupt)?,
        );
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

fn migrate_v1_ledger(legacy: LegacyLoadedMembershipLedgerV1) -> LoadedMembershipLedger {
    let pending_revision = legacy.revision.saturating_add(1);
    LoadedMembershipLedger {
        revision: legacy.revision,
        lineage_id: legacy.lineage_id,
        membership_history: legacy.membership_history,
        local_device_id: legacy.local_device_id,
        local_member_instance: legacy.local_member_instance,
        local_join_active: legacy.local_join_active,
        peer_reconciliation: legacy
            .peer_reconciliation
            .into_iter()
            .map(|(device_id, peer)| {
                (
                    device_id,
                    PeerReconciliationRecord {
                        peer_device_id: peer.peer_device_id,
                        relationship: peer.relationship,
                        // V1 水位可能由 Sponsor 本地推断，升级时必须重新取得认证 ACK。
                        confirmed_position: None,
                        sync_state: PeerHistorySyncState {
                            pending_since_revision: Some(pending_revision),
                            ..Default::default()
                        },
                        restricted_delivery: peer.restricted_delivery,
                        updated_at_ms: peer.updated_at_ms,
                    },
                )
            })
            .collect(),
        history_sync_cursor: None,
        // V2 的半成品传输不能被 V3 续传；历史本体保留，传输会由持久欠账重试。
        inbound_transfers: BTreeMap::new(),
        completed_inbound_transfers: BTreeMap::new(),
        pending_effects: legacy.pending_effects,
        membership_conflicts: BTreeMap::new(),
        membership_branch_transitions: BTreeMap::new(),
        consumed_membership_recovery_nonces: BTreeMap::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_migration_drops_untrusted_peer_watermarks_and_creates_debt() {
        let peer_id = DeviceId::new("peer-b");
        let legacy = LegacyLoadedMembershipLedgerV1 {
            revision: 7,
            lineage_id: Some("space-a".to_owned()),
            membership_history: None,
            local_device_id: Some(DeviceId::new("device-a")),
            local_member_instance: None,
            local_join_active: true,
            peer_reconciliation: BTreeMap::from([(
                peer_id.clone(),
                LegacyPeerReconciliationRecordV1 {
                    peer_device_id: peer_id.clone(),
                    relationship: MembershipHistoryRelationship::Consistent,
                    confirmed_position: Some(BaseMembershipHistoryPosition {
                        event_id: None,
                        depth: 3,
                        history_digest: [9; 32],
                    }),
                    restricted_delivery: Vec::new(),
                    updated_at_ms: 4,
                },
            )]),
            inbound_transfers: BTreeMap::new(),
            completed_inbound_transfers: BTreeMap::new(),
            pending_effects: BTreeMap::new(),
        };

        let migrated = migrate_v1_ledger(legacy);
        let peer = migrated.peer_reconciliation.get(&peer_id).unwrap();

        assert_eq!(peer.confirmed_position, None);
        assert_eq!(peer.sync_state.pending_since_revision, Some(8));
        assert_eq!(migrated.history_sync_cursor, None);
    }
}
