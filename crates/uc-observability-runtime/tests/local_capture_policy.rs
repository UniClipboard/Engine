use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_runtime::*;

#[test]
fn capture_expires_without_reinstallation_and_preserves_critical_completion() {
    let directory = tempfile::tempdir().expect("logs");
    let handle = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(
            ObservabilityResource::new(
                "1.1.0",
                DeploymentEnvironment::Test,
                OperatingSystem::Macos,
                "test",
            )
            .expect("resource"),
        )
        .with_local_logs(LocalLogConfig::new(directory.path())),
    )
    .expect("install")
    .handle();
    assert_eq!(
        handle.query_local_diagnostic_status().capture.mode,
        LocalCaptureMode::Standard
    );
    let before = handle.query_local_diagnostic_status();
    assert_eq!(
        before
            .sources
            .iter()
            .find(|s| s.source == LocalDiagnosticSource::HostShareExtension)
            .expect("source")
            .collection,
        SourceCollection::NotRegistered
    );
    NetworkRecorder::current().register_source(
        LocalDiagnosticSource::DnsDiscovery,
        SourceCapability::Partial,
        SourceCollection::Disabled,
    );
    let after = handle.query_local_diagnostic_status();
    let dns = after
        .sources
        .iter()
        .find(|s| s.source == LocalDiagnosticSource::DnsDiscovery)
        .expect("dns");
    assert_eq!(dns.collection, SourceCollection::Disabled);
    assert_eq!(dns.observed_count, 0, "登记来源不算真正的诊断事件");
    let connection = ConnectionObservation::begin(ConnectionPurpose::Admission, [1; 32]);
    connection
        .attempts()
        .begin(1, Duration::from_secs(3))
        .finish(ConnectionOutcome::Connected);
    connection.finish(ConnectionOutcome::Connected);
    let first = handle
        .start_local_diagnostic_capture(DetailedCaptureRequest {
            duration: Duration::from_secs(1),
        })
        .expect("capture");
    let again = handle
        .start_local_diagnostic_capture(DetailedCaptureRequest::default())
        .expect("reuse");
    assert_eq!(first.capture_id, again.capture_id);
    assert!(again.remaining_ms <= 1000, "重复开始不能延长期限");
    let active = ConnectionObservation::begin(ConnectionPurpose::Admission, [1; 32]);
    let attempt = active.attempts().begin(1, Duration::from_secs(3));
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(
        handle.query_local_diagnostic_status().capture.mode,
        LocalCaptureMode::Standard
    );
    attempt.finish(ConnectionOutcome::Connected);
    active.finish(ConnectionOutcome::Connected);
    let next = handle
        .start_local_diagnostic_capture(DetailedCaptureRequest::default())
        .expect("new capture");
    assert_ne!(first.capture_id, next.capture_id);
    assert_eq!(
        handle
            .stop_local_diagnostic_capture(first.capture_id.as_deref().expect("id"))
            .expect("stop"),
        StopCaptureResult::DifferentCapture
    );
    handle
        .stop_local_diagnostic_capture(next.capture_id.as_deref().expect("id"))
        .expect("stop current");
    let report = handle
        .prepare_local_diagnostic_export(Duration::from_secs(2))
        .expect("export report");
    assert_eq!(report.flush, SignalResult::Completed);
    assert!(report.status.policy_filtered_records > 0);
    assert_eq!(report.status.capture.mode, LocalCaptureMode::Standard);
    let rows: Vec<serde_json::Value> = managed_log_files(directory.path())
        .expect("files")
        .iter()
        .flat_map(|file| {
            std::fs::read_to_string(file)
                .expect("content")
                .lines()
                .map(|line| serde_json::from_str(line).expect("JSON"))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        rows.iter()
            .filter(|row| row["fields"]["event.name"] == "connection.attempt.started")
            .count(),
        1
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row["fields"]["event.name"] == "connection.attempt.finished")
            .count(),
        1,
        "详细模式期间开始的记录到期后仍应有终态"
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row["fields"]["event.name"] == "connection.finished")
            .count(),
        2
    );
    handle.shutdown(Duration::from_secs(2));
}
