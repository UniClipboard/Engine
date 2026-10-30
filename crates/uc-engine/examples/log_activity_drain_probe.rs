//! 隔离验证未完成的网络登记与清理顺序；不代表完整 Engine 的释放回执。

#[path = "../src/engine/log_activity.rs"]
mod log_activity;

use std::future::{poll_fn, Future};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::task::Poll;
use std::time::{Duration, Instant};

use log_activity::LogActivity;
use uc_engine::{EngineError, EngineErrorCategory};
use uc_observability_runtime::{
    DeploymentEnvironment, EngineLogActivity, NetworkLogBudget, NetworkLogConfig, NetworkLogState,
    NetworkProcessRole, ObservabilityConfig, ObservabilityResource, OperatingSystem,
    ProcessObservabilityRuntime, SignalResult,
};

fn operation_cancelled_error() -> EngineError {
    EngineError::new(1002, EngineErrorCategory::DeadlineExceeded, true)
}

fn main() {
    let root = tempfile::tempdir().expect("temporary root");
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
        NetworkLogConfig::new(
            root.path(),
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
        .expect("network config"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    let (mut first, _) = EngineLogActivity::begin(Duration::from_secs(5));
    assert_eq!(
        first.end(Duration::from_secs(5)),
        Some(SignalResult::Completed)
    );
    let fifo = root.path().join("network/.cleanup.lock");
    std::fs::remove_file(&fifo).expect("remove lock");
    assert!(Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo")
        .success());
    let (mut blocker, _) = EngineLogActivity::begin(Duration::from_millis(50));
    assert_eq!(
        blocker.end(Duration::from_millis(50)),
        Some(SignalResult::TimedOut)
    );
    let blocked = root.path().join("network/blocked.fifo");
    std::fs::rename(&fifo, &blocked).expect("rename FIFO");
    std::fs::write(&fifo, []).expect("restore lock");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let activity = LogActivity::default();
        let starter = activity.clone();
        let begin = tokio::spawn(async move { starter.begin().await });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!begin.is_finished(), "registration must be blocked");
        begin.abort();
        let (settled, settlement) = std::sync::mpsc::channel();
        let observer_registered = activity.on_beginning_settled(move || { settled.send(()).expect("settlement"); });
        let settled_early = settlement.try_recv().is_ok();
        let closer = activity.clone();
        let mut end = Box::pin(async move { closer.end().await });
        let ended_early = poll_fn(|context| Poll::Ready(end.as_mut().poll(context).is_ready())).await;
        let later = activity.clone();
        let mut later_begin = tokio::spawn(async move { later.begin().await });
        let mut reader = Command::new("python3")
            .args(["-c", "import os,sys\nf=os.open(sys.argv[1], os.O_RDONLY | os.O_NONBLOCK)\nprint('ready',flush=True)\nsys.stdin.read(1)\nos.close(f)"])
            .arg(&blocked).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().expect("reader");
        let mut ready = String::new();
        BufReader::new(reader.stdout.take().expect("stdout")).read_line(&mut ready).expect("reader ready");
        assert_eq!(ready.trim(), "ready");
        let later_early = tokio::time::timeout(Duration::from_millis(300), &mut later_begin).await.is_ok();
        if ended_early {
            let deadline = Instant::now() + Duration::from_secs(5);
            while handle.query_network_log_status().expect("status").state != NetworkLogState::Ready {
                assert!(Instant::now() < deadline, "reopen did not complete");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            activity.end().await.expect("cleanup after baseline");
        } else {
            tokio::time::timeout(Duration::from_secs(5), end).await.expect("cleanup timeout").expect("cleanup");
        }
        if !later_early {
            tokio::time::timeout(Duration::from_secs(5), later_begin).await.expect("later begin timeout").expect("later begin task").expect("later begin");
        }
        let settled_after = settlement.recv_timeout(Duration::from_secs(5)).is_ok();
        let extra_settlement = settlement.try_recv().is_ok();
        let held_probe = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
            .arg(&lease).status().expect("probe active lease");
        activity.end().await.expect("close later registration");
        let deadline = Instant::now() + Duration::from_secs(5);
        while activity.on_beginning_settled(|| {}) {
            assert!(Instant::now() < deadline, "later registration did not settle");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let late_observer = activity.on_beginning_settled(|| panic!("completed registration must not accept observer"));
        let probe = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
            .arg(&lease).status().expect("probe lease");
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).expect("evidence directory");
            std::fs::write(std::path::Path::new(&directory).join("log-activity-drain.json"),
                serde_json::to_vec_pretty(&serde_json::json!({"scope":"isolated Engine log activity and real network lease, controlled FIFO", "observer_registered":observer_registered, "settled_before_release":settled_early, "settled_after_release":settled_after, "duplicate_settlement":extra_settlement, "late_observer_registered":late_observer, "cleanup_returned_before_registration":ended_early, "later_registration_returned_before_cleanup":later_early, "later_registration_lease_held":!held_probe.success(), "lease_available_after_cleanup":probe.success(), "complete_startup_release":"not established"})).expect("serialize")).expect("write evidence");
        }
        reader.stdin.take().expect("stdin").write_all(b"x").expect("stop reader");
        assert!(reader.wait().expect("reader exit").success());
        assert!(observer_registered && !settled_early && settled_after && !extra_settlement && !late_observer, "accepted registration must notify settlement exactly once");
        assert!(!ended_early, "cleanup must wait for an accepted registration");
        assert!(!later_early, "later registration must wait for cleanup");
        assert!(!held_probe.success(), "later registration must retain its lease");
        assert!(probe.success(), "cleanup must release the real lease");
    });
}
