use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Text};
use uc_core::ids::SpaceId;
use uc_core::membership::{KeyEpochError, SpaceKeyMaterial, SpaceSecurityMode};

use crate::security::MasterKey;

use super::encrypted_payload::{open, seal, space_lookup_token};
use super::{backend, epoch_to_i64};

#[derive(QueryableByName)]
struct SpaceMaterialRow {
    #[diesel(sql_type = Text)]
    space_lookup_token: String,
    #[diesel(sql_type = BigInt)]
    group_epoch: i64,
    #[diesel(sql_type = Text)]
    security_mode: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
    #[diesel(sql_type = BigInt)]
    updated_at_ms: i64,
}

fn mode_name(mode: SpaceSecurityMode) -> &'static str {
    match mode {
        SpaceSecurityMode::Legacy => "legacy",
        SpaceSecurityMode::Migrating => "migrating",
        SpaceSecurityMode::Ready => "ready",
    }
}

fn space_aad(space_id: &str, epoch: i64) -> Vec<u8> {
    format!("uc-space-key-material-v1|{space_id}|{epoch}").into_bytes()
}

pub(super) fn save_space_material_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    material: &SpaceKeyMaterial,
) -> Result<(), KeyEpochError> {
    let state = material.state();
    let epoch = epoch_to_i64(state.epoch().value())?;
    let space_id = state.space_id().as_ref();
    let lookup_token = space_lookup_token(master_key, state.space_id())?;
    let encrypted = seal(master_key, material, &space_aad(space_id, epoch))?;
    diesel::sql_query(
        "INSERT INTO space_key_epoch_state \
         (space_lookup_token, group_epoch, security_mode, encrypted_payload, updated_at_ms) \
         VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(space_lookup_token) DO UPDATE SET \
         group_epoch = excluded.group_epoch, security_mode = excluded.security_mode, \
         encrypted_payload = excluded.encrypted_payload, updated_at_ms = excluded.updated_at_ms",
    )
    .bind::<Text, _>(lookup_token)
    .bind::<BigInt, _>(epoch)
    .bind::<Text, _>(mode_name(state.mode()))
    .bind::<Binary, _>(encrypted)
    .bind::<BigInt, _>(material.updated_at_ms())
    .execute(conn)
    .map_err(backend)?;
    Ok(())
}

fn decode_space_material(
    master_key: &MasterKey,
    row: SpaceMaterialRow,
    expected_space_id: &SpaceId,
) -> Result<SpaceKeyMaterial, KeyEpochError> {
    let material: SpaceKeyMaterial = open(
        master_key,
        &row.encrypted_payload,
        &space_aad(expected_space_id.as_ref(), row.group_epoch),
    )?;
    let state = material.state();
    if state.space_id() != expected_space_id
        || space_lookup_token(master_key, state.space_id())? != row.space_lookup_token
        || epoch_to_i64(state.epoch().value())? != row.group_epoch
        || mode_name(state.mode()) != row.security_mode
        || material.updated_at_ms() != row.updated_at_ms
    {
        return Err(backend("space key material row integrity mismatch"));
    }
    Ok(material)
}

pub(super) fn load_space_material_on(
    conn: &mut SqliteConnection,
    master_key: &MasterKey,
    space_id: &SpaceId,
) -> Result<Option<SpaceKeyMaterial>, KeyEpochError> {
    let lookup_token = space_lookup_token(master_key, space_id)?;
    let row = diesel::sql_query(
        "SELECT space_lookup_token, group_epoch, security_mode, \
         encrypted_payload, updated_at_ms FROM space_key_epoch_state WHERE space_lookup_token = ?",
    )
    .bind::<Text, _>(lookup_token)
    .get_result::<SpaceMaterialRow>(conn)
    .optional()
    .map_err(backend)?;
    row.map(|row| decode_space_material(master_key, row, space_id))
        .transpose()
}
