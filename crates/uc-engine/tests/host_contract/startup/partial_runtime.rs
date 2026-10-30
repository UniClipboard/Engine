//! 构造边界基线：独立进程观察资料文件；文件仍打开不能证明其负责人。
#![cfg(target_os = "macos")]

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, Operation, SecretString, StartupLifecycle,
    StartupProgress,
};
use uc_observability_runtime::{
    DeploymentEnvironment, EngineLogActivity, NetworkLogBudget, NetworkLogConfig, NetworkLogState,
    NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    ProcessObservabilityRuntime, SignalResult,
};

use super::{host, runtime_database, MemorySecureStorage};

fn open_descriptor(path: &std::path::Path) -> bool {
    let output = Command::new("lsof")
        .args(["-n", "-P", "-F", "n", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let path = path.canonicalize().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .any(|line| line.strip_prefix('n') == path.to_str())
}

/// 执行器消失后，晚到的网络登记仍属于未交出的启动负责人。
#[test]
fn executor_loss_retains_late_network_registration() {
    const CHILD: &str = "UC_TEST_LATE_LOG_REGISTRATION_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "startup::partial_runtime::executor_loss_retains_late_network_registration",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let log_root = root.path().join("process-logs");
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
            &log_root,
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
    let observability = ProcessObservabilityRuntime::install(config)
        .unwrap()
        .handle();
    let (mut initial, _) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(
        initial.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let fifo = log_root.join("network/.cleanup.lock");
    std::fs::remove_file(&fifo).unwrap();
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let (mut blocker, blocked_resume) = EngineLogActivity::begin(Duration::from_millis(50));
    let blocked_park = blocker.end(Duration::from_millis(50));
    let blocked_fifo = log_root.join("network/blocked.fifo");
    std::fs::rename(&fifo, &blocked_fifo).unwrap();
    std::fs::write(&fifo, []).unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (progress_input, progress) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let caller = runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(MemorySecureStorage::default())),
        progress_input,
        lifecycle_input,
    ));
    let deadline = Instant::now() + Duration::from_secs(20);
    while progress.snapshot().state != uc_engine::StartupState::StartingServices {
        assert!(
            Instant::now() < deadline && !caller.is_finished(),
            "startup did not reach service construction"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        !caller.is_finished(),
        "startup must be waiting for network reopen"
    );
    runtime.shutdown_timeout(Duration::from_millis(50));
    let mut reader = Command::new("python3")
        .args(["-c", "import os,sys\nf=os.open(sys.argv[1], os.O_RDONLY | os.O_NONBLOCK)\nprint('ready', flush=True)\nsys.stdin.read(1)\nos.close(f)"])
        .arg(&blocked_fifo).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut ready = String::new();
    BufReader::new(reader.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    let deadline = Instant::now() + Duration::from_secs(5);
    while observability.query_network_log_status().unwrap().state != NetworkLogState::Ready {
        assert!(Instant::now() < deadline, "late reopen did not complete");
        std::thread::sleep(Duration::from_millis(10));
    }
    let lease = observability
        .prepare_network_log_export(Duration::from_secs(5))
        .unwrap()
        .run_directory
        .join("lease.lock");
    let (mut other, _) = EngineLogActivity::begin(Duration::from_secs(5));
    let other_end = other.end(Duration::from_secs(5));
    let available = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(&lease).status().unwrap();
    assert!(matches!(available.code(), Some(0 | 1)));
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("late-network-registration.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"scope":"real Engine and network lease, FIFO timing and independent lock probe", "blocked_resume_timed_out":blocked_resume == Some(SignalResult::TimedOut), "blocked_park_timed_out":blocked_park == Some(SignalResult::TimedOut), "other_end_did_not_park":other_end.is_none(), "lease_retained":!available.success(), "retry_allowed":progress.snapshot().allowed_actions.retry})).unwrap()).unwrap();
    }
    reader.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(reader.wait().unwrap().success());
    drop(lifecycle);
    assert_eq!(blocked_resume, Some(SignalResult::TimedOut));
    assert_eq!(blocked_park, Some(SignalResult::TimedOut));
    assert!(
        other_end.is_none(),
        "abandoned startup must retain its network registration"
    );
    assert!(
        !available.success(),
        "another registration must not release the abandoned startup lease"
    );
    assert!(!progress.snapshot().allowed_actions.retry);
}

#[test]
fn executor_loss_during_engine_construction_observes_profile_descriptor() {
    const CHILD: &str = "UC_TEST_PARTIAL_RUNTIME_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "startup::partial_runtime::executor_loss_during_engine_construction_observes_profile_descriptor", "--nocapture"])
            .env(CHILD, "1")
            .status().unwrap();
        assert!(status.success());
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (engine, _) = Engine::start(
            EngineConfig::new("2.0.0"),
            host(root.path(), Box::new(storage.clone())),
        )
        .await
        .unwrap();
        engine
            .execute(Operation::CreateSpace(CreateSpaceInput {
                device_name: Some("partial startup test".into()),
                passphrase: SecretString::new("startup-test-passphrase"),
                passphrase_confirmation: SecretString::new("startup-test-passphrase"),
            }))
            .await
            .unwrap();
        engine.shutdown(Duration::from_secs(15)).await.unwrap();
    });
    let profile_database = runtime_database(
        root.path(),
        "profile-data-generations",
        "v3-payloads/profile.sqlite",
    );
    assert!(!open_descriptor(&profile_database));
    let log_root = root.path().join("process-logs");
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
            &log_root,
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
    let observability = ProcessObservabilityRuntime::install(config)
        .unwrap()
        .handle();
    let (mut activity, _) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(
        activity.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let fifo = log_root.join("network/.cleanup.lock");
    std::fs::remove_file(&fifo).unwrap();
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());

    let (progress_input, progress) = StartupProgress::channel();
    let (lifecycle_input, lifecycle) = StartupLifecycle::channel();
    let caller = runtime.spawn(Engine::start_with_lifecycle(
        EngineConfig::new("2.0.0"),
        host(root.path(), Box::new(storage)),
        progress_input,
        lifecycle_input,
    ));
    let deadline = Instant::now() + Duration::from_secs(20);
    while observability.query_network_log_status().unwrap().state != NetworkLogState::Unavailable {
        assert!(
            Instant::now() < deadline,
            "startup did not reach network reopen"
        );
        assert!(!caller.is_finished(), "startup ended before network reopen");
        std::thread::sleep(Duration::from_millis(10));
    }
    let held_before = open_descriptor(&profile_database);
    runtime.shutdown_timeout(Duration::from_millis(50));
    let mut reader = Command::new("python3")
        .args(["-c", "import os,sys\nf=os.open(sys.argv[1], os.O_RDONLY | os.O_NONBLOCK)\nprint('ready', flush=True)\nsys.stdin.read(1)\nos.close(f)"])
        .arg(&fifo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn().unwrap();
    let mut ready = String::new();
    BufReader::new(reader.stdout.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    let held_after = open_descriptor(&profile_database);
    let retry_allowed = progress.snapshot().allowed_actions.retry;
    drop(progress);
    drop(lifecycle);
    let held_after_observer = open_descriptor(&profile_database);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(std::path::Path::new(&directory).join("partial-runtime-executor-stopped.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"real Engine and profile SQLite file, controlled FIFO delay, independent descriptor probe",
            "descriptor_open_before_executor_stop":held_before, "descriptor_open_after_executor_stop":held_after,
            "descriptor_open_after_observer_drop":held_after_observer, "retry_allowed":retry_allowed,
            "descriptor_owner_verified":false
        })).unwrap()).unwrap();
    }
    reader.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(reader.wait().unwrap().success());
    assert!(
        held_before,
        "real profile database must be open at the test boundary"
    );
    assert!(
        !retry_allowed,
        "unconfirmed partial construction cannot allow retry"
    );
    assert!(
        held_after,
        "baseline must observe the open descriptor after executor loss"
    );
    assert!(
        held_after_observer,
        "baseline must observe the descriptor after observer loss"
    );
}
