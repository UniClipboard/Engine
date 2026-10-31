use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use uc_engine::{EngineError, EngineErrorCategory};

use uc_engine_uniffi::{
    core_version, flush_process_observability, install_process_observability,
    query_process_observability_health, shutdown_process_observability, BindingAnalyticsContext,
    BindingAnalyticsDeviceType, BindingAnalyticsEvent, BindingAnalyticsGroupIdentify,
    BindingAnalyticsHost, BindingAnalyticsHostError, BindingAnalyticsIdentify,
    BindingAnalyticsIdentityChange, BindingAnalyticsOs, BindingClipboardRepresentation,
    BindingClipboardRestoreMode, BindingClipboardRestoreOutcome, BindingClipboardSnapshot,
    BindingCollectorConfig, BindingConfig, BindingDeploymentEnvironment, BindingEngineState,
    BindingError, BindingErrorCategory, BindingEvent, BindingFileMetadata, BindingHost,
    BindingObservabilityConfig, BindingObservabilitySetupStatus, BindingObservabilitySignalResult,
    BindingOperationTerminal, CustomRelayMutationRejection, HostBindingError, InvitationIssued,
    MobileEngine, MobileStartupLifecycle, RelayEntrySource, RelayRoutingMode, SendReport,
};

static ENGINE_TEST_LOCK: Mutex<()> = Mutex::new(());
const ENGINE_SHUTDOWN_DEADLINE_MS: u64 = 30_000;

#[path = "public_contract/lifecycle_targets.rs"]
mod lifecycle_targets;

#[path = "public_contract/startup_lifecycle.rs"]
mod startup_lifecycle;

#[path = "public_contract/rejected_import.rs"]
mod rejected_import;

#[test]
fn core_version_uses_the_binding_package_version() {
    assert_eq!(core_version(), format!("v{}", env!("CARGO_PKG_VERSION")));
}

#[cfg(feature = "test-process-release")]
#[test]
fn binding_test_loopback_keeps_endpoint_policy() {
    use std::process::Command;
    const CHILD: &str = "UC_TEST_BINDING_LOOPBACK_POLICY_CHILD";
    const MARKER: &str = "UC_TEST_BINDING_OTLP_LOOPBACK";
    if std::env::var_os(CHILD).is_none() {
        for case in ["loopback-disabled", "nonloopback-denied"] {
            let root = tempfile::tempdir().unwrap();
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "binding_test_loopback_keeps_endpoint_policy",
                    "--nocapture",
                ])
                .env(CHILD, case)
                .env("UC_TEST_BINDING_LOOPBACK_POLICY_ROOT", root.path());
            if case == "nonloopback-denied" {
                child.env(MARKER, "1");
            } else {
                child.env_remove(MARKER);
            }
            assert!(child.status().unwrap().success(), "{case}");
        }
        return;
    }
    let case = std::env::var(CHILD).unwrap();
    let root = PathBuf::from(std::env::var_os("UC_TEST_BINDING_LOOPBACK_POLICY_ROOT").unwrap());
    let address = if case == "loopback-disabled" {
        "http://127.0.0.1:4318"
    } else {
        "http://192.0.2.1:4318"
    };
    let result = install_process_observability(
        BindingObservabilityConfig {
            service_version: "1.2.3".into(),
            environment: BindingDeploymentEnvironment::Test,
            app_channel: "test".into(),
            remote_diagnostics_enabled: true,
            collector: Some(BindingCollectorConfig {
                trace_endpoint: format!("{address}/v1/traces"),
                log_endpoint: format!("{address}/v1/logs"),
                auth_header_name: None,
                auth_header_value: None,
            }),
        },
        Arc::new(MemoryHost::new(&root)),
    );
    assert!(matches!(
        result,
        Err(BindingError::ObservabilityConfigInvalid)
    ));
    assert!(matches!(
        query_process_observability_health(),
        Err(BindingError::ObservabilityNotInstalled)
    ));
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        fs::create_dir_all(&directory).unwrap();
        fs::write(Path::new(&directory).join(format!("binding-test-{case}.json")), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding endpoint validation with test feature", "case":case, "invalid_configuration":true, "process_not_installed":true, "native_device":"not established"})).unwrap()).unwrap();
    }
}

#[cfg(feature = "test-process-release")]
#[test]
fn binding_process_release_waits_for_accepted_remote_span() {
    use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
    use prost::Message;
    use std::process::Command;
    use std::time::Duration;
    use uc_engine::observability::diagnostics::{
        complete_operation, operation_span, DiagnosticDomain, DiagnosticOperation, DiagnosticRole,
        DiagnosticSpanKind, OperationCompletion, OperationContext,
    };
    use uc_engine_uniffi::{
        probe_process_release, probe_process_release_report, probe_process_resume,
    };
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };
    const CHILD: &str = "UC_TEST_BINDING_REMOTE_RELEASE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "binding_process_release_waits_for_accepted_remote_span",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .env("UC_TEST_BINDING_OTLP_LOOPBACK", "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
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
    let root = PathBuf::from(std::env::var_os(CHILD).unwrap());
    let host = Arc::new(MemoryHost::new(&root));
    let setup = install_process_observability(
        BindingObservabilityConfig {
            service_version: "1.2.3".into(),
            environment: BindingDeploymentEnvironment::Test,
            app_channel: "test".into(),
            remote_diagnostics_enabled: true,
            collector: Some(BindingCollectorConfig {
                trace_endpoint: format!("{}/v1/traces", receiver.uri()),
                log_endpoint: format!("{}/v1/logs", receiver.uri()),
                auth_header_name: None,
                auth_header_value: None,
            }),
        },
        host.clone(),
    )
    .unwrap();
    assert_eq!(setup.remote, BindingObservabilitySetupStatus::Ready);
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "remote-release".into(),
        },
        host,
    )
    .unwrap();
    assert_eq!(
        flush_process_observability(10_000).unwrap().traces,
        BindingObservabilitySignalResult::Completed
    );
    let count_spans = || {
        executor
            .block_on(receiver.received_requests())
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/v1/traces")
            .flat_map(|request| {
                ExportTraceServiceRequest::decode(request.body.as_slice())
                    .unwrap()
                    .resource_spans
            })
            .flat_map(|resource| resource.scope_spans)
            .flat_map(|scope| scope.spans)
            .filter(|span| span.name == "profile_storage_upgrade")
            .count()
    };
    let before = count_spans();
    let span = || {
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
    let held = span();
    {
        let _entered = held.enter();
        complete();
    }
    let pending = probe_process_release(0).unwrap();
    assert_eq!(pending.0, "pending");
    engine.shutdown(30_000).unwrap();
    let waiting = probe_process_release(100).unwrap();
    assert_eq!(waiting, pending);
    let rejected = span();
    {
        let _entered = rejected.enter();
        complete();
    }
    drop(rejected);
    drop(held);
    let released = probe_process_release(30_000).unwrap();
    assert_eq!(released.0, "released");
    assert_eq!(released.1, pending.1);
    let after_release = count_spans();
    assert_eq!(after_release, before + 1);
    let resumed = probe_process_resume(30_000).unwrap();
    assert_eq!(resumed.0, "resumed");
    assert!(resumed.1 > released.1);
    let next = span();
    {
        let _entered = next.enter();
        complete();
    }
    drop(next);
    assert_eq!(
        flush_process_observability(10_000).unwrap().traces,
        BindingObservabilitySignalResult::Completed
    );
    let after_resume = count_spans();
    assert_eq!(after_resume, after_release + 1);
    executor.block_on(async {
        receiver.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/traces"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(3)))
            .mount(&receiver)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/logs"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&receiver)
            .await;
    });
    let delayed = span();
    {
        let _entered = delayed.enter();
        complete();
    }
    drop(delayed);
    let flush = std::thread::spawn(|| flush_process_observability(10_000).unwrap());
    let limit = std::time::Instant::now() + Duration::from_secs(5);
    while count_spans() == 0 && std::time::Instant::now() < limit {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(count_spans(), 1);
    assert!(!flush.is_finished());
    let blocked_release = probe_process_release(50).unwrap();
    assert_eq!(blocked_release.0, "pending");
    let flushed = flush.join().unwrap();
    assert_eq!(flushed.traces, BindingObservabilitySignalResult::Completed);
    let final_release = probe_process_release(30_000).unwrap();
    assert_eq!(final_release.0, "released");
    assert_eq!(final_release.1, blocked_release.1);
    assert_eq!(count_spans(), 1);
    executor.block_on(async {
        receiver.reset().await;
        Mock::given(method("POST"))
            .and(path("/v1/traces"))
            .respond_with(ResponseTemplate::new(400))
            .mount(&receiver)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/logs"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&receiver)
            .await;
    });
    assert_eq!(probe_process_resume(30_000).unwrap().0, "resumed");
    let rejected_export = span();
    {
        let _entered = rejected_export.enter();
        complete();
    }
    let rejection_pending = probe_process_release_report(0).unwrap();
    assert_eq!(rejection_pending.0, "pending");
    drop(rejected_export);
    let rejection_report = probe_process_release_report(30_000).unwrap();
    assert_eq!(
        rejection_report,
        (
            "released".to_owned(),
            rejection_pending.1,
            vec![],
            vec!["TraceExport".to_owned()],
            false
        )
    );
    assert_eq!(count_spans(), 1);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        fs::create_dir_all(&directory).unwrap();
        fs::write(Path::new(&directory).join("binding-process-remote-span.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding installation, real Engine, synthetic approved span, controlled loopback OTLP receiver", "pending_with_engine":pending, "pending_after_engine":waiting, "released":released, "synthetic_spans_before":before, "synthetic_spans_after_release":after_release, "fenced_span_rejected":true, "resumed":resumed, "synthetic_spans_after_resume":after_resume, "accepted_export_pending":true, "blocked_release":blocked_release, "final_release":final_release, "rejection_report":rejection_report, "rejected_span_requests":count_spans(), "production_collector":"not established", "native_device":"not established"})).unwrap()).unwrap();
    }
}

#[cfg(all(feature = "test-process-release", target_os = "macos"))]
#[test]
fn binding_process_release_waits_for_two_engines_and_resumes() {
    use std::process::Command;
    use uc_engine_uniffi::{
        configure_network_log, prepare_network_log_export, probe_fail_next_network_park,
        probe_fenced_engine_attempts, probe_hold_process_terminal_gate, probe_network_park_entered,
        probe_network_park_hold, probe_process_release, probe_process_release_report,
        probe_process_resume, probe_process_retry_report, probe_retry_join_count,
        release_process_observability, resume_process_observability,
        retry_process_observability_release, BindingNetworkLogConfig, BindingNetworkProcessRole,
        BindingProcessFailure, BindingProcessReleaseState, BindingProcessResumeState,
    };
    const CHILD: &str = "UC_TEST_BINDING_PROCESS_RELEASE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "binding_process_release_waits_for_two_engines_and_resumes",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = PathBuf::from(std::env::var_os(CHILD).unwrap());
    let host = Arc::new(MemoryHost::new(&root));
    let second_host = Arc::new(MemoryHost::new(&root.join("second")));
    configure_network_log(BindingNetworkLogConfig {
        role: BindingNetworkProcessRole::MobileApp,
        record_bytes: 1024,
        queue_records: 32,
        file_bytes: 8192,
        run_bytes: 32768,
        root_bytes: 131072,
        retention_seconds: 60,
    })
    .unwrap();
    let setup = install_process_observability(
        BindingObservabilityConfig {
            service_version: "1.2.3".into(),
            environment: BindingDeploymentEnvironment::Test,
            app_channel: "test".into(),
            remote_diagnostics_enabled: false,
            collector: None,
        },
        host.clone(),
    )
    .unwrap();
    assert_eq!(setup.local_file, BindingObservabilitySetupStatus::Ready);
    let first = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "process-first".into(),
        },
        host.clone(),
    )
    .unwrap();
    let second = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".into(),
            profile_id: "process-second".into(),
        },
        second_host,
    )
    .unwrap();
    let daily = uc_engine::observability::managed_log_files(&host.private_data.join("logs"))
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let network =
        PathBuf::from(prepare_network_log_export(5_000).unwrap().run_directory).join("lease.lock");
    let lease_available = || {
        let status = Command::new("python3").args(["-c", "import fcntl,sys\nf=open(sys.argv[1], 'r+')\ntry:\n fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)\nexcept BlockingIOError:\n sys.exit(1)"]).arg(&network).status().unwrap();
        assert!(matches!(status.code(), Some(0 | 1)));
        status.success()
    };
    let descriptor_open = || {
        let output = Command::new("lsof")
            .args(["-n", "-P", "-F", "n", "-p", &std::process::id().to_string()])
            .output()
            .unwrap();
        assert!(output.status.success());
        let path = daily.canonicalize().unwrap();
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .any(|line| line.strip_prefix('n') == path.to_str())
    };
    assert!(descriptor_open());
    assert!(!lease_available());
    let first_wait = probe_process_release(0).unwrap();
    assert_eq!(first_wait.0, "pending");
    let pending_report = probe_process_release_report(0).unwrap();
    assert_eq!(
        pending_report,
        ("pending".to_owned(), first_wait.1, vec![], vec![], false)
    );
    let (waiting_started, waiting_entered) = std::sync::mpsc::channel();
    let long_waiter = std::thread::spawn(move || {
        waiting_started.send(()).unwrap();
        probe_process_release(30_000).unwrap()
    });
    waiting_entered
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let previous_attempts = probe_fenced_engine_attempts();
    let later_host = Arc::new(MemoryHost::new(&root.join("later")));
    let (starting, started) = std::sync::mpsc::channel();
    let later_worker = std::thread::spawn(move || {
        let result = MobileEngine::start(
            BindingConfig {
                app_version: "1.2.3".into(),
                profile_id: "process-later".into(),
            },
            later_host,
        );
        let _ = starting.send(result);
    });
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while probe_fenced_engine_attempts() == previous_attempts && std::time::Instant::now() < limit {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(probe_fenced_engine_attempts() > previous_attempts);
    assert!(matches!(
        started.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    first.shutdown(30_000).unwrap();
    let second_wait = probe_process_release(50).unwrap();
    assert_eq!(second_wait, first_wait);
    assert!(!long_waiter.is_finished());
    assert!(descriptor_open());
    assert!(!lease_available());
    second.shutdown(30_000).unwrap();
    let released = probe_process_release(30_000).unwrap();
    assert_eq!(released.0, "released");
    assert_eq!(released.1, first_wait.1);
    assert_eq!(long_waiter.join().unwrap(), released);
    let released_report = probe_process_release_report(0).unwrap();
    assert_eq!(
        released_report,
        ("released".to_owned(), released.1, vec![], vec![], false)
    );
    assert!(!descriptor_open());
    assert!(lease_available());
    assert!(matches!(
        started.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    let _ = flush_process_observability(250).unwrap();
    assert!(!descriptor_open());
    assert!(lease_available());
    let resumed = probe_process_resume(30_000).unwrap();
    assert_eq!(resumed.0, "resumed");
    assert!(resumed.1 > released.1);
    assert!(descriptor_open());
    assert!(!lease_available());
    let later = started
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .unwrap();
    later_worker.join().unwrap();
    later.shutdown(30_000).unwrap();
    let final_release = probe_process_release(30_000).unwrap();
    assert_eq!(final_release.0, "released");
    assert!(final_release.1 > resumed.1);
    assert!(!descriptor_open());
    assert!(lease_available());
    let recovered = probe_process_resume(30_000).unwrap();
    assert_eq!(recovered.0, "resumed");
    assert!(!lease_available());
    probe_fail_next_network_park().unwrap();
    let failed_report = probe_process_release_report(30_000).unwrap();
    assert_eq!(failed_report.0, "failed");
    assert_eq!(failed_report.2, vec!["NetworkFile"]);
    assert!(failed_report.4);
    assert!(!lease_available());
    probe_network_park_hold(true).unwrap();
    let retry_pending = probe_process_retry_report(50).unwrap();
    assert_eq!(retry_pending.0, "pending");
    assert!(retry_pending.1 > failed_report.1);
    assert_eq!(
        (
            retry_pending.2.clone(),
            retry_pending.3.clone(),
            retry_pending.4
        ),
        (vec![], vec![], false)
    );
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !probe_network_park_entered().unwrap() && std::time::Instant::now() < limit {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(probe_network_park_entered().unwrap());
    assert!(!lease_available());
    let joined_before = probe_retry_join_count().unwrap();
    let waiter = std::thread::spawn(|| probe_process_retry_report(30_000).unwrap());
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while probe_retry_join_count().unwrap() == joined_before && std::time::Instant::now() < limit {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(probe_retry_join_count().unwrap() > joined_before);
    let queued_resume = probe_process_resume(0).unwrap();
    assert_eq!(queued_resume.0, "pending");
    assert!(queued_resume.1 > retry_pending.1);
    probe_network_park_hold(false).unwrap();
    let retry = waiter.join().unwrap();
    assert_eq!(retry.0, "released");
    assert_eq!(retry.1, retry_pending.1);
    assert_eq!(retry.2, Vec::<String>::new());
    assert_eq!(retry.3, vec!["NetworkFile"]);
    assert!(!retry.4);
    let resumed_after_retry = probe_process_resume(30_000).unwrap();
    assert_eq!(resumed_after_retry, ("resumed".to_owned(), queued_resume.1));
    assert!(!lease_available());
    let retry_report = retry.clone();
    let final_after_retry = probe_process_release_report(30_000).unwrap();
    assert_eq!(final_after_retry.0, "released");
    assert!(final_after_retry.1 > resumed_after_retry.1);
    assert!(lease_available());
    let active_again = resume_process_observability(30_000).unwrap();
    assert_eq!(
        active_again.report.state,
        BindingProcessResumeState::Resumed
    );
    probe_network_park_hold(true).unwrap();
    let old_release = release_process_observability(50).unwrap();
    assert_eq!(
        old_release.report.state,
        BindingProcessReleaseState::Pending
    );
    let old_observation = old_release.observation.expect("accepted release");
    let limit = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !probe_network_park_entered().unwrap() && std::time::Instant::now() < limit {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(probe_network_park_entered().unwrap());
    let resume = resume_process_observability(0).unwrap();
    let resume_before = resume.report;
    assert_eq!(resume_before.state, BindingProcessResumeState::Pending);
    let resume_observation = resume.observation.expect("accepted resume");
    let newer_release = release_process_observability(0).unwrap();
    assert_eq!(
        newer_release.report.state,
        BindingProcessReleaseState::Pending
    );
    assert!(
        newer_release.report.generation > resume_before.generation
            && resume_before.generation > old_release.report.generation
    );
    let newer_observation = newer_release.observation.expect("accepted newer release");
    probe_network_park_hold(false).unwrap();
    let resume_after = resume_observation.wait(30_000);
    assert_eq!(
        (resume_after.state, resume_after.generation),
        (
            BindingProcessResumeState::Superseded,
            resume_before.generation
        )
    );
    let newer_settled = newer_observation.wait(30_000);
    assert_eq!(
        (newer_settled.state, newer_settled.generation),
        (
            BindingProcessReleaseState::Released,
            newer_release.report.generation
        )
    );
    let repeated_resume = resume_observation.wait(0);
    assert_eq!(
        (repeated_resume.state, repeated_resume.generation),
        (resume_after.state, resume_after.generation)
    );
    let old_settled = old_observation.wait(0);
    assert_eq!(
        (old_settled.state, old_settled.generation),
        (
            BindingProcessReleaseState::Released,
            old_release.report.generation
        )
    );
    assert!(lease_available());
    let reopened = resume_process_observability(30_000).unwrap();
    assert_eq!(reopened.report.state, BindingProcessResumeState::Resumed);
    probe_fail_next_network_park().unwrap();
    let public_failure = release_process_observability(30_000).unwrap();
    assert_eq!(
        public_failure.report.state,
        BindingProcessReleaseState::Failed
    );
    assert_eq!(
        public_failure.report.cleanup_errors,
        vec![BindingProcessFailure::NetworkFile]
    );
    assert!(public_failure.report.explicit_retry_supported);
    assert!(!lease_available());
    let public_retry = retry_process_observability_release(30_000).unwrap();
    assert_eq!(
        public_retry.report.state,
        BindingProcessReleaseState::Released
    );
    assert!(public_retry.report.generation > public_failure.report.generation);
    assert_eq!(
        public_retry.report.operation_errors,
        vec![BindingProcessFailure::NetworkFile]
    );
    assert!(public_retry.report.cleanup_errors.is_empty());
    assert!(lease_available());
    let failed_observation = public_failure
        .observation
        .expect("failed attempt observation");
    let old_failed = failed_observation.wait(0);
    assert_eq!(
        (old_failed.state, old_failed.generation),
        (
            BindingProcessReleaseState::Failed,
            public_failure.report.generation
        )
    );
    assert!(!old_failed.explicit_retry_supported);
    let reopened = resume_process_observability(30_000).unwrap();
    assert_eq!(reopened.report.state, BindingProcessResumeState::Resumed);
    let (entered, waiting) = std::sync::mpsc::channel();
    let (unlock, blocked) = std::sync::mpsc::channel();
    let gate =
        std::thread::spawn(move || probe_hold_process_terminal_gate(entered, blocked).unwrap());
    waiting
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let deadline_error = BindingError::Engine {
        code: 1002,
        category: BindingErrorCategory::DeadlineExceeded,
        retryable: true,
    };
    let (sent, returned) = std::sync::mpsc::channel();
    let mut callers = Vec::new();
    for action in ["release", "retry", "resume"] {
        let sent = sent.clone();
        callers.push(std::thread::spawn(move || {
            let result = match action {
                "release" => release_process_observability(40).map(|_| ()),
                "retry" => retry_process_observability_release(40).map(|_| ()),
                _ => resume_process_observability(40).map(|_| ()),
            };
            let _ = sent.send((action, result));
        }));
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut early = Vec::new();
    while early.len() < 3 {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match returned.recv_timeout(remaining) {
            Ok(result) => early.push(result),
            Err(_) => break,
        }
    }
    let returned_in_time = early.len() == 3;
    let release_rejected = early.iter().any(|(action, result)| {
        *action == "release" && result.as_ref().err() == Some(&deadline_error)
    });
    let retry_rejected = early.iter().any(|(action, result)| {
        *action == "retry" && result.as_ref().err() == Some(&deadline_error)
    });
    let resume_rejected = early.iter().any(|(action, result)| {
        *action == "resume" && result.as_ref().err() == Some(&deadline_error)
    });
    let lease_held = !lease_available();
    unlock.send(()).unwrap();
    gate.join().unwrap();
    for caller in callers {
        caller.join().unwrap();
    }
    assert!(
        release_rejected && retry_rejected && resume_rejected && returned_in_time && lease_held
    );
    let final_public_release = release_process_observability(30_000).unwrap();
    assert_eq!(
        final_public_release.report.state,
        BindingProcessReleaseState::Released
    );
    assert!(lease_available());
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        fs::create_dir_all(&directory).unwrap();
        fs::write(Path::new(&directory).join("binding-process-release.json"), serde_json::to_vec_pretty(&serde_json::json!({"scope":"actual binding process install, three Engines, real daily log descriptor and network lease", "first_wait":first_wait, "pending_report":pending_report, "second_wait":second_wait, "independent_waiters_same_release":true, "fenced_engine_waited":true, "released":released, "released_report":released_report, "failed_report":failed_report, "retry_pending":retry_pending, "queued_resume":queued_resume, "retry_report":retry_report, "resumed_after_retry":resumed_after_retry, "final_after_retry":final_after_retry, "old_release":format!("{:?}",old_release.report), "resume_before":format!("{resume_before:?}"), "resume_after":format!("{resume_after:?}"), "newer_release":format!("{:?}",newer_release.report), "newer_settled":format!("{newer_settled:?}"), "formal_binding_interface":true, "admission_deadline_errors":release_rejected && retry_rejected && resume_rejected, "admission_returned_in_time":returned_in_time, "final_public_release":format!("{:?}",final_public_release.report), "public_failure":format!("{:?}",public_failure.report), "public_retry":format!("{:?}",public_retry.report), "old_failed":format!("{old_failed:?}"), "daily_and_lease_closed":true, "late_flush_kept_closed":true, "resumed":resumed, "waiting_engine_admitted_after_resume":true, "daily_and_lease_reopened":true, "final_release":final_release, "final_daily_and_lease_closed":true, "remote_collector":"not configured", "native_device":"not established"})).unwrap()).unwrap();
    }
}

#[test]
fn process_release_public_contract_without_test_features() {
    use std::process::Command;
    use uc_engine_uniffi::{
        release_process_observability, resume_process_observability, BindingProcessReleaseState,
        BindingProcessResumeState,
    };
    const CHILD: &str = "UC_PUBLIC_PROCESS_RELEASE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process_release_public_contract_without_test_features",
                "--nocapture",
            ])
            .env(CHILD, root.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let root = PathBuf::from(std::env::var_os(CHILD).unwrap());
    let host = Arc::new(MemoryHost::new(&root));
    install_process_observability(
        BindingObservabilityConfig {
            service_version: "1.2.3".to_owned(),
            environment: BindingDeploymentEnvironment::Test,
            app_channel: "test".to_owned(),
            remote_diagnostics_enabled: false,
            collector: None,
        },
        host.clone(),
    )
    .unwrap();
    let startup = uc_engine_uniffi::MobileStartupLifecycle::new();
    let startup_before = startup.wait_release(0);
    let engine = MobileEngine::start_with_lifecycle(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "public-process".to_owned(),
        },
        host,
        startup.clone(),
    )
    .unwrap();
    let startup_delivered = startup.wait_release(30_000);
    assert_eq!(
        startup_delivered.state,
        uc_engine_uniffi::BindingStartupReleaseState::Delivered
    );
    assert_eq!(startup_delivered.attempt_id, startup_before.attempt_id);
    let release = release_process_observability(20).unwrap();
    assert_eq!(release.report.state, BindingProcessReleaseState::Pending);
    let observation = release.observation.expect("accepted release");
    engine.shutdown(30_000).unwrap();
    let released = observation.wait(30_000);
    assert_eq!(released.state, BindingProcessReleaseState::Released);
    assert_eq!(released.generation, release.report.generation);
    let resume = resume_process_observability(30_000).unwrap();
    assert_eq!(resume.report.state, BindingProcessResumeState::Resumed);
    assert!(resume.report.generation > released.generation);
    let final_release = release_process_observability(30_000).unwrap();
    assert_eq!(
        final_release.report.state,
        BindingProcessReleaseState::Released
    );
    assert!(final_release.report.generation > resume.report.generation);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        fs::create_dir_all(&directory).unwrap();
        fs::write(Path::new(&directory).join("binding-public-process-production.json"),
            serde_json::to_vec_pretty(&serde_json::json!({"scope":"production binding, actual Engine and process observability", "startup_before":format!("{startup_before:?}"), "startup_delivered":format!("{startup_delivered:?}"), "initial":format!("{:?}", release.report), "released":format!("{released:?}"), "resume":format!("{:?}",resume.report), "final_release":format!("{:?}",final_release.report), "fault_injection":"none", "device_acceptance":"not established"})).unwrap()).unwrap();
    }
}

#[test]
fn process_observability_is_host_owned_reused_and_lifecycle_safe() {
    let _guard = lock(&ENGINE_TEST_LOCK);
    let directory = tempfile::tempdir().expect("temporary directory");
    let host = Arc::new(MemoryHost::new(directory.path()));
    fs::create_dir_all(&host.private_data).expect("private data directory");
    fs::write(host.private_data.join("logs"), b"blocks log directory").expect("blocking log file");
    let config = BindingObservabilityConfig {
        service_version: "1.2.3".to_owned(),
        environment: BindingDeploymentEnvironment::Test,
        app_channel: "test".to_owned(),
        remote_diagnostics_enabled: true,
        collector: Some(BindingCollectorConfig {
            trace_endpoint: "https://127.0.0.1:9/v1/traces".to_owned(),
            log_endpoint: "https://127.0.0.1:9/v1/logs".to_owned(),
            auth_header_name: Some("authorization".to_owned()),
            auth_header_value: Some("Bearer binding-private-token".to_owned()),
        }),
    };

    let debug = format!("{config:?}");
    assert!(!debug.contains("127.0.0.1"));
    assert!(!debug.contains("binding-private-token"));
    assert!(!debug.contains("1.2.3"));

    let setup =
        install_process_observability(config.clone(), host.clone()).expect("observability install");
    assert!(!setup.reused);
    assert_eq!(setup.remote, BindingObservabilitySetupStatus::Ready);
    assert_eq!(
        setup.local_file,
        BindingObservabilitySetupStatus::Unavailable
    );

    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "observability-lifecycle".to_owned(),
        },
        host.clone(),
    )
    .expect("log directory failure must not prevent engine startup");
    engine.suspend().expect("suspend");
    engine.resume().expect("resume without reinstall");
    engine.shutdown(30_000).expect("engine shutdown");

    let reused = install_process_observability(config.clone(), host.clone())
        .expect("same process config must be reused");
    assert!(reused.reused);
    let after_engine_shutdown =
        flush_process_observability(25).expect("engine shutdown must keep process runtime alive");
    assert_ne!(
        after_engine_shutdown.traces,
        BindingObservabilitySignalResult::AlreadyShutdown
    );
    assert_ne!(
        after_engine_shutdown.logs,
        BindingObservabilitySignalResult::AlreadyShutdown
    );
    let restarted = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "observability-lifecycle".to_owned(),
        },
        host.clone(),
    )
    .expect("second engine must reuse the process runtime");
    restarted.shutdown(30_000).expect("second engine shutdown");

    let mut conflicting = config;
    conflicting.app_channel = "development".to_owned();
    assert!(matches!(
        install_process_observability(conflicting, host),
        Err(BindingError::ObservabilityConfigConflict)
    ));

    let _ = flush_process_observability(25).expect("bounded flush");
    let health = query_process_observability_health().expect("observable process health");
    assert!(health.failed_remote_span_batches > 0 || health.failed_remote_log_batches > 0);
    assert_eq!(health.remote_setup_failure, None);
    let _ = shutdown_process_observability(250).expect("process shutdown");
    let after_shutdown = flush_process_observability(25).expect("closed runtime summary");
    assert_eq!(
        after_shutdown.traces,
        BindingObservabilitySignalResult::AlreadyShutdown
    );
    assert_eq!(
        after_shutdown.logs,
        BindingObservabilitySignalResult::AlreadyShutdown
    );
}

#[test]
fn engine_errors_keep_their_stable_code_category_and_retryability() {
    let cases = [
        (
            EngineErrorCategory::InvalidInput,
            BindingErrorCategory::InvalidInput,
        ),
        (
            EngineErrorCategory::InvalidState,
            BindingErrorCategory::InvalidState,
        ),
        (
            EngineErrorCategory::Unauthorized,
            BindingErrorCategory::Unauthorized,
        ),
        (
            EngineErrorCategory::NotFound,
            BindingErrorCategory::NotFound,
        ),
        (
            EngineErrorCategory::Conflict,
            BindingErrorCategory::Conflict,
        ),
        (
            EngineErrorCategory::Unavailable,
            BindingErrorCategory::Unavailable,
        ),
        (
            EngineErrorCategory::DeadlineExceeded,
            BindingErrorCategory::DeadlineExceeded,
        ),
        (
            EngineErrorCategory::Internal,
            BindingErrorCategory::Internal,
        ),
    ];

    for (index, (engine_category, binding_category)) in cases.into_iter().enumerate() {
        let code = 2000 + index as u32;
        let retryable = index % 2 == 0;
        assert_eq!(
            BindingError::from(EngineError::new(code, engine_category, retryable)),
            BindingError::Engine {
                code,
                category: binding_category,
                retryable,
            }
        );
    }

    assert_eq!(
        BindingError::from(EngineError::new(1295, EngineErrorCategory::Conflict, false,)),
        BindingError::Engine {
            code: 1295,
            category: BindingErrorCategory::Conflict,
            retryable: false,
        }
    );
}

struct MemoryHost {
    private_data: PathBuf,
    cache: PathBuf,
    temporary: PathBuf,
    values: Mutex<HashMap<String, Vec<u8>>>,
    files: Mutex<HashMap<String, TestFile>>,
    clipboard: Mutex<BindingClipboardSnapshot>,
    clipboard_writes: Mutex<Vec<BindingClipboardSnapshot>>,
    finished_files: Mutex<Vec<String>>,
    secure_read_gate: Mutex<Option<lifecycle_targets::ReadGate>>,
}

struct TestFile {
    metadata: BindingFileMetadata,
    bytes: Vec<u8>,
}

impl MemoryHost {
    fn new(root: &Path) -> Self {
        Self {
            private_data: root.join("data"),
            cache: root.join("cache"),
            temporary: root.join("temporary"),
            values: Mutex::new(HashMap::new()),
            files: Mutex::new(HashMap::new()),
            clipboard: Mutex::new(BindingClipboardSnapshot {
                identity: None,
                observed_at_ms: 0,
                representations: Vec::new(),
            }),
            clipboard_writes: Mutex::new(Vec::new()),
            finished_files: Mutex::new(Vec::new()),
            secure_read_gate: Mutex::new(None),
        }
    }

    fn values(&self) -> MutexGuard<'_, HashMap<String, Vec<u8>>> {
        match self.values.lock() {
            Ok(values) => values,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn register_input_file(
        &self,
        handle: &str,
        display_name: &str,
        mime_type: Option<&str>,
        bytes: Vec<u8>,
    ) {
        self.files().insert(
            handle.to_owned(),
            TestFile {
                metadata: BindingFileMetadata {
                    display_name: display_name.to_owned(),
                    size_bytes: bytes.len() as u64,
                    mime_type: mime_type.map(str::to_owned),
                },
                bytes,
            },
        );
    }

    fn files(&self) -> MutexGuard<'_, HashMap<String, TestFile>> {
        match self.files.lock() {
            Ok(files) => files,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn file_bytes(&self, handle: &str) -> Option<Vec<u8>> {
        self.files().get(handle).map(|file| file.bytes.clone())
    }

    fn file_finished(&self, handle: &str) -> bool {
        lock(&self.finished_files)
            .iter()
            .any(|finished| finished == handle)
    }

    fn set_clipboard(&self, snapshot: BindingClipboardSnapshot) {
        *lock(&self.clipboard) = snapshot;
    }

    fn clipboard_writes(&self) -> MutexGuard<'_, Vec<BindingClipboardSnapshot>> {
        lock(&self.clipboard_writes)
    }
}

impl BindingHost for MemoryHost {
    fn private_data_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.private_data.to_string_lossy().into_owned())
    }

    fn cache_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.cache.to_string_lossy().into_owned())
    }

    fn temporary_directory(&self) -> Result<String, HostBindingError> {
        Ok(self.temporary.to_string_lossy().into_owned())
    }

    fn secure_storage_get(&self, key: String) -> Result<Option<Vec<u8>>, HostBindingError> {
        let gate = {
            let mut gate = lock(&self.secure_read_gate);
            if gate.as_mut().is_some_and(|gate| gate.matches(&key)) {
                // 同一受控密钥的并发读取都等待放行，不能被成员维护抢走唯一阻塞机会。
                gate.clone()
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            gate.wait();
        }
        Ok(self.values().get(&key).cloned())
    }

    fn secure_storage_set(&self, key: String, value: Vec<u8>) -> Result<(), HostBindingError> {
        self.values().insert(key, value);
        Ok(())
    }

    fn secure_storage_delete(&self, key: String) -> Result<(), HostBindingError> {
        self.values().remove(&key);
        Ok(())
    }

    fn file_metadata(&self, handle: String) -> Result<BindingFileMetadata, HostBindingError> {
        self.files()
            .get(&handle)
            .map(|file| file.metadata.clone())
            .ok_or(HostBindingError::InvalidHandle)
    }

    fn file_read_chunk(
        &self,
        handle: String,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostBindingError> {
        let files = self.files();
        let file = files.get(&handle).ok_or(HostBindingError::InvalidHandle)?;
        let start = usize::try_from(offset).map_err(|_| HostBindingError::InvalidHandle)?;
        let max_bytes = usize::try_from(max_bytes).map_err(|_| HostBindingError::InvalidHandle)?;
        let end = start.saturating_add(max_bytes).min(file.bytes.len());
        file.bytes
            .get(start..end)
            .map(<[u8]>::to_vec)
            .ok_or(HostBindingError::InvalidHandle)
    }

    fn file_write_chunk(
        &self,
        handle: String,
        offset: u64,
        bytes: Vec<u8>,
    ) -> Result<(), HostBindingError> {
        let mut files = self.files();
        let file = files
            .get_mut(&handle)
            .ok_or(HostBindingError::InvalidHandle)?;
        let start = usize::try_from(offset).map_err(|_| HostBindingError::InvalidHandle)?;
        let end = start
            .checked_add(bytes.len())
            .ok_or(HostBindingError::InvalidHandle)?;
        if file.bytes.len() < end {
            file.bytes.resize(end, 0);
        }
        file.bytes[start..end].copy_from_slice(&bytes);
        Ok(())
    }

    fn file_finish_write(&self, handle: String) -> Result<(), HostBindingError> {
        if !self.files().contains_key(&handle) {
            return Err(HostBindingError::InvalidHandle);
        }
        lock(&self.finished_files).push(handle);
        Ok(())
    }

    fn clipboard_read(&self) -> Result<BindingClipboardSnapshot, HostBindingError> {
        Ok(lock(&self.clipboard).clone())
    }

    fn clipboard_write(
        &self,
        snapshot: BindingClipboardSnapshot,
    ) -> Result<Option<Vec<u8>>, HostBindingError> {
        self.clipboard_writes().push(snapshot);
        Ok(None)
    }
}

struct RecordingAnalyticsHost {
    events: Mutex<Vec<BindingAnalyticsEvent>>,
    identifies: Mutex<Vec<BindingAnalyticsIdentify>>,
    group_identifies: Mutex<Vec<BindingAnalyticsGroupIdentify>>,
    anonymous_id: Mutex<String>,
    space_person_id: Mutex<Option<String>>,
}

impl RecordingAnalyticsHost {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            identifies: Mutex::new(Vec::new()),
            group_identifies: Mutex::new(Vec::new()),
            anonymous_id: Mutex::new("018f47de-4a26-7d31-a7d8-93c20d64f813".to_owned()),
            space_person_id: Mutex::new(None),
        }
    }

    fn event_names(&self) -> Vec<String> {
        lock(&self.events)
            .iter()
            .map(|event| event.name.clone())
            .collect()
    }
}

impl BindingAnalyticsHost for RecordingAnalyticsHost {
    fn capture(&self, event: BindingAnalyticsEvent) -> Result<(), BindingAnalyticsHostError> {
        lock(&self.events).push(event);
        Ok(())
    }

    fn identify(&self, payload: BindingAnalyticsIdentify) -> Result<(), BindingAnalyticsHostError> {
        lock(&self.identifies).push(payload);
        Ok(())
    }

    fn group_identify(
        &self,
        payload: BindingAnalyticsGroupIdentify,
    ) -> Result<(), BindingAnalyticsHostError> {
        lock(&self.group_identifies).push(payload);
        Ok(())
    }

    fn adopt_space_person(
        &self,
        space_person_id: String,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError> {
        let mut current = lock(&self.space_person_id);
        let previous_distinct_id = current
            .clone()
            .unwrap_or_else(|| lock(&self.anonymous_id).clone());
        *current = Some(space_person_id.clone());
        Ok(BindingAnalyticsIdentityChange {
            previous_distinct_id,
            new_distinct_id: space_person_id,
        })
    }

    fn release_space_person(
        &self,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError> {
        let mut current = lock(&self.space_person_id);
        let anonymous_id = lock(&self.anonymous_id).clone();
        let previous_distinct_id = current.take().unwrap_or_else(|| anonymous_id.clone());
        Ok(BindingAnalyticsIdentityChange {
            previous_distinct_id,
            new_distinct_id: anonymous_id,
        })
    }

    fn current_space_person_id(&self) -> Result<Option<String>, BindingAnalyticsHostError> {
        Ok(lock(&self.space_person_id).clone())
    }

    fn reset_telemetry_identity(
        &self,
    ) -> Result<BindingAnalyticsIdentityChange, BindingAnalyticsHostError> {
        let mut current = lock(&self.space_person_id);
        let mut anonymous_id = lock(&self.anonymous_id);
        let previous_distinct_id = current.take().unwrap_or_else(|| anonymous_id.clone());
        *anonymous_id = "018f47de-4a26-7d31-a7d8-93c20d64f814".to_owned();
        Ok(BindingAnalyticsIdentityChange {
            previous_distinct_id,
            new_distinct_id: anonymous_id.clone(),
        })
    }
}

#[test]
fn mobile_host_can_create_a_space_with_analytics_and_shutdown_through_the_binding() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let analytics = Arc::new(RecordingAnalyticsHost::new());
    let engine = MobileEngine::start_with_analytics(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-contract".to_owned(),
        },
        host.clone(),
        analytics.clone(),
        BindingAnalyticsContext {
            os: BindingAnalyticsOs::Ios,
            os_version: "18.0".to_owned(),
            device_type: BindingAnalyticsDeviceType::Mobile,
            arch: "arm64".to_owned(),
            app_channel: "development".to_owned(),
        },
    )
    .expect("analytics-enabled binding engine must start");

    let created = engine
        .create_space(
            Some("mobile-contract-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must return the create-space result");
    assert!(!created.space_id.is_empty());
    assert!(!created.self_device_id.is_empty());
    assert!(!created.identity_fingerprint.is_empty());
    assert!(
        !host.values().is_empty(),
        "create-space must persist secrets through the host callback"
    );

    let event_names = analytics.event_names();
    assert!(event_names.iter().any(|name| name == "setup_started"));
    assert!(event_names.iter().any(|name| name == "device_name_set"));
    assert!(event_names.iter().any(|name| name == "setup_completed"));
    let device_name_event = lock(&analytics.events)
        .iter()
        .find(|event| event.name == "device_name_set")
        .cloned()
        .expect("device name event must be captured");
    assert!(device_name_event
        .properties_json
        .contains("name_length_bucket"));
    assert!(!device_name_event
        .properties_json
        .contains("mobile-contract-host"));
    let properties: serde_json::Value = serde_json::from_str(&device_name_event.properties_json)
        .expect("analytics event properties must be valid JSON");
    assert_eq!(properties["$os"], "ios");
    assert_eq!(properties["os"], "ios");
    assert_eq!(properties["os_version"], "18.0");
    assert_eq!(properties["$device_type"], "mobile");
    assert_eq!(properties["arch"], "arm64");
    assert_eq!(properties["app_channel"], "development");
    assert_eq!(lock(&analytics.identifies).len(), 1);
    assert_eq!(lock(&analytics.group_identifies).len(), 1);
    assert!(lock(&analytics.space_person_id).is_some());

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn dropping_a_running_binding_stops_its_worker() {
    let _test_guard = engine_test_guard();
    let (started, startup) = std::sync::mpsc::channel();
    let (drop_engine, drop_requested) = std::sync::mpsc::channel();
    let (finished, completion) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let root = tempfile::tempdir().expect("temporary host root must be available");
        let host = Arc::new(MemoryHost::new(root.path()));
        let engine = MobileEngine::start(
            BindingConfig {
                app_version: "1.2.3".to_owned(),
                profile_id: "binding-drop".to_owned(),
            },
            host,
        )
        .expect("binding engine must start");

        let _ = started.send(());
        drop_requested
            .recv()
            .expect("drop request must reach the binding owner");
        drop(engine);
        let _ = finished.send(());
    });

    startup
        .recv_timeout(std::time::Duration::from_secs(15))
        .expect("binding engine must finish starting before drop is measured");
    drop_engine
        .send(())
        .expect("binding owner must still be waiting for the drop request");
    completion
        .recv_timeout(std::time::Duration::from_secs(6))
        .expect("dropping the binding must not leave its worker waiting on the event stream");
}

#[test]
fn mobile_host_observes_suspend_and_resume_state_events() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-lifecycle".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-lifecycle-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space before lifecycle transitions");

    wait_for_state(&engine, BindingEngineState::Running);
    engine.suspend().expect("binding engine must suspend");
    wait_for_state(&engine, BindingEngineState::Suspended);
    engine.resume().expect("binding engine must resume");
    wait_for_state(&engine, BindingEngineState::Running);

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn peer_refresh_returns_explicit_connection_counts() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-peer-refresh".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-peer-refresh".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let report = engine
        .refresh_peer_connections()
        .expect("binding must refresh peer connections");
    assert_eq!(report.total, 0);
    assert_eq!(report.online, 0);
    assert_eq!(report.offline, 0);
    assert_eq!(report.errors, 0);

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn space_management_preserves_state_devices_resend_outcomes_and_local_history() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-space-management".to_owned(),
        },
        host.clone(),
    )
    .expect("binding engine must start");
    let created = engine
        .create_space(
            Some("mobile-space-manager".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let state = engine
        .query_space_state()
        .expect("binding must query setup state");
    assert!(state.has_completed);
    assert_eq!(state.space_id.as_deref(), Some(created.space_id.as_str()));
    assert_eq!(state.device_name.as_deref(), Some("mobile-space-manager"));

    let devices = engine.list_devices().expect("binding must list devices");
    assert!(devices.iter().all(|device| !device.device_id.is_empty()));
    let local_devices = devices
        .iter()
        .filter(|device| device.is_local)
        .collect::<Vec<_>>();
    assert_eq!(
        local_devices.len(),
        1,
        "the roster must identify exactly one local device"
    );
    assert!(
        local_devices[0].online,
        "the local device must be shown as online"
    );
    let remove_local = engine.remove_member(local_devices[0].device_id.clone());
    assert!(
        matches!(remove_local, Err(BindingError::Engine { .. })),
        "the binding must reject removing the local device"
    );

    let resend = engine
        .resend_entry("missing-entry".to_owned(), Vec::new(), false)
        .expect("missing entry must remain a structured business outcome");
    assert!(matches!(
        resend,
        uc_engine_uniffi::ResendEntryOutcome::NoEligibleTargets
    ));

    let remove = engine.remove_member("missing-device".to_owned());
    assert!(matches!(remove, Err(BindingError::Engine { .. })));
    let trust = engine
        .query_device_group_choices()
        .expect("binding must query current device group choices");
    let trust: serde_json::Value =
        serde_json::from_str(&trust).expect("device trust must be valid JSON");
    assert_eq!(trust["device_trust"]["local_membership"], "active");

    engine
        .leave_space()
        .expect("binding must leave the local space");
    // 离开成功意味着同一实例已回到可查询的空空间，宿主不需要先重启。
    let emptied = engine
        .query_space_state()
        .expect("the instance must stay queryable after leaving");
    assert!(!emptied.has_completed);
    assert!(emptied.space_id.is_none());
    assert!(
        engine
            .list_devices()
            .expect("the instance must list devices after leaving")
            .is_empty(),
        "leaving a space must not retain its member roster"
    );
    engine
        .query_device_group_choices()
        .expect("the join page must be able to read device group choices after leaving");
    engine
        .leave_space()
        .expect("leaving an already empty space must stay successful");
    // 仍然兼容“离开后关闭并重启”的宿主。
    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("reset binding engine must shut down within the deadline");

    let restarted = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-space-management".to_owned(),
        },
        host,
    )
    .expect("binding engine must restart after leaving");
    let left = restarted
        .query_space_state()
        .expect("restarted binding must query state after leaving");
    assert!(!left.has_completed);
    assert!(left.space_id.is_none());
    assert!(
        restarted
            .list_devices()
            .expect("restarted binding must list devices after leaving")
            .is_empty(),
        "leaving a space must not retain its member roster"
    );

    restarted
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn mobile_shutdown_timeout_can_be_joined_and_profile_reopened() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(MemoryHost::new(root.path()));
    let config = BindingConfig {
        app_version: "1.2.3".to_owned(),
        profile_id: "shutdown-retry".to_owned(),
    };
    let engine = MobileEngine::start(config.clone(), host.clone()).unwrap();
    engine
        .create_space(
            Some("shutdown-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .unwrap();
    if let Err(error) = engine.shutdown(0) {
        assert!(matches!(
            error,
            BindingError::Engine {
                category: BindingErrorCategory::DeadlineExceeded,
                ..
            }
        ));
    }
    assert!(engine.list_devices().is_err());
    engine.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS).unwrap();
    engine.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS).unwrap();
    let restarted = MobileEngine::start(config, host).unwrap();
    assert!(restarted.recover_session(true).unwrap().unlocked);
    restarted.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS).unwrap();
}

#[test]
fn mobile_suspend_deadline_allows_completion_retry_and_resume() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().unwrap();
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "suspend-deadline".to_owned(),
        },
        host,
    )
    .unwrap();
    engine
        .create_space(
            Some("suspend-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .unwrap();
    if let Err(error) = engine.suspend_with_deadline(0) {
        assert!(matches!(
            error,
            BindingError::Engine {
                category: BindingErrorCategory::DeadlineExceeded,
                ..
            }
        ));
    }
    engine
        .suspend_with_deadline(ENGINE_SHUTDOWN_DEADLINE_MS)
        .unwrap();
    wait_for_state(&engine, BindingEngineState::Suspended);
    assert!(engine.list_devices().is_err());
    engine.resume().unwrap();
    wait_for_state(&engine, BindingEngineState::Running);
    assert!(engine.list_devices().is_ok());
    engine.shutdown(ENGINE_SHUTDOWN_DEADLINE_MS).unwrap();
}

#[test]
fn mobile_host_recovers_the_same_identity_after_process_restart() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let config = BindingConfig {
        app_version: "1.2.3".to_owned(),
        profile_id: "binding-session-recovery".to_owned(),
    };
    let first =
        MobileEngine::start(config.clone(), host.clone()).expect("first binding engine must start");
    let created = first
        .create_space(
            Some("mobile-recovery-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("first binding engine must create a space");
    first
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("first binding engine must shut down");

    let restarted = MobileEngine::start(config, host).expect("restarted binding engine must start");
    let recovery = restarted
        .recover_session(true)
        .expect("restarted binding engine must recover its persisted session");
    assert!(recovery.unlocked);
    assert!(recovery.resumed);
    let local = restarted
        .query_local_device()
        .expect("restarted binding engine must expose its recovered identity");
    assert_eq!(local.device_id, created.self_device_id);
    assert_eq!(
        restarted
            .lifecycle_state()
            .expect("restarted binding engine must expose its lifecycle state"),
        BindingEngineState::Running
    );

    restarted
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("restarted binding engine must shut down");
}

#[test]
fn mobile_binding_exposes_membership_convergence() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-membership-convergence".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-convergence-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let status = engine
        .query_device_group_choices()
        .expect("binding must expose device group choices");
    let status: serde_json::Value =
        serde_json::from_str(&status).expect("device trust must be valid JSON");
    assert_eq!(status["device_trust"]["local_membership"], "active");
    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

fn wait_for_state(engine: &MobileEngine, expected: BindingEngineState) {
    for _ in 0..50 {
        if let Some(BindingEvent::StateChanged { state }) = engine.next_event(100) {
            if state == expected {
                return;
            }
        }
    }
    panic!("binding did not deliver the expected state: {expected:?}");
}

#[test]
fn completed_engine_operations_are_available_as_structured_events() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-operation-events".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-event-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    for _ in 0..50 {
        if let Some(BindingEvent::OperationFinished {
            operation_id,
            terminal,
            failure,
        }) = engine.next_event(100)
        {
            assert!(!operation_id.is_empty());
            assert_eq!(terminal, BindingOperationTerminal::Succeeded);
            assert_eq!(failure, None);
            engine
                .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
                .expect("binding engine must shut down within the deadline");
            return;
        }
    }
    panic!("binding did not deliver a completed-operation event");
}

#[test]
fn pairing_methods_return_invitation_data_and_stable_join_errors() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-pairing".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-pairing-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let invitation: InvitationIssued = engine
        .issue_invitation()
        .expect("binding must return a pairing invitation");
    assert!(!invitation.invitation_code.is_empty());
    assert!(!invitation.full_invitation.is_empty());
    assert!(invitation.expires_at_ms > 0);

    let error = engine
        .join_space(
            invitation.invitation_code,
            Some("  ".to_owned()),
            "correct horse battery staple".to_owned(),
            false,
        )
        .expect_err("blank device name must be rejected by the core");
    assert!(matches!(
        error,
        BindingError::Engine {
            category: BindingErrorCategory::InvalidInput,
            retryable: false,
            ..
        }
    ));

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

/// 保存的自定义 relay 在节点重新构建后才生效：概览必须经过“已保存未生效”再到“已生效”。
#[test]
fn relay_overview_reports_applied_relays_after_the_node_is_rebuilt() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let config = || BindingConfig {
        app_version: "1.2.3".to_owned(),
        profile_id: "binding-relay-overview-rebuild".to_owned(),
    };
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(config(), host.clone()).expect("binding engine must start");
    engine
        .add_custom_relay("https://relay-rebuild.example".to_owned(), String::new())
        .expect("add relay");
    let pending = engine
        .query_relay_overview()
        .expect("overview before rebuild");
    assert_eq!(pending.saved_mode, RelayRoutingMode::Custom);
    assert_eq!(pending.applied_mode, Some(RelayRoutingMode::BuiltIn));
    assert!(pending.change_pending);
    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("first engine must shut down");

    let rebuilt = MobileEngine::start(config(), host).expect("rebuilt engine must start");
    let applied = rebuilt
        .query_relay_overview()
        .expect("overview after rebuild");
    assert_eq!(applied.saved_mode, RelayRoutingMode::Custom);
    assert_eq!(applied.applied_mode, Some(RelayRoutingMode::Custom));
    assert!(!applied.change_pending);
    assert_eq!(applied.entries.len(), 5);
    assert!(applied
        .entries
        .iter()
        .filter(|entry| entry.source == RelayEntrySource::BuiltIn)
        .all(|entry| !entry.in_effect));
    let custom = &applied.entries[4];
    assert_eq!(custom.source, RelayEntrySource::Custom);
    assert!(custom.in_effect);
    rebuilt
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("rebuilt engine must shut down");
}

#[test]
fn custom_relay_methods_preserve_authoritative_results_and_stable_errors() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-custom-relays".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");

    assert!(engine
        .query_custom_relays()
        .expect("default relay list")
        .is_empty());

    // 内置列表是产品默认值：不进入用户的自定义列表，但概览必须列出并标明实际生效。
    let overview = engine.query_relay_overview().expect("relay overview");
    assert_eq!(overview.saved_mode, RelayRoutingMode::BuiltIn);
    assert_eq!(overview.applied_mode, Some(RelayRoutingMode::BuiltIn));
    assert!(!overview.change_pending);
    let regions: Vec<_> = overview
        .entries
        .iter()
        .map(|entry| entry.region_id.as_deref())
        .collect();
    assert_eq!(
        regions,
        [
            Some("na-east"),
            Some("na-west"),
            Some("eu"),
            Some("asia-pacific")
        ]
    );
    assert!(overview
        .entries
        .iter()
        .all(|entry| entry.source == RelayEntrySource::BuiltIn && entry.in_effect));
    assert!(!format!("{overview:?}").contains("iroh.link"));

    let added = engine
        .add_custom_relay(
            "  https://relay-a.example  ".to_owned(),
            "private-relay-token".to_owned(),
        )
        .expect("add relay with credential");
    assert_eq!(added.rejection, None);
    assert_eq!(added.relays.len(), 1);
    // 已保存但运行中的节点仍用内置列表：概览必须区分，不能把保存当成生效。
    let pending = engine
        .query_relay_overview()
        .expect("relay overview after add");
    assert_eq!(pending.saved_mode, RelayRoutingMode::Custom);
    assert_eq!(pending.applied_mode, Some(RelayRoutingMode::BuiltIn));
    assert!(pending.change_pending);
    assert_eq!(pending.entries.len(), 5);
    assert!(pending
        .entries
        .iter()
        .filter(|entry| entry.source == RelayEntrySource::BuiltIn)
        .all(|entry| entry.in_effect));
    let custom_entry = &pending.entries[4];
    assert_eq!(custom_entry.source, RelayEntrySource::Custom);
    assert_eq!(custom_entry.url, "https://relay-a.example/");
    assert!(custom_entry.credential_configured);
    assert!(!custom_entry.in_effect);
    assert_eq!(added.relays[0].url, "https://relay-a.example/");
    assert!(added.relays[0].credential_configured);
    let added_debug = format!("{added:?}");
    assert!(!added_debug.contains("relay-a"));
    assert!(!added_debug.contains("private-relay-token"));

    let queried = engine.query_custom_relays().expect("query saved relays");
    assert_eq!(queried, added.relays);
    assert!(!format!("{queried:?}").contains("relay-a"));

    let second = engine
        .add_custom_relay("https://relay-b.example".to_owned(), String::new())
        .expect("add relay without credential");
    assert_eq!(second.relays.len(), 2);
    assert!(!second.relays[1].credential_configured);

    let edited = engine
        .edit_custom_relay(
            "https://relay-b.example/".to_owned(),
            "https://relay-c.example".to_owned(),
            String::new(),
        )
        .expect("edit relay without adding a credential");
    assert_eq!(edited.rejection, None);
    assert_eq!(edited.relays[1].url, "https://relay-c.example/");
    assert!(!edited.relays[1].credential_configured);

    let duplicate = engine
        .edit_custom_relay(
            "https://relay-c.example".to_owned(),
            "https://relay-a.example".to_owned(),
            String::new(),
        )
        .expect("duplicate relay is a business rejection");
    assert!(duplicate.relays.is_empty());
    assert_eq!(
        duplicate.rejection,
        Some(CustomRelayMutationRejection::Duplicate)
    );

    let invalid = engine
        .add_custom_relay("not a relay URL".to_owned(), String::new())
        .expect("invalid URL is a business rejection");
    assert_eq!(
        invalid.rejection,
        Some(CustomRelayMutationRejection::InvalidUrl)
    );

    let missing = engine
        .delete_custom_relay("https://missing-relay.example".to_owned())
        .expect("missing relay is a business rejection");
    assert_eq!(
        missing.rejection,
        Some(CustomRelayMutationRejection::NotFound)
    );

    let malformed_token = "private\nrelay-token";
    let error = engine
        .add_custom_relay(
            "https://invalid-token.example".to_owned(),
            malformed_token.to_owned(),
        )
        .expect_err("malformed token must be invalid input");
    assert!(matches!(
        error,
        BindingError::Engine {
            code: 1395,
            category: BindingErrorCategory::InvalidInput,
            retryable: false,
        }
    ));
    assert!(!format!("{error:?}").contains(malformed_token));

    let removed = engine
        .delete_custom_relay("https://relay-a.example".to_owned())
        .expect("delete configured relay");
    assert_eq!(removed.rejection, None);
    assert_eq!(removed.relays.len(), 1);
    assert_eq!(removed.relays[0].url, "https://relay-c.example/");

    let legacy_added = engine
        .save_custom_relay(
            "https://legacy-target.example".to_owned(),
            String::new(),
            None,
        )
        .expect("legacy add");
    assert!(!legacy_added.configured);
    let legacy_edited = engine
        .save_custom_relay(
            "https://legacy-edited.example".to_owned(),
            String::new(),
            Some("https://legacy-target.example/".to_owned()),
        )
        .expect("legacy edit");
    assert!(!legacy_edited.configured);
    let legacy_deleted = engine
        .save_custom_relay(
            String::new(),
            String::new(),
            Some("https://legacy-edited.example".to_owned()),
        )
        .expect("legacy delete");
    assert!(!legacy_deleted.configured);

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn text_send_returns_a_content_free_delivery_summary() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-send-text".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-text-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let report: SendReport = engine
        .send_text("private binding text".to_owned(), Vec::new())
        .expect("binding must send text through the core");
    assert!(!report.entry_id.is_empty());
    assert!(report.at_ms > 0);
    assert_eq!(report.total_accepted, 0);
    assert_eq!(report.total_duplicate, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.total_pending, 0);
    assert!(!format!("{report:?}").contains("private binding text"));

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn image_send_returns_the_shared_content_free_delivery_summary() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-send-image".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-image-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let mut private_image = vec![0; 64 * 1024 + 1];
    private_image[..8].copy_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    let report: SendReport = engine
        .send_image(private_image.clone(), "image/png".to_owned(), Vec::new())
        .expect("binding must send an image through the core");
    assert!(!report.entry_id.is_empty());
    assert!(report.at_ms > 0);
    assert_eq!(report.total_accepted, 0);
    assert_eq!(report.total_duplicate, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.total_pending, 0);
    assert!(!format!("{report:?}").contains(&format!("{private_image:?}")));

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn file_send_reads_opaque_host_handles_and_returns_the_shared_summary() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let private_file = b"private binding file".to_vec();
    host.register_input_file(
        "input-file-1",
        "private-name.txt",
        Some("text/plain"),
        private_file.clone(),
    );
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-send-file".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-file-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let report: SendReport = engine
        .send_files(vec!["input-file-1".to_owned()], Vec::new())
        .expect("binding must send a host file through the core");
    assert!(!report.entry_id.is_empty());
    assert!(report.at_ms > 0);
    assert!(!format!("{report:?}").contains("private-name.txt"));
    assert!(!format!("{report:?}").contains("private binding file"));

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn capture_current_clipboard_reads_a_structured_host_snapshot() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        identity: None,
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"private clipboard text".to_vec(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-capture".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-capture-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let entry_id = engine
        .capture_current_clipboard()
        .expect("binding must capture the host clipboard")
        .expect("non-empty clipboard must create an entry");
    assert!(!entry_id.is_empty());

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn observed_clipboard_change_returns_a_delivery_report_for_local_content() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        identity: None,
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"private observed clipboard text".to_vec(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-observe-clipboard".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-observe-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    let report = engine
        .observe_clipboard_change(true)
        .expect("binding must process the observed clipboard change")
        .expect("local content must be dispatched");
    assert!(!report.entry_id.is_empty());
    assert!(report.at_ms > 0);
    assert_eq!(report.total_accepted, 0);
    assert_eq!(report.total_duplicate, 0);
    assert_eq!(report.total_offline, 0);
    assert_eq!(report.total_errored, 0);
    assert_eq!(report.total_pending, 0);

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn observed_clipboard_change_can_capture_without_dispatching() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        identity: None,
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"private local-only clipboard text".to_vec(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-observe-local-only".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-observe-local-only".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    assert_eq!(
        engine
            .observe_clipboard_change(false)
            .expect("binding must observe without dispatching"),
        None
    );

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn restore_clipboard_writes_a_structured_snapshot_to_the_host() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let private_text = b"private restored clipboard".to_vec();
    host.set_clipboard(BindingClipboardSnapshot {
        identity: None,
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: private_text.clone(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-restore".to_owned(),
        },
        host.clone(),
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-restore-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");
    let entry_id = engine
        .capture_current_clipboard()
        .expect("binding must capture the host clipboard")
        .expect("non-empty clipboard must create an entry");

    assert_eq!(
        engine
            .restore_clipboard(entry_id, BindingClipboardRestoreMode::Standard)
            .expect("binding must restore through the host clipboard"),
        BindingClipboardRestoreOutcome::Restored
    );
    let writes = host.clipboard_writes();
    assert_eq!(writes.len(), 1);
    assert!(matches!(
        writes[0].representations.as_slice(),
        [BindingClipboardRepresentation::Inline { bytes, .. }] if bytes == &private_text
    ));
    drop(writes);

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

#[test]
fn active_clipboard_can_be_queried_after_the_activation_event_was_missed() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        identity: None,
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"current active clipboard".to_vec(),
        }],
    });
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-query-active-clipboard".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-query-active-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");
    let entry_id = engine
        .observe_clipboard_change(true)
        .expect("binding must observe the host clipboard")
        .expect("changed clipboard must be dispatched")
        .entry_id;
    let local_device = engine
        .query_local_device()
        .expect("binding must expose the local device");

    let active = engine
        .query_active_clipboard()
        .expect("binding must query the active clipboard")
        .expect("captured clipboard must be active");

    assert_eq!(active.entry_id, entry_id);
    assert_eq!(active.activated_by, local_device.device_id);

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down");
}

#[test]
fn active_clipboard_query_returns_empty_before_the_first_activation() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-query-empty-active-clipboard".to_owned(),
        },
        host,
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-query-empty-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");

    assert_eq!(
        engine
            .query_active_clipboard()
            .expect("binding must query an empty active clipboard"),
        None
    );

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down");
}

#[test]
fn active_clipboard_query_survives_session_recovery() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    host.set_clipboard(BindingClipboardSnapshot {
        identity: None,
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: b"persisted active clipboard".to_vec(),
        }],
    });
    let config = BindingConfig {
        app_version: "1.2.3".to_owned(),
        profile_id: "binding-query-recovered-active-clipboard".to_owned(),
    };
    let first =
        MobileEngine::start(config.clone(), host.clone()).expect("first binding engine must start");
    first
        .create_space(
            Some("mobile-query-recovery-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("first binding engine must create a space");
    let entry_id = first
        .observe_clipboard_change(true)
        .expect("first binding engine must observe the clipboard")
        .expect("observed clipboard must be dispatched")
        .entry_id;
    let activated_by = first
        .query_local_device()
        .expect("first binding engine must expose the local device")
        .device_id;
    let active_before_shutdown = first
        .query_active_clipboard()
        .expect("first binding engine must query the active clipboard")
        .expect("captured clipboard must be active before shutdown");
    assert_eq!(active_before_shutdown.entry_id, entry_id);
    assert_eq!(active_before_shutdown.activated_by, activated_by);
    first
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("first binding engine must shut down");

    let restarted = MobileEngine::start(config, host).expect("restarted binding engine must start");
    let recovery = restarted
        .recover_session(true)
        .expect("restarted binding engine must recover its persisted session");
    assert!(recovery.unlocked);
    assert!(recovery.resumed);

    let active = restarted
        .query_active_clipboard()
        .expect("restarted binding engine must query the active clipboard")
        .expect("recovered active clipboard must be available");
    assert_eq!(active.entry_id, entry_id);
    assert_eq!(active.activated_by, activated_by);

    restarted
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("restarted binding engine must shut down");
}

#[test]
fn export_entry_writes_to_an_opaque_host_handle_and_finishes_it() {
    let _test_guard = engine_test_guard();
    let root = tempfile::tempdir().expect("temporary host root must be available");
    let host = Arc::new(MemoryHost::new(root.path()));
    let private_text = b"private exported clipboard".to_vec();
    host.set_clipboard(BindingClipboardSnapshot {
        identity: None,
        observed_at_ms: 1_700_000_000_000,
        representations: vec![BindingClipboardRepresentation::Inline {
            format: "text/plain".to_owned(),
            mime_type: Some("text/plain".to_owned()),
            bytes: private_text.clone(),
        }],
    });
    host.register_input_file(
        "output-file-1",
        "private-export.txt",
        Some("text/plain"),
        Vec::new(),
    );
    let engine = MobileEngine::start(
        BindingConfig {
            app_version: "1.2.3".to_owned(),
            profile_id: "binding-export".to_owned(),
        },
        host.clone(),
    )
    .expect("binding engine must start");
    engine
        .create_space(
            Some("mobile-export-host".to_owned()),
            "correct horse battery staple".to_owned(),
        )
        .expect("binding must create a space");
    let entry_id = engine
        .capture_current_clipboard()
        .expect("binding must capture the host clipboard")
        .expect("non-empty clipboard must create an entry");

    engine
        .export_entry(entry_id, "output-file-1".to_owned())
        .expect("binding must export to the host handle");
    assert_eq!(host.file_bytes("output-file-1"), Some(private_text.clone()));
    assert!(host.file_finished("output-file-1"));

    engine
        .shutdown(ENGINE_SHUTDOWN_DEADLINE_MS)
        .expect("binding engine must shut down within the deadline");
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn engine_test_guard() -> MutexGuard<'static, ()> {
    match ENGINE_TEST_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
