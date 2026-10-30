//! 失败清单：网络刷新仍在等待时，另一轮刷新不能越过同一个生命周期负责人。
#![cfg(unix)]

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use uc_observability_runtime::{
    DeploymentEnvironment, EngineLogActivity, NetworkLogBudget, NetworkLogConfig,
    NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    ProcessObservabilityRuntime, SignalResult,
};

#[test]
fn overlapping_flush_waits_for_the_network_phase() {
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
        NetworkLogConfig::new(root.path(), NetworkProcessRole::MobileApp, budget)
            .expect("network configuration"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (mut initial, _) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(
        initial.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let fifo = root.path().join("network/.cleanup.lock");
    std::fs::remove_file(&fifo).expect("remove lock");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo")
        .success());
    let (mut active, resume) = EngineLogActivity::begin(Duration::from_millis(50));
    assert_eq!(resume, Some(SignalResult::TimedOut));
    let first_handle = handle.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    let first = std::thread::spawn(move || {
        entered_tx.send(()).expect("signal first caller");
        first_handle.force_flush(Duration::from_secs(3))
    });
    entered_rx.recv().expect("first caller started");
    std::thread::sleep(Duration::from_millis(200));
    let second = handle.force_flush(Duration::from_millis(50));
    let reader = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .expect("release FIFO");
    let first = first.join().expect("first caller joined");
    let parked = active.end(Duration::from_secs(5));
    drop(reader);
    let (mut late, late_resume) = EngineLogActivity::begin(Duration::from_millis(50));
    let business_summary = handle.force_flush(Duration::from_millis(100));
    std::thread::sleep(Duration::from_millis(100));
    let after_timeout = handle.force_flush(Duration::from_millis(50));
    let late_reader = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&fifo)
        .expect("release second FIFO wait");
    let late_park = late.end(Duration::from_secs(5));
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("network-flush-coordination.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope": "real process runtime and network queue, controlled FIFO delay",
                "second_flush_timed_out": second.traces == SignalResult::TimedOut && second.logs == SignalResult::TimedOut,
                "first_flush_completed": first.traces == SignalResult::Completed && first.logs == SignalResult::Completed,
                "subsequent_park_completed": parked == Some(SignalResult::Completed),
                "business_summary_preserved_after_network_timeout": business_summary.traces == SignalResult::Completed && business_summary.logs == SignalResult::Completed,
                "pending_flush_still_owns_lifecycle": after_timeout.traces == SignalResult::TimedOut && after_timeout.logs == SignalResult::TimedOut,
                "second_resume_timed_out": late_resume == Some(SignalResult::TimedOut),
                "second_park_completed": late_park == Some(SignalResult::Completed)
            })).expect("serialize")).expect("write evidence");
    }
    drop(late_reader);
    assert_eq!(
        second.traces,
        SignalResult::TimedOut,
        "network flush must retain lifecycle ownership"
    );
    assert_eq!(second.logs, SignalResult::TimedOut);
    assert_eq!(first.traces, SignalResult::Completed);
    assert_eq!(first.logs, SignalResult::Completed);
    assert_eq!(parked, Some(SignalResult::Completed));
    assert_eq!(late_resume, Some(SignalResult::TimedOut));
    assert_eq!(business_summary.traces, SignalResult::Completed);
    assert_eq!(business_summary.logs, SignalResult::Completed);
    assert_eq!(
        after_timeout.traces,
        SignalResult::TimedOut,
        "accepted network flush must remain owned after caller timeout"
    );
    assert_eq!(after_timeout.logs, SignalResult::TimedOut);
    assert_eq!(late_park, Some(SignalResult::Completed));
    let _ = handle.shutdown(Duration::from_secs(5));
}
