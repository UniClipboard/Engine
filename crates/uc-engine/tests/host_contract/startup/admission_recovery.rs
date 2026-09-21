use std::path::{Path, PathBuf};
use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::sql_types::Binary;
use diesel::{Connection, RunQueryDsl};
use uc_engine::{
    CreateSpaceInput, DeviceMembershipSummary, Engine, EngineConfig, JoinSpaceInput,
    ListHistoryEntriesInput, Operation, OperationResult, SecretString, SendTextInput,
    StartupProgress, StartupState,
};

use super::{host, MemorySecureStorage};

fn runtime_database(root: &Path, generation_directory: &str, relative: &str) -> PathBuf {
    let databases = std::fs::read_dir(root.join("private").join(generation_directory))
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.path().join(relative))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    assert_eq!(databases.len(), 1, "expected one active runtime database");
    databases.into_iter().next().unwrap()
}

fn runtime_databases(root: &Path) -> (PathBuf, PathBuf) {
    (
        runtime_database(
            root,
            "profile-data-generations",
            "v3-payloads/profile.sqlite",
        ),
        runtime_database(root, "space-control-generations", "control.sqlite"),
    )
}

fn omit_later_runtime_tables(profile_database: &Path, control_database: &Path) {
    let mut profile =
        diesel::sqlite::SqliteConnection::establish(profile_database.to_str().unwrap()).unwrap();
    profile
        .batch_execute(
            "DROP TRIGGER invalidate_revocation_delivery_summary_after_delete;
             DROP TRIGGER invalidate_revocation_delivery_summary_after_update;
             DROP TRIGGER invalidate_space_delivery_summary_after_delete;
             DROP TRIGGER invalidate_space_delivery_summary_after_update;
             DROP TABLE group_update_source;
             DROP TABLE group_update_delivery;
             DELETE FROM __diesel_schema_migrations WHERE version = '20260913000003';",
        )
        .unwrap();
    let mut control =
        diesel::sqlite::SqliteConnection::establish(control_database.to_str().unwrap()).unwrap();
    control
        .batch_execute(
            "DROP TRIGGER invalidate_admission_recovery_summary_after_delete;
             DROP TRIGGER invalidate_admission_recovery_summary_after_update;
             DROP TABLE admission_recovery_summary;
             DROP TABLE admission_repository_record;
             DELETE FROM __diesel_schema_migrations
             WHERE version IN ('20260913000001', '20260913000002');",
        )
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn healthy_legacy_v3_without_later_tables_starts_normally_and_preserves_state() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("legacy upgrade test".into()),
            passphrase: SecretString::new("test-passphrase"),
            passphrase_confirmation: SecretString::new("test-passphrase"),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(sent) = engine
        .execute(Operation::SendText(SendTextInput {
            text: "history retained through legacy upgrade".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected saved history entry")
    };
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    let (profile_database, control_database) = runtime_databases(root.path());
    omit_later_runtime_tables(&profile_database, &control_database);

    for restart in 0..2 {
        let (input, progress) = StartupProgress::channel();
        let (engine, _events) = Engine::start_with_progress(
            EngineConfig::new("2.0.0"),
            host(root.path(), Box::new(storage.clone())),
            input,
        )
        .await
        .expect("healthy legacy profile must start normally");
        let snapshot = progress.snapshot();
        assert_eq!(snapshot.state, StartupState::Ready);
        if restart == 1 {
            assert!(!snapshot
                .upgrade
                .as_ref()
                .is_some_and(|upgrade| upgrade.required));
        }
        let OperationResult::HistoryEntries(entries) = engine
            .execute(Operation::ListHistoryEntries(ListHistoryEntriesInput {
                limit: 10,
                offset: 0,
            }))
            .await
            .unwrap()
        else {
            panic!("expected readable history")
        };
        assert!(entries.iter().any(|entry| entry.entry_id == sent.entry_id));
        let OperationResult::DeviceGroupChoices(choices) = engine
            .execute(Operation::QueryDeviceGroupChoices)
            .await
            .unwrap()
        else {
            panic!("expected readable member state")
        };
        assert_eq!(
            choices.device_trust.local_membership,
            DeviceMembershipSummary::Active
        );
        assert_eq!(choices.device_trust.devices.len(), 1);
        engine.shutdown(Duration::from_secs(15)).await.unwrap();
        drop(engine);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_table_compatibility_does_not_bypass_unreadable_admission_metadata() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("recovery test".into()),
            passphrase: SecretString::new("test-passphrase"),
            passphrase_confirmation: SecretString::new("test-passphrase"),
        }))
        .await
        .unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    let (database, control_database) = runtime_databases(root.path());
    omit_later_runtime_tables(&database, &control_database);
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    connection
        .batch_execute(
            "INSERT OR REPLACE INTO admission_repository_state (singleton_id, encrypted_payload) VALUES (1, X'FF')",
        )
        .unwrap();
    drop(connection);

    #[derive(diesel::QueryableByName)]
    struct Payload {
        #[diesel(sql_type = Binary)]
        encrypted_payload: Vec<u8>,
    }
    for _ in 0..3 {
        let (input, progress) = StartupProgress::channel();
        let (engine, _events) = Engine::start_with_progress(
            EngineConfig::new("2.0.0"),
            host(root.path(), Box::new(storage.clone())),
            input,
        )
        .await
        .expect("unverifiable admission must leave a restricted recovery session");
        assert_eq!(progress.snapshot().state, StartupState::RecoveryAvailable);
        assert!(!progress.snapshot().allowed_actions.retry);
        assert!(!progress
            .snapshot()
            .upgrade
            .as_ref()
            .is_some_and(|upgrade| upgrade.required));
        let OperationResult::ProfileRecovery(summary) = engine
            .execute(Operation::QueryProfileRecovery)
            .await
            .unwrap()
        else {
            panic!("expected recovery summary")
        };
        let admission = summary.admission.expect("admission failure must be public");
        assert_eq!(
            admission.category,
            uc_engine::AdmissionRecoveryCategory::LegacyFallbackInvalid
        );
        assert_eq!(
            admission.action,
            uc_engine::AdmissionRecoveryAction::ChooseBackup
        );
        assert!(!summary.background_ready);
        assert!(engine
            .execute(Operation::SendText(SendTextInput {
                text: "must not be saved or synced".into(),
                target_devices: Vec::new(),
            }))
            .await
            .is_err());
        assert!(engine
            .execute(Operation::JoinSpace(JoinSpaceInput {
                invitation_code: "TEST-CODE".into(),
                device_name: None,
                passphrase: SecretString::new("test-passphrase"),
                preserve_unreadable_history: false,
            }))
            .await
            .is_err());
        engine.shutdown(Duration::from_secs(15)).await.unwrap();
        drop(engine);
        let mut connection =
            diesel::sqlite::SqliteConnection::establish(database.to_str().unwrap()).unwrap();
        let payload = diesel::sql_query(
            "SELECT encrypted_payload FROM admission_repository_state WHERE singleton_id = 1",
        )
        .get_result::<Payload>(&mut connection)
        .unwrap();
        assert_eq!(payload.encrypted_payload, [0xff]);
    }
}
