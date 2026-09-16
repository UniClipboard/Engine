//! 持久发送工作索引：只在安全事实改变时提取正文，等待与失败只更新小记录。
use std::collections::{HashMap, HashSet};

use diesel::connection::Connection;
use diesel::prelude::*;
use diesel::sql_query;
use diesel::sql_types::{Binary, Text};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    GroupEpoch, GroupUpdateDispatchError, KeyEpochError, PendingGroupUpdate, RevocationStage,
    RevocationStatus,
};

use super::encrypted_payload::{open, seal, space_lookup_token};
use super::revocation::{decode_record, load_revocation_row, stage_aad};
use super::space_material::load_space_material_on;
use super::{backend, DieselSpaceSecurityStore};
use crate::db::ports::DbExecutor;
use crate::security::MasterKey;

#[derive(QueryableByName)]
struct SourceRow {
    #[diesel(sql_type = Text)]
    source_id: String,
}

#[derive(QueryableByName)]
struct SourceSummaryRow {
    #[diesel(sql_type = Binary)]
    encrypted_summary: Vec<u8>,
}

#[derive(QueryableByName)]
struct WorkRow {
    #[diesel(sql_type = Binary)]
    lookup_token: Vec<u8>,
    #[diesel(sql_type = Binary)]
    encrypted_metadata: Vec<u8>,
}

#[derive(QueryableByName)]
struct PayloadRow {
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct SourceSummary {
    task_tokens: Vec<Vec<u8>>,
}

#[derive(Clone, Serialize, Deserialize)]
struct DeliveryState {
    space_id: SpaceId,
    source: String,
    update_id: String,
    recipient: DeviceId,
    epoch: u64,
    payload_digest: [u8; 32],
    attempts: u32,
    next_attempt_ms: i64,
    rejected: bool,
}

fn aad(kind: &str, token: &[u8]) -> Vec<u8> {
    let mut bytes = format!("uc-group-update-delivery-v1|{kind}|").into_bytes();
    bytes.extend_from_slice(token);
    bytes
}

fn task_token(key: &MasterKey, source: &str, id: &str) -> Result<Vec<u8>, KeyEpochError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key.as_bytes()).map_err(backend)?;
    mac.update(&aad("lookup", source.as_bytes()));
    mac.update(&(id.len() as u64).to_be_bytes());
    mac.update(id.as_bytes());
    Ok(mac.finalize().into_bytes().to_vec())
}

fn transaction_failure(error: anyhow::Error) -> KeyEpochError {
    match error.downcast::<KeyEpochError>() {
        Ok(error) => error,
        Err(error) => backend(error),
    }
}

fn load_source_summary(
    conn: &mut SqliteConnection,
    source_kind: &str,
    source_id: &str,
) -> Result<Option<Vec<u8>>, KeyEpochError> {
    sql_query(
        "SELECT encrypted_summary FROM group_update_source \
         WHERE source_kind = ? AND source_id = ?",
    )
    .bind::<Text, _>(source_kind)
    .bind::<Text, _>(source_id)
    .get_result::<SourceSummaryRow>(conn)
    .optional()
    .map(|row| row.map(|row| row.encrypted_summary))
    .map_err(backend)
}

fn save_source_summary(
    conn: &mut SqliteConnection,
    source_kind: &str,
    source_id: &str,
    encrypted_summary: &[u8],
) -> Result<(), KeyEpochError> {
    sql_query(
        "INSERT INTO group_update_source (source_kind, source_id, encrypted_summary) \
         VALUES (?, ?, ?) ON CONFLICT(source_kind, source_id) DO UPDATE SET \
         encrypted_summary = excluded.encrypted_summary",
    )
    .bind::<Text, _>(source_kind)
    .bind::<Text, _>(source_id)
    .bind::<Binary, _>(encrypted_summary)
    .execute(conn)
    .map_err(backend)?;
    Ok(())
}

fn read_states(
    conn: &mut SqliteConnection,
    key: &MasterKey,
    scope: &str,
) -> Result<Vec<(Vec<u8>, DeliveryState)>, KeyEpochError> {
    sql_query(
        "SELECT lookup_token, encrypted_metadata FROM group_update_delivery \
         WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(scope)
    .load::<WorkRow>(conn)
    .map_err(backend)?
    .into_iter()
    .map(|row| {
        let state = open(
            key,
            &row.encrypted_metadata,
            &aad("state", &row.lookup_token),
        )?;
        Ok((row.lookup_token, state))
    })
    .collect()
}

fn save_state(
    conn: &mut SqliteConnection,
    key: &MasterKey,
    token: &[u8],
    state: &DeliveryState,
) -> Result<(), KeyEpochError> {
    let encrypted = seal(key, state, &aad("state", token))?;
    sql_query("UPDATE group_update_delivery SET encrypted_metadata = ? WHERE lookup_token = ?")
        .bind::<Binary, _>(encrypted)
        .bind::<Binary, _>(token)
        .execute(conn)
        .map_err(backend)?;
    Ok(())
}

fn reconcile_source(
    conn: &mut SqliteConnection,
    key: &MasterKey,
    space_id: &SpaceId,
    source: &str,
    updates: Vec<PendingGroupUpdate>,
) -> Result<Vec<u8>, KeyEpochError> {
    let scope = space_lookup_token(key, space_id)?;
    let existing = read_states(conn, key, &scope)?
        .into_iter()
        .filter(|(_, s)| s.source == source)
        .collect::<HashMap<_, _>>();
    let mut retained = HashSet::new();
    for update in updates {
        let token = task_token(key, source, update.update_id())?;
        retained.insert(token.clone());
        let digest: [u8; 32] = Sha256::digest(update.payload()).into();
        if let Some(state) = existing.get(&token) {
            if state.payload_digest == digest {
                continue;
            }
        }
        #[derive(Deserialize)]
        struct Epoch {
            group_epoch: u64,
        }
        let epoch = serde_json::from_slice::<Epoch>(update.payload())
            .map_err(backend)?
            .group_epoch;
        let state = DeliveryState {
            space_id: space_id.clone(),
            source: source.to_owned(),
            update_id: update.update_id().to_owned(),
            recipient: *update.recipient(),
            epoch,
            payload_digest: digest,
            attempts: 0,
            next_attempt_ms: 0,
            rejected: false,
        };
        let metadata = seal(key, &state, &aad("state", &token))?;
        let payload = seal(key, &update, &aad("payload", &token))?;
        sql_query("INSERT INTO group_update_delivery (lookup_token, space_lookup_token, encrypted_metadata, encrypted_payload) VALUES (?, ?, ?, ?) ON CONFLICT(lookup_token) DO UPDATE SET space_lookup_token = excluded.space_lookup_token, encrypted_metadata = excluded.encrypted_metadata, encrypted_payload = excluded.encrypted_payload")
            .bind::<Binary, _>(&token).bind::<Text, _>(&scope).bind::<Binary, _>(metadata).bind::<Binary, _>(payload).execute(conn).map_err(backend)?;
    }
    for token in existing.keys().filter(|token| !retained.contains(*token)) {
        sql_query("DELETE FROM group_update_delivery WHERE lookup_token = ?")
            .bind::<Binary, _>(token)
            .execute(conn)
            .map_err(backend)?;
    }
    seal(
        key,
        &SourceSummary {
            task_tokens: retained.into_iter().collect(),
        },
        &aad("source", source.as_bytes()),
    )
}

impl<E: DbExecutor> DieselSpaceSecurityStore<E> {
    pub(super) fn load_due_updates_on(
        &self,
        conn: &mut SqliteConnection,
        key: &MasterKey,
        space_id: &SpaceId,
        now_ms: i64,
        online_peer: Option<DeviceId>,
    ) -> Result<Vec<PendingGroupUpdate>, KeyEpochError> {
        conn.transaction::<_, anyhow::Error, _>(|conn| {
            let scope = space_lookup_token(key, space_id)?;
            let mut live_sources = HashSet::new();
            let mut expected_tokens = HashSet::new();
            let material = sql_query("SELECT space_lookup_token AS source_id FROM space_key_epoch_state WHERE space_lookup_token = ?")
                .bind::<Text, _>(&scope).get_result::<SourceRow>(conn).optional().map_err(backend)?;
            if let Some(row) = material {
                let source = format!("material:{}", row.source_id);
                let encrypted = match load_source_summary(conn, "space_material", &row.source_id)? {
                    Some(summary) => summary,
                    None => {
                        let updates = load_space_material_on(conn, key, space_id)?.ok_or(KeyEpochError::PersistedStateIntegrityFailed)?.pending_group_updates().to_vec();
                        let summary = reconcile_source(conn, key, space_id, &source, updates)?;
                        save_source_summary(conn, "space_material", &row.source_id, &summary)?;
                        summary
                    }
                };
                let summary: SourceSummary = open(key, &encrypted, &aad("source", source.as_bytes()))?;
                expected_tokens.extend(summary.task_tokens);
                live_sources.insert(source);
            }
            let revocations = sql_query("SELECT revocation_id AS source_id FROM member_revocation_log WHERE space_lookup_token = ?")
                .bind::<Text, _>(&scope).load::<SourceRow>(conn).map_err(backend)?;
            for row in revocations {
                let source = format!("revocation:{}", row.source_id);
                let encrypted = match load_source_summary(conn, "revocation", &row.source_id)? {
                    Some(summary) => summary,
                    None => {
                        let record_row = load_revocation_row(conn, &row.source_id).map_err(backend)?.ok_or(KeyEpochError::PersistedStateIntegrityFailed)?;
                        let record = decode_record(key, &record_row)?;
                        let updates = if matches!(record.status(), RevocationStatus::Activated | RevocationStatus::Distributing) {
                            let bytes = record_row.encrypted_stage.as_ref().ok_or(KeyEpochError::PersistedStateIntegrityFailed)?;
                            let stage: RevocationStage = open(key, bytes, &stage_aad(&row.source_id))?;
                            stage.outbox().iter().filter(|m| !m.is_confirmed()).map(|m| PendingGroupUpdate::for_generation(record.revocation_id().clone(), GroupEpoch::new(m.generation()), *m.recipient(), m.payload().to_vec())).collect()
                        } else { Vec::new() };
                        let summary = reconcile_source(conn, key, space_id, &source, updates)?;
                        save_source_summary(conn, "revocation", &row.source_id, &summary)?;
                        summary
                    }
                };
                let summary: SourceSummary = open(key, &encrypted, &aad("source", source.as_bytes()))?;
                expected_tokens.extend(summary.task_tokens);
                live_sources.insert(source);
            }
            let mut states = Vec::new();
            for (token, state) in read_states(conn, key, &scope)? {
                if state.space_id != *space_id { continue; }
                if !live_sources.contains(&state.source) {
                    sql_query("DELETE FROM group_update_delivery WHERE lookup_token = ?").bind::<Binary, _>(&token).execute(conn).map_err(backend)?;
                    continue;
                }
                if !expected_tokens.remove(&token) {
                    return Err(KeyEpochError::PersistedStateIntegrityFailed.into());
                }
                states.push((token, state));
            }
            if !expected_tokens.is_empty() {
                return Err(KeyEpochError::PersistedStateIntegrityFailed.into());
            }
            states.sort_by_key(|(_, state)| (state.epoch, state.next_attempt_ms));
            let mut held_peers = HashSet::new();
            let mut due = Vec::new();
            for (token, state) in states {
                // 同设备必须先完成前序，不能跳过等待或拒绝状态发送后序。
                if held_peers.contains(&state.recipient) { continue; }
                if state.rejected || (state.next_attempt_ms > now_ms && online_peer != Some(state.recipient)) {
                    held_peers.insert(state.recipient);
                    continue;
                }
                let row = sql_query("SELECT encrypted_payload FROM group_update_delivery WHERE lookup_token = ?")
                    .bind::<Binary, _>(&token).get_result::<PayloadRow>(conn).map_err(backend)?;
                let update: PendingGroupUpdate = open(key, &row.encrypted_payload, &aad("payload", &token))?;
                if update.update_id() != state.update_id || update.recipient() != &state.recipient || Sha256::digest(update.payload()).as_slice() != state.payload_digest {
                    return Err(KeyEpochError::PersistedStateIntegrityFailed.into());
                }
                due.push(update);
                if due.len() == 8 { break; }
            }
            Ok(due)
        })
        .map_err(transaction_failure)
    }

    pub(super) fn save_delivery_failures_on(
        &self,
        conn: &mut SqliteConnection,
        key: &MasterKey,
        space_id: &SpaceId,
        failures: &[(String, GroupUpdateDispatchError)],
        now_ms: i64,
    ) -> Result<usize, KeyEpochError> {
        conn.transaction::<_, anyhow::Error, _>(|conn| {
            let scope = space_lookup_token(key, space_id)?;
            let mut found = HashSet::new();
            for (token, mut state) in read_states(conn, key, &scope)? {
                if state.space_id != *space_id {
                    continue;
                }
                let Some((id, failure)) = failures.iter().find(|(id, _)| *id == state.update_id)
                else {
                    continue;
                };
                state.attempts = state.attempts.saturating_add(1);
                state.rejected = matches!(failure, GroupUpdateDispatchError::Rejected);
                let delay = 30_000_i64
                    .saturating_mul(1_i64 << state.attempts.saturating_sub(1).min(7))
                    .min(3_600_000);
                state.next_attempt_ms = now_ms.saturating_add(delay);
                save_state(conn, key, &token, &state)?;
                found.insert(id);
            }
            Ok(found.len())
        })
        .map_err(transaction_failure)
    }
}
