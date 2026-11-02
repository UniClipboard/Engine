//! 使用真实进程运行时验证私有释放负责人，不代表移动设备验收。
#![cfg(not(test))]
#![allow(dead_code)]

mod build_source {
    pub(crate) struct BuildSource {
        pub(crate) commit: &'static str,
        pub(crate) state: &'static str,
    }
    pub(crate) fn build_source() -> BuildSource {
        BuildSource {
            commit: "unknown",
            state: "unknown",
        }
    }
}
#[path = "../src/config.rs"]
mod config;
#[path = "../src/engine_activity.rs"]
mod engine_activity;
#[path = "../src/file_statistics.rs"]
mod file_statistics;
#[path = "../src/filter.rs"]
mod filter;
#[path = "../src/host_diagnostics.rs"]
mod host_diagnostics;
#[path = "../src/local_capture.rs"]
mod local_capture;
#[path = "../src/local_file.rs"]
mod local_file;
#[path = "../src/local_log_processor.rs"]
mod local_log_processor;
#[path = "../src/local_recording.rs"]
mod local_recording;
#[path = "../src/module_log.rs"]
mod module_log;
#[path = "../src/module_log_fields.rs"]
mod module_log_fields;
#[path = "../src/network_log/mod.rs"]
mod network_log;
#[path = "../src/offline.rs"]
mod offline;
#[path = "../src/process_census.rs"]
mod process_census;
#[path = "../src/process_identity.rs"]
mod process_identity;
#[path = "../src/process_release.rs"]
mod process_release;
#[path = "../src/remote_health.rs"]
mod remote_health;
#[path = "../src/runtime.rs"]
mod runtime;
#[path = "../src/status.rs"]
mod status;
#[path = "../src/subscriber.rs"]
mod subscriber;
#[path = "../src/subscriber_gate.rs"]
mod subscriber_gate;
pub use offline::{OfflineDiagnosticSnapshot, OfflineHistoryStorage};
#[path = "../src/telemetry.rs"]
mod telemetry;

pub use config::*;
pub use engine_activity::EngineLogActivity;
pub use file_statistics::{FileSourceCounts, LocalDiagnosticSource};
pub use host_diagnostics::{
    HostDiagnosticAction, HostDiagnosticEvent, HostDiagnosticFailure, HostDiagnosticOutcome,
    HostDiagnosticReceipt, HostDiagnosticRecordStatus, HostDiagnosticSource, HostLifecycleState,
    HostNetworkKind,
};
pub use local_capture::{
    CaptureEndReason, DetailedCaptureRequest, LocalCaptureMode, LocalCaptureStatus,
    LocalDiagnosticError, LocalDiagnosticExportReport, LocalDiagnosticStatus, SourceCapability,
    SourceCollection, SourceCoverage, StopCaptureResult,
};
pub use local_file::{managed_log_files, migrate_managed_log_files};
pub use module_log::ModuleLogCounts;
pub use network_log::{
    NetworkCaptureMode, NetworkCaptureStatus, NetworkLibraryStatus, NetworkLogBudget,
    NetworkLogConfig, NetworkLogCounters, NetworkLogError, NetworkLogExportReport, NetworkLogFile,
    NetworkLogState, NetworkLogStatus, NetworkProcessRole, NETWORK_LOG_SCHEMA_VERSION,
};
pub use runtime::*;
pub use status::*;
pub use subscriber::HostLogLayer;

use process_release::{
    ProcessFailureKind, ProcessReleaseErrors, ProcessReleaseResult, ProcessResumeResult,
};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use uc_observability_contract::uc_warn;

fn descriptor_open(path: &Path) -> bool {
    let output = Command::new("lsof")
        .args(["-n", "-P", "-F", "n", "-p", &std::process::id().to_string()])
        .output()
        .expect("lsof");
    assert!(output.status.success());
    let path = path.canonicalize().expect("canonical file");
    String::from_utf8(output.stdout)
        .expect("probe output")
        .lines()
        .any(|line| line.strip_prefix('n') == path.to_str())
}

fn lease_available(path: &Path) -> bool {
    let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"])
        .arg(path).status().expect("probe lease");
    assert!(matches!(status.code(), Some(0 | 1)));
    status.success()
}

#[cfg(feature = "test-process-release")]
fn generation_exhaustion_probe() {
    use process_release::{ProcessReleaseCoordinator, ReleaseStart, ResumeStart};
    let coordinator = ProcessReleaseCoordinator::probe_active_generation(u64::MAX - 1);
    let (generation, observation) = match coordinator.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("last release was not accepted"),
    };
    let last_pending = observation.wait(Duration::ZERO);
    let exhausted_resume_failed = match coordinator.begin_resume() {
        ResumeStart::Existing {
            observation: Some(result),
        } => matches!(
            result.wait(Duration::ZERO),
            ProcessResumeResult::Failed {
                generation: u64::MAX
            }
        ),
        _ => false,
    };
    let last_still_pending = observation.wait(Duration::ZERO) == last_pending;
    coordinator.finish_release(
        generation,
        true,
        false,
        false,
        ProcessReleaseErrors::default(),
    );
    let last = observation.wait(Duration::ZERO);
    let retained = coordinator.invalidate_for_engine_retention(true);
    let after_retention = coordinator.wait_release(Duration::ZERO);
    let duplicate = retained
        .as_ref()
        .is_some_and(|(next, _)| *next == generation);

    let earlier = ProcessReleaseCoordinator::probe_active_generation(u64::MAX - 2);
    let (early_generation, early_observation) = match earlier.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("earlier release was not accepted"),
    };
    earlier.finish_release(
        early_generation,
        true,
        false,
        false,
        ProcessReleaseErrors::default(),
    );
    let before = early_observation.wait(Duration::ZERO);
    let resume = earlier.begin_resume();
    let (resume_started, resume_failed) = match resume {
        ResumeStart::Started { .. } => (true, false),
        ResumeStart::Existing { observation } => (
            false,
            observation.is_some_and(|value| {
                matches!(
                    value.wait(Duration::ZERO),
                    ProcessResumeResult::Failed {
                        generation: u64::MAX
                    }
                )
            }),
        ),
    };
    let restart = earlier.invalidate_for_engine_retention(true);
    let pending = earlier.wait_release(Duration::ZERO);
    if let Some((generation, _)) = restart {
        earlier.finish_release(
            generation,
            true,
            false,
            false,
            ProcessReleaseErrors::default(),
        );
    }
    let after = earlier.wait_release(Duration::ZERO);
    let prior_stable = before == early_observation.wait(Duration::ZERO);
    let queued = ProcessReleaseCoordinator::probe_active_generation(u64::MAX - 2);
    let (queued_generation, queued_observation) = match queued.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("queued release not accepted"),
    };
    let rejected_queued_resume = match queued.begin_resume() {
        ResumeStart::Existing {
            observation: Some(result),
        } => matches!(
            result.wait(Duration::ZERO),
            ProcessResumeResult::Failed {
                generation: u64::MAX
            }
        ),
        _ => false,
    };
    let queued_still_pending = matches!(queued_observation.wait(Duration::ZERO), ProcessReleaseResult::Pending { generation } if generation == queued_generation);
    queued.finish_release(
        queued_generation,
        true,
        false,
        false,
        ProcessReleaseErrors::default(),
    );
    let queued_released = matches!(queued_observation.wait(Duration::ZERO), ProcessReleaseResult::Released { generation, .. } if generation == queued_generation);

    let valid = exhausted_resume_failed
        && last_still_pending
        && matches!(
            last_pending,
            ProcessReleaseResult::Pending {
                generation: u64::MAX
            }
        )
        && resume_failed
        && rejected_queued_resume
        && queued_still_pending
        && queued_released
        && matches!(
            last,
            ProcessReleaseResult::Failed {
                generation: u64::MAX
            }
        )
        && !duplicate
        && !resume_started
        && matches!(
            pending,
            ProcessReleaseResult::Pending {
                generation: u64::MAX
            }
        )
        && matches!(
            after,
            ProcessReleaseResult::Failed {
                generation: u64::MAX
            }
        )
        && prior_stable;
    let result = serde_json::json!({"scope":"controlled production coordinator at generation exhaustion", "last":format!("{last:?}"), "duplicate_generation":duplicate, "after_retention":format!("{after_retention:?}"), "earlier_receipt":format!("{before:?}"), "resume_started":resume_started, "resume_failed":resume_failed, "last_pending":format!("{last_pending:?}"), "exhausted_resume_failed":exhausted_resume_failed, "last_still_pending":last_still_pending, "rejected_queued_resume":rejected_queued_resume, "queued_still_pending":queued_still_pending, "queued_released":queued_released, "retention_pending":format!("{pending:?}"), "retention_final":format!("{after:?}"), "prior_stable":prior_stable, "valid":valid});
    if let Some(dir) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&dir).expect("directory");
        std::fs::write(
            Path::new(&dir).join("process-generation-exhaustion.json"),
            serde_json::to_vec_pretty(&result).expect("json"),
        )
        .expect("artifact");
    }
    assert!(
        valid,
        "generation exhaustion must not authorize stale releases"
    );
}

#[cfg(unix)]
fn main() {
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--terminal-admission-deadline") {
        terminal_admission_deadline_probe();
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--release-spawn-error") {
        release_spawn_error_probe();
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--remote-export-error") {
        remote_export_error_probe(false, false);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--remote-export-file-error") {
        remote_export_error_probe(true, false);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--remote-log-export-error") {
        remote_export_error_probe(false, true);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--daily-error-queued") {
        daily_error_queued_probe(false);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--daily-error-queued-spawn-failure") {
        daily_error_queued_probe(true);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--census-shutdown-race") {
        census_shutdown_race_probe();
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--network-resume-held") {
        network_resume_held_probe(false, false, false);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--network-release-after-resume-held") {
        network_resume_held_probe(true, false, false);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--network-retry-with-resume-held") {
        network_resume_held_probe(false, true, false);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--queued-release-spawn-error") {
        network_resume_held_probe(true, false, true);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--network-park-retry") {
        network_park_retry_probe(false);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--both-file-errors") {
        network_park_retry_probe(true);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--daily-park-retry") {
        daily_park_retry_probe(false);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--daily-park-terminal") {
        daily_park_retry_probe(true);
        return;
    }
    #[cfg(feature = "test-process-release")]
    if std::env::args().any(|arg| arg == "--generation-exhaustion") {
        generation_exhaustion_probe();
        return;
    }
    if std::env::args().any(|arg| arg == "--failed-resume") {
        failed_resume_probe();
        return;
    }
    if std::env::args().any(|arg| arg == "--release-during-resume") {
        release_during_resume_probe();
        return;
    }
    if std::env::args().any(|arg| arg == "--release-after-queued-resume") {
        release_after_queued_resume_probe();
        return;
    }
    if std::env::args().any(|arg| arg == "--export-overlap") {
        export_overlap_probe();
        return;
    }
    if std::env::args().any(|arg| arg == "--superseded-resume") {
        superseded_resume_probe();
        return;
    }
    if std::env::args().any(|arg| arg == "--remote-span-overlap") {
        remote_span_overlap_probe(false);
        return;
    }
    if std::env::args().any(|arg| arg == "--lazy-remote-span") {
        remote_span_overlap_probe(true);
        return;
    }
    if std::env::args().any(|arg| arg == "--trace-admission-race") {
        trace_admission_race_probe(false);
        return;
    }
    if std::env::args().any(|arg| arg == "--trace-after-resume") {
        trace_admission_race_probe(true);
        return;
    }
    if std::env::args().any(|arg| arg == "--trace-panic-before") {
        trace_callback_panic_probe(false);
        return;
    }
    if std::env::args().any(|arg| arg == "--trace-panic-after") {
        trace_callback_panic_probe(true);
        return;
    }
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")))
    .with_network_logs(
        NetworkLogConfig::new(
            root.path().join("network-logs"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (mut first, _) = EngineLogActivity::begin(Duration::from_secs(5));
    let (mut second, _) = EngineLogActivity::begin(Duration::from_secs(5));
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    assert!(descriptor_open(&daily) && !lease_available(&lease));
    let initial = handle.release_process_internal(Duration::from_millis(30));
    let (rejected, rejected_result) = EngineLogActivity::begin(Duration::from_millis(30));
    let denied = !rejected.is_admitted() && rejected_result == Some(SignalResult::TimedOut);
    first.end(Duration::from_secs(5));
    let repeated = handle.release_process_internal(Duration::from_millis(30));
    let still_pending = repeated == initial;
    second.end(Duration::from_secs(5));
    let released = handle.release_process_internal(Duration::from_secs(10));
    let closed = !descriptor_open(&daily) && lease_available(&lease);
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "late", "late probe");
    let _ = handle.force_flush(Duration::from_millis(50));
    let still_closed = !descriptor_open(&daily) && lease_available(&lease);
    let resumed = handle.resume_process_internal(Duration::from_secs(10));
    let reopened = descriptor_open(&daily) && !lease_available(&lease);
    let (mut next, _) = EngineLogActivity::begin(Duration::from_secs(5));
    let second_release = handle.release_process_internal(Duration::from_millis(30));
    let queued_resume = handle.resume_process_internal(Duration::ZERO);
    let stale_receipt = handle.observe_process_release_internal(Duration::ZERO);
    let retry_during_resume = handle
        .retry_process_release_observed_internal(Duration::ZERO)
        .0;
    next.end(Duration::from_secs(5));
    let resumed_after_pending = handle.resume_process_internal(Duration::from_secs(10));
    let reopened_again = descriptor_open(&daily) && !lease_available(&lease);
    let before_terminal = handle.release_process_internal(Duration::from_secs(10));
    let saved = root.path().join("saved-daily-file");
    std::fs::rename(&daily, &saved).expect("move daily file");
    assert!(Command::new("mkfifo")
        .arg(&daily)
        .status()
        .expect("mkfifo")
        .success());
    let interrupted_resume = handle.resume_process_internal(Duration::from_millis(50));
    let shutdown_pending = handle.shutdown(Duration::from_millis(50));
    let terminal_while_pending = handle.resume_process_internal(Duration::ZERO);
    let wait_started = std::time::Instant::now();
    let terminal_bounded_wait = handle.resume_process_internal(Duration::from_millis(50));
    let waited_for_pending = wait_started.elapsed() >= Duration::from_millis(25);
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&daily)
        .expect("release blocked file open");
    let limit = std::time::Instant::now() + Duration::from_secs(5);
    let terminal_resume = loop {
        let result = handle.resume_process_internal(Duration::from_millis(50));
        if matches!(result, ProcessResumeResult::Failed { .. })
            || std::time::Instant::now() >= limit
        {
            break result;
        }
    };
    let shutdown = handle.shutdown(Duration::from_secs(5));
    drop(reader);
    std::fs::remove_file(&daily).expect("remove FIFO");
    std::fs::rename(&saved, &daily).expect("restore daily file");
    let repeated_terminal_resume = handle.resume_process_internal(Duration::from_millis(50));
    let limit = std::time::Instant::now() + Duration::from_secs(5);
    while handle
        .query_network_log_status()
        .expect("network status")
        .state
        != NetworkLogState::Closed
        && std::time::Instant::now() < limit
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let terminal_closed = !descriptor_open(&daily) && lease_available(&lease);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-owner.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"private production process runtime, real daily and network files, independent lsof and flock probes; controlled Engine registrations",
            "initial_pending":matches!(initial, ProcessReleaseResult::Pending { .. }), "registration_denied":denied,
            "pending_after_one_end":still_pending, "released":matches!(released, ProcessReleaseResult::Released { .. }),
            "files_closed":closed, "late_work_kept_files_closed":still_closed, "resume_succeeded":matches!(resumed, ProcessResumeResult::Resumed { .. }), "files_reopened":reopened,
            "second_release_pending":matches!(second_release, ProcessReleaseResult::Pending { .. }),
            "resume_queued":matches!(queued_resume, ProcessResumeResult::Pending { .. }),
            "old_receipt_invalidated":matches!(stale_receipt, ProcessReleaseResult::Pending { .. }),
            "retry_observes_current_generation":retry_during_resume == stale_receipt,
            "queued_resume_completed":matches!(resumed_after_pending, ProcessResumeResult::Resumed { .. }), "files_reopened_again":reopened_again,
            "release_before_terminal":matches!(before_terminal, ProcessReleaseResult::Released { .. }),
            "interrupted_resume_pending":matches!(interrupted_resume, ProcessResumeResult::Pending { .. }),
            "shutdown_did_not_report_completed":matches!(shutdown_pending.logs, SignalResult::TimedOut | SignalResult::Failed),
            "terminal_resume_pending_while_work_active":matches!(terminal_while_pending, ProcessResumeResult::Pending { .. }),
            "terminal_pending_wait_honored_deadline":matches!(terminal_bounded_wait, ProcessResumeResult::Pending { .. }) && waited_for_pending,
            "terminal_shutdown_settled":matches!(shutdown.traces, SignalResult::Completed | SignalResult::AlreadyShutdown) && matches!(shutdown.logs, SignalResult::Completed | SignalResult::AlreadyShutdown),
            "terminal_resume_failed":matches!(terminal_resume, ProcessResumeResult::Failed { .. }),
            "terminal_resume_stable":terminal_resume == repeated_terminal_resume,
            "terminal_files_closed":terminal_closed,
            "device_acceptance":"not established"
        })).expect("serialize")).expect("write evidence");
    }
    assert!(matches!(initial, ProcessReleaseResult::Pending { .. }) && denied && still_pending);
    assert!(
        matches!(
            released,
            ProcessReleaseResult::Released {
                operation_failed: false,
                ..
            }
        ) && closed
            && still_closed
            && matches!(resumed, ProcessResumeResult::Resumed { .. })
            && reopened
    );
    assert!(matches!(
        second_release,
        ProcessReleaseResult::Pending { .. }
    ));
    assert!(matches!(queued_resume, ProcessResumeResult::Pending { .. }));
    assert_eq!(retry_during_resume, stale_receipt);
    assert!(matches!(
        stale_receipt,
        ProcessReleaseResult::Pending { .. }
    ));
    if let (
        ProcessReleaseResult::Pending { generation: old },
        ProcessReleaseResult::Pending {
            generation: current,
        },
    ) = (second_release, stale_receipt)
    {
        assert!(
            current > old,
            "queued resume must invalidate the old generation"
        );
    }
    assert!(matches!(resumed_after_pending, ProcessResumeResult::Resumed { .. }) && reopened_again);
    assert!(matches!(
        before_terminal,
        ProcessReleaseResult::Released { .. }
    ));
    assert!(matches!(
        interrupted_resume,
        ProcessResumeResult::Pending { .. }
    ));
    assert!(matches!(
        shutdown_pending.logs,
        SignalResult::TimedOut | SignalResult::Failed
    ));
    assert!(
        matches!(terminal_while_pending, ProcessResumeResult::Pending { .. }),
        "accepted resume must stay pending while its worker is active"
    );
    assert!(
        matches!(terminal_bounded_wait, ProcessResumeResult::Pending { .. }) && waited_for_pending,
        "pending resume must honor the caller wait deadline"
    );
    assert!(matches!(
        shutdown.traces,
        SignalResult::Completed | SignalResult::AlreadyShutdown
    ));
    assert!(matches!(
        shutdown.logs,
        SignalResult::Completed | SignalResult::AlreadyShutdown
    ));
    assert!(
        matches!(terminal_resume, ProcessResumeResult::Failed { .. }),
        "terminal shutdown cannot report a resumed process"
    );
    assert_eq!(terminal_resume, repeated_terminal_resume);
    assert!(
        terminal_closed,
        "terminal shutdown must not be followed by a late reopen"
    );
}

#[cfg(feature = "test-process-release")]
fn release_spawn_error_probe() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")));
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    assert!(descriptor_open(&daily));
    runtime::PROBE_RELEASE_SPAWN_FAILURE.store(true, Ordering::SeqCst);
    let (first, observation) = handle.release_process_observed_internal(Duration::from_secs(5));
    let observation = observation.expect("observation");
    let report = handle.safe_release_report_internal(first, &observation);
    let error = observation.probe_coordinator_error();
    let repeated = handle.release_process_internal(Duration::ZERO);
    let retried = handle
        .retry_process_release_observed_internal(Duration::ZERO)
        .0;
    let held = descriptor_open(&daily);
    let source = error.as_ref().is_some_and(|error| {
        error.kind() == std::io::ErrorKind::WouldBlock
            && error
                .get_ref()
                .and_then(|source| source.downcast_ref::<std::io::Error>())
                .is_some_and(|source| source.kind() == std::io::ErrorKind::PermissionDenied)
    });
    let (registration, result) = EngineLogActivity::begin(Duration::from_millis(20));
    let fenced = !registration.is_admitted() && result == Some(SignalResult::Failed);
    let valid = matches!(first, ProcessReleaseResult::Failed { .. })
        && report.result == first
        && report.cleanup_errors == vec![ProcessFailureKind::ReleaseWorker]
        && !report.explicit_retry_supported
        && first == repeated
        && first == retried
        && held
        && source
        && fenced;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("process-release-spawn-error.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual process runtime and daily file with injected release worker creation failure", "safe_report":format!("{report:?}"), "first":format!("{first:?}"), "repeated":format!("{repeated:?}"), "retry":format!("{retried:?}"), "descriptor_held":held, "source_retained":source, "registration_fenced":fenced, "native_device":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        valid,
        "release spawn failure must retain its source and owner"
    );
}

#[cfg(feature = "test-process-release")]
fn census_shutdown_race_probe() {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().expect("root");
    process_census::PROBE_CENSUS_ENABLED.store(true, Ordering::SeqCst);
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")));
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    let first = handle.release_process_internal(Duration::from_secs(10));
    assert!(matches!(first, ProcessReleaseResult::Released { .. }));
    assert!(!descriptor_open(&daily));
    runtime::PROBE_CENSUS_RESUME_BLOCK.store(true, Ordering::SeqCst);
    let (initial, observation) = handle.resume_process_observed_internal(Duration::from_millis(50));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !runtime::PROBE_CENSUS_RESUME_ENTERED.load(Ordering::SeqCst)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(runtime::PROBE_CENSUS_RESUME_ENTERED.load(Ordering::SeqCst));
    let shutdown = handle.shutdown(Duration::from_secs(5));
    let closed = !descriptor_open(&daily);
    let before = process_census::PROBE_CENSUS_READS.load(Ordering::SeqCst);
    runtime::PROBE_CENSUS_RESUME_BLOCK.store(false, Ordering::SeqCst);
    let resumed = observation
        .expect("resume observation")
        .wait(Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(100));
    let after = process_census::PROBE_CENSUS_READS.load(Ordering::SeqCst);
    let closed_after = !descriptor_open(&daily);
    handle.probe_stop_census();
    let valid = matches!(initial, ProcessResumeResult::Pending { .. })
        && matches!(resumed, ProcessResumeResult::Failed { .. })
        && before > 0
        && shutdown.traces == SignalResult::Completed
        && shutdown.logs == SignalResult::Completed
        && before == after
        && closed
        && closed_after;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("process-census-shutdown-race.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real process runtime and daily file; controlled synthetic census and held resume", "initial_resume":format!("{initial:?}"), "shutdown":format!("{shutdown:?}"), "settled_resume":format!("{resumed:?}"), "reads_before":before, "reads_after":after, "descriptor_closed":closed, "descriptor_closed_after":closed_after, "native_device":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        valid,
        "terminal shutdown must not leave a late census worker"
    );
}

#[cfg(feature = "test-process-release")]
fn network_resume_held_probe(queue_release: bool, retry_release: bool, fail_queued_spawn: bool) {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().expect("root");
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
            root.path().join("network-logs"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    assert!(!lease_available(&lease));
    network_log::PROBE_NETWORK_PARK_BLOCK.store(true, Ordering::SeqCst);
    let release_handle = handle.clone();
    let release = std::thread::spawn(move || {
        release_handle.release_process_observed_internal(Duration::from_secs(10))
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !network_log::PROBE_NETWORK_PARK_ENTERED.load(Ordering::SeqCst)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(network_log::PROBE_NETWORK_PARK_ENTERED.load(Ordering::SeqCst));
    let (release_pending, release_observation) =
        handle.release_process_observed_internal(Duration::ZERO);
    let pending_report = handle.safe_release_report_internal(
        release_pending,
        &release_observation.expect("pending observation"),
    );
    assert!(
        matches!(pending_report.result, ProcessReleaseResult::Pending { .. })
            && pending_report.cleanup_errors.is_empty()
            && pending_report.operation_errors.is_empty()
            && !pending_report.explicit_retry_supported
    );
    let resume_handle = handle.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let resume = std::thread::spawn(move || {
        let result = resume_handle.resume_process_observed_internal(Duration::from_millis(50));
        sender.send(result).expect("send resume");
    });
    let early = receiver.recv_timeout(Duration::from_secs(1));
    if retry_release {
        let retry_handle = handle.clone();
        let (retry_sender, retry_receiver) = std::sync::mpsc::channel();
        let retry = std::thread::spawn(move || {
            retry_sender
                .send(
                    retry_handle
                        .retry_process_release_observed_internal(Duration::from_millis(50))
                        .0,
                )
                .expect("send retry")
        });
        let early_retry = retry_receiver.recv_timeout(Duration::from_secs(1));
        let held = !lease_available(&lease);
        network_log::PROBE_NETWORK_PARK_BLOCK.store(false, Ordering::SeqCst);
        let (released, old) = release.join().expect("release waiter");
        resume.join().expect("resume waiter");
        retry.join().expect("retry waiter");
        let (initial_resume, resume_observation) = early.expect("early resume");
        let returned_early = early_retry.is_ok();
        let retried = early_retry.unwrap_or_else(|_| {
            retry_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("late retry")
        });
        let settled_resume =
            resume_observation.map(|observation| observation.wait(Duration::from_secs(10)));
        let old_result = old.map(|observation| observation.wait(Duration::ZERO));
        let reopened = !lease_available(&lease);
        let final_release = handle.release_process_internal(Duration::from_secs(10));
        let final_closed = lease_available(&lease);
        let valid = returned_early
            && held
            && reopened
            && final_closed
            && old_result == Some(released)
            && matches!((released, retried, initial_resume, settled_resume, final_release), (ProcessReleaseResult::Released { generation: a, .. }, ProcessReleaseResult::Pending { generation: b }, ProcessResumeResult::Pending { generation: c }, Some(ProcessResumeResult::Resumed { generation: d }), ProcessReleaseResult::Released { generation: e, .. }) if a < b && b == c && c == d && d < e);
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).expect("directory");
            std::fs::write(Path::new(&directory).join("process-network-retry-with-resume-held.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real process runtime, network lease, controlled held park", "returned_early":returned_early, "lease_held":held, "first_release":format!("{released:?}"), "retry":format!("{retried:?}"), "resume":format!("{initial_resume:?}"), "resume_settled":format!("{settled_resume:?}"), "old":format!("{old_result:?}"), "lease_reopened":reopened, "final_release":format!("{final_release:?}"), "final_lease_closed":final_closed, "native_device":"not established"})).expect("json")).expect("artifact");
        }
        assert!(
            valid,
            "retry must observe the current generation while resume is queued"
        );
        return;
    }
    if queue_release {
        if fail_queued_spawn {
            runtime::PROBE_RELEASE_SPAWN_FAILURE.store(true, Ordering::SeqCst);
        }
        let queued_handle = handle.clone();
        let (queued_sender, queued_receiver) = std::sync::mpsc::channel();
        let queued = std::thread::spawn(move || {
            let result = queued_handle.release_process_observed_internal(Duration::from_millis(50));
            queued_sender.send(result).expect("send queued release");
        });
        let early_queued = queued_receiver.recv_timeout(Duration::from_secs(1));
        let lease_held = !lease_available(&lease);
        network_log::PROBE_NETWORK_PARK_BLOCK.store(false, Ordering::SeqCst);
        let (released, old) = release.join().expect("release waiter");
        resume.join().expect("resume waiter");
        queued.join().expect("queued release waiter");
        let (initial_resume, resume_observation) = early.expect("early resume");
        let returned_early = early_queued.is_ok();
        let (initial_queued, queued_observation) = match early_queued {
            Ok(result) => result,
            Err(_) => queued_receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("late queued release"),
        };
        let settled_resume =
            resume_observation.map(|observation| observation.wait(Duration::from_secs(10)));
        let queued_observation = queued_observation.expect("queued observation");
        let settled_queued = queued_observation.wait(Duration::from_secs(10));
        let old_result = old.map(|observation| observation.wait(Duration::ZERO));
        let final_closed = lease_available(&lease);
        if fail_queued_spawn {
            let error = queued_observation.probe_coordinator_error();
            let repeated_error = handle
                .release_process_observed_internal(Duration::ZERO)
                .1
                .and_then(|observation| observation.probe_coordinator_error());
            let source = error.as_ref().is_some_and(|error| {
                error.kind() == std::io::ErrorKind::WouldBlock
                    && error
                        .get_ref()
                        .and_then(|source| source.downcast_ref::<std::io::Error>())
                        .is_some_and(|source| source.kind() == std::io::ErrorKind::PermissionDenied)
            });
            let same_source = match (&error, &repeated_error) {
                (Some(first), Some(repeated)) => std::sync::Arc::ptr_eq(first, repeated),
                _ => false,
            };
            let repeated = handle.release_process_internal(Duration::ZERO);
            let retry = handle
                .retry_process_release_observed_internal(Duration::ZERO)
                .0;
            let valid = returned_early
                && lease_held
                && !final_closed
                && source
                && same_source
                && repeated == settled_queued
                && retry == settled_queued
                && old_result == Some(released)
                && matches!((released, initial_resume, settled_resume, initial_queued, settled_queued), (ProcessReleaseResult::Released { generation:a, .. }, ProcessResumeResult::Pending { generation:b }, Some(ProcessResumeResult::Superseded { generation:c }), ProcessReleaseResult::Pending { generation:d }, ProcessReleaseResult::Failed { generation:e }) if a < b && b == c && c < d && d == e);
            if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
                std::fs::create_dir_all(&directory).expect("directory");
                std::fs::write(Path::new(&directory).join("process-queued-release-spawn-error.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual process runtime and real network lease, controlled held park and failed worker creation", "old_release":format!("{released:?}"), "resume":format!("{settled_resume:?}"), "queued":format!("{settled_queued:?}"), "lease_held":!final_closed, "source_retained":source, "same_source":same_source, "stable_retry":retry == settled_queued, "native_device":"not established"})).expect("json")).expect("artifact");
            }
            assert!(
                valid,
                "queued spawn failure must retain source and keep the lease unconfirmed"
            );
            return;
        }
        let valid = returned_early
            && lease_held
            && final_closed
            && matches!((released, initial_resume, settled_resume, initial_queued, settled_queued), (ProcessReleaseResult::Released { generation: a, .. }, ProcessResumeResult::Pending { generation: b }, Some(ProcessResumeResult::Superseded { generation: c }), ProcessReleaseResult::Pending { generation: d }, ProcessReleaseResult::Released { generation: e, .. }) if a < b && b == c && c < d && d == e)
            && old_result == Some(released);
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).expect("directory");
            std::fs::write(Path::new(&directory).join("process-network-release-after-resume-held.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real process runtime, network lease, controlled held park", "returned_early":returned_early, "lease_held":lease_held, "first_release":format!("{released:?}"), "resume":format!("{initial_resume:?}"), "resume_settled":format!("{settled_resume:?}"), "queued":format!("{initial_queued:?}"), "queued_settled":format!("{settled_queued:?}"), "old":format!("{old_result:?}"), "final_lease_closed":final_closed, "native_device":"not established"})).expect("json")).expect("artifact");
        }
        assert!(
            valid,
            "queued release must be accepted while the earlier park is held"
        );
        return;
    }
    let lease_held = !lease_available(&lease);
    network_log::PROBE_NETWORK_PARK_BLOCK.store(false, Ordering::SeqCst);
    let (released, old) = release.join().expect("release waiter");
    resume.join().expect("resume waiter");
    let (initial_resume, observation) = match early {
        Ok(result) => result,
        Err(_) => receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("late resume"),
    };
    let settled = observation
        .as_ref()
        .map(|observation| observation.wait(Duration::from_secs(10)));
    let old_result = old
        .as_ref()
        .map(|observation| observation.wait(Duration::ZERO));
    let reopened = !lease_available(&lease);
    let final_release = handle.release_process_internal(Duration::from_secs(10));
    let final_closed = lease_available(&lease);
    let valid = matches!(initial_resume, ProcessResumeResult::Pending { .. })
        && matches!((released, initial_resume, settled, final_release), (ProcessReleaseResult::Released { generation: a, .. }, ProcessResumeResult::Pending { generation: b }, Some(ProcessResumeResult::Resumed { generation: c }), ProcessReleaseResult::Released { generation: d, .. }) if a < b && b == c && c < d)
        && lease_held
        && matches!(settled, Some(ProcessResumeResult::Resumed { .. }))
        && reopened
        && matches!(released, ProcessReleaseResult::Released { .. })
        && matches!(old_result, Some(ProcessReleaseResult::Released { .. }))
        && matches!(final_release, ProcessReleaseResult::Released { .. })
        && final_closed;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("process-network-resume-held.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual process runtime, real network lease, controlled park hold", "early_resume":format!("{initial_resume:?}"), "lease_held":lease_held, "release":format!("{released:?}"), "settled_resume":format!("{settled:?}"), "old_observation":format!("{old_result:?}"), "lease_reopened":reopened, "final_release":format!("{final_release:?}"), "final_lease_closed":final_closed, "native_device":"not established"})).expect("json")).expect("artifact");
    }
    assert!(valid, "resume must be accepted while release is held");
}

#[cfg(feature = "test-process-release")]
#[cfg(unix)]
fn network_park_retry_probe(both_files: bool) {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().expect("root");
    let mut config = ObservabilityConfig::new(
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
            root.path().join("network-logs"),
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
        .expect("network"),
    );
    if both_files {
        config = config.with_local_logs(LocalLogConfig::new(root.path().join("daily")));
        local_file::PROBE_DAILY_PARK_FAILURE.store(true, Ordering::SeqCst);
    }
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    assert!(!lease_available(&lease));
    network_log::PROBE_NETWORK_PARK_FAILURE.store(true, Ordering::SeqCst);
    let (first, old) = handle.release_process_observed_internal(Duration::from_secs(10));
    let old = old.expect("observation");
    let first_report = handle.safe_release_report_internal(first, &old);
    let first_error = old.probe_network_error();
    let first_local_error = old.probe_local_error();
    let repeated_error = handle
        .release_process_observed_internal(Duration::ZERO)
        .1
        .and_then(|observation| observation.probe_network_error());
    let same_observed_error = match (&first_error, &repeated_error) {
        (Some(first), Some(repeated)) => std::sync::Arc::ptr_eq(first, repeated),
        _ => false,
    };
    let repeated = handle.release_process_internal(Duration::ZERO);
    let held = !lease_available(&lease);
    network_log::PROBE_NETWORK_PARK_FAILURE.store(true, Ordering::SeqCst);
    let second = handle
        .retry_process_release_observed_internal(Duration::from_secs(10))
        .0;
    let second_error = handle
        .release_process_observed_internal(Duration::ZERO)
        .1
        .and_then(|observation| observation.probe_network_error());
    let second_repeated = handle.release_process_internal(Duration::ZERO);
    let second_held = !lease_available(&lease);
    runtime::PROBE_RETRY_GATE_ARM.store(true, Ordering::SeqCst);
    let delayed_handle = handle.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let delayed = std::thread::spawn(move || {
        let result = delayed_handle
            .retry_process_release_observed_internal(Duration::from_millis(50))
            .0;
        sender.send(result).expect("send delayed result");
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !runtime::PROBE_RETRY_GATE_ENTERED.load(Ordering::SeqCst)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(runtime::PROBE_RETRY_GATE_ENTERED.load(Ordering::SeqCst));
    network_log::PROBE_NETWORK_PARK_BLOCK.store(true, Ordering::SeqCst);
    let retry_handle = handle.clone();
    let waiter = std::thread::spawn(move || {
        retry_handle
            .retry_process_release_observed_internal(Duration::from_secs(10))
            .0
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !network_log::PROBE_NETWORK_PARK_ENTERED.load(Ordering::SeqCst)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    let entered = network_log::PROBE_NETWORK_PARK_ENTERED.load(Ordering::SeqCst);
    runtime::PROBE_RETRY_GATE_RELEASE.store(true, Ordering::SeqCst);
    let raced = receiver.recv_timeout(Duration::from_secs(1));
    let pending = handle
        .retry_process_release_observed_internal(Duration::ZERO)
        .0;
    let pending_observation = handle.release_process_internal(Duration::ZERO);
    let pending_held = !lease_available(&lease);
    network_log::PROBE_NETWORK_PARK_BLOCK.store(false, Ordering::SeqCst);
    let retried = waiter.join().expect("retry waiter");
    delayed.join().expect("delayed retry waiter");
    let (_, released_observation) = handle.release_process_observed_internal(Duration::ZERO);
    let released_observation = released_observation.expect("released observation");
    let released_report = handle.safe_release_report_internal(retried, &released_observation);
    let operation_error = handle
        .release_process_observed_internal(Duration::ZERO)
        .1
        .and_then(|observation| observation.probe_network_operation_error());
    let operation_local_error = handle
        .release_process_observed_internal(Duration::ZERO)
        .1
        .and_then(|observation| observation.probe_operation_error());
    let local_preserved = match (&first_local_error, &operation_local_error) {
        (Some(first), Some(operation)) => both_files && std::sync::Arc::ptr_eq(first, operation),
        (None, None) => !both_files,
        _ => false,
    };
    let same_first = match (&first_error, &operation_error) {
        (Some(first), Some(operation)) => std::sync::Arc::ptr_eq(first, operation),
        _ => false,
    };
    let second_distinct = match (&first_error, &second_error) {
        (Some(first), Some(second)) => !std::sync::Arc::ptr_eq(first, second),
        _ => false,
    };
    let old_after = old.wait(Duration::ZERO);
    let old_source_stable = match (&first_error, old.probe_network_error()) {
        (Some(first), Some(after)) => std::sync::Arc::ptr_eq(first, &after),
        _ => false,
    };
    let closed = lease_available(&lease);
    let resume = handle.resume_process_internal(Duration::from_secs(10));
    let reopened = !lease_available(&lease);
    let (final_release, final_observation) =
        handle.release_process_observed_internal(Duration::from_secs(10));
    let fresh_error_absent = final_observation.is_some_and(|observation| {
        observation.probe_network_operation_error().is_none()
            && observation.probe_operation_error().is_none()
    });
    let final_closed = lease_available(&lease);
    let valid = matches!((first, second, retried), (ProcessReleaseResult::Failed { generation: a }, ProcessReleaseResult::Failed { generation: b }, ProcessReleaseResult::Released { generation: c, operation_failed: true }) if a < b && b < c)
        && first_report.result == first
        && first_report.cleanup_errors
            == if both_files {
                vec![
                    ProcessFailureKind::DailyFile,
                    ProcessFailureKind::NetworkFile,
                ]
            } else {
                vec![ProcessFailureKind::NetworkFile]
            }
        && first_report.explicit_retry_supported
        && released_report.operation_errors
            == if both_files {
                vec![
                    ProcessFailureKind::DailyFile,
                    ProcessFailureKind::NetworkFile,
                ]
            } else {
                vec![ProcessFailureKind::NetworkFile]
            }
        && released_report.cleanup_errors.is_empty()
        && first_error.as_ref().is_some_and(|error| {
            error.kind() == std::io::ErrorKind::PermissionDenied
                && error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<std::io::Error>())
                    .and_then(std::io::Error::raw_os_error)
                    == Some(libc::EACCES)
        })
        && same_first
        && second_distinct
        && same_observed_error
        && old_source_stable
        && local_preserved
        && fresh_error_absent
        && first == repeated
        && first == old_after
        && second == second_repeated
        && held
        && second_held
        && entered
        && pending_held
        && pending == pending_observation
        && matches!(raced, Ok(ProcessReleaseResult::Pending { generation }) if matches!(pending, ProcessReleaseResult::Pending { generation: other } if generation == other))
        && matches!((pending, retried), (ProcessReleaseResult::Pending { generation: a }, ProcessReleaseResult::Released { generation: b, .. }) if a == b)
        && closed
        && matches!(resume, ProcessResumeResult::Resumed { .. })
        && reopened
        && matches!(final_release, ProcessReleaseResult::Released { .. })
        && final_closed;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join(if both_files { "process-both-file-errors.json" } else { "process-network-park-retry.json" }), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual process runtime, real network lease, controlled park failures", "first":format!("{first:?}"), "repeated":format!("{repeated:?}"), "lease_held":held, "second":format!("{second:?}"), "second_repeated":format!("{second_repeated:?}"), "second_lease_held":second_held, "first_safe_report":format!("{first_report:?}"), "released_safe_report":format!("{released_report:?}"), "network_source_preserved":same_first, "network_source_repeated":same_observed_error, "network_source_stable":old_source_stable, "fresh_error_absent":fresh_error_absent, "second_error_distinct":second_distinct, "local_source_preserved":local_preserved, "both_files":both_files, "raced":format!("{raced:?}"), "pending":format!("{pending:?}"), "pending_observation":format!("{pending_observation:?}"), "pending_lease_held":pending_held, "retry":format!("{retried:?}"), "old_observation":format!("{old_after:?}"), "lease_closed":closed, "resume":format!("{resume:?}"), "lease_reopened":reopened, "final_release":format!("{final_release:?}"), "final_lease_closed":final_closed, "native_device":"not established"})).expect("json")).expect("artifact");
    }
    assert!(valid, "explicit retry must settle the live network closer");
}

#[cfg(feature = "test-process-release")]
fn daily_error_queued_probe(fail_queued_spawn: bool) {
    use std::sync::atomic::Ordering;
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")))
    .with_network_logs(
        NetworkLogConfig::new(
            root.path().join("network"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    assert!(!lease_available(&lease));
    local_file::PROBE_DAILY_PARK_FAILURE.store(true, Ordering::SeqCst);
    let (first, first_observation) =
        handle.release_process_observed_internal(Duration::from_secs(10));
    let first_error = first_observation
        .expect("first observation")
        .probe_local_error()
        .expect("first source");
    assert!(matches!(first, ProcessReleaseResult::Failed { .. }));
    local_file::PROBE_DAILY_PARK_BLOCK.store(true, Ordering::SeqCst);
    let retry_handle = handle.clone();
    let retry = std::thread::spawn(move || {
        retry_handle
            .retry_process_release_observed_internal(Duration::from_secs(10))
            .0
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !local_file::PROBE_DAILY_PARK_ENTERED.load(Ordering::SeqCst)
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(local_file::PROBE_DAILY_PARK_ENTERED.load(Ordering::SeqCst));
    let (retry_pending, retry_observation) =
        handle.release_process_observed_internal(Duration::ZERO);
    let (resume_pending, resume_observation) =
        handle.resume_process_observed_internal(Duration::ZERO);
    let queued_waiter_handle = handle.clone();
    let queued_waiter = std::thread::spawn(move || {
        queued_waiter_handle.retry_process_release_observed_internal(Duration::from_secs(10))
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while runtime::PROBE_QUEUED_RESUME_RETRY_COUNT.load(Ordering::SeqCst) == 0
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(runtime::PROBE_QUEUED_RESUME_RETRY_COUNT.load(Ordering::SeqCst) > 0);
    if fail_queued_spawn {
        runtime::PROBE_RELEASE_SPAWN_FAILURE.store(true, Ordering::SeqCst);
    }
    let (queued_pending, queued_observation) =
        handle.release_process_observed_internal(Duration::ZERO);
    local_file::PROBE_DAILY_PARK_BLOCK.store(false, Ordering::SeqCst);
    let retry_result = retry.join().expect("retry waiter");
    let retry_observation = retry_observation.expect("retry observation");
    let queued_observation = queued_observation.expect("queued observation");
    let resume_result = resume_observation
        .expect("resume observation")
        .wait(Duration::from_secs(10));
    let queued_result = queued_observation.wait(Duration::from_secs(10));
    let (waited_result, waited_observation) = queued_waiter.join().expect("queued waiter");
    let waited_observation = waited_observation.expect("terminal result has observation");
    let waited_report = handle.safe_release_report_internal(waited_result, &waited_observation);
    let paired = waited_result == queued_result
        && waited_report.result == queued_result
        && waited_report.operation_errors == vec![ProcessFailureKind::DailyFile]
        && waited_observation
            .probe_operation_error()
            .is_some_and(|error| std::sync::Arc::ptr_eq(&error, &first_error))
        && waited_report.cleanup_errors
            == if fail_queued_spawn {
                vec![ProcessFailureKind::ReleaseWorker]
            } else {
                vec![]
            };
    let retry_source = retry_observation.probe_operation_error();
    let queued_source = queued_observation.probe_operation_error();
    let retry_preserved = retry_source
        .as_ref()
        .is_some_and(|error| std::sync::Arc::ptr_eq(error, &first_error));
    let queued_preserved = queued_source
        .as_ref()
        .is_some_and(|error| std::sync::Arc::ptr_eq(error, &first_error));
    let lease_closed = lease_available(&lease);
    if fail_queued_spawn {
        let source = queued_observation.probe_coordinator_error();
        let source_retained = source.as_ref().is_some_and(|error| {
            error.kind() == std::io::ErrorKind::WouldBlock
                && error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<std::io::Error>())
                    .is_some_and(|source| source.kind() == std::io::ErrorKind::PermissionDenied)
        });
        let (_, repeated) = handle.release_process_observed_internal(Duration::ZERO);
        let repeated = repeated.expect("repeated observation");
        let repeated_source = repeated.probe_coordinator_error();
        let repeated_prior = repeated.probe_operation_error();
        let same_errors = match (&source, &repeated_source, &repeated_prior) {
            (Some(first), Some(second), Some(prior)) => {
                std::sync::Arc::ptr_eq(first, second) && std::sync::Arc::ptr_eq(prior, &first_error)
            }
            _ => false,
        };
        let valid = paired
            && retry_preserved
            && queued_preserved
            && source_retained
            && same_errors
            && !lease_closed
            && matches!((first, retry_pending, retry_result, resume_pending, resume_result, queued_pending, queued_result),
                (ProcessReleaseResult::Failed { generation:a }, ProcessReleaseResult::Pending { generation:b }, ProcessReleaseResult::Released { generation:c, operation_failed:true }, ProcessResumeResult::Pending { generation:d }, ProcessResumeResult::Superseded { generation:e }, ProcessReleaseResult::Pending { generation:f }, ProcessReleaseResult::Failed { generation:g }) if a < b && b == c && c < d && d == e && e < f && f == g);
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).expect("directory");
            std::fs::write(Path::new(&directory).join("process-daily-error-queued-spawn-failure.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real daily file and network lease with controlled close and worker creation failures", "queued":format!("{queued_result:?}"), "waited_result":format!("{waited_result:?}"), "waited_report":format!("{waited_report:?}"), "paired":paired, "source_retained":source_retained, "prior_error_retained":queued_preserved, "repeated_errors_stable":same_errors, "lease_held":!lease_closed, "native_device":"not established"})).expect("json")).expect("artifact");
        }
        assert!(
            valid,
            "queued worker failure must preserve current and prior errors"
        );
        return;
    }
    let valid = paired
        && retry_preserved
        && queued_preserved
        && lease_closed
        && matches!((first, retry_pending, retry_result, resume_pending, resume_result, queued_pending, queued_result),
            (ProcessReleaseResult::Failed { generation:a }, ProcessReleaseResult::Pending { generation:b }, ProcessReleaseResult::Released { generation:c, operation_failed:true }, ProcessResumeResult::Pending { generation:d }, ProcessResumeResult::Superseded { generation:e }, ProcessReleaseResult::Pending { generation:f }, ProcessReleaseResult::Released { generation:g, operation_failed:true }) if a < b && b == c && c < d && d == e && e < f && f == g);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("process-daily-error-queued.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"real daily file and network lease with controlled close failure and held park", "first":format!("{first:?}"), "retry":format!("{retry_result:?}"), "resume":format!("{resume_result:?}"), "queued":format!("{queued_result:?}"), "waited_result":format!("{waited_result:?}"), "waited_report":format!("{waited_report:?}"), "paired":paired, "retry_preserved":retry_preserved, "queued_preserved":queued_preserved, "lease_closed":lease_closed, "native_device":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        valid,
        "queued release must preserve the prior operation error"
    );
}

#[cfg(feature = "test-process-release")]
#[cfg(unix)]
fn daily_park_retry_probe(terminal: bool) {
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")));
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    assert!(descriptor_open(&daily));
    if terminal {
        *local_file::lock(&local_file::PROBE_DAILY_EXIT_GATE) = Some(root.path().to_path_buf());
        local_file::PROBE_DAILY_PARK_PANIC.store(true, std::sync::atomic::Ordering::SeqCst);
    } else {
        local_file::PROBE_DAILY_PARK_FAILURE.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let (first, observation) = handle.release_process_observed_internal(Duration::from_secs(10));
    let old = observation.expect("release observation");
    let first_report = handle.safe_release_report_internal(first, &old);
    let first_error = old.probe_local_error();
    let (_, repeated_observation) = handle.release_process_observed_internal(Duration::ZERO);
    let repeated_error =
        repeated_observation.and_then(|observation| observation.probe_local_error());
    let same_error = match (&first_error, &repeated_error) {
        (Some(first), Some(repeated)) => std::sync::Arc::ptr_eq(first, repeated),
        (None, None) => true,
        _ => false,
    };
    let repeated = handle.release_process_internal(Duration::ZERO);
    let held = descriptor_open(&daily);
    if terminal {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !root.path().join("worker-exiting").is_file() && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(root.path().join("worker-exiting").is_file());
    }
    let mut second_error = None;
    let failed_again = if terminal {
        None
    } else {
        local_file::PROBE_DAILY_PARK_FAILURE.store(true, std::sync::atomic::Ordering::SeqCst);
        let failed = handle
            .retry_process_release_observed_internal(Duration::from_secs(10))
            .0;
        assert!(
            matches!((first, failed), (ProcessReleaseResult::Failed { generation: original }, ProcessReleaseResult::Failed { generation: next }) if next > original)
        );
        assert_eq!(failed, handle.release_process_internal(Duration::ZERO));
        second_error = handle
            .release_process_observed_internal(Duration::ZERO)
            .1
            .and_then(|observation| observation.probe_local_error());
        assert!(descriptor_open(&daily));
        Some(failed)
    };
    let retried = handle
        .retry_process_release_observed_internal(Duration::from_secs(10))
        .0;
    let old_after_retry = old.wait(Duration::ZERO);
    let old_report_after_retry = handle.safe_release_report_internal(old_after_retry, &old);
    let old_error_after_retry = old.probe_local_error();
    let old_error_stable = match (&first_error, &old_error_after_retry) {
        (Some(first), Some(after)) => std::sync::Arc::ptr_eq(first, after),
        (None, None) => true,
        _ => false,
    };
    if terminal {
        std::fs::write(root.path().join("allow-exit"), b"allow").expect("release worker");
        let repeated_retry = handle
            .retry_process_release_observed_internal(Duration::ZERO)
            .0;
        let stable = matches!(first, ProcessReleaseResult::Failed { .. })
            && first_report.cleanup_errors == vec![ProcessFailureKind::Unconfirmed]
            && !first_report.explicit_retry_supported
            && first_error.is_none()
            && same_error
            && old_error_stable
            && first == repeated
            && first == retried
            && first == repeated_retry
            && first == old_after_retry;
        if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
            std::fs::create_dir_all(&directory).expect("directory");
            std::fs::write(Path::new(&directory).join("process-daily-park-terminal.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual process runtime and injected daily file worker panic", "first":format!("{first:?}"), "retry":format!("{retried:?}"), "repeated_retry":format!("{repeated_retry:?}"), "old_observation":format!("{old_after_retry:?}"), "descriptor_held_when_observed":held, "io_error_absent":first_error.is_none(), "same_error":same_error, "old_error_stable":old_error_stable, "stable_failed":stable, "native_device":"not established"})).expect("json")).expect("artifact");
        }
        assert!(stable, "terminated file worker must not be retried");
        return;
    }
    let (_, released_observation) = handle.release_process_observed_internal(Duration::ZERO);
    let released_observation = released_observation.expect("released observation");
    let released_report = handle.safe_release_report_internal(retried, &released_observation);
    let released_operation_error = released_observation.probe_operation_error();
    let first_preserved = match (&first_error, &released_operation_error) {
        (Some(first), Some(operation)) => std::sync::Arc::ptr_eq(first, operation),
        _ => false,
    };
    let second_distinct = match (&first_error, &second_error) {
        (Some(first), Some(second)) => !std::sync::Arc::ptr_eq(first, second),
        _ => false,
    };
    let closed = !descriptor_open(&daily);
    let resume = handle.resume_process_internal(Duration::from_secs(10));
    let reopened = descriptor_open(&daily);
    let (final_release, final_observation) =
        handle.release_process_observed_internal(Duration::from_secs(10));
    let fresh_error_absent =
        final_observation.is_some_and(|observation| observation.probe_operation_error().is_none());
    let final_closed = !descriptor_open(&daily);
    let valid = matches!(first, ProcessReleaseResult::Failed { .. })
        && first_report.cleanup_errors == vec![ProcessFailureKind::DailyFile]
        && first_report.explicit_retry_supported
        && !old_report_after_retry.explicit_retry_supported
        && old_report_after_retry.cleanup_errors == vec![ProcessFailureKind::DailyFile]
        && released_report.cleanup_errors.is_empty()
        && released_report.operation_errors == vec![ProcessFailureKind::DailyFile]
        && !released_report.explicit_retry_supported
        && same_error
        && old_error_stable
        && first_preserved
        && second_distinct
        && fresh_error_absent
        && first_error.as_ref().is_some_and(|error| {
            error.kind() == std::io::ErrorKind::PermissionDenied
                && error
                    .get_ref()
                    .and_then(|source| source.downcast_ref::<std::io::Error>())
                    .and_then(std::io::Error::raw_os_error)
                    == Some(libc::EACCES)
        })
        && first == repeated
        && first == old_after_retry
        && held
        && matches!((first, retried), (ProcessReleaseResult::Failed { generation: first }, ProcessReleaseResult::Released { generation: next, operation_failed: true }) if next > first)
        && closed
        && matches!(resume, ProcessResumeResult::Resumed { .. })
        && reopened
        && matches!(final_release, ProcessReleaseResult::Released { .. })
        && final_closed;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("process-daily-park-retry.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual process runtime, real daily file and controlled park failure", "first":format!("{first:?}"), "repeated":format!("{repeated:?}"), "descriptor_held_after_failure":held, "second_failure":format!("{failed_again:?}"), "second_error_distinct":second_distinct, "first_error_preserved_in_release":first_preserved, "fresh_release_error_absent":fresh_error_absent, "same_error":same_error, "old_error_stable":old_error_stable, "first_safe_report":format!("{first_report:?}"), "released_safe_report":format!("{released_report:?}"), "old_safe_report_after_retry":format!("{old_report_after_retry:?}"), "first_error_kind":first_error.as_ref().map(|error| format!("{:?}", error.kind())), "first_error_source_os_code":first_error.as_ref().and_then(|error| error.get_ref()).and_then(|source| source.downcast_ref::<std::io::Error>()).and_then(std::io::Error::raw_os_error), "retry":format!("{retried:?}"), "old_observation":format!("{old_after_retry:?}"), "descriptor_closed_after_retry":closed, "resume":format!("{resume:?}"), "descriptor_reopened":reopened, "final_release":format!("{final_release:?}"), "final_descriptor_closed":final_closed, "native_device":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        valid,
        "explicit retry must settle the live daily file closer"
    );
}

fn trace_callback_panic_probe(after: bool) {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    };
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::{Layer, Registry};
    use uc_observability_contract::diagnostics::{
        operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
        OperationContext,
    };

    struct PanicLayer(Arc<AtomicBool>);
    impl Layer<Registry> for PanicLayer {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            _: &tracing::span::Id,
            _: Context<'_, Registry>,
        ) {
            if attrs.metadata().target() == "uc.telemetry" && self.0.swap(false, Ordering::SeqCst) {
                panic!("controlled callback failure");
            }
        }
    }
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_remote(
        OtlpHttpConfig::new_loopback("http://127.0.0.1:9/v1/traces", "http://127.0.0.1:9/v1/logs")
            .expect("remote"),
    );
    let (telemetry, _) = telemetry::TelemetryRuntime::new(&config, None);
    let armed = Arc::new(AtomicBool::new(false));
    let mut layers: Vec<HostLogLayer> = Vec::new();
    if !after {
        layers.push(Box::new(PanicLayer(Arc::clone(&armed))));
    }
    layers.push(telemetry.trace_boundary(true).expect("boundary"));
    if after {
        layers.push(Box::new(PanicLayer(Arc::clone(&armed))));
    }
    layers.extend(telemetry.layers());
    layers.push(telemetry.trace_boundary(false).expect("boundary"));
    tracing::subscriber::set_global_default(
        telemetry.guard_subscriber(tracing_subscriber::registry().with(layers)),
    )
    .expect("subscriber");
    let make_span = || {
        operation_span(OperationContext {
            domain: DiagnosticDomain::Storage,
            operation: DiagnosticOperation::ProfileStorageUpgrade,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
        })
    };
    let live = make_span();
    armed.store(true, Ordering::SeqCst);
    let panicked = std::panic::catch_unwind(make_span).is_err();
    telemetry.close_admission();
    let draining = telemetry.clone();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let result = draining.wait_accepted_spans(&|| false).unwrap_or(false);
        let _ = sender.send(format!("{result:?}"));
    });
    let pending = receiver.recv_timeout(Duration::from_millis(100)).is_err();
    drop(live);
    let result = receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|_| "timeout".to_owned());
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("trace-callback-panic.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production subscriber and telemetry with injected callback panic", "after_boundary":after, "panic_observed":panicked, "other_span_pending":pending, "drain_result":result, "process_release_receipt":"not established", "device_acceptance":"not established"})).expect("serialize")).expect("write evidence");
    }
    assert!(
        panicked && pending && result == "false",
        "callback failure must remain unconfirmed after other work drains"
    );
}

fn trace_admission_race_probe(resume_first: bool) {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use prost::Message;
    use std::sync::{mpsc, Mutex};
    use tracing_subscriber::layer::{Context, SubscriberExt};
    use tracing_subscriber::{Layer, Registry};
    use uc_observability_contract::diagnostics::{
        operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole, DiagnosticSpanKind,
        OperationContext,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct HoldLayer {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl Layer<Registry> for HoldLayer {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            _: &tracing::span::Id,
            _: Context<'_, Registry>,
        ) {
            if attrs.metadata().target() == "uc.telemetry" {
                self.entered.send(()).expect("signal callback");
                self.release
                    .lock()
                    .expect("release lock")
                    .recv()
                    .expect("release callback");
            }
        }
    }
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("executor");
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
    .expect("remote");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_remote(remote);
    let (telemetry, _) = telemetry::TelemetryRuntime::new(&config, None);
    let (entered, waiting) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let hold: HostLogLayer = Box::new(HoldLayer {
        entered,
        release: Mutex::new(released),
    });
    let mut layers: Vec<HostLogLayer> = vec![hold];
    layers.push(telemetry.trace_boundary(true).expect("first boundary"));
    layers.extend(telemetry.layers());
    layers.push(telemetry.trace_boundary(false).expect("final boundary"));
    tracing::subscriber::set_global_default(
        telemetry.guard_subscriber(tracing_subscriber::registry().with(layers)),
    )
    .expect("subscriber");
    if !resume_first {
        telemetry.close_admission();
    }
    let (created, created_wait) = mpsc::channel();
    let (finish, finish_wait) = mpsc::channel();
    let mut worker = Some(std::thread::spawn(move || {
        let span = operation_span(OperationContext {
            domain: DiagnosticDomain::Storage,
            operation: DiagnosticOperation::ProfileStorageUpgrade,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
        });
        span.record("uc.outcome", "ok");
        span.record("otel.status_code", "OK");
        created.send(()).expect("created");
        finish_wait.recv().expect("finish");
        drop(span);
    }));
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("blocked callback");
    if resume_first {
        telemetry.close_admission();
    }
    let draining = telemetry.clone();
    let (drained, drain_wait) = mpsc::channel();
    let drainer = std::thread::spawn(move || {
        draining.wait_accepted_spans(&|| false);
        drained.send(()).expect("drained");
    });
    let wait_pending = drain_wait.recv_timeout(Duration::from_millis(50)).is_err();
    if resume_first {
        release.send(()).expect("release callback");
        created_wait
            .recv_timeout(Duration::from_secs(5))
            .expect("span created");
        finish.send(()).expect("finish span");
        worker
            .take()
            .expect("span worker")
            .join()
            .expect("span worker");
        drain_wait
            .recv_timeout(Duration::from_secs(5))
            .expect("accepted span drained");
    } else {
        assert!(!wait_pending, "unregistered span must not block the drain");
    }
    drainer.join().expect("drain worker");
    telemetry.pause();
    let first_flush = telemetry.force_flush();
    let resumed = if !resume_first {
        let resumed = telemetry.resume();
        release.send(()).expect("release callback");
        created_wait
            .recv_timeout(Duration::from_secs(5))
            .expect("span created");
        finish.send(()).expect("finish span");
        worker
            .take()
            .expect("span worker")
            .join()
            .expect("span worker");
        resumed
    } else {
        telemetry.resume()
    };
    let second_flush = telemetry.force_flush();
    let requests = executor
        .block_on(receiver.received_requests())
        .expect("requests");
    let spans = requests
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
        .flat_map(|request| {
            ExportTraceServiceRequest::decode(request.body.as_slice())
                .expect("traces")
                .resource_spans
        })
        .flat_map(|resource| resource.scope_spans)
        .flat_map(|scope| scope.spans)
        .count();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("trace-admission-race.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"production subscriber gate and telemetry layers, controlled callback, local OTLP receiver", "callback_accepted_before_close":resume_first, "wait_pending":wait_pending, "first_flush":format!("{:?}", first_flush.traces), "second_flush":format!("{:?}", second_flush.traces), "resumed":resumed, "received_spans":spans, "process_release_receipt":"not established", "device_acceptance":"not established"})).expect("serialize")).expect("write evidence");
    }
    assert!(
        resumed
            && first_flush.traces == SignalResult::Completed
            && second_flush.traces == SignalResult::Completed
            && (if resume_first {
                wait_pending && spans == 1
            } else {
                !wait_pending && spans == 0
            }),
        "old callback must not export after resume"
    );
    let _ = telemetry.shutdown(Duration::from_secs(5));
}

#[cfg(feature = "test-process-release")]
fn remote_export_error_probe(fail_file: bool, fail_logs: bool) {
    use uc_observability_contract::diagnostics::{
        complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
        DiagnosticSpanKind, OperationCompletion, OperationContext,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("executor");
    let receiver = executor.block_on(async {
        let receiver = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/traces"))
            .respond_with(ResponseTemplate::new(if fail_logs { 200 } else { 400 }))
            .mount(&receiver)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/logs"))
            .respond_with(ResponseTemplate::new(if fail_logs { 400 } else { 200 }))
            .mount(&receiver)
            .await;
        receiver
    });
    let root = tempfile::tempdir().expect("root");
    let remote = OtlpHttpConfig::new_loopback(
        &format!("{}/v1/traces", receiver.uri()),
        &format!("{}/v1/logs", receiver.uri()),
    )
    .expect("remote");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_remote(remote)
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")));
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    if fail_file {
        local_file::PROBE_DAILY_PARK_FAILURE.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let span = operation_span(OperationContext {
        domain: DiagnosticDomain::Storage,
        operation: DiagnosticOperation::ProfileStorageUpgrade,
        role: DiagnosticRole::Local,
        kind: DiagnosticSpanKind::Internal,
    });
    {
        let _entered = span.enter();
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            Duration::from_millis(1),
        ));
    }
    let (pending, observation) =
        handle.release_process_observed_internal(Duration::from_millis(50));
    drop(span);
    let observation = observation.expect("observation");
    let first = observation.wait(Duration::from_secs(10));
    let first_file_error = observation.probe_local_error();
    let operation_error_for = |observation: &process_release::ReleaseObservation| {
        if fail_logs {
            observation.probe_log_operation_error()
        } else {
            observation.probe_trace_operation_error()
        }
    };
    let first_trace_error = operation_error_for(&observation);
    let (released, current_observation) = if fail_file {
        let result = handle
            .retry_process_release_observed_internal(Duration::from_secs(10))
            .0;
        let (_, observation) = handle.release_process_observed_internal(Duration::ZERO);
        (result, observation.expect("retry observation"))
    } else {
        (first, observation)
    };
    let (repeated, repeated_observation) = handle.release_process_observed_internal(Duration::ZERO);
    let report = handle.safe_release_report_internal(released, &current_observation);
    let trace_error = operation_error_for(&current_observation);
    let other_signal_error_absent = if fail_logs {
        current_observation.probe_trace_operation_error().is_none()
    } else {
        current_observation.probe_log_operation_error().is_none()
    };
    let operation_file_error = current_observation.probe_operation_error();
    let file_error_preserved = match (&first_file_error, &operation_file_error) {
        (Some(first), Some(operation)) => fail_file && std::sync::Arc::ptr_eq(first, operation),
        (None, None) => !fail_file,
        _ => false,
    };
    let trace_error_preserved = match (&first_trace_error, &trace_error) {
        (Some(first), Some(current)) => std::sync::Arc::ptr_eq(first, current),
        _ => false,
    };
    let repeated_file_error = repeated_observation
        .as_ref()
        .and_then(|observation| observation.probe_operation_error());
    let repeated_error =
        repeated_observation.and_then(|observation| operation_error_for(&observation));
    let same_error = match (&trace_error, &repeated_error) {
        (Some(first), Some(repeated)) => std::sync::Arc::ptr_eq(first, repeated),
        _ => false,
    };
    let requests = executor
        .block_on(receiver.received_requests())
        .expect("requests");
    let trace_requests = requests
        .iter()
        .filter(|request| request.url.path() == if fail_logs { "/v1/logs" } else { "/v1/traces" })
        .count();
    let closed = !descriptor_open(&daily);
    let health = handle.health();
    let failed_exports = if fail_logs {
        health.failed_remote_log_batches
    } else {
        health.failed_remote_span_batches
    };
    let same_file_error = match (&operation_file_error, &repeated_file_error) {
        (Some(first), Some(repeated)) => std::sync::Arc::ptr_eq(first, repeated),
        (None, None) => true,
        _ => false,
    };
    let resume = handle.resume_process_internal(Duration::from_secs(10));
    let (fresh, fresh_observation) =
        handle.release_process_observed_internal(Duration::from_secs(10));
    let fresh_clear = fresh_observation.is_some_and(|observation| {
        observation.probe_trace_operation_error().is_none()
            && observation.probe_log_operation_error().is_none()
            && observation.probe_operation_error().is_none()
    });
    let valid = matches!(pending, ProcessReleaseResult::Pending { .. })
        && report.result == released
        && report.cleanup_errors.is_empty()
        && report.operation_errors
            == if fail_file {
                vec![
                    ProcessFailureKind::DailyFile,
                    ProcessFailureKind::TraceExport,
                ]
            } else if fail_logs {
                vec![ProcessFailureKind::LogExport]
            } else {
                vec![ProcessFailureKind::TraceExport]
            }
        && !report.explicit_retry_supported
        && matches!(
            released,
            ProcessReleaseResult::Released {
                operation_failed: true,
                ..
            }
        )
        && matches!(
            trace_error.as_deref(),
            Some(opentelemetry_sdk::error::OTelSdkError::InternalFailure(_))
        )
        && file_error_preserved
        && trace_error_preserved
        && (!fail_file || matches!(first, ProcessReleaseResult::Failed { .. }))
        && same_error
        && other_signal_error_absent
        && same_file_error
        && matches!(resume, ProcessResumeResult::Resumed { .. })
        && matches!(
            fresh,
            ProcessReleaseResult::Released {
                operation_failed: false,
                ..
            }
        )
        && fresh_clear
        && released == repeated
        && trace_requests > 0
        && closed
        && failed_exports > 0;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join(if fail_file { "process-remote-export-file-error.json" } else if fail_logs { "process-remote-log-export-error.json" } else { "process-remote-export-error.json" }), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual process runtime, local OTLP rejection, real daily descriptor", "signal":if fail_logs {"logs"} else {"traces"}, "safe_report":format!("{report:?}"), "pending":format!("{pending:?}"), "first":format!("{first:?}"), "released":format!("{released:?}"), "repeated":format!("{repeated:?}"), "rejected_requests":trace_requests, "failed_export_batches":failed_exports, "descriptor_closed":closed, "file_error_preserved":file_error_preserved, "sdk_error_preserved":trace_error_preserved, "same_error":same_error, "other_signal_error_absent":other_signal_error_absent, "same_file_error":same_file_error, "fresh_error_absent":fresh_clear, "production_collector":"not established", "native_device":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        valid,
        "rejected trace export must be an operation error after file closure"
    );
}

fn remote_span_overlap_probe(lazy: bool) {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use prost::Message;
    use uc_observability_contract::diagnostics::{
        complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
        DiagnosticSpanKind, OperationCompletion, OperationContext,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("executor");
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
    .expect("remote");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_remote(remote);
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let make_span = || {
        operation_span(OperationContext {
            domain: DiagnosticDomain::Storage,
            operation: DiagnosticOperation::ProfileStorageUpgrade,
            role: DiagnosticRole::Local,
            kind: DiagnosticSpanKind::Internal,
        })
    };
    let complete = || {
        complete_operation(OperationCompletion::succeeded(
            DiagnosticDomain::Storage,
            DiagnosticOperation::ProfileStorageUpgrade,
            DiagnosticRole::Local,
            Duration::from_millis(1),
        ))
    };
    let old = make_span();
    if lazy {
        old.record("uc.outcome", "ok");
        old.record("otel.status_code", "OK");
    } else {
        let _entered = old.enter();
        complete();
    }
    let pending = handle.release_process_internal(Duration::from_millis(50));
    let late = make_span();
    {
        let _entered = late.enter();
        complete();
    }
    drop(late);
    drop(old);
    let released = handle.observe_process_release_internal(Duration::from_secs(10));
    let before_resume = executor
        .block_on(receiver.received_requests())
        .expect("requests");
    let spans_before = before_resume
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
        .flat_map(|request| {
            ExportTraceServiceRequest::decode(request.body.as_slice())
                .expect("traces")
                .resource_spans
        })
        .flat_map(|resource| resource.scope_spans)
        .flat_map(|scope| scope.spans)
        .count();
    let resumed = handle.resume_process_internal(Duration::from_secs(10));
    let new = make_span();
    {
        let _entered = new.enter();
        complete();
    }
    drop(new);
    let flush = handle.force_flush(Duration::from_secs(10));
    let after_resume = executor
        .block_on(receiver.received_requests())
        .expect("requests");
    let spans_after = after_resume
        .iter()
        .filter(|request| request.url.path() == "/v1/traces")
        .flat_map(|request| {
            ExportTraceServiceRequest::decode(request.body.as_slice())
                .expect("traces")
                .resource_spans
        })
        .flat_map(|resource| resource.scope_spans)
        .flat_map(|scope| scope.spans)
        .count();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-remote-span-overlap.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"private real process runtime and local OTLP receiver; approved held span", "lazy":lazy,
            "release_pending":matches!(pending, ProcessReleaseResult::Pending { .. }), "released":matches!(released, ProcessReleaseResult::Released { .. }),
            "spans_before_resume":spans_before, "resumed":matches!(resumed, ProcessResumeResult::Resumed { .. }),
            "flush_completed":flush.traces == SignalResult::Completed, "spans_after_resume":spans_after,
            "production_collector":"not established", "device_acceptance":"not established"
        })).expect("serialize")).expect("write evidence");
    }
    assert!(matches!(pending, ProcessReleaseResult::Pending { .. }));
    assert!(matches!(released, ProcessReleaseResult::Released { .. }) && spans_before == 1);
    assert!(
        matches!(resumed, ProcessResumeResult::Resumed { .. })
            && flush.traces == SignalResult::Completed
            && spans_after == 2
    );
    let _ = handle.shutdown(Duration::from_secs(5));
}

fn superseded_resume_probe() {
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")))
    .with_network_logs(
        NetworkLogConfig::new(
            root.path().join("network-logs"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    let directory = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory;
    let lease = directory.join("lease.lock");
    let segment = directory.join("network.20990101.99999.jsonl");
    std::fs::File::create(&segment)
        .expect("segment")
        .set_len(256 * 1024 * 1024)
        .expect("sparse segment");
    let exporter = handle.clone();
    let export =
        std::thread::spawn(move || exporter.prepare_network_log_export(Duration::from_secs(5)));
    let limit = std::time::Instant::now() + Duration::from_secs(10);
    while !descriptor_open(&segment) && !export.is_finished() && std::time::Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(5));
    }
    let export_open = descriptor_open(&segment);
    let first_release = handle.release_process_internal(Duration::from_millis(30));
    let waiter = handle.clone();
    let worker =
        std::thread::spawn(move || waiter.resume_process_internal(Duration::from_secs(20)));
    let (resume, observation) = loop {
        let (result, observation) = handle.resume_process_observed_internal(Duration::ZERO);
        if matches!(result, ProcessResumeResult::Pending { .. })
            || std::time::Instant::now() >= limit
        {
            break (result, observation);
        }
    };
    let observation = observation.expect("accepted resume observation");
    let release = handle.release_process_internal(Duration::from_millis(30));
    let pending_while_blocked = !worker.is_finished() && descriptor_open(&segment);
    assert!(export.join().expect("export worker").is_ok());
    let observed = worker.join().expect("resume waiter");
    let completed = handle.observe_process_release_internal(Duration::from_secs(10));
    let closed = !descriptor_open(&daily) && lease_available(&lease);
    let old_result_after_phase_advance = observation.wait(Duration::ZERO);
    let repeated_old_result = observation.wait(Duration::ZERO);
    let old_result_preserved = old_result_after_phase_advance == observed
        && repeated_old_result == old_result_after_phase_advance;
    std::fs::remove_file(&segment).expect("remove sparse segment");
    let same_generation = match resume {
        ProcessResumeResult::Pending { generation } => {
            format!("{observed:?}").contains(&format!("generation: {generation}"))
        }
        _ => false,
    };
    let superseded = format!("{observed:?}").starts_with("Superseded");
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-superseded-resume.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"private process owner and real network export; independent descriptor and network lease probes",
            "export_open":export_open, "first_release_pending":matches!(first_release, ProcessReleaseResult::Pending { .. }),
            "resume_pending":matches!(resume, ProcessResumeResult::Pending { .. }), "release_pending":matches!(release, ProcessReleaseResult::Pending { .. }),
            "waiter_pending_while_blocked":pending_while_blocked, "observed":format!("{observed:?}"), "superseded":superseded,
            "same_generation":same_generation, "old_result_after_phase_advance":format!("{old_result_after_phase_advance:?}"), "old_result_preserved":old_result_preserved, "release_completed":matches!(completed, ProcessReleaseResult::Released { .. }), "resources_closed":closed,
            "device_acceptance":"not established"
        })).expect("serialize")).expect("write evidence");
    }
    assert!(
        export_open
            && matches!(first_release, ProcessReleaseResult::Pending { .. })
            && pending_while_blocked
            && superseded
            && same_generation
            && old_result_preserved
    );
    assert!(
        matches!(
            completed,
            ProcessReleaseResult::Released {
                operation_failed: false,
                ..
            }
        ) && closed
    );
    let _ = handle.shutdown(Duration::from_secs(5));
}

fn export_overlap_probe() {
    let root = tempfile::tempdir().expect("root");
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
            root.path().join("network-logs"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let directory = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("initial export")
        .run_directory;
    let segment = directory.join("network.20990101.99999.jsonl");
    std::fs::File::create(&segment)
        .expect("segment")
        .set_len(128 * 1024 * 1024)
        .expect("sparse segment");
    let exporter = handle.clone();
    let worker =
        std::thread::spawn(move || exporter.prepare_network_log_export(Duration::from_secs(5)));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !descriptor_open(&segment)
        && !worker.is_finished()
        && std::time::Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    let open = descriptor_open(&segment);
    let first = handle.release_process_internal(Duration::from_millis(50));
    let open_when_returned = descriptor_open(&segment);
    let denied_pending = matches!(
        handle.prepare_network_log_export(Duration::from_secs(5)),
        Err(NetworkLogError::AlreadyShutdown)
    );
    let export_ok = worker.join().expect("export thread").is_ok();
    let released = handle.observe_process_release_internal(Duration::from_secs(10));
    let closed = !descriptor_open(&segment);
    std::fs::remove_file(&segment).expect("remove sparse segment");
    let denied = matches!(
        handle.prepare_network_log_export(Duration::from_secs(5)),
        Err(NetworkLogError::AlreadyShutdown)
    );
    let resumed = handle.resume_process_internal(Duration::from_secs(10));
    let allowed = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .is_ok();
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-network-export-overlap.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"private process owner and actual network export of a sparse segment; independent lsof probe",
            "export_descriptor_observed":open, "release_pending":matches!(first, ProcessReleaseResult::Pending { .. }),
            "descriptor_open_after_wait":open_when_returned, "new_export_denied_while_pending":denied_pending, "export_completed":export_ok,
            "release_completed":matches!(released, ProcessReleaseResult::Released { .. }), "descriptor_closed":closed,
            "later_export_denied":denied, "resume_succeeded":matches!(resumed, ProcessResumeResult::Resumed { .. }), "export_after_resume":allowed,
            "device_acceptance":"not established"
        })).expect("serialize")).expect("write evidence");
    }
    assert!(
        open && open_when_returned
            && denied_pending
            && matches!(first, ProcessReleaseResult::Pending { .. })
    );
    assert!(
        export_ok && matches!(released, ProcessReleaseResult::Released { .. }) && closed && denied
    );
    assert!(matches!(resumed, ProcessResumeResult::Resumed { .. }) && allowed);
    let _ = handle.shutdown(Duration::from_secs(5));
}

#[cfg(unix)]
fn release_after_queued_resume_probe() {
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")))
    .with_network_logs(
        NetworkLogConfig::new(
            root.path().join("network-logs"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    let (mut active, _) = EngineLogActivity::begin(Duration::from_secs(5));
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    let (first, first_observation) =
        handle.release_process_observed_internal(Duration::from_millis(30));
    let first_observation = first_observation.expect("first release observation");
    let waiting_handle = handle.clone();
    let (wait_started, wait_ready) = std::sync::mpsc::channel();
    let first_waiter = std::thread::spawn(move || {
        let (initial, observation) =
            waiting_handle.release_process_observed_internal(Duration::ZERO);
        wait_started.send(initial).expect("waiter started");
        observation
            .expect("waiter observation")
            .wait(Duration::from_millis(300))
    });
    assert_eq!(wait_ready.recv().expect("waiter ready"), first);
    let resume = handle.resume_process_internal(Duration::ZERO);
    let last = handle.release_process_internal(Duration::ZERO);
    let repeated = handle.release_process_internal(Duration::ZERO);
    let first_wait_result = first_waiter.join().expect("first waiter");
    let first_wait_same = matches!((first, first_wait_result), (ProcessReleaseResult::Pending { generation: a }, ProcessReleaseResult::Pending { generation: b }) if a == b);
    let generations = match (first, resume, last) {
        (
            ProcessReleaseResult::Pending { generation: a },
            ProcessResumeResult::Pending { generation: b },
            ProcessReleaseResult::Pending { generation: c },
        ) => a < b && b < c,
        _ => false,
    };
    let (rejected, result) = EngineLogActivity::begin(Duration::from_millis(30));
    let fenced = !rejected.is_admitted() && result == Some(SignalResult::TimedOut);
    let saved = root.path().join("saved-queued-daily");
    std::fs::rename(&daily, &saved).expect("move daily");
    assert!(Command::new("mkfifo")
        .arg(&daily)
        .status()
        .expect("mkfifo")
        .success());
    active.end(Duration::from_secs(5));
    let limit = std::time::Instant::now() + Duration::from_secs(5);
    while descriptor_open(&saved) && std::time::Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(10));
    }
    let first_closed = !descriptor_open(&saved) && lease_available(&lease);
    let pending_reopen =
        handle.observe_process_release_internal(Duration::from_millis(100)) == last;
    let first_receipt_before = first_observation.wait(Duration::ZERO);
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&daily)
        .expect("unblock queued reopen");
    let completed = handle.release_process_internal(Duration::from_secs(10));
    drop(reader);
    let reopened_file_closed = !descriptor_open(&daily);
    std::fs::remove_file(&daily).expect("remove fifo");
    std::fs::rename(&saved, &daily).expect("restore daily");
    let closed = reopened_file_closed && !descriptor_open(&daily) && lease_available(&lease);
    let stable = completed == handle.release_process_internal(Duration::ZERO);
    let first_receipt_after = first_observation.wait(Duration::ZERO);
    let first_receipt_stable = first_receipt_before == first_receipt_after
        && matches!((first, first_receipt_after),
            (ProcessReleaseResult::Pending { generation: expected }, ProcessReleaseResult::Released { generation, operation_failed: false }) if expected == generation);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-release-after-queued-resume.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"private process owner; controlled registration and real daily and network files; independent descriptor and lease probes",
            "three_generations":generations, "first_wait_result":format!("{first_wait_result:?}"), "first_wait_same_generation":first_wait_same, "first_receipt_before":format!("{first_receipt_before:?}"), "first_receipt_after":format!("{first_receipt_after:?}"), "first_receipt_stable":first_receipt_stable, "same_pending":last == repeated, "registration_fenced":fenced,
            "first_resources_closed":first_closed, "pending_while_reopen_blocked":pending_reopen,
            "released":matches!(completed, ProcessReleaseResult::Released { .. }), "resources_closed":closed, "stable":stable,
            "device_acceptance":"not established"
        })).expect("serialize")).expect("write evidence");
    }
    assert!(
        generations
            && first_wait_same
            && first_receipt_stable
            && last == repeated
            && fenced
            && first_closed
            && pending_reopen
    );
    assert!(matches!(completed, ProcessReleaseResult::Released { .. }) && closed && stable);
    let _ = handle.shutdown(Duration::from_secs(5));
}

#[cfg(unix)]
fn release_during_resume_probe() {
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")))
    .with_network_logs(
        NetworkLogConfig::new(
            root.path().join("network-logs"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    let initial = handle.release_process_internal(Duration::from_secs(10));
    let saved = root.path().join("saved-daily");
    std::fs::rename(&daily, &saved).expect("move daily");
    assert!(Command::new("mkfifo")
        .arg(&daily)
        .status()
        .expect("mkfifo")
        .success());
    let resume = handle.resume_process_internal(Duration::from_millis(50));
    let waiting = handle.clone();
    let (started, ready) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        started.send(()).expect("signal waiter");
        waiting.resume_process_internal(Duration::from_secs(10))
    });
    ready.recv().expect("waiter started");
    std::thread::sleep(Duration::from_millis(20));
    let release = handle.release_process_internal(Duration::from_millis(30));
    let repeated = handle.release_process_internal(Duration::ZERO);
    let (rejected, result) = EngineLogActivity::begin(Duration::from_millis(30));
    let fenced = !rejected.is_admitted() && result == Some(SignalResult::TimedOut);
    let reader = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(&daily)
        .expect("unblock reopen");
    let completed = handle.release_process_internal(Duration::from_secs(10));
    let failed_resume = waiter.join().expect("resume waiter");
    drop(reader);
    let reopened_file_closed = !descriptor_open(&daily);
    std::fs::remove_file(&daily).expect("remove fifo");
    std::fs::rename(&saved, &daily).expect("restore daily");
    let closed = reopened_file_closed && !descriptor_open(&daily) && lease_available(&lease);
    let stable = completed == handle.release_process_internal(Duration::ZERO);
    let newer = match (resume, release) {
        (
            ProcessResumeResult::Pending { generation: old },
            ProcessReleaseResult::Pending { generation: new },
        ) => new > old,
        _ => false,
    };
    let same_failed_generation = matches!((resume, failed_resume), (ProcessResumeResult::Pending { generation: old }, ProcessResumeResult::Failed { generation: observed }) if old == observed);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-release-during-resume.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"private process owner; blocked real daily reopen; independent daily descriptor and network lease probes",
            "initial_release":matches!(initial, ProcessReleaseResult::Released { .. }), "resume_pending":matches!(resume, ProcessResumeResult::Pending { .. }),
            "new_release_generation":newer, "repeated_pending":release == repeated, "registration_fenced":fenced,
            "failed_resume_keeps_generation":same_failed_generation,
            "released":matches!(completed, ProcessReleaseResult::Released { .. }), "resources_closed":closed, "result_stable":stable,
            "device_acceptance":"not established"
        })).expect("serialize")).expect("write evidence");
    }
    assert!(
        matches!(initial, ProcessReleaseResult::Released { .. })
            && newer
            && release == repeated
            && fenced
            && same_failed_generation
    );
    assert!(matches!(completed, ProcessReleaseResult::Released { .. }) && closed && stable);
    let _ = handle.shutdown(Duration::from_secs(5));
}

fn failed_resume_probe() {
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")))
    .with_network_logs(
        NetworkLogConfig::new(
            root.path().join("network-logs"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("files")
        .into_iter()
        .next()
        .expect("daily");
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    assert!(matches!(
        handle.release_process_internal(Duration::from_secs(10)),
        ProcessReleaseResult::Released { .. }
    ));
    let network = root.path().join("network-logs/network");
    let saved = root.path().join("saved-network");
    std::fs::rename(&network, &saved).expect("move network directory");
    std::fs::write(&network, b"blocked").expect("block network directory");
    let (failed, observation) = handle.resume_process_observed_internal(Duration::from_secs(10));
    let observation = observation.expect("accepted failed resume observation");
    std::fs::remove_file(&network).expect("remove blocker");
    std::fs::rename(&saved, &network).expect("restore network directory");
    let retained = descriptor_open(&daily) && lease_available(&lease);
    let (rejected, result) = EngineLogActivity::begin(Duration::from_millis(30));
    let fenced = !rejected.is_admitted() && result == Some(SignalResult::TimedOut);
    let stable = failed == handle.resume_process_internal(Duration::from_millis(30));
    let retry = handle
        .retry_process_release_observed_internal(Duration::from_secs(10))
        .0;
    let cleaned = !descriptor_open(&daily) && lease_available(&lease);
    let retry_stable = retry == handle.release_process_internal(Duration::from_millis(30));
    let failed_after_retry =
        observation.wait(Duration::ZERO) == failed && observation.wait(Duration::ZERO) == failed;
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-owner-failed-resume.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"private production process runtime; real daily descriptor and network lease; controlled registration",
            "resume_failed":matches!(failed, ProcessResumeResult::Failed { .. }), "partial_resources_retained":retained,
            "registration_fenced":fenced, "failure_stable":stable,
            "retry_released_with_operation_failure":matches!(retry, ProcessReleaseResult::Released { operation_failed:true, .. }),
            "resources_closed":cleaned, "retry_result_stable":retry_stable, "failed_observation_after_retry":failed_after_retry, "device_acceptance":"not established"
        })).expect("serialize")).expect("write evidence");
    }
    assert!(matches!(failed, ProcessResumeResult::Failed { .. }) && retained && fenced && stable);
    assert!(
        matches!(
            retry,
            ProcessReleaseResult::Released {
                operation_failed: true,
                ..
            }
        ) && cleaned
            && retry_stable
            && failed_after_retry
    );
    let _ = handle.shutdown(Duration::from_secs(5));
}

#[cfg(feature = "test-process-release")]
fn terminal_admission_deadline_probe() {
    use std::sync::mpsc;
    let root = tempfile::tempdir().expect("root");
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.0.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Other,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(root.path().join("daily")))
    .with_network_logs(
        NetworkLogConfig::new(
            root.path().join("network"),
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
        .expect("network"),
    );
    let handle = ProcessObservabilityRuntime::install(config)
        .expect("install")
        .handle();
    uc_warn!(target: "uc_infra_profile::process_owner_probe", error_kind = "baseline", "probe");
    assert_eq!(
        ProcessObservabilityRuntime::flush_local_logs(Duration::from_secs(5)),
        SignalResult::Completed
    );
    let daily = managed_log_files(&root.path().join("daily"))
        .expect("daily")
        .into_iter()
        .next()
        .expect("file");
    let lease = handle
        .prepare_network_log_export(Duration::from_secs(5))
        .expect("export")
        .run_directory
        .join("lease.lock");
    let (entered, waiting) = mpsc::channel();
    let (unlock, blocked) = mpsc::channel();
    let guard_handle = handle.clone();
    let guard = std::thread::spawn(move || guard_handle.probe_hold_terminal_gate(entered, blocked));
    waiting
        .recv_timeout(Duration::from_secs(5))
        .expect("gate held");
    let (done, results) = mpsc::channel();
    let mut workers = Vec::new();
    for (action, timeout_ms) in [
        ("release", 0),
        ("retry", 0),
        ("resume", 0),
        ("release", 40),
        ("retry", 40),
        ("resume", 40),
    ] {
        let owner = handle.clone();
        let done = done.clone();
        workers.push(std::thread::spawn(move || {
            let rejected = match action {
                "release" => matches!(
                    owner.release_process(Duration::from_millis(timeout_ms)),
                    Err(ProcessAdmissionError::DeadlineExceeded)
                ),
                "retry" => matches!(
                    owner.retry_process_release(Duration::from_millis(timeout_ms)),
                    Err(ProcessAdmissionError::DeadlineExceeded)
                ),
                _ => matches!(
                    owner.resume_process(Duration::from_millis(timeout_ms)),
                    Err(ProcessAdmissionError::DeadlineExceeded)
                ),
            };
            done.send((action, timeout_ms, rejected)).expect("return");
        }));
    }
    let mut early = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_millis(400);
    while early.len() < 6 {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            break;
        }
        match results.recv_timeout(left) {
            Ok(result) => early.push(result),
            Err(_) => break,
        }
    }
    let held = descriptor_open(&daily) && !lease_available(&lease);
    unlock.send(()).expect("unlock");
    guard.join().expect("guard");
    for worker in workers {
        worker.join().expect("caller");
    }
    let bounded = early.len() == 6 && early.iter().all(|item| item.2);
    let (final_result, _) = handle
        .release_process(Duration::from_secs(10))
        .expect("accepted release");
    let closed = matches!(
        final_result.result,
        ProcessReleaseResult::Released { generation: 1, .. }
    ) && !descriptor_open(&daily)
        && lease_available(&lease);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("directory");
        std::fs::write(Path::new(&directory).join("terminal-admission-deadline.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual process runtime, held terminal gate, real daily descriptor and network lease", "bounded":bounded, "early":early, "resources_held":held, "final_release":format!("{:?}",final_result.result), "resources_closed":closed, "device_acceptance":"not established"})).expect("json")).expect("artifact");
    }
    assert!(
        bounded && held && closed,
        "unaccepted actions must meet deadlines and a later release must close resources"
    );
}

#[cfg(not(unix))]
fn main() {
    panic!("process owner probe requires Unix FIFOs, lsof, and Python fcntl");
}
