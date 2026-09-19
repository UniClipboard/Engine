use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uc_observability_contract::diagnostics::connectivity::*;
use uc_observability_contract::diagnostics::record_profile_upgrade_backup_failure;
use uc_observability_runtime::*;

const CONSOLE_CHILD_ENV: &str = "UC_OBSERVABILITY_CONSOLE_CHILD";
const CONSOLE_DIRECTORY_ENV: &str = "UC_OBSERVABILITY_CONSOLE_DIRECTORY";

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
fn system_console_child() {
    let Ok(format) = std::env::var(CONSOLE_CHILD_ENV) else {
        return;
    };
    let directory = std::env::var(CONSOLE_DIRECTORY_ENV).expect("console log directory");
    let format = match format.as_str() {
        "disabled" => SystemLogFormat::Disabled,
        "human-readable" => SystemLogFormat::HumanReadable,
        "human-readable-ansi" => SystemLogFormat::HumanReadableAnsi,
        "json" => SystemLogFormat::Json,
        _ => panic!("unexpected console format"),
    };
    let config = ObservabilityConfig::new(
        ObservabilityResource::new(
            "1.1.0",
            DeploymentEnvironment::Test,
            OperatingSystem::Linux,
            "test",
        )
        .expect("resource"),
    )
    .with_local_logs(LocalLogConfig::new(directory));
    let host_layer: HostLogLayer = Box::new(tracing_subscriber::layer::Identity::new());
    let handle = ProcessObservabilityRuntime::install_with_host_layers_and_system_log_format(
        config.clone(),
        vec![host_layer],
        format,
    )
    .expect("install")
    .handle();
    assert!(matches!(
        ProcessObservabilityRuntime::install_with_system_log_format(config.clone(), format),
        Ok(InstallOutcome::Reused(_))
    ));
    let conflicting_format = if format == SystemLogFormat::Disabled {
        SystemLogFormat::Json
    } else {
        SystemLogFormat::Disabled
    };
    assert!(matches!(
        ProcessObservabilityRuntime::install_with_system_log_format(config, conflicting_format),
        Err(InstallError::AlreadyInstalled)
    ));
    complete_admission_authentication_failure(
        AuthenticationFailure::ContinuationCredential(CredentialFailure::RecordMissing),
        Duration::from_millis(3),
    );
    assert_eq!(
        handle.force_flush(Duration::from_secs(5)).logs,
        SignalResult::Completed
    );
    handle.shutdown(Duration::from_secs(5));
}

#[test]
fn host_selects_console_format_without_changing_json_files_or_duplicating_events() {
    for (format, expects_console, expects_json, expects_ansi) in [
        ("disabled", false, false, false),
        ("human-readable", true, false, false),
        ("human-readable-ansi", true, false, true),
        ("json", true, true, false),
    ] {
        let directory = tempfile::tempdir().expect("logs");
        let output = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "system_console_child", "--nocapture"])
            .env(CONSOLE_CHILD_ENV, format)
            .env(CONSOLE_DIRECTORY_ENV, directory.path())
            .output()
            .expect("run console child");
        assert!(
            output.status.success(),
            "{format} child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let console = [output.stdout, output.stderr].concat();
        let console = String::from_utf8(console).expect("UTF8 console");
        let matching_lines = console
            .lines()
            .filter(|line| line.contains("uc.operation.completed"))
            .collect::<Vec<_>>();
        assert_eq!(matching_lines.len(), usize::from(expects_console));
        if let Some(line) = matching_lines.first() {
            let is_json = line.find('{').is_some_and(|start| {
                serde_json::from_str::<serde_json::Value>(&line[start..]).is_ok()
            });
            assert_eq!(
                is_json, expects_json,
                "unexpected {format} console line: {line}"
            );
            assert_eq!(
                line.contains("\u{1b}["),
                expects_ansi,
                "unexpected ANSI behavior for {format}: {line:?}"
            );
        }

        let engine_output = std::fs::read_dir(directory.path())
            .expect("files")
            .map(|entry| std::fs::read_to_string(entry.expect("file").path()).expect("content"))
            .collect::<String>();
        let matching_rows = engine_output
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON"))
            .filter(|row| row["fields"]["event.name"] == "uc.operation.completed")
            .count();
        assert_eq!(matching_rows, 1);
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
    record_profile_upgrade_backup_failure(
        "capture_profile_files",
        "permission_denied",
        Some("PermissionDenied"),
        Some(5),
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
        2
    );
    assert!(engine_output.contains("record_missing"));
    assert!(engine_output.contains("profile_upgrade.backup.failed"));
    assert!(engine_output.contains("capture_profile_files"));
    assert!(engine_output.contains("permission_denied"));
    assert!(engine_output.contains("PermissionDenied"));
    assert!(engine_output.contains("\"io_error_code\":5"));
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
