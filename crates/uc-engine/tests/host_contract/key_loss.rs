#[cfg(feature = "dev-tools")]
use std::net::{Ipv4Addr, Ipv6Addr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use diesel::connection::SimpleConnection;
use diesel::Connection;
use uc_engine::{
    AdmissionRecoveryCategory, ChangeEncryptionPassphraseInput, CreateSpaceInput, Engine,
    EngineConfig, EngineEvent, HistoryEntryInput, Operation, OperationResult, ProfileRecoveryLoss,
    ProfileRecoveryState, SecretString, SendTextInput, StartupProgress, StartupState,
    UnlockSpaceInput,
};

use uc_engine::error_codes::PROFILE_RECOVERY_REQUIRED_CODE;

use super::{
    startup::{assert_profile_recovery_required, host, runtime_database},
    MemorySecureStorage,
};

const PASSPHRASE: &str = "key-recovery-test-passphrase";

async fn create_profile(root: &std::path::Path, storage: &MemorySecureStorage) -> (String, String) {
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root, Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("key recovery test".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    let original = send_text(&engine, "history before key loss").await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);
    let keys = storage.values().keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        keys.len(),
        1,
        "only the automatic unlock entry stays in keyring"
    );
    assert!(keys[0].starts_with("kek:v1:"));
    assert!(root.join("private/vault/profile-secrets-v1").is_file());
    (original, keys[0].clone())
}

async fn send_text(engine: &Engine, text: &str) -> String {
    let result = engine
        .execute(Operation::SendText(SendTextInput {
            text: text.into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(sent) = result else {
        panic!("expected saved entry")
    };
    sent.entry_id
}

async fn assert_history(engine: &Engine, entry_id: String, expected: &str) {
    let result = engine
        .execute(Operation::GetHistoryEntry(HistoryEntryInput { entry_id }))
        .await
        .unwrap();
    let OperationResult::HistoryEntry(entry) = result else {
        panic!("expected history entry")
    };
    assert_eq!(entry.content, expected);
}

async fn restart_after_loss(all: bool) {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (original, kek_name) = create_profile(root.path(), &storage).await;

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .expect("intact control restart");
    assert_history(&engine, original.clone(), "history before key loss").await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    storage.values().retain(|key, _| !all && key != &kek_name);
    let before_wrong = storage.values().clone();
    let (progress_input, progress) = StartupProgress::channel();
    let (engine, _events) = Engine::start_with_progress(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
        progress_input,
    )
    .await
    .expect("key loss must leave passphrase recovery accessible");
    assert_eq!(progress.snapshot().state, StartupState::RecoveryAvailable);
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::AwaitingPassphrase);
    assert!(!summary.background_ready);

    let wrong = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new("wrong-passphrase"),
        }))
        .await
        .unwrap_err();
    assert_eq!(
        wrong.category(),
        uc_engine::EngineErrorCategory::Unauthorized
    );
    assert_eq!(*storage.values(), before_wrong);

    engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected recovered summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::Recovered);
    assert!(summary.background_ready);
    assert_history(&engine, original.clone(), "history before key loss").await;
    let new_entry = send_text(&engine, "history after key recovery").await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .expect("recovered key material must survive restart");
    assert_history(&engine, original, "history before key loss").await;
    assert_history(&engine, new_entry, "history after key recovery").await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_unlock_key_recovers_history_and_survives_restart() {
    restart_after_loss(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_entire_keyring_recovers_history_and_survives_restart() {
    restart_after_loss(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_unlock_key_recovers_with_original_passphrase() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (entry, kek_name) = create_profile(root.path(), &storage).await;
    storage.values().insert(kek_name, vec![0; 32]);

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .expect("wrong automatic unlock material must leave recovery accessible");
    engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    assert_history(&engine, entry, "history before key loss").await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovered_credentials_enter_admission_recovery_when_repository_is_unreadable() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, kek_name) = create_profile(root.path(), &storage).await;
    let database = runtime_database(
        root.path(),
        "profile-data-generations",
        "v3-payloads/profile.sqlite",
    );
    let mut connection =
        diesel::sqlite::SqliteConnection::establish(database.to_str().unwrap()).unwrap();
    connection
        .batch_execute(
            "INSERT OR REPLACE INTO admission_repository_state (singleton_id, encrypted_payload) \
             VALUES (1, X'FF')",
        )
        .unwrap();
    drop(connection);
    storage.values().remove(&kek_name);

    let (input, progress) = StartupProgress::channel();
    let (engine, _events) = Engine::start_with_progress(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
        input,
    )
    .await
    .expect("missing automatic credential must expose passphrase recovery");
    assert_eq!(progress.snapshot().state, StartupState::RecoveryAvailable);

    let error = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect_err("unreadable admission repository must block normal startup");
    assert_eq!(error.code(), PROFILE_RECOVERY_REQUIRED_CODE);
    assert!(!error.is_retryable());
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected admission recovery summary")
    };
    assert_eq!(
        summary.state,
        ProfileRecoveryState::AdmissionRecoveryRequired
    );
    assert!(!summary.can_submit_passphrase);
    assert!(!summary.restart_required);
    assert!(!summary.background_ready);
    assert_eq!(
        summary.admission.unwrap().category,
        AdmissionRecoveryCategory::LegacyFallbackInvalid
    );
    assert_profile_recovery_required(
        engine
            .execute(Operation::SendText(SendTextInput {
                text: "restricted recovery must not save content".into(),
                target_devices: Vec::new(),
            }))
            .await,
    );
    assert_profile_recovery_required(engine.execute(Operation::QueryDeviceGroupChoices).await);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_space_creation_is_not_reported_as_failed_when_recovery_refresh_fails() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    let interrupted_write = root
        .path()
        .join("private/vault/profile-secrets-v1.preparing");
    std::fs::create_dir_all(&interrupted_write).unwrap();

    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("refresh failure".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();

    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::Failed);
    assert!(!summary.can_submit_passphrase);
    assert!(!summary.restart_required);
    assert!(summary.background_ready);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovered_event_is_not_published_after_the_committed_unlock_refresh_fails() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, kek_name) = create_profile(root.path(), &storage).await;
    storage.values().remove(&kek_name);

    let (engine, mut events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    let current_reads = storage.kek_reads.load(std::sync::atomic::Ordering::SeqCst);
    storage
        .fail_kek_read_at
        .store(current_reads + 6, std::sync::atomic::Ordering::SeqCst);

    let unlock = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await;
    assert!(
        unlock.is_ok(),
        "unlock failed after {} secure storage reads: {unlock:?}",
        storage.kek_reads.load(std::sync::atomic::Ordering::SeqCst)
    );

    let mut states = Vec::new();
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(100), events.next()).await
    {
        if let EngineEvent::ProfileRecoveryChanged(summary) = event {
            states.push(summary.state);
        }
    }
    assert_eq!(states.last(), Some(&ProfileRecoveryState::Failed));
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::Failed);
    assert!(summary.background_ready);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_unlock_write_failure_leaves_recovery_retryable() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, kek_name) = create_profile(root.path(), &storage).await;
    storage.values().remove(&kek_name);

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    storage
        .fail_kek_writes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let error = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code(), 1223);
    assert!(error.is_retryable());

    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected failed recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::Failed);
    assert!(summary.can_submit_passphrase);
    assert!(!summary.restart_required);
    assert!(!summary.background_ready);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_key_removal_is_repaired_before_restart() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, kek_name) = create_profile(root.path(), &storage).await;
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    storage.values().remove(&kek_name);
    engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    assert!(storage.values().contains_key(&kek_name));
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_recovery_requests_are_serialized() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, kek_name) = create_profile(root.path(), &storage).await;
    storage.values().remove(&kek_name);
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    let engine = Arc::new(engine);
    let first = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move {
            engine
                .execute(Operation::UnlockSpace(UnlockSpaceInput {
                    passphrase: SecretString::new(PASSPHRASE),
                }))
                .await
        }
    });
    let second = tokio::spawn({
        let engine = Arc::clone(&engine);
        async move {
            engine
                .execute(Operation::UnlockSpace(UnlockSpaceInput {
                    passphrase: SecretString::new(PASSPHRASE),
                }))
                .await
        }
    });
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn corrupt_recovery_file_is_not_reported_as_missing_key() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, _kek_name) = create_profile(root.path(), &storage).await;
    std::fs::write(
        root.path().join("private/vault/profile-secrets-v1"),
        b"corrupt",
    )
    .unwrap();
    let result = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await;
    assert!(
        result.is_err(),
        "corrupt data must remain a startup failure"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn passphrase_change_rewraps_recovery_without_replacing_history() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (entry, kek_name) = create_profile(root.path(), &storage).await;
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::ChangeEncryptionPassphrase(
            ChangeEncryptionPassphraseInput {
                passphrase: SecretString::new("replacement-passphrase"),
                passphrase_confirmation: SecretString::new("replacement-passphrase"),
            },
        ))
        .await
        .unwrap();
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);
    storage.values().remove(&kek_name);

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    assert!(engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .is_err());
    engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new("replacement-passphrase"),
        }))
        .await
        .unwrap();
    assert_history(&engine, entry, "history before key loss").await;
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_keyring_loss_without_userdata_copy_reports_partial_recovery() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, _kek_name) = create_profile(root.path(), &storage).await;
    std::fs::remove_file(root.path().join("private/vault/profile-secrets-v1")).unwrap();
    storage.values().clear();

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::PartiallyRecoverable);
    assert!(!summary.losses.is_empty());
    assert!(!summary.can_submit_passphrase);
    assert!(engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .is_err());
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected terminal partial recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::PartiallyRecoverable);
    assert!(!summary.can_submit_passphrase);
    assert!(storage.values().is_empty());
    assert!(!root
        .path()
        .join("private/vault/profile-secrets-v1")
        .exists());
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected partial recovery after restart")
    };
    assert_eq!(summary.state, ProfileRecoveryState::PartiallyRecoverable);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn legacy_material_lost_while_waiting_for_passphrase_ends_in_partial_recovery() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, kek_name) = create_profile(root.path(), &storage).await;
    storage.values().remove(&kek_name);

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
    )
    .await
    .unwrap();
    assert!(matches!(
        engine
            .execute(Operation::QueryEncryptionState)
            .await
            .unwrap(),
        OperationResult::EncryptionState(state) if state.initialized && !state.session_ready
    ));
    assert!(engine
        .execute(Operation::SendText(SendTextInput {
            text: "recovery runtime stays restricted".into(),
            target_devices: Vec::new(),
        }))
        .await
        .is_err());
    std::fs::remove_file(root.path().join("private/vault/profile-secrets-v1")).unwrap();

    let error = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code(), 1221);
    assert!(!error.is_retryable());
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected partial recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::PartiallyRecoverable);
    assert!(!summary.can_submit_passphrase);
    assert!(!summary.losses.is_empty());
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_legacy_device_identity_reports_only_identity_loss() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, _kek_name) = create_profile(root.path(), &storage).await;
    std::fs::remove_file(root.path().join("private/vault/profile-secrets-v1")).unwrap();
    let paths = uc_core::app_dirs::AppPaths::with_base_data_local_dir(root.path().join("private"));
    std::fs::remove_dir_all(paths.iroh_identity_dir()).unwrap();
    storage.values().clear();
    let retained = storage
        .removed_values()
        .iter()
        .filter(|(name, _)| name.as_str() != "iroh-identity:v1")
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect::<Vec<_>>();
    storage.values().extend(retained);
    assert!(!storage.values().contains_key("iroh-identity:v1"));

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .unwrap();
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected identity loss recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::PartiallyRecoverable);
    assert_eq!(summary.losses, vec![ProfileRecoveryLoss::DeviceIdentity]);
    assert!(!summary.can_submit_passphrase);
    assert!(engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .is_err());
    assert!(!storage.values().contains_key("iroh-identity:v1"));
    assert!(!root
        .path()
        .join("private/vault/profile-secrets-v1")
        .exists());
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_network_identity_of_a_space_member_is_reported_instead_of_replaced() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, _kek_name) = create_profile(root.path(), &storage).await;
    let paths = uc_core::app_dirs::AppPaths::with_base_data_local_dir(root.path().join("private"));
    std::fs::remove_dir_all(paths.iroh_identity_dir()).unwrap();

    let (progress_input, progress) = StartupProgress::channel();
    let (engine, _events) = Engine::start_with_progress(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
        progress_input,
    )
    .await
    .expect("identity loss must leave recovery accessible");
    assert_eq!(progress.snapshot().state, StartupState::RecoveryAvailable);
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected identity loss recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::PartiallyRecoverable);
    assert_eq!(summary.losses, vec![ProfileRecoveryLoss::DeviceIdentity]);
    assert!(!summary.can_submit_passphrase);
    assert!(
        !paths.iroh_identity_dir().exists(),
        "a space member must not silently receive a new network identity"
    );

    assert!(matches!(
        engine.execute(Operation::FactoryResetSpace).await,
        Ok(OperationResult::SpaceFactoryReset)
    ));
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected post-reset recovery summary")
    };
    assert!(summary.restart_required);
    assert!(summary.losses.is_empty());
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage.clone())),
    )
    .await
    .expect("a reset profile must start as a fresh installation");
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected fresh recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::NotRequired);
    let OperationResult::SetupState(setup) =
        engine.execute(Operation::QuerySetupState).await.unwrap()
    else {
        panic!("expected setup state")
    };
    assert!(!setup.has_completed);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[cfg(feature = "dev-tools")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_background_start_requires_restart_and_never_returns_to_recovering() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_entry, kek_name) = create_profile(root.path(), &storage).await;
    storage.values().remove(&kek_name);

    let ipv6 = UdpSocket::bind((Ipv6Addr::UNSPECIFIED, 0)).unwrap();
    let port = ipv6.local_addr().unwrap().port();
    let ipv4 = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).ok();
    let config = EngineConfig::new("2.0.0").with_test_iroh_bind_port(port);
    let (engine, _events) =
        Engine::start(config.clone(), host(root.path(), Box::new(storage.clone())))
            .await
            .expect("key loss must leave recovery accessible before background startup");

    let failure = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect_err("occupied network port must fail recovered background startup");
    assert_eq!(failure.code(), 1101);
    assert_eq!(
        failure.category(),
        uc_engine::EngineErrorCategory::Unavailable
    );
    assert!(!failure.is_retryable());

    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected failed recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::Failed);
    assert!(!summary.can_submit_passphrase);
    assert!(!summary.background_ready);
    assert!(summary.restart_required);

    assert!(engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .is_err());
    let OperationResult::ProfileRecovery(summary) = engine
        .execute(Operation::QueryProfileRecovery)
        .await
        .unwrap()
    else {
        panic!("expected failed recovery summary")
    };
    assert_eq!(summary.state, ProfileRecoveryState::Failed);
    assert!(!summary.can_submit_passphrase);
    assert!(summary.restart_required);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(engine);

    drop(ipv4);
    drop(ipv6);
    let (engine, _events) = Engine::start(config, host(root.path(), Box::new(storage)))
        .await
        .expect("restart must continue after the dependency becomes available");
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

struct RecoveryPanicClipboard {
    panicked: Arc<std::sync::atomic::AtomicBool>,
    lent: bool,
    keyslot: std::path::PathBuf,
    lease: Option<std::fs::File>,
    panic_worker: bool,
}

impl uc_engine::HostClipboard for RecoveryPanicClipboard {
    fn read(&self) -> Result<uc_engine::HostClipboardSnapshot, uc_engine::HostCapabilityError> {
        uc_engine::HostClipboard::read(&super::EmptyClipboard)
    }

    fn write(
        &self,
        snapshot: uc_engine::HostClipboardSnapshot,
    ) -> Result<(), uc_engine::HostCapabilityError> {
        uc_engine::HostClipboard::write(&super::EmptyClipboard, snapshot)
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn uc_engine::HostClipboardChangeStream>>, uc_engine::HostCapabilityError>
    {
        if self.lent {
            return Ok(None);
        }
        self.lent = true;
        Ok(Some(Box::new(RecoveryPanicStream {
            panicked: Arc::clone(&self.panicked),
            keyslot: self.keyslot.clone(),
            lease: self.lease.take(),
            panic_worker: self.panic_worker,
        })))
    }
}

struct RecoveryPanicStream {
    panicked: Arc<std::sync::atomic::AtomicBool>,
    keyslot: std::path::PathBuf,
    lease: Option<std::fs::File>,
    panic_worker: bool,
}

#[async_trait::async_trait]
impl uc_engine::HostClipboardChangeStream for RecoveryPanicStream {
    async fn next(
        &mut self,
    ) -> Result<uc_engine::HostClipboardChange, uc_engine::HostCapabilityError> {
        std::fs::write(&self.keyslot, b"{}").unwrap();
        self.panicked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if self.panic_worker {
            panic!("controlled recovered runtime worker failure");
        }
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), uc_engine::HostCapabilityError> {
        self.lease.take();
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_unlock_failure_retains_unconfirmed_runtime() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_, key) = create_profile(root.path(), &storage).await;
    storage.values().remove(&key);
    let panicked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lease_path = root.path().join("host-stream.lease");
    let lease = std::fs::File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    let capabilities = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(storage),
        Box::new(RecoveryPanicClipboard {
            panicked: Arc::clone(&panicked),
            lent: false,
            keyslot: root.path().join("private/vault/keyslot.json"),
            lease: Some(lease),
            panic_worker: true,
        }),
        Box::new(super::EmptyFiles),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), capabilities)
        .await
        .unwrap();
    let unlock = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await;
    let worker_failed = panicked.load(std::sync::atomic::Ordering::SeqCst);
    let lease_available = || {
        let status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&lease_path).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    let held_before_shutdown = !lease_available();
    let retry_rejected = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .is_err();
    let outcome = engine.shutdown_with_outcome(Duration::from_secs(15)).await;
    let failed_shutdown = matches!(outcome, uc_engine::ResourceReleaseOutcome::Failed { .. });
    let held_after_shutdown = !lease_available();
    let repeated = engine.shutdown_with_outcome(Duration::from_secs(15)).await;
    let repeated_failed = matches!(repeated, uc_engine::ResourceReleaseOutcome::Failed { .. });
    let held_after_repeated = !lease_available();
    let artifact = serde_json::json!({"scope":"actual recovered Engine, controlled temporary keyslot corruption, clipboard task panic and independent host-stream lock", "unlock_failed":unlock.is_err(), "unlock_error_code":unlock.as_ref().err().map(|error| error.code()), "worker_failed":worker_failed, "host_stream_lock_held_before_shutdown":held_before_shutdown, "retry_rejected":retry_rejected, "shutdown_unconfirmed":failed_shutdown, "host_stream_lock_held_after_shutdown":held_after_shutdown, "repeated_shutdown_unconfirmed":repeated_failed, "host_stream_lock_held_after_repeated_shutdown":held_after_repeated, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("recovered-unlock-unconfirmed.json"),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    assert!(
        unlock.is_err()
            && worker_failed
            && held_before_shutdown
            && retry_rejected
            && failed_shutdown
            && held_after_shutdown
            && repeated_failed
            && held_after_repeated
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovered_unlock_failure_releases_after_successful_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (_, key) = create_profile(root.path(), &storage).await;
    storage.values().remove(&key);
    let entered = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lease_path = root.path().join("host-stream.lease");
    let lease = std::fs::File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    let capabilities = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(storage),
        Box::new(RecoveryPanicClipboard {
            panicked: Arc::clone(&entered),
            lent: false,
            keyslot: root.path().join("private/vault/keyslot.json"),
            lease: Some(lease),
            panic_worker: false,
        }),
        Box::new(super::EmptyFiles),
    );
    let (engine, _) = Engine::start(EngineConfig::new("2.0.0"), capabilities)
        .await
        .unwrap();
    let unlock = engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await;
    let stream_entered = entered.load(std::sync::atomic::Ordering::SeqCst);
    let outcome = engine.shutdown_with_outcome(Duration::from_secs(15)).await;
    let released = matches!(outcome, uc_engine::ResourceReleaseOutcome::Released { .. });
    let status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&lease_path).status().unwrap();
    assert!(matches!(status.code(), Some(0 | 1)));
    let available = status.success();
    let artifact = serde_json::json!({"scope":"actual recovered Engine, temporary keyslot corruption, successful runtime cleanup and independent host-stream lock", "unlock_failed":unlock.is_err(), "stream_entered":stream_entered, "shutdown_released":released, "host_stream_lock_available":available, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("recovered-unlock-released.json"),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    assert!(unlock.is_err() && stream_entered && released && available);
}

#[cfg(target_os = "macos")]
struct RecoveryFifoClipboard {
    keyslot: std::path::PathBuf,
    lease: Option<std::fs::File>,
    entered: Option<std::sync::mpsc::Sender<()>>,
    shutdown_calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(target_os = "macos")]
impl uc_engine::HostClipboard for RecoveryFifoClipboard {
    fn read(&self) -> Result<uc_engine::HostClipboardSnapshot, uc_engine::HostCapabilityError> {
        uc_engine::HostClipboard::read(&super::EmptyClipboard)
    }
    fn write(
        &self,
        snapshot: uc_engine::HostClipboardSnapshot,
    ) -> Result<(), uc_engine::HostCapabilityError> {
        uc_engine::HostClipboard::write(&super::EmptyClipboard, snapshot)
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn uc_engine::HostClipboardChangeStream>>, uc_engine::HostCapabilityError>
    {
        Ok(self.entered.take().map(|entered| {
            Box::new(RecoveryFifoStream {
                keyslot: self.keyslot.clone(),
                lease: self.lease.take(),
                entered: Some(entered),
                shutdown_calls: Arc::clone(&self.shutdown_calls),
            }) as Box<dyn uc_engine::HostClipboardChangeStream>
        }))
    }
}

#[cfg(target_os = "macos")]
struct RecoveryFifoStream {
    keyslot: std::path::PathBuf,
    lease: Option<std::fs::File>,
    entered: Option<std::sync::mpsc::Sender<()>>,
    shutdown_calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(target_os = "macos")]
#[async_trait::async_trait]
impl uc_engine::HostClipboardChangeStream for RecoveryFifoStream {
    async fn next(
        &mut self,
    ) -> Result<uc_engine::HostClipboardChange, uc_engine::HostCapabilityError> {
        if let Some(entered) = self.entered.take() {
            let replacement = self.keyslot.with_extension("fifo");
            assert!(std::process::Command::new("mkfifo")
                .arg(&replacement)
                .status()
                .unwrap()
                .success());
            std::fs::rename(replacement, &self.keyslot).unwrap();
            entered.send(()).unwrap();
        }
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), uc_engine::HostCapabilityError> {
        self.shutdown_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.lease.take();
        Ok(())
    }
}

#[cfg(target_os = "macos")]
#[test]
fn recovery_executor_loss_keeps_accepted_unlock_and_runtime() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;

    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let setup = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (_, key) = setup.block_on(create_profile(root.path(), &storage));
    drop(setup);
    storage.values().remove(&key);
    let fifo = root.path().join("private/vault/keyslot.json");
    let lease_path = root.path().join("host-stream.lease");
    let lease = std::fs::File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    let (entered, ready) = mpsc::channel();
    let host = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(storage),
        Box::new(RecoveryFifoClipboard {
            keyslot: fifo.clone(),
            lease: Some(lease),
            entered: Some(entered),
            shutdown_calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }),
        Box::new(super::EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let engine = Arc::new(engine);
    let waiting = Arc::clone(&engine);
    let (finished, finished_result) = mpsc::channel();
    executor.spawn(async move {
        let result = waiting
            .execute(Operation::UnlockSpace(UnlockSpaceInput {
                passphrase: SecretString::new(PASSPHRASE),
            }))
            .await;
        let _ = finished.send(result.as_ref().err().map(|error| error.code()));
    });
    ready
        .recv_timeout(Duration::from_secs(10))
        .expect("runtime stream started");
    let mut writer = Command::new("python3")
        .args(["-c", "import errno,os,sys,time\nend=time.monotonic()+10\nwhile True:\n try:\n  f=os.open(sys.argv[1], os.O_WRONLY|os.O_NONBLOCK)\n  break\n except OSError as e:\n  if e.errno!=errno.ENXIO or time.monotonic()>=end: raise\n  time.sleep(.01)\nprint('ready',flush=True)\nsys.stdin.read(1)\nos.close(f)"])
        .arg(&fifo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready_line = String::new();
    BufReader::new(writer.stdout.take().unwrap())
        .read_line(&mut ready_line)
        .unwrap();
    assert_eq!(
        ready_line.trim(),
        "ready",
        "unlock result: {:?}",
        finished_result.try_recv()
    );
    executor.shutdown_timeout(Duration::from_millis(100));
    let lock_available = || {
        let status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&lease_path).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    let observer = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let held_before = !lock_available();
    let recovery = observer.block_on(engine.execute(Operation::QueryProfileRecovery));
    let restart_required = matches!(recovery, Ok(OperationResult::ProfileRecovery(summary)) if summary.state == ProfileRecoveryState::Failed && summary.restart_required && !summary.can_submit_passphrase);
    let retry_rejected = observer
        .block_on(engine.execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        })))
        .is_err();
    let zero_wait = observer.block_on(engine.shutdown_with_outcome(Duration::ZERO));
    let zero_not_released = !matches!(
        zero_wait,
        uc_engine::ResourceReleaseOutcome::Released { .. }
    );
    let outcome = observer.block_on(engine.shutdown_with_outcome(Duration::from_secs(2)));
    let not_released = matches!(outcome, uc_engine::ResourceReleaseOutcome::Failed { .. });
    let held_during_read = !lock_available();
    writer.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(writer.wait().unwrap().success());
    let repeated = observer.block_on(engine.shutdown_with_outcome(Duration::from_secs(2)));
    let repeated_not_released =
        matches!(repeated, uc_engine::ResourceReleaseOutcome::Failed { .. });
    let held_after_read = !lock_available();
    let artifact = serde_json::json!({"scope":"actual recovered Engine, executor loss during accepted FIFO read, independent host-stream lock", "fifo_reader_started":true, "host_lock_held_before_shutdown":held_before, "recovery_restart_required":restart_required, "retry_rejected":retry_rejected, "zero_wait_not_released":zero_not_released, "shutdown_failed_while_read_pending":not_released, "host_lock_held_while_read_pending":held_during_read, "repeated_shutdown_failed":repeated_not_released, "host_lock_held_after_read":held_after_read, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("recovery-executor-loss.json"),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    assert!(
        held_before
            && restart_required
            && retry_rejected
            && zero_not_released
            && not_released
            && held_during_read
            && repeated_not_released
            && held_after_read
    );
}

#[cfg(target_os = "macos")]
#[test]
fn process_release_after_abandoned_recovery() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };

    const CHILD: &str = "UC_TEST_ABANDONED_RECOVERY_PROCESS_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "key_loss::process_release_after_abandoned_recovery",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .unwrap(),
    )
    .with_network_logs(
        NetworkLogConfig::new(
            root.path().join("process-logs"),
            NetworkProcessRole::MobileApp,
            NetworkLogBudget {
                record_bytes: 1024,
                queue_records: 32,
                file_bytes: 4096,
                run_bytes: 8192,
                root_bytes: 16384,
                retention: Duration::from_secs(60),
            },
        )
        .unwrap(),
    );
    let process = ProcessObservabilityRuntime::install(config)
        .unwrap()
        .handle();
    let shutdown_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let storage = MemorySecureStorage::default();
    let setup = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (_, key) = setup.block_on(create_profile(root.path(), &storage));
    drop(setup);
    storage.values().remove(&key);
    let fifo = root.path().join("private/vault/keyslot.json");
    let lease_path = root.path().join("host-stream.lease");
    let lease = std::fs::File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    let (entered, ready) = mpsc::channel();
    let host = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(storage),
        Box::new(RecoveryFifoClipboard {
            keyslot: fifo.clone(),
            lease: Some(lease),
            entered: Some(entered),
            shutdown_calls: Arc::clone(&shutdown_calls),
        }),
        Box::new(super::EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let engine = Arc::new(engine);
    let log_lease = process
        .prepare_network_log_export(Duration::from_secs(5))
        .unwrap()
        .run_directory
        .join("lease.lock");
    let survivor_executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (survivor, _) = survivor_executor
        .block_on(Engine::start(
            EngineConfig::new("2.0.0"),
            super::startup::host(
                &root.path().join("survivor"),
                Box::new(MemorySecureStorage::default()),
            ),
        ))
        .unwrap();
    let waiting = Arc::clone(&engine);
    executor.spawn(async move {
        let _ = waiting
            .execute(Operation::UnlockSpace(UnlockSpaceInput {
                passphrase: SecretString::new(PASSPHRASE),
            }))
            .await;
    });
    ready.recv_timeout(Duration::from_secs(10)).unwrap();
    let mut writer = Command::new("python3").args(["-c", "import errno,os,sys,time\nend=time.monotonic()+10\nwhile True:\n try:\n  f=os.open(sys.argv[1], os.O_WRONLY|os.O_NONBLOCK)\n  break\n except OSError as e:\n  if e.errno!=errno.ENXIO or time.monotonic()>=end: raise\n  time.sleep(.01)\nprint('ready',flush=True)\nsys.stdin.read(1)\nos.close(f)"]).arg(&fifo).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(writer.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    drop(engine);
    executor.shutdown_timeout(Duration::from_millis(100));
    let lock_available = |path: &std::path::Path| {
        let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    let host_held_after_drop = !lock_available(&lease_path);
    let survivor_released = matches!(
        survivor_executor.block_on(survivor.shutdown_with_outcome(Duration::from_secs(15))),
        uc_engine::ResourceReleaseOutcome::Released { .. }
    );
    drop(survivor);
    drop(survivor_executor);
    let log_held_before = !lock_available(&log_lease);
    let result = process.test_process_release(Duration::from_secs(3));
    let log_held_after = !lock_available(&log_lease);
    let host_held_after_release = !lock_available(&lease_path);
    let repeated = process.test_process_release(Duration::ZERO);
    let terminal_retry = process.test_retry_process_release(Duration::from_secs(3));
    let terminal_generation = process.test_process_release_generation();
    let repeated_terminal_retry = process.test_retry_process_release(Duration::ZERO);
    let terminal_generation_stable =
        process.test_process_release_generation() == terminal_generation;
    let no_terminal_close = shutdown_calls.load(std::sync::atomic::Ordering::SeqCst) == 0;
    let _terminal = process.shutdown(Duration::from_secs(3));
    let after_terminal = process.test_process_release(Duration::ZERO);
    let host_held_after_terminal = !lock_available(&lease_path);
    writer.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(writer.wait().unwrap().success());
    let artifact = serde_json::json!({"scope":"two actual Engines, process owner, detached FIFO read, independent host and process log locks", "fifo_reader_started":true, "engine_and_observer_dropped":true, "host_lock_held_after_drop":host_held_after_drop, "other_engine_released":survivor_released, "process_log_lock_held_before":log_held_before, "process_release":result, "process_log_lock_held_after":log_held_after, "host_lock_held_after_release":host_held_after_release, "repeated_release":repeated, "terminal_retry_failed":terminal_retry.0 == "failed", "repeated_terminal_retry_failed":repeated_terminal_retry.0 == "failed", "terminal_generation_stable":terminal_generation_stable, "terminal_closer_not_invoked":no_terminal_close, "release_after_terminal_shutdown":after_terminal, "host_lock_held_after_terminal":host_held_after_terminal, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("abandoned-recovery-process.json"),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    assert!(
        host_held_after_drop
            && survivor_released
            && log_held_before
            && result != "released"
            && log_held_after
            && host_held_after_release
            && repeated != "released"
            && terminal_retry.0 == "failed"
            && repeated_terminal_retry.0 == "failed"
            && terminal_generation_stable
            && no_terminal_close
            && after_terminal != "released"
            && host_held_after_terminal
    );
}

#[cfg(unix)]
struct ConstructionBlockedClipboard {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    returned: std::sync::mpsc::Sender<()>,
    lease: Option<std::fs::File>,
    close_calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(unix)]
impl uc_engine::HostClipboard for ConstructionBlockedClipboard {
    fn read(&self) -> Result<uc_engine::HostClipboardSnapshot, uc_engine::HostCapabilityError> {
        uc_engine::HostClipboard::read(&super::EmptyClipboard)
    }

    fn write(
        &self,
        snapshot: uc_engine::HostClipboardSnapshot,
    ) -> Result<(), uc_engine::HostCapabilityError> {
        uc_engine::HostClipboard::write(&super::EmptyClipboard, snapshot)
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn uc_engine::HostClipboardChangeStream>>, uc_engine::HostCapabilityError>
    {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        self.returned.send(()).unwrap();
        Ok(self.lease.take().map(|lease| {
            Box::new(ConstructionBlockedStream(
                Some(lease),
                Arc::clone(&self.close_calls),
            )) as Box<dyn uc_engine::HostClipboardChangeStream>
        }))
    }
}

#[cfg(unix)]
struct ConstructionBlockedStream(Option<std::fs::File>, Arc<std::sync::atomic::AtomicUsize>);

#[cfg(unix)]
#[async_trait::async_trait]
impl uc_engine::HostClipboardChangeStream for ConstructionBlockedStream {
    async fn next(
        &mut self,
    ) -> Result<uc_engine::HostClipboardChange, uc_engine::HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), uc_engine::HostCapabilityError> {
        self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.0.take();
        Ok(())
    }
}

#[cfg(unix)]
#[test]
fn recovery_executor_loss_during_runtime_construction() {
    recovery_construction_shutdown_race(true);
}

#[cfg(unix)]
#[test]
fn recovery_construction_completes_during_shutdown() {
    recovery_construction_shutdown_race(false);
}

#[cfg(unix)]
fn recovery_construction_shutdown_race(stop_executor: bool) {
    use std::sync::mpsc;
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let setup = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (_, key) = setup.block_on(create_profile(root.path(), &storage));
    drop(setup);
    storage.values().remove(&key);
    let path = root.path().join("construction-host.lease");
    let lease = std::fs::File::create(&path).unwrap();
    lease.lock().unwrap();
    let close_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (entered, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let (returned, completed) = mpsc::channel();
    let host = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(storage),
        Box::new(ConstructionBlockedClipboard {
            entered,
            release: std::sync::Mutex::new(blocked),
            returned,
            lease: Some(lease),
            close_calls: Arc::clone(&close_calls),
        }),
        Box::new(super::EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let engine = Arc::new(engine);
    let operation_engine = Arc::clone(&engine);
    let (operation_done, operation_result) = mpsc::channel();
    executor.spawn(async move {
        let result = operation_engine
            .execute(Operation::UnlockSpace(UnlockSpaceInput {
                passphrase: SecretString::new(PASSPHRASE),
            }))
            .await;
        let _ = operation_done.send(result.as_ref().err().map(|error| error.code()));
    });
    ready
        .recv_timeout(Duration::from_secs(20))
        .expect("recovery construction reached host callback");
    let mut executor = Some(executor);
    if stop_executor {
        executor
            .take()
            .unwrap()
            .shutdown_timeout(Duration::from_millis(100));
    }
    let observer = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let lock_available = || {
        let status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&path).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    let held = !lock_available();
    let while_blocked = observer.block_on(engine.shutdown_with_outcome(Duration::from_millis(200)));
    let blocked_pending = matches!(while_blocked, uc_engine::ResourceReleaseOutcome::Pending);
    let held_during_wait = !lock_available();
    release.send(()).unwrap();
    completed.recv_timeout(Duration::from_secs(10)).unwrap();
    let operation_completed = operation_result.try_recv().is_ok();
    let shutdown = observer.block_on(engine.shutdown_with_outcome(Duration::from_secs(3)));
    let failed = matches!(shutdown, uc_engine::ResourceReleaseOutcome::Failed { .. });
    let released = matches!(shutdown, uc_engine::ResourceReleaseOutcome::Released { .. });
    let held_after = !lock_available();
    let calls = close_calls.load(std::sync::atomic::Ordering::SeqCst);
    let valid_settlement =
        (failed && held_after && calls == 0) || (released && !held_after && calls == 1);
    let artifact = serde_json::json!({"scope":"real recovered Engine, host callback held before runtime construction returns, independent host lock", "executor_stopped":stop_executor, "callback_entered":true, "callback_returned":true, "shutdown_pending_while_callback_blocked":blocked_pending, "operation_completed":operation_completed, "host_lock_held":held, "host_lock_held_during_wait":held_during_wait, "shutdown_while_blocked":format!("{while_blocked:?}"), "shutdown_result":format!("{shutdown:?}"), "host_lock_held_after_shutdown":held_after, "close_calls":calls, "valid_settlement":valid_settlement, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join(if stop_executor {
                "recovery-construction-executor-loss.json"
            } else {
                "recovery-construction-shutdown-race.json"
            }),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    drop(executor);
    assert!(held && held_during_wait && blocked_pending && valid_settlement);
}

#[cfg(unix)]
struct RecoveryBackupGateStorage {
    storage: MemorySecureStorage,
    backup_root: std::path::PathBuf,
    end: BackupGateEnd,
    gate: std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}

#[cfg(unix)]
impl uc_engine::HostSecureStorage for RecoveryBackupGateStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, uc_engine::HostCapabilityError> {
        if key.starts_with("kek:v1:") && self.backup_root.exists() {
            if let Some((entered, release)) = self.gate.lock().unwrap().take() {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(30)).unwrap();
                match self.end {
                    BackupGateEnd::Success => {}
                    BackupGateEnd::Error => {
                        return Err(uc_engine::HostCapabilityError::new(
                            uc_engine::HostCapabilityErrorCategory::Unavailable,
                            "controlled backup failure",
                        ))
                    }
                    BackupGateEnd::Panic => panic!("controlled backup worker panic"),
                }
            }
        }
        uc_engine::HostSecureStorage::get(&self.storage, key)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), uc_engine::HostCapabilityError> {
        uc_engine::HostSecureStorage::set(&self.storage, key, value)
    }

    fn delete(&self, key: &str) -> Result<(), uc_engine::HostCapabilityError> {
        uc_engine::HostSecureStorage::delete(&self.storage, key)
    }
}

#[cfg(unix)]
struct RecoveryLeaseClipboard {
    lease: std::sync::Mutex<Option<std::fs::File>>,
    close_calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(unix)]
impl uc_engine::HostClipboard for RecoveryLeaseClipboard {
    fn read(&self) -> Result<uc_engine::HostClipboardSnapshot, uc_engine::HostCapabilityError> {
        uc_engine::HostClipboard::read(&super::EmptyClipboard)
    }

    fn write(
        &self,
        snapshot: uc_engine::HostClipboardSnapshot,
    ) -> Result<(), uc_engine::HostCapabilityError> {
        uc_engine::HostClipboard::write(&super::EmptyClipboard, snapshot)
    }

    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn uc_engine::HostClipboardChangeStream>>, uc_engine::HostCapabilityError>
    {
        Ok(self.lease.lock().unwrap().take().map(|lease| {
            Box::new(ConstructionBlockedStream(
                Some(lease),
                Arc::clone(&self.close_calls),
            )) as Box<dyn uc_engine::HostClipboardChangeStream>
        }))
    }
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum BackupGateEnd {
    Success,
    Error,
    Panic,
}

#[cfg(unix)]
#[test]
fn recovery_shutdown_waits_for_detached_backup() {
    recovery_shutdown_during_backup(BackupGateEnd::Success, false, false);
}

#[cfg(unix)]
#[test]
fn recovery_shutdown_waits_for_failed_detached_backup() {
    recovery_shutdown_during_backup(BackupGateEnd::Error, false, false);
}

#[cfg(unix)]
#[test]
fn recovery_shutdown_waits_for_panicked_detached_backup() {
    recovery_shutdown_during_backup(BackupGateEnd::Panic, false, false);
}

#[cfg(unix)]
#[test]
fn recovery_backup_survives_shutdown_executor_loss() {
    recovery_shutdown_during_backup(BackupGateEnd::Success, true, false);
}

#[cfg(unix)]
#[test]
fn factory_reset_shutdown_waits_for_detached_backup() {
    recovery_shutdown_during_backup(BackupGateEnd::Success, false, true);
}

#[cfg(unix)]
#[test]
fn factory_reset_shutdown_waits_for_failed_backup() {
    recovery_shutdown_during_backup(BackupGateEnd::Error, false, true);
}

#[cfg(unix)]
#[test]
fn factory_reset_shutdown_waits_for_panicked_backup() {
    recovery_shutdown_during_backup(BackupGateEnd::Panic, false, true);
}

#[cfg(unix)]
#[test]
fn factory_reset_backup_survives_shutdown_executor_loss() {
    recovery_shutdown_during_backup(BackupGateEnd::Success, true, true);
}

#[cfg(unix)]
fn recovery_shutdown_during_backup(
    end: BackupGateEnd,
    lose_shutdown_executor: bool,
    factory_reset: bool,
) {
    use std::sync::mpsc;
    const CHILD: &str = "UC_TEST_RECOVERY_BACKUP_CHILD";
    let (ending, filename, scenario) = if factory_reset && lose_shutdown_executor {
        (
            "factory_reset_shutdown_executor_lost",
            "factory-reset-backup-executor-loss.json",
            "key_loss::factory_reset_backup_survives_shutdown_executor_loss",
        )
    } else if factory_reset {
        match end {
            BackupGateEnd::Success => (
                "factory_reset",
                "factory-reset-detached-backup.json",
                "key_loss::factory_reset_shutdown_waits_for_detached_backup",
            ),
            BackupGateEnd::Error => (
                "factory_reset_error",
                "factory-reset-detached-backup-error.json",
                "key_loss::factory_reset_shutdown_waits_for_failed_backup",
            ),
            BackupGateEnd::Panic => (
                "factory_reset_panic",
                "factory-reset-detached-backup-panic.json",
                "key_loss::factory_reset_shutdown_waits_for_panicked_backup",
            ),
        }
    } else if lose_shutdown_executor {
        (
            "shutdown_executor_lost",
            "recovery-backup-shutdown-executor-loss.json",
            "key_loss::recovery_backup_survives_shutdown_executor_loss",
        )
    } else {
        match end {
            BackupGateEnd::Success => (
                "success",
                "recovery-detached-backup.json",
                "key_loss::recovery_shutdown_waits_for_detached_backup",
            ),
            BackupGateEnd::Error => (
                "error",
                "recovery-detached-backup-error.json",
                "key_loss::recovery_shutdown_waits_for_failed_detached_backup",
            ),
            BackupGateEnd::Panic => (
                "panic",
                "recovery-detached-backup-panic.json",
                "key_loss::recovery_shutdown_waits_for_panicked_detached_backup",
            ),
        }
    };
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", scenario, "--nocapture"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let setup = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (_, key) = setup.block_on(create_profile(root.path(), &storage));
    drop(setup);
    if factory_reset {
        let paths =
            uc_core::app_dirs::AppPaths::with_base_data_local_dir(root.path().join("private"));
        std::fs::remove_dir_all(paths.iroh_identity_dir()).unwrap();
    } else {
        storage.values().remove(&key);
    }
    std::fs::write(
        root.path().join("private/upgrade-cursor.json"),
        br#"{"schema_version":1,"last_seen_version":"1.0.0"}"#,
    )
    .unwrap();
    let backup_root = root.path().join("private-upgrade-backups");
    let (entered, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let host_path = root.path().join("host-stream.lease");
    let host_lease = std::fs::File::create(&host_path).unwrap();
    host_lease.lock().unwrap();
    let close_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(RecoveryBackupGateStorage {
            storage,
            backup_root: backup_root.clone(),
            end,
            gate: std::sync::Mutex::new(Some((entered, blocked))),
        }),
        Box::new(RecoveryLeaseClipboard {
            lease: std::sync::Mutex::new(Some(host_lease)),
            close_calls: Arc::clone(&close_calls),
        }),
        Box::new(super::EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let engine = Arc::new(engine);
    let operation_engine = Arc::clone(&engine);
    executor.spawn(async move {
        let operation = if factory_reset {
            Operation::FactoryResetSpace
        } else {
            Operation::UnlockSpace(UnlockSpaceInput {
                passphrase: SecretString::new(PASSPHRASE),
            })
        };
        let _ = operation_engine.execute(operation).await;
    });
    ready
        .recv_timeout(Duration::from_secs(20))
        .expect("recovery backup worker reached storage gate");
    let lease = std::fs::read_dir(&backup_root)
        .unwrap()
        .map(|entry| entry.unwrap().path().join(".lease"))
        .find(|path| path.exists())
        .unwrap();
    let lock_available = || {
        let status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&lease).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    let held_before = !lock_available();
    executor.shutdown_timeout(Duration::from_millis(100));
    let observer = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let first = observer.block_on(engine.shutdown_with_outcome(Duration::from_millis(200)));
    let (observer, second) = if lose_shutdown_executor {
        observer.shutdown_timeout(Duration::from_millis(100));
        let replacement = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let result = replacement.block_on(engine.shutdown_with_outcome(Duration::from_millis(200)));
        (replacement, result)
    } else {
        let result = observer.block_on(engine.shutdown_with_outcome(Duration::ZERO));
        (observer, result)
    };
    let held_during = !lock_available();
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !lock_available() {
        assert!(
            std::time::Instant::now() < deadline,
            "backup worker must settle"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let final_result = observer.block_on(engine.shutdown_with_outcome(Duration::from_secs(5)));
    let repeated = observer.block_on(engine.shutdown_with_outcome(Duration::from_secs(5)));
    let first_pending = matches!(first, uc_engine::ResourceReleaseOutcome::Pending);
    let second_pending = matches!(second, uc_engine::ResourceReleaseOutcome::Pending);
    let final_failed = matches!(
        final_result,
        uc_engine::ResourceReleaseOutcome::Failed { .. }
    );
    let repeated_failed = matches!(repeated, uc_engine::ResourceReleaseOutcome::Failed { .. });
    drop(engine);
    let host_status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&host_path).status().unwrap();
    assert!(matches!(host_status.code(), Some(0 | 1)));
    let host_retained = !host_status.success();
    let close_count = close_calls.load(std::sync::atomic::Ordering::SeqCst);
    let artifact = serde_json::json!({"scope":"actual recovery construction, production profile backup and independent backup lease", "worker_end":ending, "factory_reset":factory_reset, "shutdown_executor_lost":lose_shutdown_executor, "host_lock_retained_after_engine_drop":host_retained, "host_close_calls":close_count, "backup_callback_entered":true, "lease_held_before_executor_stop":held_before, "lease_held_while_shutdown_waits":held_during, "first_shutdown":format!("{first:?}"), "second_shutdown":format!("{second:?}"), "first_pending":first_pending, "second_pending":second_pending, "lease_available_after_worker":lock_available(), "final_shutdown":format!("{final_result:?}"), "repeated_shutdown":format!("{repeated:?}"), "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join(filename),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    assert!(
        held_before
            && held_during
            && first_pending
            && second_pending
            && final_failed
            && repeated_failed
            && host_retained
            && close_count == 0
    );
}

#[cfg(unix)]
#[test]
fn process_waits_for_backup_after_shutdown_executor_loss() {
    process_waits_for_recovery_backup(false);
}

#[cfg(unix)]
#[test]
fn process_waits_for_factory_reset_backup_after_shutdown_executor_loss() {
    process_waits_for_recovery_backup(true);
}

#[cfg(unix)]
fn process_waits_for_recovery_backup(factory_reset: bool) {
    use std::sync::mpsc;
    use uc_observability_runtime::{
        DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
        ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_PROCESS_RECOVERY_BACKUP_CHILD";
    let (scenario, filename) = if factory_reset {
        (
            "key_loss::process_waits_for_factory_reset_backup_after_shutdown_executor_loss",
            "process-factory-reset-backup-shutdown-executor-loss.json",
        )
    } else {
        (
            "key_loss::process_waits_for_backup_after_shutdown_executor_loss",
            "process-recovery-backup-shutdown-executor-loss.json",
        )
    };
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", scenario, "--nocapture"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let setup = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (_, key) = setup.block_on(create_profile(root.path(), &storage));
    drop(setup);
    if factory_reset {
        let paths =
            uc_core::app_dirs::AppPaths::with_base_data_local_dir(root.path().join("private"));
        std::fs::remove_dir_all(paths.iroh_identity_dir()).unwrap();
    } else {
        storage.values().remove(&key);
    }
    std::fs::write(
        root.path().join("private/upgrade-cursor.json"),
        br#"{"schema_version":1,"last_seen_version":"1.0.0"}"#,
    )
    .unwrap();
    let process = ProcessObservabilityRuntime::install(ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .unwrap(),
    ))
    .unwrap()
    .handle();
    let backup_root = root.path().join("private-upgrade-backups");
    let host_path = root.path().join("host-stream.lease");
    let host_lease = std::fs::File::create(&host_path).unwrap();
    host_lease.lock().unwrap();
    let close_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (entered, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let host = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(RecoveryBackupGateStorage {
            storage,
            backup_root: backup_root.clone(),
            end: BackupGateEnd::Success,
            gate: std::sync::Mutex::new(Some((entered, blocked))),
        }),
        Box::new(RecoveryLeaseClipboard {
            lease: std::sync::Mutex::new(Some(host_lease)),
            close_calls: Arc::clone(&close_calls),
        }),
        Box::new(super::EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let engine = Arc::new(engine);
    let operation_engine = Arc::clone(&engine);
    executor.spawn(async move {
        let operation = if factory_reset {
            Operation::FactoryResetSpace
        } else {
            Operation::UnlockSpace(UnlockSpaceInput {
                passphrase: SecretString::new(PASSPHRASE),
            })
        };
        let _ = operation_engine.execute(operation).await;
    });
    ready
        .recv_timeout(Duration::from_secs(20))
        .expect("backup reached storage gate");
    let lease = std::fs::read_dir(&backup_root)
        .unwrap()
        .map(|entry| entry.unwrap().path().join(".lease"))
        .find(|path| path.exists())
        .unwrap();
    let available = |path: &std::path::Path| {
        let status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    executor.shutdown_timeout(Duration::from_millis(100));
    let shutdown_executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let engine_wait =
        shutdown_executor.block_on(engine.shutdown_with_outcome(Duration::from_millis(200)));
    shutdown_executor.shutdown_timeout(Duration::from_millis(100));
    drop(engine);
    let host_held = !available(&host_path);
    let backup_held = !available(&lease);
    let process_wait = process.test_process_release(Duration::from_millis(300));
    let process_retry = process.test_process_release(Duration::ZERO);
    release.send(()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !available(&lease) {
        assert!(std::time::Instant::now() < deadline, "backup must settle");
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = process.test_process_release(Duration::from_secs(5));
    let repeated = process.test_process_release(Duration::ZERO);
    let host_after = !available(&host_path);
    let count = close_calls.load(std::sync::atomic::Ordering::SeqCst);
    let artifact = serde_json::json!({"scope":"actual Engine, process owner, detached profile backup and independent locks", "factory_reset":factory_reset, "engine_wait":format!("{engine_wait:?}"), "backup_held":backup_held, "host_held":host_held, "process_wait":process_wait, "process_repeated_wait":process_retry, "backup_available_after_worker":available(&lease), "process_after_backup":result, "process_repeated_after_backup":repeated, "host_held_after_backup":host_after, "host_close_calls":count, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join(filename),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    assert!(
        matches!(engine_wait, uc_engine::ResourceReleaseOutcome::Pending)
            && backup_held
            && host_held
            && process_wait == "pending"
            && process_retry == "pending"
            && result == "failed"
            && repeated == "failed"
            && host_after
            && count == 0
    );
}

#[cfg(unix)]
#[test]
fn successful_recovery_handoff_joins_shutdown() {
    use std::sync::mpsc;
    const CHILD: &str = "UC_TEST_RECOVERY_HANDOFF_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "key_loss::successful_recovery_handoff_joins_shutdown",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let setup = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (_, key) = setup.block_on(create_profile(root.path(), &storage));
    drop(setup);
    storage.values().remove(&key);
    let path = root.path().join("handoff-host.lease");
    let lease = std::fs::File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (entered, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let (returned, callback_returned) = mpsc::channel();
    let host = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(storage),
        Box::new(ConstructionBlockedClipboard {
            entered,
            release: std::sync::Mutex::new(blocked),
            returned,
            lease: Some(lease),
            close_calls: Arc::clone(&calls),
        }),
        Box::new(super::EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let engine = Arc::new(engine);
    let operation_engine = Arc::clone(&engine);
    let (operation_sent, operation_wait) = mpsc::channel();
    executor.spawn(async move {
        let result = operation_engine
            .execute(Operation::UnlockSpace(UnlockSpaceInput {
                passphrase: SecretString::new(PASSPHRASE),
            }))
            .await;
        let _ = operation_sent.send(result.is_ok());
    });
    ready
        .recv_timeout(Duration::from_secs(20))
        .expect("constructor reached host callback");
    let available = || {
        let status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&path).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    let held_before = !available();
    let shutdown_engine = Arc::clone(&engine);
    let (shutdown_started, shutdown_ready) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let observer = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        shutdown_started.send(()).unwrap();
        observer.block_on(shutdown_engine.shutdown_with_outcome(Duration::from_secs(20)))
    });
    shutdown_ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while executor.block_on(engine.lifecycle_state()) != uc_engine::EngineState::ShuttingDown {
        assert!(
            std::time::Instant::now() < deadline,
            "first shutdown must start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let waiting = executor.block_on(engine.shutdown_with_outcome(Duration::from_millis(200)));
    let pending = matches!(waiting, uc_engine::ResourceReleaseOutcome::Pending);
    let held_during = !available();
    release.send(()).unwrap();
    callback_returned
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    let operation_success = operation_wait
        .recv_timeout(Duration::from_secs(20))
        .unwrap_or(false);
    let result = thread.join().unwrap();
    let released = matches!(result, uc_engine::ResourceReleaseOutcome::Released { .. });
    let lock_free = available();
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let artifact = serde_json::json!({"scope":"actual recovered Engine, successful construction concurrent with Engine shutdown, independent host lock", "host_callback_entered":true, "host_callback_returned":true, "held_before":held_before, "held_during_shutdown":held_during, "shutdown_wait":format!("{waiting:?}"), "recovery_succeeded":operation_success, "shutdown_result":format!("{result:?}"), "host_lock_available":lock_free, "host_close_calls":count, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join("recovery-handoff-shutdown.json"),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    assert!(
        held_before
            && held_during
            && pending
            && operation_success
            && released
            && lock_free
            && count == 1
    );
}

#[cfg(unix)]
struct RebuildGateStorage {
    storage: MemorySecureStorage,
    keyslot: std::path::PathBuf,
    armed: Arc<std::sync::atomic::AtomicBool>,
    reads: std::sync::atomic::AtomicUsize,
    gate: std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}

#[cfg(unix)]
impl uc_engine::HostSecureStorage for RebuildGateStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, uc_engine::HostCapabilityError> {
        if self.armed.load(std::sync::atomic::Ordering::SeqCst)
            && key == "profile_lifecycle_marker:v1"
            && !self.keyslot.exists()
            && self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 3
        {
            if let Some((entered, release)) = self.gate.lock().unwrap().take() {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(30)).unwrap();
            }
        }
        uc_engine::HostSecureStorage::get(&self.storage, key)
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), uc_engine::HostCapabilityError> {
        uc_engine::HostSecureStorage::set(&self.storage, key, value)
    }
    fn delete(&self, key: &str) -> Result<(), uc_engine::HostCapabilityError> {
        uc_engine::HostSecureStorage::delete(&self.storage, key)
    }
}

#[cfg(unix)]
#[test]
fn ready_reset_reconstruction_survives_executor_loss() {
    ready_reset_reconstruction_loss(false);
}

#[cfg(unix)]
#[test]
fn process_waits_for_ready_reset_reconstruction() {
    ready_reset_reconstruction_loss(true);
}

#[cfg(unix)]
fn ready_reset_reconstruction_loss(process_observer: bool) {
    use std::sync::mpsc;
    use uc_observability_runtime::{
        DeploymentEnvironment, ObservabilityConfig, ObservabilityResource, OperatingSystem,
        ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_READY_RESET_REBUILD_CHILD";
    let (scenario, filename) = if process_observer {
        (
            "key_loss::process_waits_for_ready_reset_reconstruction",
            "process-ready-reset-rebuild.json",
        )
    } else {
        (
            "key_loss::ready_reset_reconstruction_survives_executor_loss",
            "ready-reset-rebuild-executor-loss.json",
        )
    };
    if std::env::var_os(CHILD).is_none() {
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", scenario, "--nocapture"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let process = process_observer.then(|| {
        ProcessObservabilityRuntime::install(ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.0.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Other,
                "test",
            )
            .unwrap(),
        ))
        .unwrap()
        .handle()
    });
    let storage = MemorySecureStorage::default();
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (entered, ready) = mpsc::channel();
    let (release, blocked) = mpsc::channel();
    let host_path = root.path().join("host-stream.lease");
    let lease = std::fs::File::create(&host_path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host = uc_engine::HostCapabilities::new(
        uc_engine::HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(RebuildGateStorage {
            storage,
            keyslot: root.path().join("private/vault/keyslot.json"),
            armed: Arc::clone(&armed),
            reads: std::sync::atomic::AtomicUsize::new(0),
            gate: std::sync::Mutex::new(Some((entered, blocked))),
        }),
        Box::new(RecoveryLeaseClipboard {
            lease: std::sync::Mutex::new(Some(lease)),
            close_calls: Arc::clone(&calls),
        }),
        Box::new(super::EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    executor
        .block_on(engine.execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("rebuild fixture".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        })))
        .unwrap();
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let engine = Arc::new(engine);
    let operation_engine = Arc::clone(&engine);
    let (done, finished) = mpsc::channel();
    executor.spawn(async move {
        let result = operation_engine.execute(Operation::FactoryResetSpace).await;
        let _ = done.send(result.is_ok());
    });
    ready
        .recv_timeout(Duration::from_secs(20))
        .expect("fresh runtime reached lifecycle read");
    executor.shutdown_timeout(Duration::from_millis(100));
    let observer = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let first = observer.block_on(engine.shutdown_with_outcome(Duration::from_millis(200)));
    let available = || {
        let status = std::process::Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&host_path).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    let held_during = !available();
    let (result, repeated, process_pending) = if let Some(process) = process {
        observer.shutdown_timeout(Duration::from_millis(100));
        drop(engine);
        let pending = process.test_process_release(Duration::from_millis(300));
        let duplicate = process.test_process_release(Duration::ZERO);
        release.send(()).unwrap();
        let result = process.test_process_release(Duration::from_secs(10));
        let repeated = process.test_process_release(Duration::ZERO);
        (
            result,
            repeated,
            pending == "pending" && duplicate == "pending",
        )
    } else {
        release.send(()).unwrap();
        let result = observer.block_on(engine.shutdown_with_outcome(Duration::from_secs(10)));
        let repeated = observer.block_on(engine.shutdown_with_outcome(Duration::from_secs(5)));
        drop(engine);
        (
            if matches!(result, uc_engine::ResourceReleaseOutcome::Failed { .. }) {
                "failed"
            } else {
                "other"
            },
            if matches!(repeated, uc_engine::ResourceReleaseOutcome::Failed { .. }) {
                "failed"
            } else {
                "other"
            },
            true,
        )
    };
    let operation_completed = finished.try_recv().is_ok();
    let held_after = !available();
    let count = calls.load(std::sync::atomic::Ordering::SeqCst);
    let artifact = serde_json::json!({"scope":"actual ready Engine, factory reset reconstruction, controlled host read and independent host lock", "reconstruction_callback_entered":true, "process_observer":process_observer, "process_pending":process_pending, "first_shutdown":format!("{first:?}"), "host_lock_held_during_callback":held_during, "reconstruction_observer_completed":operation_completed, "final_shutdown":format!("{result:?}"), "repeated_shutdown":format!("{repeated:?}"), "host_lock_retained_after_engine_drop":held_after, "host_close_calls":count, "device_acceptance":"not established"});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            std::path::Path::new(&dir).join(filename),
            serde_json::to_vec_pretty(&artifact).unwrap(),
        )
        .unwrap();
    }
    assert!(
        matches!(first, uc_engine::ResourceReleaseOutcome::Pending)
            && held_during
            && process_pending
            && result == "failed"
            && repeated == "failed"
            && held_after
            && count == 0
    );
}
