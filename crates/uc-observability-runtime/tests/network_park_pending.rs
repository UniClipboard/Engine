//! 失败清单：停车或终止关闭仍在等待时，状态不得报告文件已经关闭。
#![cfg(unix)]

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;
use std::time::Duration;

use uc_observability_runtime::{
    DeploymentEnvironment, EngineLogActivity, NetworkLogBudget, NetworkLogConfig, NetworkLogState,
    NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    ProcessObservabilityRuntime, SignalResult,
};

#[test]
fn pending_park_does_not_claim_the_network_writer_is_closed() {
    let root = tempfile::tempdir().expect("temporary root");
    let budget = NetworkLogBudget {
        record_bytes: 1024,
        queue_records: 32,
        file_bytes: 4096,
        run_bytes: 8192,
        root_bytes: 16384,
        retention: Duration::from_secs(60),
    };
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_network_logs(
        NetworkLogConfig::new(root.path(), NetworkProcessRole::MobileApp, budget).expect("config"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (mut initial, _) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(
        initial.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let cleanup = root.path().join("network/.cleanup.lock");
    std::fs::remove_file(&cleanup).expect("remove cleanup lock");
    assert!(Command::new("mkfifo")
        .arg(&cleanup)
        .status()
        .expect("mkfifo")
        .success());
    let (mut resumed, resume) = EngineLogActivity::begin(Duration::from_millis(50));
    let pending = resumed.end(Duration::from_millis(50));
    let repeated = resumed.end(Duration::ZERO);
    let state = handle.query_network_log_status().expect("status").state;
    // 先释放受控阻塞，再检查断言，避免失败时遗留阻塞的文件线程。
    let reader = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&cleanup)
        .expect("release FIFO");
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let late_park_state = loop {
        let current = handle.query_network_log_status().expect("status").state;
        if current == NetworkLogState::Closed || std::time::Instant::now() >= deadline {
            break current;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let completed = resumed.end(Duration::from_secs(5));
    drop(reader);
    assert_eq!(resume, Some(SignalResult::TimedOut));
    assert_eq!(pending, Some(SignalResult::TimedOut));
    assert_eq!(repeated, Some(SignalResult::TimedOut));
    assert_eq!(
        state,
        NetworkLogState::Unavailable,
        "pending park must not report closed"
    );
    assert_eq!(
        late_park_state,
        NetworkLogState::Closed,
        "accepted parking must publish its eventual acknowledgement without another request"
    );
    assert_eq!(completed, Some(SignalResult::Completed));
    assert_eq!(
        handle.query_network_log_status().expect("status").state,
        NetworkLogState::Closed
    );
    let (late, second_resume) = EngineLogActivity::begin(Duration::from_millis(50));
    let _ = handle.shutdown(Duration::from_millis(50));
    std::thread::sleep(Duration::from_millis(100));
    let closing_state = handle.query_network_log_status().expect("status").state;
    let shutdown_reader = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&cleanup)
        .expect("release shutdown FIFO");
    drop(late);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let settled_state = loop {
        let current = handle.query_network_log_status().expect("status").state;
        if current == NetworkLogState::Closed || std::time::Instant::now() >= deadline {
            break current;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(second_resume, Some(SignalResult::TimedOut));
    assert_eq!(
        closing_state,
        NetworkLogState::Unavailable,
        "pending terminal shutdown must not report closed"
    );
    assert_eq!(
        settled_state,
        NetworkLogState::Closed,
        "the accepted terminal shutdown must retain its eventual acknowledgement"
    );
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        let result = serde_json::json!({"scenario":"network_park_pending", "scope":"real file worker with controlled FIFO delay", "resume":"timed_out", "park":"timed_out", "repeat_wait":"timed_out", "pending_status":"unavailable", "late_park_without_new_request":"closed", "completed_park":"closed", "pending_shutdown_status":"unavailable", "eventual_shutdown_status":"closed"});
        std::fs::write(
            std::path::Path::new(&directory).join("network-park-pending.json"),
            serde_json::to_vec_pretty(&result).expect("serialize"),
        )
        .expect("write evidence");
    }
    drop(shutdown_reader);
}
