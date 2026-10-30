//! 受控备份工作结算与真实租约验证；不代表完整启动回执。
#![allow(dead_code)]
#[path = "../../uc-infra-profile/src/security/profile_upgrade_backup/activity.rs"]
mod activity;
use activity::ProfileUpgradeBackupActivity;
use std::fs::File;
use std::path::Path;
use std::process::Command;
use std::sync::{mpsc, Arc};
use std::time::Duration;

fn available(path: &Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(path).status().expect("lock probe");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

fn main() {
    let root = tempfile::tempdir().expect("root");
    let path = root.path().join("lease");
    let lease = File::create(&path).expect("lease");
    lease.lock().expect("lock");
    let activity = Arc::new(ProfileUpgradeBackupActivity::default());
    let first = activity.begin();
    let second = activity.begin();
    let (sent, received) = mpsc::channel();
    let registered = activity.on_idle(move || {
        sent.send(()).expect("settled");
    });
    first
        .finish(Ok::<(), uc_application::deps::ProfileUpgradeBackupError>(()))
        .expect("first worker");
    let first_pending = received.recv_timeout(Duration::from_millis(100)).is_err();
    let held = !available(&path);
    drop(lease);
    second
        .finish(Ok::<(), uc_application::deps::ProfileUpgradeBackupError>(()))
        .expect("second worker");
    let completed = received.recv_timeout(Duration::from_secs(2)).is_ok();
    let duplicate = received.try_recv().is_ok();
    let free = available(&path);
    let after = activity.on_idle(|| panic!("idle observer must not be accepted"));
    let queued = activity.begin();
    let (queued_sent, queued_wait) = mpsc::channel();
    let queued_registered = activity.on_idle(move || {
        queued_sent.send(()).expect("queued settled");
    });
    drop(queued);
    let queued_settled = queued_wait.recv_timeout(Duration::from_secs(2)).is_ok();
    let queued_abnormal =
        activity.first_abnormal() == Some(activity::BackupWorkerAbnormal::Abandoned);
    let panic_activity = Arc::new(ProfileUpgradeBackupActivity::default());
    let panicking = panic_activity.begin();
    let (panic_sent, panic_wait) = mpsc::channel();
    let panic_registered = panic_activity.on_idle(move || {
        panic_sent.send(()).expect("panic settled");
    });
    let task = std::thread::spawn(move || {
        let _guard = panicking;
        panic!("controlled backup worker panic");
    });
    let panicked = task.join().is_err();
    let panic_settled = panic_wait.recv_timeout(Duration::from_secs(2)).is_ok();
    let panic_abnormal =
        panic_activity.first_abnormal() == Some(activity::BackupWorkerAbnormal::Panicked);
    let failures = Arc::new(ProfileUpgradeBackupActivity::default());
    let failing = failures.begin();
    let source = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "controlled");
    let caller_error = failing
        .finish::<()>(Err(uc_application::deps::ProfileUpgradeBackupError {
            source: source.into(),
        }))
        .expect_err("worker error");
    let retained_error = failures.first_failure().expect("retained error");
    let caller_source =
        uc_observability_contract::error_source::find_source::<std::io::Error>(&caller_error)
            .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied);
    let retained_source = uc_observability_contract::error_source::find_source::<std::io::Error>(
        retained_error.as_ref(),
    )
    .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied);

    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("backup-activity.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production backup activity, controlled workers, independent file-lock probe", "observer_registered":registered, "pending_after_one_of_two":first_pending, "lock_held":held, "completed":completed, "duplicate":duplicate, "lock_available":free, "idle_observer_registered":after, "queued_observer_registered":queued_registered, "queued_settled":queued_settled, "queued_abnormal_recorded":queued_abnormal, "panic_observer_registered":panic_registered, "worker_panicked":panicked, "panic_settled":panic_settled, "panic_abnormal_recorded":panic_abnormal, "caller_source_retained":caller_source, "owner_source_retained":retained_source, "full_startup_release":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        registered
            && first_pending
            && held
            && completed
            && !duplicate
            && free
            && !after
            && queued_registered
            && queued_settled
            && queued_abnormal
            && panic_registered
            && panicked
            && panic_settled
            && panic_abnormal
            && caller_source
            && retained_source
    );
}
