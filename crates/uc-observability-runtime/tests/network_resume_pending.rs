//! 失败清单：已接受的恢复在等待超时后完成时，状态不能长期停留在未确认。
#![cfg(unix)]

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;
use std::time::{Duration, Instant};

use uc_observability_runtime::{
    DeploymentEnvironment, EngineLogActivity, NetworkLogBudget, NetworkLogConfig, NetworkLogState,
    NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    ProcessObservabilityRuntime, SignalResult,
};

#[test]
fn accepted_resume_publishes_its_late_result() {
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
    let (mut first, _) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(
        first.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let cleanup = root.path().join("network/.cleanup.lock");
    std::fs::remove_file(&cleanup).expect("remove cleanup lock");
    assert!(Command::new("mkfifo")
        .arg(&cleanup)
        .status()
        .expect("mkfifo")
        .success());
    let (mut first_resume, first_result) = EngineLogActivity::begin(Duration::from_millis(50));
    let parked_pending = first_resume.end(Duration::from_millis(50));
    let blocked = root.path().join("network/blocked.fifo");
    std::fs::rename(&cleanup, &blocked).expect("retain blocked FIFO");
    std::fs::write(&cleanup, []).expect("restore regular cleanup lock");
    let (mut resumed, result) = EngineLogActivity::begin(Duration::from_millis(50));
    let pending = handle.query_network_log_status().expect("status").state;
    let reader = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&blocked)
        .expect("release FIFO");
    let deadline = Instant::now() + Duration::from_secs(2);
    let late = loop {
        let state = handle.query_network_log_status().expect("status").state;
        if state == NetworkLogState::Ready || Instant::now() >= deadline {
            break state;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let export = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export");
    let lease = export.run_directory.join("lease.lock");
    let probe = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(&lease).status().expect("probe lease");
    assert!(matches!(probe.code(), Some(0 | 1)), "lease probe failed");
    let lease_held = !probe.success();
    let park = resumed.end(Duration::from_secs(5));
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("create evidence directory");
        std::fs::write(
            std::path::Path::new(&directory).join("network-resume-pending.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope": "real network file worker with controlled FIFO delay",
                "resume_timed_out": result == Some(SignalResult::TimedOut),
                "first_resume_timed_out": first_result == Some(SignalResult::TimedOut),
                "intervening_park_timed_out": parked_pending == Some(SignalResult::TimedOut),
                "pending_unavailable": pending == NetworkLogState::Unavailable,
                "late_ready_without_new_request": late == NetworkLogState::Ready,
                "independent_probe_observed_lease_held": lease_held,
                "subsequent_park_completed": park == Some(SignalResult::Completed)
            }))
            .expect("serialize evidence"),
        )
        .expect("write evidence");
    }
    drop(reader);
    assert_eq!(first_result, Some(SignalResult::TimedOut));
    assert_eq!(parked_pending, Some(SignalResult::TimedOut));
    assert_eq!(result, Some(SignalResult::TimedOut));
    assert_eq!(pending, NetworkLogState::Unavailable);
    assert_eq!(
        late,
        NetworkLogState::Ready,
        "accepted resume must publish its late result"
    );
    assert!(lease_held, "late resume must acquire the real lease");
    assert_eq!(park, Some(SignalResult::Completed));
    let _ = handle.shutdown(Duration::from_secs(5));
}
