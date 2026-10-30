//! 真实日文件的停车与恢复探测；不代表完整进程释放回执。
#![allow(dead_code)]

#[path = "../src/file_statistics.rs"]
mod file_statistics;
#[path = "../src/local_file.rs"]
mod local_file;

mod remote_health {
    pub(crate) fn record_local_file_failed(_: &'static str) {}
    pub(crate) fn record_local_file_recovered() {}
}

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use file_statistics::LocalDiagnosticSource;
use local_file::{managed_log_files, LocalFileRuntime};
use uc_observability_runtime::{SignalResult, LOCAL_LOG_MAX_BYTES, LOCAL_LOG_RETENTION_DAYS};

#[cfg(target_os = "macos")]
fn descriptor_open(path: &Path) -> bool {
    let output = Command::new("lsof")
        .args(["-n", "-P", "-F", "n", "-p", &std::process::id().to_string()])
        .output()
        .expect("descriptor probe");
    assert!(output.status.success(), "descriptor probe failed");
    let canonical = path.canonicalize().expect("canonical file");
    String::from_utf8(output.stdout)
        .expect("probe output")
        .lines()
        .any(|line| line.strip_prefix('n') == canonical.to_str())
}

#[cfg(target_os = "macos")]
fn main() {
    let root = tempfile::tempdir().expect("temporary root");
    let logs = root.path().join("logs");
    let daily = LocalFileRuntime::new(&logs).expect("daily queue");
    let writer = daily.writer();
    writer.write_record(b"first\n", LocalDiagnosticSource::Runtime);
    assert_eq!(
        daily.flush_result(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let file = managed_log_files(&logs)
        .expect("managed files")
        .into_iter()
        .next()
        .expect("daily file");
    let initially_open = descriptor_open(&file);
    let first_park = daily.park(Duration::from_secs(5));
    let closed_after_park = !descriptor_open(&file);
    writer.write_record(b"late\n", LocalDiagnosticSource::Runtime);
    let late_flush = daily.flush_result(Duration::from_secs(5));
    let closed_after_late_work = !descriptor_open(&file);
    let parked_directory = root.path().join("parked-logs");
    std::fs::rename(&logs, &parked_directory).expect("move parked directory");
    std::fs::write(&logs, b"block directory").expect("block reopening");
    let failed_resume = daily.unpark(Duration::from_secs(5));
    std::fs::remove_file(&logs).expect("remove reopening block");
    std::fs::rename(&parked_directory, &logs).expect("restore logs");
    let flush_after_failure = daily.flush_result(Duration::from_secs(5));
    let closed_after_failure = !descriptor_open(&file);
    let resume = daily.unpark(Duration::from_secs(5));
    let reopened = descriptor_open(&file);
    let last_park = daily.park(Duration::from_secs(5));
    let finally_closed = !descriptor_open(&file);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("daily-file-park.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"scope":"production daily-file queue, real managed file and independent lsof probe", "initially_open":initially_open, "first_park_completed":first_park == SignalResult::Completed, "closed_after_park":closed_after_park, "late_flush_completed":late_flush == SignalResult::Completed, "closed_after_late_record_and_flush":closed_after_late_work, "failed_resume_reported":failed_resume == SignalResult::Failed, "flush_after_failure_completed":flush_after_failure == SignalResult::Completed, "closed_after_failed_resume_and_flush":closed_after_failure, "resume_completed":resume == SignalResult::Completed, "reopened_after_resume":reopened, "last_park_completed":last_park == SignalResult::Completed, "closed_after_last_park":finally_closed, "complete_process_release":"not established"})).expect("serialize")).expect("write evidence");
    }
    assert!(
        initially_open
            && closed_after_park
            && closed_after_late_work
            && closed_after_failure
            && reopened
            && finally_closed
    );
    assert_eq!(first_park, SignalResult::Completed);
    assert_eq!(late_flush, SignalResult::Completed);
    assert_eq!(failed_resume, SignalResult::Failed);
    assert_eq!(flush_after_failure, SignalResult::Completed);
    assert_eq!(resume, SignalResult::Completed);
    assert_eq!(last_park, SignalResult::Completed);
    assert!(daily.shutdown(Duration::from_secs(5)));
}

#[cfg(not(target_os = "macos"))]
fn main() {
    panic!("daily descriptor probe requires macOS lsof");
}
