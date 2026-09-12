use std::sync::{Arc, Mutex};
use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_runtime::*;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("capture").extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
#[test]
fn one_process_can_keep_host_logs_and_route_engine_records_only_to_the_common_runtime() {
    let directory = tempfile::tempdir().expect("logs");
    let capture = Capture::default();
    let host_layer: HostLogLayer = Box::new(
        tracing_subscriber::fmt::layer()
            .without_time()
            .with_ansi(false)
            .with_writer(capture.clone()),
    );
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.1.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Macos,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(directory.path()));
    let handle =
        ProcessObservabilityRuntime::install_with_host_layers(config.clone(), vec![host_layer])
            .expect("install")
            .handle();
    tracing::info!(target: "host.test", "host event");
    tracing::info!(target: "uc_infra::private", "PRIVATE_ENGINE_PAYLOAD");
    tracing::info!(target: "iroh::private", "PRIVATE_NETWORK_PAYLOAD");
    let host_span = tracing::info_span!(target: "host.test", "host_request");
    let entered = host_span.enter();
    complete_admission_authentication_failure(
        AuthenticationFailure::ContinuationCredential(CredentialFailure::RecordMissing),
        Duration::from_millis(3),
    );
    drop(entered);
    drop(host_span);
    assert_eq!(
        handle.force_flush(Duration::from_secs(5)).logs,
        SignalResult::Completed
    );
    let host_output = String::from_utf8(capture.0.lock().expect("output").clone()).expect("UTF8");
    assert!(
        host_output.contains("host event"),
        "the host must retain its original logs"
    );
    assert!(!host_output.contains("uc.telemetry"));
    assert!(!host_output.contains("PRIVATE_ENGINE_PAYLOAD"));
    assert!(!host_output.contains("PRIVATE_NETWORK_PAYLOAD"));
    let engine_output = std::fs::read_dir(directory.path())
        .expect("files")
        .map(|e| std::fs::read_to_string(e.expect("file").path()).expect("content"))
        .collect::<String>();
    assert_eq!(
        engine_output
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON"))
            .filter(|row| row["target"] != "uc.diagnostics")
            .count(),
        1
    );
    assert!(engine_output.contains("record_missing"));
    assert!(!engine_output.contains("host event"));
    assert!(matches!(
        ProcessObservabilityRuntime::install(config.clone()),
        Ok(InstallOutcome::Reused(_))
    ));
    let extra: HostLogLayer = Box::new(tracing_subscriber::layer::Identity::new());
    assert!(matches!(
        ProcessObservabilityRuntime::install_with_host_layers(config, vec![extra]),
        Err(InstallError::AlreadyInstalled)
    ));
    handle.shutdown(Duration::from_secs(5));
}
