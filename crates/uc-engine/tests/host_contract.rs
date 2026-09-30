use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use uc_engine::{
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostClipboard,
    HostClipboardRepresentation, HostClipboardSnapshot, HostDirectories, HostFileAccess,
    HostFileHandle, HostFileMetadata, HostSecureStorage,
};

#[path = "host_contract/key_loss.rs"]
mod key_loss;

#[path = "host_contract/startup.rs"]
mod startup;

#[path = "host_contract/lease.rs"]
mod lease;

#[path = "host_contract/stale_callback.rs"]
mod stale_callback;

use lease::{find_lease, open_lease};

/// 固定端口被占用：`RecoverNetwork` 返回公开的 1102，恢复状态带出稳定类别，清除端口后可再恢复。
#[tokio::test(flavor = "multi_thread")]
async fn occupied_fixed_port_reaches_the_recovery_status_with_a_stable_category() {
    use uc_engine::error_codes::LISTEN_PORT_UNAVAILABLE_CODE;
    use uc_engine::{
        CreateSpaceInput, Engine, EngineConfig, NetworkRecoveryFailureSummary,
        NetworkRecoveryPhaseSummary, NetworkSettingsPatch, Operation, OperationResult,
        SecretString, SettingsPatch,
    };

    let root = tempfile::tempdir().unwrap();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(EmptyClipboard),
        Box::new(EmptyFiles),
    );
    let (engine, _events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("port category".into()),
            passphrase: SecretString::new("port-category-passphrase"),
            passphrase_confirmation: SecretString::new("port-category-passphrase"),
        }))
        .await
        .unwrap();
    let occupied = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)).unwrap();
    let port = occupied.local_addr().unwrap().port();
    engine
        .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
            network: Some(NetworkSettingsPatch {
                listen_port: Some(port),
                ..Default::default()
            }),
            ..Default::default()
        })))
        .await
        .unwrap();

    let error = engine
        .execute(Operation::RecoverNetwork)
        .await
        .expect_err("an occupied fixed port must fail the rebuild");
    assert_eq!(error.code(), LISTEN_PORT_UNAVAILABLE_CODE);
    assert!(!error.is_retryable());

    let OperationResult::NetworkRecoveryStatus(status) = engine
        .execute(Operation::QueryNetworkRecoveryStatus)
        .await
        .unwrap()
    else {
        panic!("expected the recovery status");
    };
    assert_eq!(status.phase, NetworkRecoveryPhaseSummary::Failed);
    assert!(!status.retryable);
    assert_eq!(
        status.failure,
        Some(NetworkRecoveryFailureSummary::ListenPortUnavailable)
    );
    drop(occupied);
    let _ = engine.shutdown(std::time::Duration::from_secs(15)).await;
}

/// 网络设置更新：整次全有或全无，拒绝带结构化分类且不含用户输入原文，0/空列表表示清除。
#[tokio::test(flavor = "multi_thread")]
async fn network_settings_updates_are_structured_and_all_or_nothing() {
    use uc_engine::{
        Engine, EngineConfig, NetworkSettingsPatch, Operation, OperationResult, SettingsPatch,
        SettingsRejection, SettingsUpdateOutcome, TrustedNetworkRejectionKind,
    };

    let root = tempfile::tempdir().unwrap();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(EmptyClipboard),
        Box::new(EmptyFiles),
    );
    let (engine, _events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    let update = |network: NetworkSettingsPatch| {
        let engine = &engine;
        async move {
            let result = engine
                .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
                    network: Some(network),
                    ..Default::default()
                })))
                .await
                .unwrap();
            let OperationResult::SettingsUpdated(outcome) = result else {
                panic!("expected a settings update outcome");
            };
            outcome
        }
    };
    let network_now = || async {
        let OperationResult::Settings(settings) =
            engine.execute(Operation::QuerySettings).await.unwrap()
        else {
            panic!("expected settings");
        };
        (
            settings.network.trusted_networks,
            settings.network.listen_port,
        )
    };

    let saved = update(NetworkSettingsPatch {
        trusted_networks: Some(vec!["10.8.0.0/24".into()]),
        listen_port: Some(4242),
        ..Default::default()
    })
    .await;
    assert!(matches!(saved, SettingsUpdateOutcome::Updated(_)));
    assert_eq!(
        network_now().await,
        (vec!["10.8.0.0/24".to_string()], Some(4242))
    );

    for (entries, index, kind) in [
        (
            vec!["10.9.0.0/24", "8.8.8.0/24"],
            1,
            TrustedNetworkRejectionKind::OutsidePrivateSpace,
        ),
        (
            vec!["garbage-entry"],
            0,
            TrustedNetworkRejectionKind::InvalidFormat,
        ),
        (
            vec!["10.9.0.0/24", "10.9.0.7/24"],
            1,
            TrustedNetworkRejectionKind::Duplicate,
        ),
    ] {
        // 同一次提交里还带一个合法的端口变更：拒绝后端口也必须保持不变。
        let outcome = update(NetworkSettingsPatch {
            trusted_networks: Some(entries.iter().map(|entry| entry.to_string()).collect()),
            listen_port: Some(5555),
            ..Default::default()
        })
        .await;
        let SettingsUpdateOutcome::Rejected { reason, rejection } = outcome else {
            panic!("expected a rejection for {entries:?}");
        };
        assert_eq!(rejection, SettingsRejection::TrustedNetwork { index, kind });
        for entry in &entries {
            assert!(
                !reason.contains(entry),
                "the reason must not echo user input"
            );
        }
        assert_eq!(
            network_now().await,
            (vec!["10.8.0.0/24".to_string()], Some(4242)),
            "a rejected update must save nothing"
        );
    }

    let cleared = update(NetworkSettingsPatch {
        trusted_networks: Some(Vec::new()),
        listen_port: Some(0),
        ..Default::default()
    })
    .await;
    assert!(matches!(cleared, SettingsUpdateOutcome::Updated(_)));
    assert_eq!(network_now().await, (Vec::new(), None));

    engine
        .shutdown(std::time::Duration::from_secs(15))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn suspended_engine_releases_profile_lease_and_can_resume() {
    use std::time::Duration;
    use uc_engine::{
        CreateSpaceInput, Engine, EngineConfig, EngineState, HistoryEntryInput, Operation,
        OperationResult, SecretString, SendTextInput,
    };

    let root = tempfile::tempdir().unwrap();
    let secure_storage = MemorySecureStorage::default();
    let fail_secure_reads = Arc::clone(&secure_storage.fail_reads);
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(secure_storage),
        Box::new(EmptyClipboard),
        Box::new(EmptyFiles),
    );
    let (engine, _events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("suspension test".into()),
            passphrase: SecretString::new("suspension-test-passphrase"),
            passphrase_confirmation: SecretString::new("suspension-test-passphrase"),
        }))
        .await
        .unwrap();
    let sent = engine
        .execute(Operation::SendText(SendTextInput {
            text: "suspension test content".into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(sent) = sent else {
        panic!("expected saved entry")
    };
    let path = find_lease(root.path()).expect("created profile lease");
    assert!(open_lease(&path).try_lock().is_err());
    engine.suspend().await.unwrap();
    let released = open_lease(&path).try_lock().is_ok();
    if released {
        let other_owner = open_lease(&path);
        other_owner.try_lock().unwrap();
        assert!(engine.resume().await.is_err());
        assert_eq!(engine.lifecycle_state().await, EngineState::Quiesced);
        drop(other_owner);
        fail_secure_reads.store(true, Ordering::SeqCst);
        let failed_resume = engine.resume().await;
        assert!(failed_resume.is_err(), "本地资料不可读时不能报告恢复成功");
        assert_eq!(engine.lifecycle_state().await, EngineState::Quiesced);
        assert!(open_lease(&path).try_lock().is_ok(), "失败恢复必须交还租约");
        assert!(engine
            .execute(Operation::SendText(SendTextInput {
                text: "must not be saved during failed recovery".into(),
                target_devices: Vec::new(),
            }))
            .await
            .is_err());
        fail_secure_reads.store(false, Ordering::SeqCst);
        engine.resume().await.unwrap();
        let detail = engine
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: sent.entry_id,
            }))
            .await
            .unwrap();
        let OperationResult::HistoryEntry(detail) = detail else {
            panic!("expected history detail")
        };
        assert_eq!(detail.content, "suspension test content");
        engine
            .execute(Operation::SendText(SendTextInput {
                text: "resumed test content".into(),
                target_devices: Vec::new(),
            }))
            .await
            .unwrap();
        assert!(open_lease(&path).try_lock().is_err());
        engine.suspend().await.unwrap();
        assert!(open_lease(&path).try_lock().is_ok());
    }
    engine.resume().await.unwrap();
    assert!(open_lease(&path).try_lock().is_err());
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    assert!(engine.resume().await.is_err());
    assert!(open_lease(&path).try_lock().is_ok());
    assert!(released, "暂停成功后仍持有 profile 文件锁");
}

#[derive(Clone, Default)]
struct MemorySecureStorage {
    values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    removed_values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    fail_reads: Arc<AtomicBool>,
    kek_reads: Arc<AtomicUsize>,
    fail_kek_read_at: Arc<AtomicUsize>,
    fail_kek_writes: Arc<AtomicBool>,
}

impl MemorySecureStorage {
    fn values(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        match self.values.lock() {
            Ok(values) => values,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn removed_values(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        match self.removed_values.lock() {
            Ok(values) => values,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl HostSecureStorage for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        if key.starts_with("kek:v1:") {
            let read = self.kek_reads.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_kek_read_at.load(Ordering::SeqCst) == read {
                return Err(HostCapabilityError::new(
                    HostCapabilityErrorCategory::Unavailable,
                    "secure storage read failure injected by recovery test",
                ));
            }
        }
        if self.fail_reads.load(Ordering::SeqCst) && key.starts_with("kek:v1:") {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Unavailable,
                "secure storage read failure injected by lifecycle test",
            ));
        }
        Ok(self.values().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        if key.starts_with("kek:v1:") && self.fail_kek_writes.load(Ordering::SeqCst) {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Unavailable,
                "secure storage write failure injected by recovery test",
            ));
        }
        self.values().insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        if let Some(value) = self.values().remove(key) {
            self.removed_values().insert(key.to_owned(), value);
        }
        Ok(())
    }
}

struct EmptyClipboard;

impl HostClipboard for EmptyClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 1,
            representations: Vec::new(),
        })
    }

    fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

struct EmptyFiles;

impl HostFileAccess for EmptyFiles {
    fn metadata(&self, _handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        Ok(HostFileMetadata {
            display_name: "private.txt".into(),
            size_bytes: 0,
            mime_type: Some("text/plain".into()),
        })
    }

    fn read_chunk(
        &self,
        _handle: &HostFileHandle,
        _offset: u64,
        _max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        Ok(Vec::new())
    }

    fn write_chunk(
        &self,
        _handle: &HostFileHandle,
        _offset: u64,
        _bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        Ok(())
    }

    fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

#[test]
fn host_capabilities_are_constructible_without_internal_ports() {
    let capabilities = HostCapabilities::new(
        HostDirectories::new(
            PathBuf::from("/private/data"),
            PathBuf::from("/private/cache"),
            PathBuf::from("/private/temp"),
            PathBuf::from("/private/logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(EmptyClipboard),
        Box::new(EmptyFiles),
    );

    let debug = format!("{capabilities:?}");
    assert!(!debug.contains("/private"));
    assert!(!debug.contains("MemorySecureStorage"));
    assert_eq!(
        capabilities.directories().private_data(),
        PathBuf::from("/private/data")
    );
    let snapshot = match capabilities.clipboard().read() {
        Ok(snapshot) => snapshot,
        Err(error) => panic!("clipboard read failed: {error}"),
    };
    assert!(snapshot.representations.is_empty());
}

#[test]
fn host_boundary_debug_output_redacts_user_content_and_details() {
    let snapshot = HostClipboardSnapshot {
        observed_at_ms: 1,
        representations: vec![
            HostClipboardRepresentation::Inline {
                format: "public.utf8-plain-text".into(),
                mime_type: Some("text/plain".into()),
                bytes: b"private clipboard text".to_vec(),
            },
            HostClipboardRepresentation::File {
                format: "files".into(),
                handle: HostFileHandle::new("private-handle"),
                display_name: "private.txt".into(),
                mime_type: Some("text/plain".into()),
                size_bytes: 42,
            },
        ],
    };
    let metadata = HostFileMetadata {
        display_name: "private.txt".into(),
        size_bytes: 42,
        mime_type: Some("text/plain".into()),
    };
    let error = HostCapabilityError::new(
        HostCapabilityErrorCategory::Unavailable,
        "secret platform error detail",
    );
    let debug = format!("{snapshot:?} {metadata:?} {error:?}");

    for secret in [
        "private clipboard text",
        "private-handle",
        "private.txt",
        "secret platform error detail",
    ] {
        assert!(!debug.contains(secret), "debug output leaked {secret}");
    }
    assert_eq!(error.category(), HostCapabilityErrorCategory::Unavailable);
}
