use std::fs;
use std::time::Duration;

use super::{persistent_engine_host, MemoryHostSecureStorage, ENGINE_TEST_LOCK};
use crate::{CreateSpaceInput, Engine, EngineConfig, Operation, OperationResult, SecretString};

#[tokio::test]
async fn startup_upgrade_retains_file_backup_across_factory_reset() {
    let _guard = ENGINE_TEST_LOCK.lock().await;
    let temporary = tempfile::tempdir().unwrap();
    let storage = MemoryHostSecureStorage::default();
    let host = || persistent_engine_host(temporary.path(), storage.clone());
    let (old, _) = Engine::start(EngineConfig::new("1.2.3"), host())
        .await
        .unwrap();
    old.execute(Operation::CreateSpace(CreateSpaceInput {
        device_name: Some("Backup Test Device".into()),
        passphrase: SecretString::new("correct horse"),
        passphrase_confirmation: SecretString::new("correct horse"),
    }))
    .await
    .unwrap();
    old.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(old);

    let (upgraded, _) = Engine::start(EngineConfig::new("1.2.4"), host())
        .await
        .unwrap();
    let backups = fs::read_dir(temporary.path().join("private-upgrade-backups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let record = fs::read(backups.join("current")).unwrap();
    assert!(!record.is_empty());
    assert!(!storage
        .values()
        .keys()
        .any(|key| key.starts_with("profile_backup_archive_key:")));
    upgraded.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(upgraded);

    let (restarted, _) = Engine::start(EngineConfig::new("1.2.4"), host())
        .await
        .unwrap();
    assert_eq!(fs::read(backups.join("current")).unwrap(), record);
    assert_eq!(
        restarted
            .execute(Operation::FactoryResetSpace)
            .await
            .unwrap(),
        OperationResult::SpaceFactoryReset
    );
    assert_eq!(fs::read(backups.join("current")).unwrap(), record);
    assert!(backups.join("security-current").exists());
    restarted.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(restarted);

    let (fresh, _) = Engine::start(EngineConfig::new("1.2.4"), host())
        .await
        .unwrap();
    assert_eq!(fs::read(backups.join("current")).unwrap(), record);
    fresh.shutdown(Duration::from_secs(15)).await.unwrap();
}
