#[cfg(feature = "dev-tools")]
use std::net::{Ipv4Addr, Ipv6Addr, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use uc_engine::{
    ChangeEncryptionPassphraseInput, CreateSpaceInput, Engine, EngineConfig, EngineEvent,
    HistoryEntryInput, Operation, OperationResult, ProfileRecoveryLoss, ProfileRecoveryState,
    SecretString, SendTextInput, StartupProgress, StartupState, UnlockSpaceInput,
};

use super::{startup::host, MemorySecureStorage};

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
