use std::time::Duration;
use uc_observability_runtime::*;

#[test]
fn host_tokens_are_source_bound_and_resume_ends_an_uncertain_capture() {
    let directory = tempfile::tempdir().expect("logs");
    let handle = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Ios,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(directory.path())),
    )
    .expect("install")
    .handle();
    handle
        .register_host_diagnostic_source(
            HostDiagnosticSource::Application,
            SourceCapability::Supported,
        )
        .expect("register");
    handle
        .register_host_diagnostic_source(
            HostDiagnosticSource::ShareExtension,
            SourceCapability::Partial,
        )
        .expect("register extension");
    assert!(handle
        .register_host_diagnostic_source(
            HostDiagnosticSource::BackgroundService,
            SourceCapability::Supported
        )
        .is_err());
    let start = handle.record_host_diagnostic(
        HostDiagnosticSource::Application,
        HostDiagnosticEvent::Begin {
            action: HostDiagnosticAction::RuntimeStart,
        },
    );
    let token = start.token.expect("token");
    let wrong = handle.record_host_diagnostic(
        HostDiagnosticSource::ShareExtension,
        HostDiagnosticEvent::Finish {
            token: token.clone(),
            outcome: HostDiagnosticOutcome::Completed,
        },
    );
    assert_eq!(wrong.status, HostDiagnosticRecordStatus::InvalidToken);
    let finish = handle.record_host_diagnostic(
        HostDiagnosticSource::Application,
        HostDiagnosticEvent::Finish {
            token: token.clone(),
            outcome: HostDiagnosticOutcome::Completed,
        },
    );
    assert_eq!(finish.status, HostDiagnosticRecordStatus::Accepted);
    assert_eq!(
        handle
            .record_host_diagnostic(
                HostDiagnosticSource::Application,
                HostDiagnosticEvent::Finish {
                    token,
                    outcome: HostDiagnosticOutcome::Completed
                }
            )
            .status,
        HostDiagnosticRecordStatus::InvalidToken
    );
    handle
        .start_local_diagnostic_capture(DetailedCaptureRequest::default())
        .expect("capture");
    handle.record_host_diagnostic(
        HostDiagnosticSource::Application,
        HostDiagnosticEvent::Lifecycle {
            state: HostLifecycleState::Background,
        },
    );
    handle.record_host_diagnostic(
        HostDiagnosticSource::Application,
        HostDiagnosticEvent::Lifecycle {
            state: HostLifecycleState::Foreground,
        },
    );
    let status = handle.query_local_diagnostic_status();
    assert_eq!(status.capture.mode, LocalCaptureMode::Standard);
    assert_eq!(
        status.capture.end_reason,
        Some(CaptureEndReason::SuspensionExpiryUnknown)
    );
    assert_eq!(
        handle
            .prepare_local_diagnostic_export(Duration::from_secs(2))
            .expect("prepare")
            .flush,
        SignalResult::Completed
    );
    let output = managed_log_files(directory.path())
        .expect("files")
        .iter()
        .map(|file| std::fs::read_to_string(file).expect("content"))
        .collect::<String>();
    assert!(output.contains("host.action.finished"));
    assert!(output.contains("host.lifecycle.changed"));
    assert!(!output.contains("background_service\",\"action"));
    handle
        .start_local_diagnostic_capture(DetailedCaptureRequest::default())
        .expect("new capture");
    let pending = handle.record_host_diagnostic(
        HostDiagnosticSource::Application,
        HostDiagnosticEvent::Begin {
            action: HostDiagnosticAction::RuntimeStop,
        },
    );
    assert!(pending.token.is_some());
    handle.shutdown(Duration::from_secs(2));
    assert_eq!(
        handle.query_local_diagnostic_status().capture.end_reason,
        Some(CaptureEndReason::RuntimeShutdown)
    );
    let final_output = managed_log_files(directory.path())
        .expect("files")
        .iter()
        .map(|file| std::fs::read_to_string(file).expect("content"))
        .collect::<String>();
    assert!(final_output.contains("diagnostics.run.ended"));
    assert!(final_output.contains("interrupted"));
}
