use diesel::prelude::*;
use diesel::sql_types::{BigInt, Binary, Nullable, Text};
use tempfile::{tempdir, TempDir};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    BeginRevocationOutcome, BootstrapId, ContentKeyId, GroupEpoch, LegacyBootstrapRecord,
    LegacyBootstrapRepositoryPort, LegacyBootstrapStage, LegacyBootstrapStatus,
    LegacyUpgradeDescriptor, LegacyUpgradeId, LegacyUpgradeRequest, PendingGroupUpdate,
    PreparedRevocationResolution, RevocationId, RevocationOutboxMessage, RevocationRecord,
    RevocationRepositoryPort, RevocationStage, RevocationStatus, SpaceKeyMaterial, SpaceKeyState,
    SpaceSecurityStateResetPort,
};
use uc_core::space_access::PreparedGroupJoin;

use super::DieselSpaceSecurityStore;
use crate::db::executor::DieselSqliteExecutor;
use crate::db::pool::{init_db_pool, DbPool};
use crate::security::legacy_upgrade::{LegacyUpgradeAttemptStore, PreparedLegacyUpgradeAttempt};
use crate::security::{InMemorySession, MasterKey};

fn make_repo() -> (
    DieselSpaceSecurityStore<DieselSqliteExecutor>,
    DbPool,
    TempDir,
) {
    let tempdir = tempdir().unwrap();
    let database_url = tempdir.path().join("revocation.sqlite");
    let pool = init_db_pool(database_url.to_str().unwrap()).unwrap();
    let session = InMemorySession::new();
    session.set_master_key(MasterKey::from_bytes(&[0x5a; 32]).unwrap());
    let repo = DieselSpaceSecurityStore::new(DieselSqliteExecutor::new(pool.clone()), session);
    (repo, pool, tempdir)
}

fn reopen_repo(pool: &DbPool) -> DieselSpaceSecurityStore<DieselSqliteExecutor> {
    let session = InMemorySession::new();
    session.set_master_key(MasterKey::from_bytes(&[0x5a; 32]).unwrap());
    DieselSpaceSecurityStore::new(DieselSqliteExecutor::new(pool.clone()), session)
}

fn ready_state() -> SpaceKeyState {
    let mut state = SpaceKeyState::legacy(SpaceId::from_str("space-sensitive"));
    state.mark_migrating().unwrap();
    state
        .mark_ready(
            ContentKeyId::from_string("content-key-current").unwrap(),
            uc_core::membership::ProtectionGroupId::generate(),
        )
        .unwrap();
    state
}

fn prepared(id: &str) -> RevocationRecord {
    prepared_for_space(id, "space-sensitive")
}

fn prepared_for_space(id: &str, space_id: &str) -> RevocationRecord {
    RevocationRecord::prepare(
        RevocationId::from_string(id).unwrap(),
        SpaceId::from_str(space_id),
        DeviceId::new("removed-device-sensitive"),
        GroupEpoch::new(1),
        100,
    )
    .unwrap()
}

fn staged(mut record: RevocationRecord) -> RevocationStage {
    record.transition_to(RevocationStatus::Staged, 110).unwrap();
    let mut next_state = ready_state();
    next_state
        .rotate(ContentKeyId::from_string("content-key-next").unwrap())
        .unwrap();
    RevocationStage::new(
        record,
        next_state,
        b"group-state-sensitive".to_vec(),
        b"key-catalog-sensitive".to_vec(),
        vec![
            RevocationOutboxMessage::new(
                DeviceId::new("retained-device-sensitive"),
                b"commit-sensitive".to_vec(),
            ),
            RevocationOutboxMessage::new(
                DeviceId::new("second-retained-device-sensitive"),
                b"second-commit-sensitive".to_vec(),
            ),
        ],
    )
    .unwrap()
}

fn staged_legacy_bootstrap() -> LegacyBootstrapStage {
    let mut record = LegacyBootstrapRecord::prepare(
        BootstrapId::from_string("bootstrap-sensitive").unwrap(),
        SpaceId::from_str("space-sensitive"),
        DeviceId::new("sponsor-sensitive"),
        vec![DeviceId::new("retained-device-sensitive")],
        100,
    )
    .unwrap();
    record
        .transition_to(LegacyBootstrapStatus::Staged, 110)
        .unwrap();
    LegacyBootstrapStage::new(
        record,
        SpaceKeyMaterial::new(
            ready_state(),
            b"mls-group-state-sensitive".to_vec(),
            b"key-catalog-sensitive".to_vec(),
            110,
        ),
    )
    .unwrap()
}

async fn seed_current_space(
    repo: &DieselSpaceSecurityStore<DieselSqliteExecutor>,
) -> SpaceKeyMaterial {
    let material = SpaceKeyMaterial::new(
        ready_state(),
        b"old-group-state-sensitive".to_vec(),
        b"old-key-catalog-sensitive".to_vec(),
        90,
    );
    repo.save_space_material(&material).await.unwrap();
    material
}

#[tokio::test]
async fn begin_is_idempotent_for_the_same_space_and_target() {
    let (repo, _pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    let first = prepared("revocation-first");
    let duplicate = prepared("revocation-duplicate");

    assert_eq!(
        repo.begin_revocation(&first).await.unwrap(),
        BeginRevocationOutcome::Begun(first.clone())
    );
    assert_eq!(
        repo.begin_revocation(&duplicate).await.unwrap(),
        BeginRevocationOutcome::Existing(first)
    );
}

#[tokio::test]
async fn begin_replaces_an_obsolete_prepared_revocation_after_the_epoch_advances() {
    let (repo, _pool, _tempdir) = make_repo();
    let current = seed_current_space(&repo).await;
    let stale = prepared("revocation-stale");
    repo.begin_revocation(&stale).await.unwrap();

    let mut advanced_state = current.state().clone();
    advanced_state
        .rotate(ContentKeyId::from_string("content-key-advanced").unwrap())
        .unwrap();
    repo.save_space_material(&SpaceKeyMaterial::new(
        advanced_state,
        b"advanced-group-state-sensitive".to_vec(),
        b"advanced-key-catalog-sensitive".to_vec(),
        150,
    ))
    .await
    .unwrap();
    let replacement = RevocationRecord::prepare(
        RevocationId::from_string("revocation-replacement").unwrap(),
        SpaceId::from_str("space-sensitive"),
        DeviceId::new("removed-device-sensitive"),
        GroupEpoch::new(2),
        200,
    )
    .unwrap();

    assert_eq!(
        repo.begin_revocation(&replacement).await.unwrap(),
        BeginRevocationOutcome::Begun(replacement.clone())
    );
    assert!(repo
        .get_revocation(stale.revocation_id())
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.list_incomplete_revocations().await.unwrap(),
        vec![replacement]
    );
}

#[tokio::test]
async fn begin_rejects_a_concurrent_revocation_for_another_member() {
    let (repo, _pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    let first = prepared("revocation-first-target");
    repo.begin_revocation(&first).await.unwrap();
    let second = RevocationRecord::prepare(
        RevocationId::from_string("revocation-second-target").unwrap(),
        SpaceId::from_str("space-sensitive"),
        DeviceId::new("another-removed-device"),
        GroupEpoch::new(1),
        101,
    )
    .unwrap();

    assert!(repo.begin_revocation(&second).await.is_err());
}

#[derive(QueryableByName)]
struct RawCiphertexts {
    #[diesel(sql_type = Binary)]
    encrypted_record: Vec<u8>,
    #[diesel(sql_type = Nullable<Binary>)]
    encrypted_stage: Option<Vec<u8>>,
}

#[derive(QueryableByName)]
struct RawSpaceCiphertext {
    #[diesel(sql_type = Text)]
    space_lookup_token: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

#[derive(QueryableByName)]
struct RawLegacyUpgradeCiphertext {
    #[diesel(sql_type = Text)]
    peer_lookup_token: String,
    #[diesel(sql_type = Binary)]
    encrypted_payload: Vec<u8>,
}

#[derive(QueryableByName)]
struct RowCount {
    #[diesel(sql_type = BigInt)]
    count: i64,
}

#[tokio::test]
async fn all_sensitive_revocation_payloads_are_ciphertext_at_rest() {
    let (repo, pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    let prepared = prepared("revocation-encrypted");
    repo.begin_revocation(&prepared).await.unwrap();
    repo.stage_revocation(&staged(prepared)).await.unwrap();

    let mut conn = pool.get().unwrap();
    let row = diesel::sql_query(
        "SELECT encrypted_record, encrypted_stage FROM member_revocation_log LIMIT 1",
    )
    .get_result::<RawCiphertexts>(&mut conn)
    .unwrap();
    let space = diesel::sql_query(
        "SELECT space_lookup_token, encrypted_payload FROM space_key_epoch_state LIMIT 1",
    )
    .get_result::<RawSpaceCiphertext>(&mut conn)
    .unwrap();
    let mut persisted = row.encrypted_record;
    persisted.extend(row.encrypted_stage.unwrap());
    persisted.extend(space.space_lookup_token.as_bytes());
    persisted.extend(space.encrypted_payload);

    for plaintext in [
        "space-sensitive",
        "content-key-current",
        "removed-device-sensitive",
        "retained-device-sensitive",
        "group-state-sensitive",
        "key-catalog-sensitive",
        "commit-sensitive",
        "old-group-state-sensitive",
        "old-key-catalog-sensitive",
    ] {
        assert!(
            !persisted
                .windows(plaintext.len())
                .any(|window| window == plaintext.as_bytes()),
            "plaintext leaked into database: {plaintext}"
        );
    }
}

#[tokio::test]
async fn pending_group_update_is_encrypted_and_survives_restart_until_acknowledged() {
    let (repo, pool, _tempdir) = make_repo();
    let mut material = seed_current_space(&repo).await;
    let update = PendingGroupUpdate::persistent(
        DeviceId::new("pending-recipient-sensitive"),
        b"pending-commit-sensitive".to_vec(),
    );
    let update_id = update.update_id().to_string();
    material.add_pending_group_updates([update.clone()], 100);
    repo.save_space_material(&material).await.unwrap();

    let mut conn = pool.get().unwrap();
    let row = diesel::sql_query(
        "SELECT space_lookup_token, encrypted_payload FROM space_key_epoch_state LIMIT 1",
    )
    .get_result::<RawSpaceCiphertext>(&mut conn)
    .unwrap();
    drop(conn);
    for plaintext in ["pending-recipient-sensitive", "pending-commit-sensitive"] {
        assert!(
            !row.encrypted_payload
                .windows(plaintext.len())
                .any(|window| window == plaintext.as_bytes()),
            "plaintext leaked into database: {plaintext}"
        );
    }

    drop(repo);
    let reopened = reopen_repo(&pool);
    let mut restored = reopened
        .load_space_material(&SpaceId::from_str("space-sensitive"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.pending_group_updates(), &[update]);

    assert!(restored.acknowledge_group_update(&update_id, 110));
    reopened.save_space_material(&restored).await.unwrap();
    assert!(reopen_repo(&pool)
        .load_space_material(&SpaceId::from_str("space-sensitive"))
        .await
        .unwrap()
        .unwrap()
        .pending_group_updates()
        .is_empty());
}

#[tokio::test]
async fn activation_atomically_publishes_the_staged_space() {
    let (repo, _pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    let prepared = prepared("revocation-activate");
    repo.begin_revocation(&prepared).await.unwrap();
    let stage = staged(prepared);
    repo.stage_revocation(&stage).await.unwrap();

    let activated = repo
        .activate_revocation(stage.record().revocation_id(), 120)
        .await
        .unwrap();
    let loaded = repo
        .load_space_material(stage.record().space_id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(activated.status(), RevocationStatus::Activated);
    assert_eq!(loaded.state(), stage.next_space_state());
    assert_eq!(loaded.group_state(), stage.group_state());
    assert_eq!(loaded.key_catalog(), stage.key_catalog());
}

#[tokio::test]
async fn activation_preserves_pending_admission_updates() {
    let (repo, _pool, _tempdir) = make_repo();
    let mut current = seed_current_space(&repo).await;
    let pending = PendingGroupUpdate::persistent(
        DeviceId::new("offline-retained-member"),
        b"pending-admission-update".to_vec(),
    );
    let removed_member_pending = PendingGroupUpdate::persistent(
        DeviceId::new("removed-device-sensitive"),
        b"obsolete-target-update".to_vec(),
    );
    current.add_pending_group_updates([pending.clone(), removed_member_pending], 100);
    repo.save_space_material(&current).await.unwrap();
    let prepared = prepared("revocation-preserve-admission");
    repo.begin_revocation(&prepared).await.unwrap();
    let stage = staged(prepared);
    repo.stage_revocation(&stage).await.unwrap();

    repo.activate_revocation(stage.record().revocation_id(), 120)
        .await
        .unwrap();

    assert_eq!(
        repo.load_space_material(stage.record().space_id())
            .await
            .unwrap()
            .unwrap()
            .pending_group_updates(),
        &[pending]
    );
}

#[tokio::test]
async fn staged_revocation_survives_repository_restart() {
    let (repo, pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    let prepared = prepared("revocation-restart-stage");
    repo.begin_revocation(&prepared).await.unwrap();
    let stage = staged(prepared);
    repo.stage_revocation(&stage).await.unwrap();
    drop(repo);

    let reopened = reopen_repo(&pool);
    assert_eq!(
        reopened
            .load_staged_revocation(stage.record().revocation_id())
            .await
            .unwrap(),
        Some(stage)
    );
}

#[tokio::test]
async fn incomplete_revocations_are_discovered_after_restart() {
    let (repo, pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    let prepared = prepared("revocation-restart-list");
    repo.begin_revocation(&prepared).await.unwrap();
    let stage = staged(prepared);
    repo.stage_revocation(&stage).await.unwrap();
    drop(repo);

    let reopened = reopen_repo(&pool);
    assert_eq!(
        reopened.list_incomplete_revocations().await.unwrap(),
        vec![stage.record().clone()]
    );
}

#[tokio::test]
async fn verified_absent_target_finishes_prepared_revocation_after_restart() {
    let (repo, pool, _tempdir) = make_repo();
    let material = seed_current_space(&repo).await;
    let prepared = RevocationRecord::prepare_with_recipients(
        RevocationId::from_string("prepared-absent-after-restart").unwrap(),
        SpaceId::from_str("space-sensitive"),
        DeviceId::new("already-absent-device-sensitive"),
        vec![DeviceId::new("retained-device-sensitive")],
        material.state().epoch(),
        100,
    )
    .unwrap();
    repo.begin_revocation(&prepared).await.unwrap();
    drop(repo);

    let reopened = reopen_repo(&pool);
    let resolved = reopened
        .resolve_prepared_revocation(
            prepared.revocation_id(),
            PreparedRevocationResolution::TargetAbsent(material.clone()),
            110,
        )
        .await
        .unwrap();
    assert_eq!(resolved.status(), RevocationStatus::Complete);

    assert_eq!(
        reopened
            .get_revocation(prepared.revocation_id())
            .await
            .unwrap()
            .unwrap()
            .status(),
        RevocationStatus::Complete
    );
    assert_eq!(
        reopened
            .load_space_material(prepared.space_id())
            .await
            .unwrap()
            .unwrap(),
        material
    );
    assert!(reopened
        .load_staged_revocation(prepared.revocation_id())
        .await
        .unwrap()
        .is_none());

    let mut conn = pool.get().unwrap();
    let row = diesel::sql_query(
        "SELECT encrypted_record, encrypted_stage FROM member_revocation_log LIMIT 1",
    )
    .get_result::<RawCiphertexts>(&mut conn)
    .unwrap();
    assert!(row.encrypted_stage.is_none());
    for plaintext in [
        "space-sensitive",
        "already-absent-device-sensitive",
        "retained-device-sensitive",
    ] {
        assert!(!row
            .encrypted_record
            .windows(plaintext.len())
            .any(|window| window == plaintext.as_bytes()));
    }
}

#[tokio::test]
async fn verified_present_target_restages_prepared_revocation_from_current_epoch() {
    let (repo, pool, _tempdir) = make_repo();
    let material = seed_current_space(&repo).await;
    let prepared = RevocationRecord::prepare(
        RevocationId::from_string("prepared-present-after-restart").unwrap(),
        SpaceId::from_str("space-sensitive"),
        DeviceId::new("present-device-sensitive"),
        material.state().epoch(),
        100,
    )
    .unwrap();
    repo.begin_revocation(&prepared).await.unwrap();
    let mut advanced_state = material.state().clone();
    advanced_state
        .rotate(ContentKeyId::from_string("content-key-advanced").unwrap())
        .unwrap();
    let advanced = SpaceKeyMaterial::new(
        advanced_state.clone(),
        b"advanced-group-state-sensitive".to_vec(),
        b"advanced-key-catalog-sensitive".to_vec(),
        150,
    );
    repo.save_space_material(&advanced).await.unwrap();
    drop(repo);

    let reopened = reopen_repo(&pool);
    let mut rebuilt = prepared.clone();
    rebuilt
        .rebase_prepared(advanced.state().epoch(), 200)
        .unwrap();
    rebuilt
        .transition_to(RevocationStatus::Staged, 200)
        .unwrap();
    let mut next_state = advanced.state().clone();
    next_state
        .rotate(ContentKeyId::from_string("content-key-rebuilt").unwrap())
        .unwrap();
    let stage = RevocationStage::new(
        rebuilt,
        next_state,
        b"rebuilt-group-state-sensitive".to_vec(),
        b"rebuilt-key-catalog-sensitive".to_vec(),
        Vec::new(),
    )
    .unwrap();

    let resolved = reopened
        .resolve_prepared_revocation(
            prepared.revocation_id(),
            PreparedRevocationResolution::TargetPresent {
                current_material: advanced.clone(),
                stage: stage.clone(),
            },
            200,
        )
        .await
        .unwrap();

    assert_eq!(resolved, stage.record().clone());
    assert_eq!(resolved.status(), RevocationStatus::Staged);
    assert_eq!(resolved.previous_epoch(), advanced.state().epoch());
    assert_eq!(
        reopened
            .load_space_material(prepared.space_id())
            .await
            .unwrap()
            .unwrap(),
        advanced
    );
    assert_eq!(
        reopened
            .load_staged_revocation(prepared.revocation_id())
            .await
            .unwrap(),
        Some(stage)
    );
}

#[tokio::test]
async fn clearing_space_security_state_preserves_current_space_and_removes_old_space_records() {
    let (repo, pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    repo.begin_revocation(&prepared("revocation-to-clear"))
        .await
        .unwrap();
    repo.save_space_material(&SpaceKeyMaterial::new(
        SpaceKeyState::legacy(SpaceId::from_str("other-space-sensitive")),
        b"old-other-group-state-sensitive".to_vec(),
        b"old-other-key-catalog-sensitive".to_vec(),
        90,
    ))
    .await
    .unwrap();
    repo.begin_revocation(&prepared_for_space(
        "other-space-revocation-to-clear",
        "other-space-sensitive",
    ))
    .await
    .unwrap();
    repo.begin_legacy_bootstrap(
        &LegacyBootstrapRecord::prepare(
            BootstrapId::from_string("bootstrap-to-clear").unwrap(),
            SpaceId::from_str("legacy-other-space-sensitive"),
            DeviceId::new("sponsor-sensitive"),
            vec![DeviceId::new("retained-device-sensitive")],
            100,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    repo.clear_space_security_state_except(&SpaceId::from_str("space-sensitive"))
        .await
        .unwrap();

    let mut conn = pool.get().unwrap();
    assert_eq!(
        diesel::sql_query("SELECT COUNT(*) AS count FROM member_revocation_log")
            .get_result::<RowCount>(&mut conn)
            .unwrap()
            .count,
        1
    );
    assert_eq!(
        diesel::sql_query("SELECT COUNT(*) AS count FROM space_key_epoch_state")
            .get_result::<RowCount>(&mut conn)
            .unwrap()
            .count,
        1
    );
    assert_eq!(
        diesel::sql_query("SELECT COUNT(*) AS count FROM legacy_space_bootstrap_log")
            .get_result::<RowCount>(&mut conn)
            .unwrap()
            .count,
        0
    );
}

#[tokio::test]
async fn clearing_space_security_state_preserves_current_legacy_bootstrap() {
    let (repo, pool, _tempdir) = make_repo();
    repo.begin_legacy_bootstrap(
        &LegacyBootstrapRecord::prepare(
            BootstrapId::from_string("bootstrap-to-preserve").unwrap(),
            SpaceId::from_str("space-sensitive"),
            DeviceId::new("sponsor-sensitive"),
            vec![DeviceId::new("retained-device-sensitive")],
            100,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    repo.save_space_material(&SpaceKeyMaterial::new(
        SpaceKeyState::legacy(SpaceId::from_str("other-space-sensitive")),
        b"old-other-group-state-sensitive".to_vec(),
        b"old-other-key-catalog-sensitive".to_vec(),
        90,
    ))
    .await
    .unwrap();
    repo.begin_revocation(&prepared_for_space(
        "other-space-revocation-to-clear",
        "other-space-sensitive",
    ))
    .await
    .unwrap();

    repo.clear_space_security_state_except(&SpaceId::from_str("space-sensitive"))
        .await
        .unwrap();

    let mut conn = pool.get().unwrap();
    assert_eq!(
        diesel::sql_query("SELECT COUNT(*) AS count FROM member_revocation_log")
            .get_result::<RowCount>(&mut conn)
            .unwrap()
            .count,
        0
    );
    assert_eq!(
        diesel::sql_query("SELECT COUNT(*) AS count FROM space_key_epoch_state")
            .get_result::<RowCount>(&mut conn)
            .unwrap()
            .count,
        0
    );
    assert_eq!(
        diesel::sql_query("SELECT COUNT(*) AS count FROM legacy_space_bootstrap_log")
            .get_result::<RowCount>(&mut conn)
            .unwrap()
            .count,
        1
    );
}

#[tokio::test]
async fn failed_activation_rolls_back_epoch_and_revocation_status() {
    let (repo, pool, _tempdir) = make_repo();
    let original = seed_current_space(&repo).await;
    let prepared = prepared("revocation-rollback");
    repo.begin_revocation(&prepared).await.unwrap();
    let stage = staged(prepared);
    repo.stage_revocation(&stage).await.unwrap();
    let mut conn = pool.get().unwrap();
    diesel::sql_query(
        "CREATE TRIGGER fail_space_activation BEFORE UPDATE ON space_key_epoch_state \
         BEGIN SELECT RAISE(ABORT, 'forced activation failure'); END",
    )
    .execute(&mut conn)
    .unwrap();
    drop(conn);

    assert!(repo
        .activate_revocation(stage.record().revocation_id(), 120)
        .await
        .is_err());
    assert_eq!(
        repo.get_revocation(stage.record().revocation_id())
            .await
            .unwrap()
            .unwrap()
            .status(),
        RevocationStatus::Staged
    );
    assert_eq!(
        repo.load_space_material(stage.record().space_id())
            .await
            .unwrap()
            .unwrap(),
        original
    );
}

#[tokio::test]
async fn staged_legacy_bootstrap_survives_repository_restart() {
    let (repo, pool, _tempdir) = make_repo();
    let stage = staged_legacy_bootstrap();
    let prepared = LegacyBootstrapRecord::prepare(
        stage.record().bootstrap_id().clone(),
        stage.record().space_id().clone(),
        stage.record().sponsor_device_id().clone(),
        stage.record().pending_readmission().to_vec(),
        stage.record().created_at_ms(),
    )
    .unwrap();
    repo.begin_legacy_bootstrap(&prepared).await.unwrap();
    repo.stage_legacy_bootstrap(&stage).await.unwrap();
    drop(repo);

    assert_eq!(
        reopen_repo(&pool)
            .load_legacy_bootstrap_stage(stage.record().bootstrap_id())
            .await
            .unwrap(),
        Some(stage)
    );
}

#[tokio::test]
async fn legacy_bootstrap_activation_is_atomic_and_waits_for_readmission() {
    let (repo, pool, _tempdir) = make_repo();
    let stage = staged_legacy_bootstrap();
    let bootstrap_id = stage.record().bootstrap_id().clone();

    assert_eq!(
        repo.begin_legacy_bootstrap(
            &LegacyBootstrapRecord::prepare(
                bootstrap_id.clone(),
                SpaceId::from_str("space-sensitive"),
                DeviceId::new("sponsor-sensitive"),
                vec![DeviceId::new("retained-device-sensitive")],
                100,
            )
            .unwrap()
        )
        .await
        .unwrap()
        .status(),
        LegacyBootstrapStatus::Prepared
    );
    repo.stage_legacy_bootstrap(&stage).await.unwrap();

    let mut conn = pool.get().unwrap();
    let row = diesel::sql_query(
        "SELECT encrypted_record, encrypted_stage FROM legacy_space_bootstrap_log LIMIT 1",
    )
    .get_result::<RawCiphertexts>(&mut conn)
    .unwrap();
    let mut persisted = row.encrypted_record;
    persisted.extend(row.encrypted_stage.unwrap());
    for plaintext in [
        "space-sensitive",
        "sponsor-sensitive",
        "retained-device-sensitive",
        "mls-group-state-sensitive",
        "key-catalog-sensitive",
    ] {
        assert!(
            !persisted
                .windows(plaintext.len())
                .any(|window| window == plaintext.as_bytes()),
            "plaintext leaked into bootstrap database row: {plaintext}"
        );
    }
    drop(conn);

    let activated = repo
        .activate_legacy_bootstrap(&bootstrap_id, 120)
        .await
        .unwrap();
    assert_eq!(
        activated.status(),
        LegacyBootstrapStatus::AwaitingReadmission
    );
    assert_eq!(
        repo.load_space_material(&SpaceId::from_str("space-sensitive"))
            .await
            .unwrap()
            .unwrap(),
        stage.material().clone()
    );
    assert_eq!(
        repo.list_incomplete_legacy_bootstraps_for_space(&SpaceId::from_str("space-sensitive"))
            .await
            .unwrap(),
        vec![activated.clone()]
    );

    let completed = repo
        .acknowledge_legacy_readmission(
            &bootstrap_id,
            &DeviceId::new("retained-device-sensitive"),
            130,
        )
        .await
        .unwrap();
    assert_eq!(completed.status(), LegacyBootstrapStatus::Complete);
    assert!(repo
        .list_incomplete_legacy_bootstraps_for_space(&SpaceId::from_str("space-sensitive"))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn completed_legacy_bootstrap_is_not_read_by_the_space_status_lookup() {
    let (repo, pool, _tempdir) = make_repo();
    let stage = staged_legacy_bootstrap();
    let bootstrap_id = stage.record().bootstrap_id().clone();
    let retained_member = DeviceId::new("retained-device-sensitive");
    repo.begin_legacy_bootstrap(
        &LegacyBootstrapRecord::prepare(
            bootstrap_id.clone(),
            SpaceId::from_str("space-sensitive"),
            DeviceId::new("sponsor-sensitive"),
            vec![retained_member.clone()],
            100,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    repo.stage_legacy_bootstrap(&stage).await.unwrap();
    repo.activate_legacy_bootstrap(&bootstrap_id, 120)
        .await
        .unwrap();
    repo.acknowledge_legacy_readmission(&bootstrap_id, &retained_member, 130)
        .await
        .unwrap();

    let mut conn = pool.get().unwrap();
    diesel::sql_query(
        "UPDATE legacy_space_bootstrap_log SET encrypted_record = ? WHERE bootstrap_id = ?",
    )
    .bind::<Binary, _>(b"unreadable-obsolete-bootstrap".to_vec())
    .bind::<Text, _>(bootstrap_id.as_str())
    .execute(&mut conn)
    .unwrap();

    assert!(repo
        .list_non_complete_legacy_bootstraps_for_space(&SpaceId::from_str("space-sensitive"))
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn space_scoped_legacy_bootstrap_lists_do_not_decrypt_other_spaces() {
    let (repo, pool, _tempdir) = make_repo();
    let unrelated_space = SpaceId::from_str("old-space-sensitive");
    repo.begin_legacy_bootstrap(
        &LegacyBootstrapRecord::prepare(
            BootstrapId::from_string("old-space-bootstrap-sensitive").unwrap(),
            unrelated_space,
            DeviceId::new("old-space-sponsor-sensitive"),
            Vec::new(),
            100,
        )
        .unwrap(),
    )
    .await
    .unwrap();
    drop(repo);

    let session = InMemorySession::new();
    session.set_master_key(MasterKey::from_bytes(&[0x6b; 32]).unwrap());
    let current_repo = DieselSpaceSecurityStore::new(DieselSqliteExecutor::new(pool), session);
    let active_space = SpaceId::from_str("current-space-sensitive");

    assert!(current_repo
        .list_incomplete_legacy_bootstraps_for_space(&active_space)
        .await
        .unwrap()
        .is_empty());
    assert!(current_repo
        .list_non_complete_legacy_bootstraps_for_space(&active_space)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn distribution_progress_survives_restart_and_completes_after_all_confirmations() {
    let (repo, pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    let prepared = prepared("revocation-distribution");
    repo.begin_revocation(&prepared).await.unwrap();
    let stage = staged(prepared);
    let revocation_id = stage.record().revocation_id().clone();
    repo.stage_revocation(&stage).await.unwrap();
    repo.activate_revocation(&revocation_id, 120).await.unwrap();

    let distributing = repo.start_distribution(&revocation_id, 130).await.unwrap();
    assert_eq!(distributing.status(), RevocationStatus::Distributing);
    let waiting = repo
        .acknowledge_recipient(
            &revocation_id,
            &DeviceId::new("retained-device-sensitive"),
            140,
        )
        .await
        .unwrap();
    assert_eq!(waiting.status(), RevocationStatus::Distributing);
    drop(repo);

    let reopened = reopen_repo(&pool);
    let resumed = reopened
        .load_staged_revocation(&revocation_id)
        .await
        .unwrap()
        .unwrap();
    assert!(resumed.outbox()[0].is_confirmed());
    assert!(!resumed.outbox()[1].is_confirmed());

    let complete = reopened
        .acknowledge_recipient(
            &revocation_id,
            &DeviceId::new("second-retained-device-sensitive"),
            150,
        )
        .await
        .unwrap();
    assert_eq!(complete.status(), RevocationStatus::Complete);
    assert!(reopened
        .load_staged_revocation(&revocation_id)
        .await
        .unwrap()
        .is_none());
    assert!(reopened
        .list_incomplete_revocations()
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn permanent_loss_recovery_is_atomic_and_survives_restart() {
    let (repo, pool, _tempdir) = make_repo();
    seed_current_space(&repo).await;
    let mut record = RevocationRecord::prepare_with_recipients(
        RevocationId::from_string("revocation-recovery").unwrap(),
        SpaceId::from_str("space-sensitive"),
        DeviceId::new("removed-device-sensitive"),
        vec![
            DeviceId::new("lost-device-sensitive"),
            DeviceId::new("retained-device-sensitive"),
        ],
        GroupEpoch::new(1),
        100,
    )
    .unwrap();
    repo.begin_revocation(&record).await.unwrap();
    record.transition_to(RevocationStatus::Staged, 110).unwrap();
    let mut first_state = ready_state();
    first_state
        .rotate(ContentKeyId::from_string("content-key-next").unwrap())
        .unwrap();
    let stage = RevocationStage::new(
        record,
        first_state,
        b"group-state-sensitive".to_vec(),
        b"key-catalog-sensitive".to_vec(),
        vec![
            RevocationOutboxMessage::new(
                DeviceId::new("lost-device-sensitive"),
                b"lost-commit-sensitive".to_vec(),
            ),
            RevocationOutboxMessage::new(
                DeviceId::new("retained-device-sensitive"),
                b"retained-commit-sensitive".to_vec(),
            ),
        ],
    )
    .unwrap();
    let revocation_id = stage.record().revocation_id().clone();
    repo.stage_revocation(&stage).await.unwrap();
    repo.activate_revocation(&revocation_id, 120).await.unwrap();
    repo.start_distribution(&revocation_id, 130).await.unwrap();
    repo.acknowledge_recipient(
        &revocation_id,
        &DeviceId::new("retained-device-sensitive"),
        140,
    )
    .await
    .unwrap();
    let mut recovery = repo
        .load_staged_revocation(&revocation_id)
        .await
        .unwrap()
        .unwrap();
    let mut next_state = recovery.next_space_state().clone();
    next_state
        .rotate(ContentKeyId::from_string("content-key-recovery").unwrap())
        .unwrap();
    recovery
        .append_recovery_generation(
            &DeviceId::new("lost-device-sensitive"),
            next_state.clone(),
            b"recovery-group-state-sensitive".to_vec(),
            b"recovery-key-catalog-sensitive".to_vec(),
            vec![RevocationOutboxMessage::new(
                DeviceId::new("retained-device-sensitive"),
                b"recovery-commit-sensitive".to_vec(),
            )],
            150,
        )
        .unwrap();
    let material = SpaceKeyMaterial::new(
        next_state,
        b"recovery-group-state-sensitive".to_vec(),
        b"recovery-key-catalog-sensitive".to_vec(),
        150,
    );

    repo.commit_revocation_recovery(&recovery, &material)
        .await
        .unwrap();
    assert!(repo
        .commit_revocation_recovery(&recovery, &material)
        .await
        .is_err());
    drop(repo);

    let reopened = reopen_repo(&pool);
    let restored = reopened
        .load_staged_revocation(&revocation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored.generation_count(), 2);
    assert_eq!(
        restored.removed_device_ids(),
        vec![
            DeviceId::new("removed-device-sensitive"),
            DeviceId::new("lost-device-sensitive")
        ]
    );
    assert_eq!(
        restored.pending_recipient_device_ids(),
        vec![DeviceId::new("retained-device-sensitive")]
    );
    assert_eq!(
        reopened
            .load_space_material(&SpaceId::from_str("space-sensitive"))
            .await
            .unwrap()
            .unwrap()
            .state()
            .epoch(),
        GroupEpoch::new(3)
    );
}

#[tokio::test]
async fn pending_legacy_upgrade_join_survives_restart_without_plaintext_storage() {
    let (repo, pool, _tempdir) = make_repo();
    let peer = DeviceId::new("peer-device-sensitive");
    let request = LegacyUpgradeRequest::unsigned(
        DeviceId::new("local-device-sensitive"),
        peer,
        LegacyUpgradeDescriptor::legacy(LegacyUpgradeId::from_bytes([0x71; 32])),
        b"public-key-package-sensitive".to_vec(),
    )
    .with_proof(b"proof-sensitive".to_vec());
    let pending = PreparedLegacyUpgradeAttempt::new(
        request,
        PreparedGroupJoin::new(
            b"public-key-package-sensitive".to_vec(),
            b"private-join-state-sensitive".to_vec(),
        ),
    );

    repo.save_pending_attempt(&peer, &pending, 100)
        .await
        .unwrap();
    drop(repo);

    let reopened = reopen_repo(&pool);
    let restored = reopened.load_pending_attempt(&peer).await.unwrap().unwrap();
    assert_eq!(restored.request(), pending.request());
    assert_eq!(
        restored.pending_group_join().private_state(),
        pending.pending_group_join().private_state()
    );

    let mut conn = pool.get().unwrap();
    let row = diesel::sql_query(
        "SELECT peer_lookup_token, encrypted_payload FROM legacy_upgrade_pending_join LIMIT 1",
    )
    .get_result::<RawLegacyUpgradeCiphertext>(&mut conn)
    .unwrap();
    assert_ne!(row.peer_lookup_token, peer.as_str());
    let row_bytes = [
        row.peer_lookup_token.as_bytes(),
        row.encrypted_payload.as_slice(),
    ]
    .concat();
    for sensitive in [
        peer.as_str().as_bytes(),
        b"local-device-sensitive".as_slice(),
        b"proof-sensitive".as_slice(),
        b"private-join-state-sensitive".as_slice(),
    ] {
        assert!(!row_bytes
            .windows(sensitive.len())
            .any(|window| window == sensitive));
    }

    reopened.clear_pending_attempt(&peer).await.unwrap();
    assert!(reopened
        .load_pending_attempt(&peer)
        .await
        .unwrap()
        .is_none());
}
