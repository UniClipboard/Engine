use std::fs;
use std::path::Path;

use uc_ohos_napi::{
    core_version, install_process_observability, query_process_observability_health,
    shutdown_process_observability, OhHostDirectories, OhObservabilityConfig,
};

#[test]
fn core_version_uses_the_binding_package_version() {
    assert_eq!(core_version(), format!("v{}", env!("CARGO_PKG_VERSION")));
}

#[tokio::test]
async fn process_observability_health_is_public_and_current() {
    let root = std::env::temp_dir().join(format!(
        "uc-ohos-observability-health-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("health test root");

    let setup = install_process_observability(
        OhObservabilityConfig {
            service_version: "1.2.3".to_owned(),
            environment: "test".to_owned(),
            app_channel: "test".to_owned(),
            remote_diagnostics_enabled: false,
            collector: None,
        },
        OhHostDirectories {
            private_data_directory: root.join("private").to_string_lossy().into_owned(),
            cache_directory: root.join("cache").to_string_lossy().into_owned(),
            temporary_directory: root.join("temporary").to_string_lossy().into_owned(),
        },
    )
    .expect("install process observability");

    let health = query_process_observability_health().expect("query process health");
    assert_eq!(health.remote, setup.remote);
    assert_eq!(health.local_file, setup.local_file);
    assert_eq!(health.dropped_local_records, setup.dropped_local_records);
    assert_eq!(health.dropped_remote_spans, 0.0);
    assert_eq!(health.dropped_remote_logs, 0.0);
    assert_eq!(health.failed_remote_span_batches, 0.0);
    assert_eq!(health.failed_remote_log_batches, 0.0);

    use uc_ohos_napi::*;
    register_host_diagnostic_source(
        OhHostDiagnosticSource::Application,
        OhSourceCapability::Supported,
    )
    .expect("source");
    let capture = start_local_diagnostic_capture(1_000).expect("capture");
    assert_eq!(capture.mode, "detailed");
    let action = begin_host_diagnostic(
        OhHostDiagnosticSource::Application,
        OhHostDiagnosticAction::RuntimeStart,
    )
    .expect("begin");
    let finished = finish_host_diagnostic(
        OhHostDiagnosticSource::Application,
        action.token.expect("token"),
        OhHostDiagnosticOutcome::Completed,
        None,
    )
    .expect("finish");
    assert_eq!(finished.status, "accepted");
    let report = prepare_local_diagnostic_export(1_000)
        .await
        .expect("export");
    assert_eq!(report.flush, "completed");
    assert!(!report.other_processes_flushed);
    assert!(report
        .files
        .iter()
        .any(|source| source.written_count != "0"));
    assert_eq!(
        stop_local_diagnostic_capture(capture.capture_id.expect("capture id")).expect("stop"),
        "stopped"
    );

    let _ = shutdown_process_observability(1_000).await;
    let _ = fs::remove_dir_all(root);
}

#[test]
fn typescript_contract_exposes_process_observability_health() {
    let declarations =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("ohos/index.d.ts"))
            .expect("OHOS declarations must be readable");

    for required in [
        "export interface OhObservabilityHealth",
        "droppedRemoteSpans: number",
        "droppedRemoteLogs: number",
        "failedRemoteSpanBatches: number",
        "failedRemoteLogBatches: number",
        "queryProcessObservabilityHealth(): OhObservabilityHealth",
    ] {
        assert!(
            declarations.contains(required),
            "OHOS declarations missing {required}"
        );
    }
}

#[test]
fn typescript_contract_exposes_join_upgrade_status() {
    let declarations = include_str!("../ohos/index.d.ts");
    for required in [
        "export interface OhJoinSpaceStatus",
        "peerUpgradeRequired: boolean",
        "joinSpace(",
        "cancelJoinSpace(joinId: string): Promise<OhJoinSpaceStatus>",
        "queryDeviceGroupChoices(): Promise<string>",
    ] {
        assert!(
            declarations.contains(required),
            "OHOS declarations missing {required}"
        );
    }
}
