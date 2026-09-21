use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::sql_types::Binary;
use diesel::{Connection, RunQueryDsl};
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, JoinSpaceInput, Operation, OperationResult,
    SecretString, SendTextInput, StartupProgress, StartupState,
};

use super::{host, MemorySecureStorage};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_admission_metadata_starts_restricted_recovery() {
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

    let generations = root.path().join("private/profile-data-generations");
    let generation = std::fs::read_dir(generations)
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| entry.file_type().unwrap().is_dir())
        .unwrap();
    let database = generation.path().join("v3-payloads/profile.sqlite");
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
