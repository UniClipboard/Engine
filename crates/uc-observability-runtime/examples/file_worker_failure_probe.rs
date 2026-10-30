//! 隔离验证文件线程终止后的已接受停车请求。真实锁不代表完整进程资源范围。
#![allow(dead_code)]

#[path = "../src/file_statistics.rs"]
mod file_statistics;
#[path = "../src/local_file.rs"]
mod local_file;

mod remote_health {
    pub(crate) fn record_local_file_failed(_: &'static str) {}
    pub(crate) fn record_local_file_recovered() {}
}

use std::fs::File;
use std::io;
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::time::Duration;

use local_file::{FileQueue, HealthHooks, RecordCounters, RecordStore};
use uc_observability_runtime::{SignalResult, LOCAL_LOG_MAX_BYTES, LOCAL_LOG_RETENTION_DAYS};

struct ControlledStore {
    _lease: File,
    entered: mpsc::Sender<()>,
    release: mpsc::Receiver<()>,
}

impl RecordStore for ControlledStore {
    fn store_record(&mut self, _: &[u8], _: usize) -> io::Result<()> {
        Ok(())
    }

    fn settle(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn park(&mut self) -> io::Result<()> {
        self.entered.send(()).expect("signal entry");
        self.release.recv().expect("release worker");
        panic!("controlled file worker termination");
    }
}

struct Counters;
impl RecordCounters for Counters {
    fn accepted(&self, _: usize) {}
    fn queue_dropped(&self, _: usize) {}
    fn write_failed(&self, _: usize) {}
}

fn main() {
    let root = tempfile::tempdir().expect("temporary root");
    let lease_path = root.path().join("controlled.lease");
    let lease = File::create(&lease_path).expect("create lease");
    lease.lock().expect("hold lease");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let (accepted_tx, accepted_rx) = mpsc::channel();
    let queue = FileQueue::spawn(
        ControlledStore {
            _lease: lease,
            entered: entered_tx,
            release: release_rx,
        },
        Arc::new(Counters),
        HealthHooks {
            failed: |_| {},
            recovered: || {},
        },
        "controlled-file-worker",
        8,
    )
    .expect("start queue");
    let result = queue.park_with_observer(
        Duration::from_millis(50),
        Box::new(move |success| {
            let _ = result_tx.send(success);
        }),
        Box::new(move || {
            let _ = accepted_tx.send(());
        }),
    );
    accepted_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("accepted");
    entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("entered");
    release_tx.send(()).expect("release");
    let acknowledgement = result_rx.recv_timeout(Duration::from_secs(2));
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let probe = loop {
        let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
            .arg(&lease_path).status().expect("probe lease");
        if status.success() || std::time::Instant::now() >= deadline {
            break status;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(matches!(probe.code(), Some(0 | 1)), "probe failed");
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("file-worker-failure.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "scope": "isolated production file queue, controlled store panic, independent file lock probe",
                "caller_timed_out": result == SignalResult::TimedOut,
                "terminal_failure_observed": matches!(acknowledgement, Ok(false)),
                "worker_lock_available": probe.success(),
                "complete_process_release": "not established"
            })).expect("serialize")).expect("write evidence");
    }
    assert_eq!(result, SignalResult::TimedOut);
    assert!(
        matches!(acknowledgement, Ok(false)),
        "accepted request must report worker termination"
    );
    assert!(
        probe.success(),
        "controlled worker lock must be released after termination"
    );
}
