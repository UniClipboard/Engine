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
    // 重置作废保护材料的密文副本；文件副本继续保留。
    assert!(!backups.join("security-current").exists());
    assert!(security_record_files(&backups).is_empty());
    restarted.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(restarted);

    let (fresh, _) = Engine::start(EngineConfig::new("1.2.4"), host())
        .await
        .unwrap();
    assert_eq!(fs::read(backups.join("current")).unwrap(), record);
    fresh.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(fresh);

    // 出厂重置后的第一次版本提升：不能被已作废的安全记录挡住。
    let (bumped, _) = Engine::start(EngineConfig::new("1.2.5"), host())
        .await
        .unwrap();
    assert!(backups.join("security-current").exists());
    bumped.shutdown(Duration::from_secs(15)).await.unwrap();
}

fn security_record_files(backups: &std::path::Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(backups)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("record"))
        .collect()
}

/// 历史上卡死的形态：重置清掉了保护材料，备份目录里却还留着旧的安全记录。
#[tokio::test]
async fn startup_upgrade_replaces_stale_security_records_left_by_an_earlier_reset() {
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
    let stale_pointer = fs::read(backups.join("security-current")).unwrap();
    let stale_records = security_record_files(&backups)
        .into_iter()
        .map(|path| (path.clone(), fs::read(path).unwrap()))
        .collect::<Vec<_>>();
    assert!(!stale_records.is_empty());
    assert_eq!(
        upgraded
            .execute(Operation::FactoryResetSpace)
            .await
            .unwrap(),
        OperationResult::SpaceFactoryReset
    );
    upgraded.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(upgraded);
    let (fresh, _) = Engine::start(EngineConfig::new("1.2.4"), host())
        .await
        .unwrap();
    fresh.shutdown(Duration::from_secs(15)).await.unwrap();
    drop(fresh);
    let current = fs::read(backups.join("current")).unwrap();
    // 还原旧版本重置留下的状态：记录仍在，保护它的密钥已随重置消失。
    fs::write(backups.join("security-current"), &stale_pointer).unwrap();
    for (path, bytes) in &stale_records {
        fs::write(path, bytes).unwrap();
    }

    let (bumped, _) = Engine::start(EngineConfig::new("1.2.5"), host())
        .await
        .unwrap();

    assert_ne!(
        fs::read(backups.join("security-current")).unwrap(),
        stale_pointer
    );
    assert!(stale_records.iter().all(|(path, _)| !path.exists()));
    assert_ne!(fs::read(backups.join("current")).unwrap(), current);
    bumped.shutdown(Duration::from_secs(15)).await.unwrap();
}
