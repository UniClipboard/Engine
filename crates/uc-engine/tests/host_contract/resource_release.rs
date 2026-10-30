#![cfg(any(target_os = "macos", target_os = "linux"))]

use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, EngineEvent, EngineState, HostCapabilities,
    HostCapabilityError, HostCapabilityErrorCategory, HostClipboard, HostClipboardChange,
    HostClipboardChangeStream, HostClipboardSnapshot, HostDirectories, LifecycleAction, Operation,
    SecretString,
};

use super::{EmptyClipboard, EmptyFiles, MemorySecureStorage};

struct Stream {
    lease: Option<File>,
    fail: Arc<AtomicBool>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl HostClipboardChangeStream for Stream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }

    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "controlled required closer failure",
            ));
        }
        self.lease.take();
        Ok(())
    }
}

struct Clipboard(Option<Stream>);
impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        EmptyClipboard.read()
    }
    fn write(&self, value: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        EmptyClipboard.write(value)
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .take()
            .map(|s| Box::new(s) as Box<dyn HostClipboardChangeStream>))
    }
}

fn available(path: &Path) -> bool {
    let output = Command::new("python3").arg("-c").arg(
        "import sys,fcntl; f=open(sys.argv[1], 'r+');\ntry: fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)\nexcept BlockingIOError: sys.exit(1)"
    ).arg(path).status().unwrap();
    output.success()
}

fn evidence(name: &str, value: serde_json::Value) {
    let root = std::env::var_os("UC_TEST_ARTIFACTS_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "target/test-artifacts/resource-release".into());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join(format!("{name}.json")),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
}

/// 两个真实 Engine 共用网络日志；一个 Engine 结束不能释放另一个 Engine 仍需的租约。
#[tokio::test(flavor = "multi_thread")]
async fn shared_network_lease_waits_for_both_engines() {
    const CHILD: &str = "UC_TEST_SHARED_NETWORK_LEASE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::shared_network_lease_waits_for_both_engines",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
        SignalResult,
    };

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
            root.path().join("logs"),
            NetworkProcessRole::MobileApp,
            NetworkLogBudget {
                record_bytes: 1024,
                queue_records: 16,
                file_bytes: 4096,
                run_bytes: 8192,
                root_bytes: 16384,
                retention: Duration::from_secs(60),
            },
        )
        .unwrap(),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .unwrap()
        .handle();
    let host = |name: &str| {
        let base = root.path().join(name);
        HostCapabilities::new(
            HostDirectories::new(
                base.join("private"),
                base.join("cache"),
                base.join("temp"),
                base.join("logs"),
            ),
            Box::new(MemorySecureStorage::default()),
            Box::new(EmptyClipboard),
            Box::new(EmptyFiles),
        )
    };
    let (first, _) = Engine::start(EngineConfig::new("1.0.0"), host("first"))
        .await
        .unwrap();
    let (second, _) = Engine::start(EngineConfig::new("1.0.0"), host("second"))
        .await
        .unwrap();
    let export = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .unwrap();
    assert_eq!(export.flush, SignalResult::Completed);
    let lease = export.run_directory.join("lease.lock");
    assert!(!available(&lease));
    first.suspend().await.unwrap();
    assert!(!available(&lease), "second Engine still requires the lease");
    second.suspend().await.unwrap();
    assert!(available(&lease), "last Engine must release the lease");
    first.resume().await.unwrap();
    assert!(!available(&lease), "resume must reacquire the lease");
    first.shutdown(Duration::from_secs(15)).await.unwrap();
    second.shutdown(Duration::from_secs(15)).await.unwrap();
    assert!(available(&lease));
    evidence(
        "shared-network-two-engines",
        serde_json::json!({
            "scope": "two real Engines, network lease, independent flock process",
            "after_first_suspend": "held",
            "after_second_suspend": "available",
            "after_first_resume": "held",
            "after_shutdown": "available",
            "process_release_receipt": "not implemented"
        }),
    );
    let _ = handle.shutdown(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn required_closer_failure_retains_real_lock_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("required-closer.lease");
    let file = File::create(&path).unwrap();
    file.lock().unwrap();
    let fail = Arc::new(AtomicBool::new(true));
    let calls = Arc::new(AtomicUsize::new(0));
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(Clipboard(Some(Stream {
            lease: Some(file),
            fail: fail.clone(),
            calls: calls.clone(),
        }))),
        Box::new(EmptyFiles),
    );
    let (engine, _events) = Engine::start(EngineConfig::new("2.0.0"), host)
        .await
        .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("release acceptance".into()),
            passphrase: SecretString::new("release-acceptance-passphrase"),
            passphrase_confirmation: SecretString::new("release-acceptance-passphrase"),
        }))
        .await
        .unwrap();
    assert!(!available(&path));
    let failed = engine.shutdown(Duration::from_secs(15)).await;
    let retained = !available(&path);
    evidence(
        "required-closer",
        serde_json::json!({"shutdown_error":failed.is_err(), "lock_retained_after_failure":retained, "calls":calls.load(Ordering::SeqCst)}),
    );
    assert!(
        failed.is_err(),
        "required closer failure must prevent successful release"
    );
    assert!(retained, "the owner must retain its failed closer handle");
    fail.store(false, Ordering::SeqCst);
    use uc_engine::ResourceReleaseOutcome;
    let recovered = engine.shutdown_with_outcome(Duration::from_secs(15)).await;
    assert!(matches!(
        recovered,
        ResourceReleaseOutcome::Released { error: Some(_) }
    ));
    assert!(available(&path));
    let count = calls.load(Ordering::SeqCst);
    assert!(count >= 2);
    assert_eq!(
        engine.shutdown_with_outcome(Duration::from_secs(15)).await,
        recovered
    );
    assert_eq!(
        engine.suspend_with_outcome(Duration::from_secs(15)).await,
        recovered
    );
    assert_eq!(calls.load(Ordering::SeqCst), count);
    evidence(
        "required-closer-recovered",
        serde_json::json!({"release_confirmed":true,"previous_error_preserved":true,"independent_lock_available":true,"repeat_did_not_close_again":true,"calls":count}),
    );
}

/// 独立进程防止进程观测关闭影响其他测试。
#[tokio::test(flavor = "multi_thread")]
async fn network_log_park_failure_emits_lifecycle_failure() {
    const CHILD: &str = "UC_TEST_LOG_PARK_FAILURE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::network_log_park_failure_emits_lifecycle_failure",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
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
            root.path().join("network"),
            NetworkProcessRole::MobileApp,
            NetworkLogBudget {
                record_bytes: 1024,
                queue_records: 16,
                file_bytes: 4096,
                run_bytes: 8192,
                root_bytes: 16384,
                retention: Duration::from_secs(60),
            },
        )
        .unwrap(),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .unwrap()
        .handle();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(EmptyClipboard),
        Box::new(EmptyFiles),
    );
    let (engine, mut events) = Engine::start(EngineConfig::new("1.0.0"), host)
        .await
        .unwrap();
    tokio::task::spawn_blocking(move || handle.shutdown(Duration::from_secs(5)))
        .await
        .unwrap();
    let error = engine.suspend().await.unwrap_err();
    assert_eq!(engine.lifecycle_state().await, EngineState::Quiesced);
    let observed = tokio::time::timeout(Duration::from_millis(500), async {
        while let Some(event) = events.next().await {
            if let EngineEvent::LifecycleFailed {
                action,
                error: observed,
            } = event
            {
                assert_eq!(action, LifecycleAction::Suspend);
                assert_eq!(observed, error);
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    evidence(
        "network-log-park-event",
        serde_json::json!({
            "caller_received_error": true, "state_quiesced": true,
            "lifecycle_failure_observed": observed,
            "injection": "process observability shutdown before Engine suspend",
        }),
    );
    assert!(
        observed,
        "network-log park failure must emit LifecycleFailed"
    );
}

struct RetryableProcessStream {
    lease: Option<File>,
    calls: Arc<AtomicUsize>,
    retry_started: std::sync::mpsc::Sender<()>,
    retry_release: Arc<tokio::sync::Notify>,
    failures_before_success: usize,
}

#[async_trait]
impl HostClipboardChangeStream for RetryableProcessStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.failures_before_success {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "controlled close failure",
            ));
        }
        self.retry_started.send(()).unwrap();
        self.retry_release.notified().await;
        self.lease.take();
        Ok(())
    }
    fn supports_shutdown_retry(&self) -> bool {
        true
    }
}

struct RetryableProcessClipboard(Option<RetryableProcessStream>);
impl HostClipboard for RetryableProcessClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        EmptyClipboard.read()
    }
    fn write(&self, value: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        EmptyClipboard.write(value)
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

#[test]
fn process_retains_and_retries_abandoned_engine_closer() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_ABANDONED_RETRY_CLOSER_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::process_retains_and_retries_abandoned_engine_closer",
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
    let lease_path = root.path().join("host-stream.lease");
    let lease = File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (retry_started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let capabilities = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(lease),
            calls: Arc::clone(&calls),
            retry_started,
            retry_release: Arc::clone(&release),
            failures_before_success: 1,
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), capabilities))
        .unwrap();
    let first = executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(10)));
    let first_failed = matches!(first, uc_engine::ResourceReleaseOutcome::Failed { .. });
    let held_after_failure = !available(&lease_path);
    drop(engine);
    drop(executor);
    let held_after_drop = !available(&lease_path);
    let retained_count = process.test_retained_engine_count();
    let initial_process = process.test_process_release(Duration::from_secs(3));
    let no_implicit_retry = calls.load(Ordering::SeqCst) == 1;
    let retry_process = process.clone();
    let retry = std::thread::spawn(move || {
        retry_process.test_retry_process_release(Duration::from_secs(8))
    });
    let retry_joined = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    let held_while_retry = !available(&lease_path);
    let concurrent = process.test_retry_process_release(Duration::ZERO);
    release.notify_one();
    let retried = retry.join().unwrap();
    let lock_available = available(&lease_path);
    let repeated = process.test_retry_process_release(Duration::ZERO);
    let count = calls.load(Ordering::SeqCst);
    let remaining_count = process.test_retained_engine_count();
    let artifact = serde_json::json!({"scope":"actual Engine, retryable host stream and process owner, independent host lock", "first_engine_shutdown_failed":first_failed, "host_lock_held_after_failure":held_after_failure, "host_lock_held_after_engine_drop":held_after_drop, "retained_engine_count":retained_count, "initial_process_release":initial_process, "no_implicit_retry":no_implicit_retry, "explicit_retry_started":retry_joined, "host_lock_held_while_retry":held_while_retry, "concurrent_retry":concurrent.0, "retry_result":retried.0, "original_failure_preserved":retried.1, "host_lock_available_after_retry":lock_available, "repeated_result":repeated.0, "remaining_engine_count":remaining_count, "close_calls":count, "device_acceptance":"not established"});
    evidence("abandoned-engine-closer", artifact);
    assert!(
        first_failed
            && held_after_failure
            && held_after_drop
            && retained_count == 1
            && initial_process == "failed"
            && no_implicit_retry
            && retry_joined
            && held_while_retry
            && concurrent.0 == "pending"
            && retried == ("released", true)
            && lock_available
            && repeated == retried
            && count == 2
            && remaining_count == 0
    );
}

#[test]
fn suspended_engine_orphan_invalidates_process_receipt() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_SUSPENDED_ENGINE_ORPHAN_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::suspended_engine_orphan_invalidates_process_receipt",
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
    let lease_path = root.path().join("host-stream.lease");
    let lease = File::create(&lease_path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (retry_started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let capabilities = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temporary"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(lease),
            calls: Arc::clone(&calls),
            retry_started,
            retry_release: Arc::clone(&release),
            failures_before_success: 1,
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), capabilities))
        .unwrap();
    let suspended = executor.block_on(engine.suspend()).is_ok();
    let original = process.test_process_release(Duration::from_secs(5));
    let original_generation = process.test_process_release_generation();
    let failed = executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(10)));
    let engine_failed = matches!(failed, uc_engine::ResourceReleaseOutcome::Failed { .. });
    drop(engine);
    drop(executor);
    let retained = !available(&lease_path);
    let current = process.test_process_release(Duration::from_secs(2));
    let current_generation = process.test_process_release_generation();
    let retry_process = process.clone();
    let retry = std::thread::spawn(move || {
        retry_process.test_retry_process_release(Duration::from_secs(8))
    });
    let retry_started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    release.notify_one();
    let result = retry.join().unwrap();
    let lock_available = available(&lease_path);
    let artifact = serde_json::json!({"scope":"actual suspended Engine, process release generations and independent host lock", "engine_suspended":suspended, "original_process_release":original, "original_generation":original_generation, "engine_shutdown_failed":engine_failed, "host_lock_retained":retained, "current_process_release":current, "current_generation":current_generation, "retry_started":retry_started, "retry_result":result.0, "host_lock_available_after_retry":lock_available, "device_acceptance":"not established"});
    evidence("suspended-engine-orphan", artifact);
    assert!(
        suspended
            && original == "released"
            && engine_failed
            && retained
            && current != "released"
            && current_generation > original_generation
            && retry_started
            && result.0 == "released"
            && lock_available
    );
}

#[test]
fn orphan_retry_waits_for_another_active_engine() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_ORPHAN_RETRY_OTHER_ENGINE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::orphan_retry_waits_for_another_active_engine",
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
    let first_path = root.path().join("first.lease");
    let first_lease = File::create(&first_path).unwrap();
    first_lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (retry_started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let first_host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("first/private"),
            root.path().join("first/cache"),
            root.path().join("first/temp"),
            root.path().join("first/logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(first_lease),
            calls: Arc::clone(&calls),
            retry_started,
            retry_release: Arc::clone(&release),
            failures_before_success: 1,
        }))),
        Box::new(EmptyFiles),
    );
    let second_path = root.path().join("second.lease");
    let second_lease = File::create(&second_path).unwrap();
    second_lease.lock().unwrap();
    let second_calls = Arc::new(AtomicUsize::new(0));
    let second_host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("second/private"),
            root.path().join("second/cache"),
            root.path().join("second/temp"),
            root.path().join("second/logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(Clipboard(Some(Stream {
            lease: Some(second_lease),
            fail: Arc::new(AtomicBool::new(false)),
            calls: Arc::clone(&second_calls),
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (first, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), first_host))
        .unwrap();
    let (second, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), second_host))
        .unwrap();
    let log_lease = process
        .prepare_network_log_export(Duration::from_secs(5))
        .unwrap()
        .run_directory
        .join("lease.lock");
    let first_failed = matches!(
        executor.block_on(first.shutdown_with_outcome(Duration::from_secs(10))),
        uc_engine::ResourceReleaseOutcome::Failed { .. }
    );
    drop(first);
    let initial = process.test_process_release(Duration::from_secs(3));
    let retry_process = process.clone();
    let retry = std::thread::spawn(move || {
        retry_process.test_retry_process_release(Duration::from_secs(15))
    });
    let started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    release.notify_one();
    let limit = std::time::Instant::now() + Duration::from_secs(5);
    while !available(&first_path) && std::time::Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(10));
    }
    let first_closed = available(&first_path);
    let second_held = !available(&second_path) && second_calls.load(Ordering::SeqCst) == 0;
    let log_held = !available(&log_lease);
    let pending = process.test_retry_process_release(Duration::ZERO).0 == "pending";
    let second_result = executor.block_on(second.shutdown_with_outcome(Duration::from_secs(10)));
    let second_released = matches!(
        second_result,
        uc_engine::ResourceReleaseOutcome::Released { .. }
    );
    drop(second);
    let result = retry.join().unwrap();
    let second_closed = available(&second_path);
    let log_closed = available(&log_lease);
    let artifact = serde_json::json!({"scope":"two real Engines, orphaned retryable closer, independent host and process locks", "first_shutdown_failed":first_failed, "initial_process_release":initial, "retry_started":started, "first_lock_closed":first_closed, "other_engine_untouched":second_held, "process_log_lock_held":log_held, "process_pending_for_other_engine":pending, "other_engine_released":second_released, "other_host_lock_closed":second_closed, "process_log_lock_closed":log_closed, "final_process_release":result.0, "prior_failure_preserved":result.1, "device_acceptance":"not established"});
    evidence("orphan-retry-other-engine", artifact);
    assert!(
        first_failed
            && initial == "failed"
            && started
            && first_closed
            && second_held
            && log_held
            && pending
            && second_released
            && second_closed
            && log_closed
            && result == ("released", true)
    );
}

#[test]
fn new_orphan_during_retry_waits_for_running_closer() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_NEW_ORPHAN_DURING_RETRY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::new_orphan_during_retry_waits_for_running_closer",
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
    let first_path = root.path().join("first.lease");
    let first_lease = File::create(&first_path).unwrap();
    first_lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (retry_started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let first_host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("first/private"),
            root.path().join("first/cache"),
            root.path().join("first/temp"),
            root.path().join("first/logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(first_lease),
            calls: Arc::clone(&calls),
            retry_started,
            retry_release: Arc::clone(&release),
            failures_before_success: 1,
        }))),
        Box::new(EmptyFiles),
    );
    let second_path = root.path().join("second.lease");
    let second_lease = File::create(&second_path).unwrap();
    second_lease.lock().unwrap();
    let second_calls = Arc::new(AtomicUsize::new(0));
    let second_host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("second/private"),
            root.path().join("second/cache"),
            root.path().join("second/temp"),
            root.path().join("second/logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(Clipboard(Some(Stream {
            lease: Some(second_lease),
            fail: Arc::new(AtomicBool::new(false)),
            calls: Arc::clone(&second_calls),
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (first, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), first_host))
        .unwrap();
    let (second, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), second_host))
        .unwrap();
    let first_failed = matches!(
        executor.block_on(first.shutdown_with_outcome(Duration::from_secs(10))),
        uc_engine::ResourceReleaseOutcome::Failed { .. }
    );
    drop(first);
    let initial = process.test_process_release(Duration::from_secs(3));
    let retry_process = process.clone();
    let retry = std::thread::spawn(move || {
        retry_process.test_retry_process_release(Duration::from_secs(15))
    });
    let started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    drop(second);
    let pending_while_first_blocked =
        process.test_retry_process_release(Duration::ZERO).0 == "pending";
    let both_locks_held = !available(&first_path) && !available(&second_path);
    release.notify_one();
    let first_retry = retry.join().unwrap();
    let first_closed = available(&first_path);
    let second_held = !available(&second_path) && second_calls.load(Ordering::SeqCst) == 0;
    let second_retry = process.test_retry_process_release(Duration::from_secs(10));
    let second_closed = available(&second_path);
    let artifact = serde_json::json!({"scope":"two real Engines, new orphan while a retryable closer is blocked, independent host locks", "first_shutdown_failed":first_failed, "initial_process_release":initial, "retry_started":started, "retry_pending_after_new_orphan":pending_while_first_blocked, "both_locks_held":both_locks_held, "first_retry_result":first_retry.0, "first_lock_closed":first_closed, "second_lock_retained_without_implicit_close":second_held, "second_retry_result":second_retry.0, "second_lock_closed":second_closed, "prior_failure_preserved":second_retry.1, "device_acceptance":"not established"});
    evidence("new-orphan-during-retry", artifact);
    assert!(
        first_failed
            && initial == "failed"
            && started
            && pending_while_first_blocked
            && both_locks_held
            && first_retry.0 == "failed"
            && first_closed
            && second_held
            && second_retry == ("released", true)
            && second_closed
    );
}

#[test]
fn orphan_retry_failure_keeps_the_same_closer() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_ORPHAN_RETRY_AGAIN_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::orphan_retry_failure_keeps_the_same_closer",
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
    let path = root.path().join("host.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (retry_started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(lease),
            calls: Arc::clone(&calls),
            retry_started,
            retry_release: Arc::clone(&release),
            failures_before_success: 2,
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let first_engine_error =
        match executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(10))) {
            uc_engine::ResourceReleaseOutcome::Failed { error } => error,
            other => panic!("expected Engine failure: {other:?}"),
        };
    let first_failed = true;
    drop(engine);
    drop(executor);
    let initial = process.test_process_release(Duration::from_secs(3));
    let initial_errors = process.test_process_engine_errors();
    let initial_report = process.test_process_safe_report();
    let initial_report_valid = initial_report == Some((vec!["Engine".into()], vec![], true));
    let first_retained = initial_errors
        .0
        .iter()
        .any(|error| error.downcast_ref::<uc_engine::EngineError>() == Some(&first_engine_error));
    #[cfg(feature = "dev-tools")]
    if std::env::var_os("UC_TEST_ORPHAN_RETRY_EXECUTOR_FAILURE").is_some() {
        uc_engine::probe_fail_next_orphan_retry_executor();
        let failed_retry = process.test_retry_process_release(Duration::from_secs(5));
        let errors = process.test_process_engine_errors();
        let terminal_report = process.test_process_safe_report();
        let terminal_report_valid =
            terminal_report == Some((vec!["Engine".into()], vec!["Engine".into()], false));
        let io_source = errors.0.iter().any(|error| {
            error.downcast_ref::<std::io::Error>().is_some_and(|error| {
                error.kind() == std::io::ErrorKind::WouldBlock
                    && error
                        .get_ref()
                        .and_then(|source| source.downcast_ref::<std::io::Error>())
                        .is_some_and(|source| source.kind() == std::io::ErrorKind::PermissionDenied)
            })
        });
        let prior_source = errors.1.iter().any(|error| {
            error.downcast_ref::<uc_engine::EngineError>() == Some(&first_engine_error)
        });
        let held = !available(&path);
        let attempts = calls.load(Ordering::SeqCst);
        let repeated = process.test_process_release(Duration::ZERO);
        let retried_again = process.test_retry_process_release(Duration::ZERO);
        let stable = failed_retry.0 == "failed"
            && repeated == "failed"
            && retried_again.0 == "failed"
            && calls.load(Ordering::SeqCst) == attempts
            && attempts == 1;
        evidence(
            "orphan-retry-executor-failure",
            serde_json::json!({"scope":"real Engine and host lock with injected retry executor creation failure", "initial_error_retained":first_retained, "initial_safe_report":initial_report_valid, "terminal_safe_report":terminal_report_valid, "io_source_retained":io_source, "prior_engine_error_retained":prior_source, "lock_held":held, "stable_without_retry":stable, "device_acceptance":"not established"}),
        );
        assert!(
            first_retained
                && initial_report_valid
                && terminal_report_valid
                && io_source
                && prior_source
                && held
                && stable
        );
        return;
    }
    let failed_retry = process.test_retry_process_release(Duration::from_secs(5));
    let second_errors = process.test_process_engine_errors();
    let second_retained = second_errors
        .0
        .iter()
        .any(|error| error.downcast_ref::<uc_engine::EngineError>() == Some(&first_engine_error));
    let distinct_attempts = initial_errors
        .0
        .first()
        .zip(second_errors.0.first())
        .is_some_and(|(first, second)| !Arc::ptr_eq(first, second));
    let held_after_retry = !available(&path);
    let calls_after_failure = calls.load(Ordering::SeqCst);
    let wait_only = process.test_process_release(Duration::ZERO);
    let no_implicit_retry = calls.load(Ordering::SeqCst) == calls_after_failure;
    let retry_process = process.clone();
    let retry = std::thread::spawn(move || {
        retry_process.test_retry_process_release(Duration::from_secs(10))
    });
    let started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    let held_during_retry = !available(&path);
    release.notify_one();
    let final_result = retry.join().unwrap();
    let lock_available = available(&path);
    let final_errors = process.test_process_engine_errors();
    let final_report_valid =
        process.test_process_safe_report() == Some((vec![], vec!["Engine".into()], false));
    let prior_retained = final_errors
        .1
        .iter()
        .any(|error| error.downcast_ref::<uc_engine::EngineError>() == Some(&first_engine_error));
    let first_source_retained = initial_errors
        .0
        .first()
        .is_some_and(|first| final_errors.1.iter().any(|error| Arc::ptr_eq(first, error)));
    let one_error_per_owner = final_errors.1.len() == 1;
    let artifact = serde_json::json!({"scope":"actual Engine, twice-failing retryable closer and independent host lock", "first_engine_shutdown_failed":first_failed, "initial_process_release":initial, "first_retry_result":failed_retry.0, "host_lock_retained_after_failed_retry":held_after_retry, "calls_after_failed_retry":calls_after_failure, "wait_only_result":wait_only, "no_implicit_retry":no_implicit_retry, "second_retry_started":started, "host_lock_held_during_second_retry":held_during_retry, "final_process_release":final_result.0, "prior_failure_preserved":final_result.1, "first_engine_error_retained":first_retained, "initial_safe_report":initial_report_valid, "final_safe_report":final_report_valid, "second_engine_error_retained":second_retained, "prior_engine_error_retained":prior_retained, "distinct_attempt_errors":distinct_attempts, "first_source_retained":first_source_retained, "one_error_per_owner":one_error_per_owner, "host_lock_available":lock_available, "total_close_calls":calls.load(Ordering::SeqCst), "device_acceptance":"not established"});
    evidence("orphan-retry-fails-again", artifact);
    assert!(
        first_failed
            && first_retained
            && initial_report_valid
            && final_report_valid
            && second_retained
            && prior_retained
            && distinct_attempts
            && first_source_retained
            && one_error_per_owner
            && initial == "failed"
            && failed_retry.0 == "failed"
            && held_after_retry
            && calls_after_failure == 2
            && wait_only == "failed"
            && no_implicit_retry
            && started
            && held_during_retry
            && final_result == ("released", true)
            && lock_available
            && calls.load(Ordering::SeqCst) == 3
    );
}

#[cfg(feature = "dev-tools")]
#[test]
fn orphan_retry_executor_failure_retains_source_and_owner() {
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "resource_release::orphan_retry_failure_keeps_the_same_closer",
            "--nocapture",
        ])
        .env("UC_TEST_ORPHAN_RETRY_AGAIN_CHILD", "1")
        .env("UC_TEST_ORPHAN_RETRY_EXECUTOR_FAILURE", "1")
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn process_joins_shutdown_after_engine_handle_drops() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_PROCESS_JOIN_DROPPED_ENGINE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::process_joins_shutdown_after_engine_handle_drops",
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
    let path = root.path().join("host.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(lease),
            calls: Arc::clone(&calls),
            retry_started: started,
            retry_release: Arc::clone(&release),
            failures_before_success: 0,
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let engine_wait = executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(1)));
    let engine_pending = matches!(engine_wait, uc_engine::ResourceReleaseOutcome::Pending);
    let closer_started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    let second_wait = executor.block_on(engine.shutdown_with_outcome(Duration::ZERO));
    let second_pending = matches!(second_wait, uc_engine::ResourceReleaseOutcome::Pending);
    drop(engine);
    let host_held = !available(&path);
    let process_wait = process.test_process_release(Duration::from_millis(200));
    let process_pending = process_wait == "pending";
    release.notify_one();
    let completed = process.test_process_release(Duration::from_secs(10));
    let host_free = available(&path);
    let count = calls.load(Ordering::SeqCst);
    drop(executor);
    let artifact = serde_json::json!({"scope":"actual Engine, in-flight host close and process owner, independent host lock", "engine_wait_pending":engine_pending, "second_engine_wait_pending":second_pending, "closer_started":closer_started, "host_lock_held_after_engine_drop":host_held, "process_wait_pending":process_pending, "process_result_after_close":completed, "host_lock_available":host_free, "close_calls":count, "device_acceptance":"not established"});
    evidence("dropped-engine-inflight-close", artifact);
    assert!(
        engine_pending
            && second_pending
            && closer_started
            && host_held
            && process_pending
            && completed == "released"
            && host_free
            && count == 1
    );
}

#[test]
fn suspended_engine_inflight_close_advances_process_generation() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_SUSPENDED_INFLIGHT_CLOSE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::suspended_engine_inflight_close_advances_process_generation",
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
    let path = root.path().join("host.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(lease),
            calls: Arc::clone(&calls),
            retry_started: started,
            retry_release: Arc::clone(&release),
            failures_before_success: 0,
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let suspended = executor.block_on(engine.suspend()).is_ok();
    let original = process.test_process_release(Duration::from_secs(5));
    let original_generation = process.test_process_release_generation();
    let wait = executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(1)));
    let engine_pending = matches!(wait, uc_engine::ResourceReleaseOutcome::Pending);
    let closer_started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    drop(engine);
    let held = !available(&path);
    let pending = process.test_process_release(Duration::from_millis(200));
    let current_generation = process.test_process_release_generation();
    release.notify_one();
    let final_result = process.test_process_release(Duration::from_secs(10));
    let free = available(&path);
    let count = calls.load(Ordering::SeqCst);
    drop(executor);
    let artifact = serde_json::json!({"scope":"actual suspended Engine and pending host close after an earlier process release", "engine_suspended":suspended, "original_result":original, "original_generation":original_generation, "engine_wait_pending":engine_pending, "closer_started":closer_started, "host_lock_held":held, "current_result":pending, "current_generation":current_generation, "final_result":final_result, "host_lock_available":free, "close_calls":count, "device_acceptance":"not established"});
    evidence("suspended-inflight-generation", artifact);
    assert!(
        suspended
            && original == "released"
            && engine_pending
            && closer_started
            && held
            && pending == "pending"
            && current_generation > original_generation
            && final_result == "released"
            && free
            && count == 1
    );
}

#[test]
fn terminated_engine_close_remains_terminal() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_TERMINATED_ENGINE_CLOSE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::terminated_engine_close_remains_terminal",
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
    let path = root.path().join("host.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let (started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(lease),
            calls: Arc::clone(&calls),
            retry_started: started,
            retry_release: release,
            failures_before_success: 0,
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let wait = executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(1)));
    let pending = matches!(wait, uc_engine::ResourceReleaseOutcome::Pending);
    let closer_started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    let second_wait = executor.block_on(engine.shutdown_with_outcome(Duration::ZERO));
    let second_pending = matches!(second_wait, uc_engine::ResourceReleaseOutcome::Pending);
    drop(engine);
    executor.shutdown_timeout(Duration::from_millis(100));
    let lock_held = !available(&path);
    let first = process.test_process_release(Duration::from_secs(3));
    let generation = process.test_process_release_generation();
    let retry = process.test_retry_process_release(Duration::from_secs(1));
    let repeated = process.test_retry_process_release(Duration::ZERO);
    let generation_stable = process.test_process_release_generation() == generation;
    let still_held = !available(&path);
    let count = calls.load(Ordering::SeqCst);
    let artifact = serde_json::json!({"scope":"actual Engine, terminated shutdown executor, real host closer and independent lock", "engine_wait_pending":pending, "second_engine_wait_pending":second_pending, "closer_started":closer_started, "host_lock_held_after_executor_loss":lock_held, "process_result":first, "explicit_retry_result":retry.0, "repeated_retry_result":repeated.0, "generation_stable":generation_stable, "host_lock_still_held":still_held, "close_calls":count, "device_acceptance":"not established"});
    evidence("terminated-engine-close", artifact);
    assert!(
        pending
            && second_pending
            && closer_started
            && lock_held
            && first == "failed"
            && retry.0 == "failed"
            && repeated.0 == "failed"
            && generation_stable
            && still_held
            && count == 1
    );
}

#[test]
fn pending_orphan_arriving_during_retry_stays_pending() {
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, ProcessObservabilityRuntime,
    };
    const CHILD: &str = "UC_TEST_PENDING_ORPHAN_DURING_RETRY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::pending_orphan_arriving_during_retry_stays_pending",
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
    let first_path = root.path().join("first.lease");
    let first_lease = File::create(&first_path).unwrap();
    first_lease.lock().unwrap();
    let first_calls = Arc::new(AtomicUsize::new(0));
    let (first_started, first_waiting) = std::sync::mpsc::channel();
    let first_release = Arc::new(tokio::sync::Notify::new());
    let first_host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("first/private"),
            root.path().join("first/cache"),
            root.path().join("first/temp"),
            root.path().join("first/logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(first_lease),
            calls: Arc::clone(&first_calls),
            retry_started: first_started,
            retry_release: Arc::clone(&first_release),
            failures_before_success: 1,
        }))),
        Box::new(EmptyFiles),
    );
    let second_path = root.path().join("second.lease");
    let second_lease = File::create(&second_path).unwrap();
    second_lease.lock().unwrap();
    let second_calls = Arc::new(AtomicUsize::new(0));
    let (second_started, second_waiting) = std::sync::mpsc::channel();
    let second_release = Arc::new(tokio::sync::Notify::new());
    let second_host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("second/private"),
            root.path().join("second/cache"),
            root.path().join("second/temp"),
            root.path().join("second/logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(RetryableProcessClipboard(Some(RetryableProcessStream {
            lease: Some(second_lease),
            calls: Arc::clone(&second_calls),
            retry_started: second_started,
            retry_release: Arc::clone(&second_release),
            failures_before_success: 0,
        }))),
        Box::new(EmptyFiles),
    );
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let (first, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), first_host))
        .unwrap();
    let (second, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), second_host))
        .unwrap();
    let first_failed = matches!(
        executor.block_on(first.shutdown_with_outcome(Duration::from_secs(10))),
        uc_engine::ResourceReleaseOutcome::Failed { .. }
    );
    drop(first);
    let initial = process.test_process_release(Duration::from_secs(3));
    let retry_process = process.clone();
    let retry = std::thread::spawn(move || {
        retry_process.test_retry_process_release(Duration::from_secs(15))
    });
    let first_retry_started = first_waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    let second_wait = executor.block_on(second.shutdown_with_outcome(Duration::from_secs(1)));
    let second_pending = matches!(second_wait, uc_engine::ResourceReleaseOutcome::Pending);
    let second_close_started = second_waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    drop(second);
    let both_held = !available(&first_path) && !available(&second_path);
    let pending_before_first = process.test_retry_process_release(Duration::ZERO).0 == "pending";
    first_release.notify_one();
    let limit = std::time::Instant::now() + Duration::from_secs(5);
    while !available(&first_path) && std::time::Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(10));
    }
    let first_closed = available(&first_path);
    let second_still_held = !available(&second_path);
    let pending_after_first = process
        .test_retry_process_release(Duration::from_millis(200))
        .0
        == "pending";
    second_release.notify_one();
    let result = retry.join().unwrap();
    let second_closed = available(&second_path);
    let count = second_calls.load(Ordering::SeqCst);
    drop(executor);
    let artifact = serde_json::json!({"scope":"two real Engines, one retry and one already accepted close with independent locks", "first_shutdown_failed":first_failed, "initial_process_release":initial, "first_retry_started":first_retry_started, "second_engine_wait_pending":second_pending, "second_close_started":second_close_started, "both_locks_held":both_held, "process_pending_before_first_close":pending_before_first, "first_lock_closed":first_closed, "second_lock_still_held":second_still_held, "process_pending_after_first_close":pending_after_first, "process_result":result.0, "prior_failure_preserved":result.1, "second_lock_closed":second_closed, "second_close_calls":count, "device_acceptance":"not established"});
    evidence("pending-orphan-during-retry", artifact);
    assert!(
        first_failed
            && initial == "failed"
            && first_retry_started
            && second_pending
            && second_close_started
            && both_held
            && pending_before_first
            && first_closed
            && second_still_held
            && pending_after_first
            && result == ("released", true)
            && second_closed
            && count == 1
    );
}

struct SpanHoldingProcessStream {
    lease: Option<File>,
    calls: Arc<AtomicUsize>,
    span: Arc<std::sync::Mutex<Option<tracing::Span>>>,
    started: std::sync::mpsc::Sender<()>,
    release: Arc<tokio::sync::Notify>,
    failures_before_success: usize,
}

#[async_trait]
impl HostClipboardChangeStream for SpanHoldingProcessStream {
    async fn next(&mut self) -> Result<HostClipboardChange, HostCapabilityError> {
        std::future::pending().await
    }
    async fn shutdown(&mut self) -> Result<(), HostCapabilityError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) < self.failures_before_success {
            return Err(HostCapabilityError::new(
                HostCapabilityErrorCategory::Io,
                "controlled close failure",
            ));
        }
        self.started.send(()).unwrap();
        self.release.notified().await;
        self.span.lock().unwrap().take();
        self.lease.take();
        Ok(())
    }
    fn supports_shutdown_retry(&self) -> bool {
        true
    }
}
struct SpanHoldingProcessClipboard(Option<SpanHoldingProcessStream>);
impl HostClipboard for SpanHoldingProcessClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        EmptyClipboard.read()
    }
    fn write(&self, value: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        EmptyClipboard.write(value)
    }
    fn take_change_stream(
        &mut self,
    ) -> Result<Option<Box<dyn HostClipboardChangeStream>>, HostCapabilityError> {
        Ok(self
            .0
            .take()
            .map(|stream| Box::new(stream) as Box<dyn HostClipboardChangeStream>))
    }
}

#[test]
fn process_retry_closes_engine_before_waiting_for_its_span() {
    use uc_observability_contract::diagnostics::{
        operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
        OperationContext,
    };
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, OtlpHttpConfig,
        ProcessObservabilityRuntime,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    const CHILD: &str = "UC_TEST_ORPHAN_SPAN_RETRY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::process_retry_closes_engine_before_waiting_for_its_span",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let receiver = executor.block_on(async {
        let receiver = MockServer::start().await;
        for route in ["/v1/traces", "/v1/logs"] {
            Mock::given(method("POST"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200))
                .mount(&receiver)
                .await;
        }
        receiver
    });
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .unwrap();
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .unwrap(),
    )
    .with_remote(remote)
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
    let path = root.path().join("host.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    span.record("uc.outcome", "ok");
    span.record("otel.status_code", "OK");
    let span = Arc::new(std::sync::Mutex::new(Some(span)));
    let calls = Arc::new(AtomicUsize::new(0));
    let (started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(SpanHoldingProcessClipboard(Some(
            SpanHoldingProcessStream {
                lease: Some(lease),
                calls: Arc::clone(&calls),
                span: Arc::clone(&span),
                started,
                release: Arc::clone(&release),
                failures_before_success: 1,
            },
        ))),
        Box::new(EmptyFiles),
    );
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let first_failed = matches!(
        executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(10))),
        uc_engine::ResourceReleaseOutcome::Failed { .. }
    );
    drop(engine);
    let initial = process.test_process_release(Duration::from_millis(300));
    let retry_process = process.clone();
    let retry = std::thread::spawn(move || {
        retry_process.test_retry_process_release(Duration::from_secs(8))
    });
    let retry_started = waiting.recv_timeout(Duration::from_secs(2)).is_ok();
    let span_held = span.lock().unwrap().is_some();
    let lock_held = !available(&path);
    let pending = process.test_retry_process_release(Duration::ZERO).0 == "pending";
    if !retry_started {
        span.lock().unwrap().take();
    }
    release.notify_one();
    let result = retry.join().unwrap();
    let lock_available = available(&path);
    let artifact = serde_json::json!({"scope":"actual Engine, retained closer, accepted diagnostic span, local telemetry receiver and independent host lock", "first_engine_shutdown_failed":first_failed, "initial_process_release":initial, "retry_started":retry_started, "span_held_during_retry":span_held, "host_lock_held_during_retry":lock_held, "retry_wait_pending":pending, "retry_result":result.0, "prior_failure_preserved":result.1, "host_lock_available":lock_available, "close_calls":calls.load(Ordering::SeqCst), "device_acceptance":"not established"});
    evidence("orphan-span-retry", artifact);
    assert!(
        first_failed
            && initial == "failed"
            && retry_started
            && span_held
            && lock_held
            && pending
            && result == ("released", true)
            && lock_available
            && calls.load(Ordering::SeqCst) == 2
    );
}

#[test]
fn orphan_arriving_during_span_drain_unblocks_retry() {
    use uc_observability_contract::diagnostics::{
        operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
        OperationContext,
    };
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, OtlpHttpConfig,
        ProcessObservabilityRuntime,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    const CHILD: &str = "UC_TEST_ORPHAN_DURING_SPAN_DRAIN_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::orphan_arriving_during_span_drain_unblocks_retry",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let receiver = executor.block_on(async {
        let receiver = MockServer::start().await;
        for route in ["/v1/traces", "/v1/logs"] {
            Mock::given(method("POST"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200))
                .mount(&receiver)
                .await;
        }
        receiver
    });
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .unwrap();
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .unwrap(),
    )
    .with_remote(remote)
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
    let path = root.path().join("host.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    span.record("uc.outcome", "ok");
    span.record("otel.status_code", "OK");
    let span = Arc::new(std::sync::Mutex::new(Some(span)));
    let calls = Arc::new(AtomicUsize::new(0));
    let (started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(SpanHoldingProcessClipboard(Some(
            SpanHoldingProcessStream {
                lease: Some(lease),
                calls: Arc::clone(&calls),
                span: Arc::clone(&span),
                started,
                release: Arc::clone(&release),
                failures_before_success: 1,
            },
        ))),
        Box::new(EmptyFiles),
    );
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let suspended = executor.block_on(engine.suspend()).is_ok();
    let initial = process.test_process_release(Duration::from_millis(300));
    let first_failed = matches!(
        executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(10))),
        uc_engine::ResourceReleaseOutcome::Failed { .. }
    );
    drop(engine);
    let after_drop = process.test_process_release(Duration::from_millis(500));
    if after_drop != "failed" {
        span.lock().unwrap().take();
        let _ = process.test_process_release(Duration::from_secs(5));
    }
    let retry_process = process.clone();
    let retry = std::thread::spawn(move || {
        retry_process.test_retry_process_release(Duration::from_secs(8))
    });
    let retry_started = waiting.recv_timeout(Duration::from_secs(2)).is_ok();
    let span_held = span.lock().unwrap().is_some();
    let lock_held = !available(&path);
    let pending = process.test_retry_process_release(Duration::ZERO).0 == "pending";
    if !retry_started {
        span.lock().unwrap().take();
    }
    release.notify_one();
    let result = retry.join().unwrap();
    let lock_available = available(&path);
    let artifact = serde_json::json!({"scope":"actual Engine, retained closer, accepted diagnostic span, local telemetry receiver and independent host lock", "engine_suspended":suspended, "initial_process_release":initial, "first_engine_shutdown_failed":first_failed, "after_engine_drop":after_drop, "retry_started":retry_started, "span_held_during_retry":span_held, "host_lock_held_during_retry":lock_held, "retry_wait_pending":pending, "retry_result":result.0, "prior_failure_preserved":result.1, "host_lock_available":lock_available, "close_calls":calls.load(Ordering::SeqCst), "device_acceptance":"not established"});
    evidence("orphan-during-span-drain", artifact);
    assert!(
        suspended
            && first_failed
            && initial == "pending"
            && after_drop == "failed"
            && retry_started
            && span_held
            && lock_held
            && pending
            && result == ("released", true)
            && lock_available
            && calls.load(Ordering::SeqCst) == 2
    );
}

#[test]
fn pending_orphan_during_span_drain_waits_for_closer() {
    use uc_observability_contract::diagnostics::{
        operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
        OperationContext,
    };
    use uc_observability_runtime::{
        DeploymentEnvironment, NetworkLogBudget, NetworkLogConfig, NetworkProcessRole,
        ObservabilityConfig, ObservabilityResource, OperatingSystem, OtlpHttpConfig,
        ProcessObservabilityRuntime,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    const CHILD: &str = "UC_TEST_PENDING_ORPHAN_SPAN_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_release::pending_orphan_during_span_drain_waits_for_closer",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()
        .unwrap();
    let receiver = executor.block_on(async {
        let receiver = MockServer::start().await;
        for route in ["/v1/traces", "/v1/logs"] {
            Mock::given(method("POST"))
                .and(path(route))
                .respond_with(ResponseTemplate::new(200))
                .mount(&receiver)
                .await;
        }
        receiver
    });
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .unwrap();
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .unwrap(),
    )
    .with_remote(remote)
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
    let path = root.path().join("host.lease");
    let lease = File::create(&path).unwrap();
    lease.lock().unwrap();
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    span.record("uc.outcome", "ok");
    span.record("otel.status_code", "OK");
    let span = Arc::new(std::sync::Mutex::new(Some(span)));
    let calls = Arc::new(AtomicUsize::new(0));
    let (started, waiting) = std::sync::mpsc::channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.path().join("private"),
            root.path().join("cache"),
            root.path().join("temp"),
            root.path().join("logs"),
        ),
        Box::new(MemorySecureStorage::default()),
        Box::new(SpanHoldingProcessClipboard(Some(
            SpanHoldingProcessStream {
                lease: Some(lease),
                calls: Arc::clone(&calls),
                span: Arc::clone(&span),
                started,
                release: Arc::clone(&release),
                failures_before_success: 0,
            },
        ))),
        Box::new(EmptyFiles),
    );
    let (engine, _) = executor
        .block_on(Engine::start(EngineConfig::new("2.0.0"), host))
        .unwrap();
    let suspended = executor.block_on(engine.suspend()).is_ok();
    let initial = process.test_process_release(Duration::from_millis(300));
    let engine_wait = executor.block_on(engine.shutdown_with_outcome(Duration::from_secs(1)));
    let engine_pending = matches!(engine_wait, uc_engine::ResourceReleaseOutcome::Pending);
    let closer_started = waiting.recv_timeout(Duration::from_secs(3)).is_ok();
    drop(engine);
    let after_drop = process.test_process_release(Duration::from_millis(300));
    let span_held = span.lock().unwrap().is_some();
    let lock_held = !available(&path);
    release.notify_one();
    let result = process.test_process_release(Duration::from_secs(10));
    let lock_available = available(&path);
    let artifact = serde_json::json!({"scope":"actual suspended Engine, accepted host close, diagnostic span and local telemetry receiver", "engine_suspended":suspended, "initial_process_release":initial, "engine_wait_pending":engine_pending, "engine_wait_result":format!("{engine_wait:?}"), "closer_started":closer_started, "process_after_drop":after_drop, "span_held":span_held, "host_lock_held":lock_held, "process_result":result, "host_lock_available":lock_available, "close_calls":calls.load(Ordering::SeqCst), "device_acceptance":"not established"});
    evidence("pending-orphan-span-drain", artifact);
    assert!(
        suspended
            && initial == "pending"
            && engine_pending
            && closer_started
            && after_drop == "pending"
            && span_held
            && lock_held
            && result == "released"
            && lock_available
            && calls.load(Ordering::SeqCst) == 1
    );
}
